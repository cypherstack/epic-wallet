// Copyright 2023 The Epic Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The main interface for SQLite
//! Has the main operations to handle the SQLite database
//! This was built to replace LMDB

use crate::serialization as ser;
use crate::serialization::Serializable;
use crate::Error;
use sqlite::{self, Connection};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const SQLITE_MAX_RETRIES: u8 = 3;
static SQLITE_FILENAME: &str = "epic.db";

/// Basic struct holding the SQLite database connection
pub struct Store {
	db: Connection,
}

impl Store {
	pub fn new(db_path: PathBuf) -> Result<Store, sqlite::Error> {
		let db_path = db_path.join(SQLITE_FILENAME);
		let db: Connection = sqlite::open(db_path)?;
		
		// Validate SQLite version compatibility.
		Self::validate_sqlite_version(&db)?;
		
		Store::check_or_create(&db)?;
		Ok(Store { db })
	}
	
	/// Validate that we're using a compatible SQLite version.
	fn validate_sqlite_version(db: &Connection) -> Result<(), sqlite::Error> {
		let version_query = "SELECT sqlite_version()";
		match db.prepare(version_query) {
			Ok(mut stmt) => {
				if let Ok(sqlite::State::Row) = stmt.next() {
					let version: String = stmt.read("sqlite_version()").unwrap_or_else(|_| "unknown".to_string());
					log::info!("Using SQLite version: {}", version);
					
					// Warn if not using bundled version (this is best-effort detection).
					if !cfg!(bundled_sqlite) {
						log::warn!("Not using bundled SQLite - may encounter compatibility issues");
					}
				}
			}
			Err(e) => {
				log::error!("Failed to query SQLite version: {:?}", e);
				return Err(e);
			}
		}
		Ok(())
	}

	/// Handle the creation of the database
	/// New resource create use the 'IF NOT EXISTS' to avoid recreation
	pub fn check_or_create(db: &Connection) -> Result<(), sqlite::Error> {
		let creation = r#"
		-- Create the database table
		CREATE TABLE IF NOT EXISTS data (
			id INTEGER PRIMARY KEY,
			key BLOB NOT NULL UNIQUE,
			prefix TEXT,
			data TEXT NOT NULL,
			q_tx_id INTEGER,
			q_confirmed INTEGER,
			q_tx_status TEXT);

		-- Create indexes for queriable columns
		CREATE INDEX IF NOT EXISTS prefix_index ON data (prefix);
		CREATE INDEX IF NOT EXISTS q_tx_id_index ON data (q_tx_id);
		CREATE INDEX IF NOT EXISTS q_confirmed_index ON data (q_confirmed);
		CREATE INDEX IF NOT EXISTS q_tx_status_index ON data (q_tx_status);

		-- Configure SQLite WAL level
		-- This is optimized for multithread usage
		PRAGMA journal_mode=WAL; -- better write-concurrency
		PRAGMA synchronous=NORMAL; -- fsync only in critical moments
		PRAGMA wal_checkpoint(TRUNCATE); -- free some space by truncating possibly massive WAL files from the last run.
		"#;
		db.execute(creation)
	}

	/// Returns a single value of the database
	/// This returns a Serializable enum
	pub fn get(&self, key: &[u8]) -> Option<Serializable> {
		let query = r#"
			SELECT
				data
			FROM
				data
			WHERE
				key = ?
			LIMIT 1;
		"#;
		match self.db.prepare(query) {
			Ok(mut statement) => {
				statement.bind((1, key)).ok()?;
				match statement.next() {
					Ok(sqlite::State::Row) => {
						let data = statement.read::<String, _>("data").ok()?;
						ser::deserialize(&data).ok()
					}
					_ => None,
				}
			}
			Err(_) => None,
		}
	}

	/// Encapsulation for get function
	/// Gets a `Readable` value from the db, provided its key
	pub fn get_ser(&self, key: &[u8]) -> Option<Serializable> {
		self.get(key)
	}

	/// Check if a key exists on the database
	pub fn exists(&self, key: &[u8]) -> Result<bool, Error> {
		let query = r#"
			SELECT
				1
			FROM
				data
			WHERE
				key = ?
			LIMIT 1;
		"#;
		let mut statement = self.db.prepare(query).map_err(|_| Error::GenericError("Failed to prepare exists query".to_string()))?;
		statement.bind((1, key)).map_err(|_| Error::GenericError("Failed to bind key parameter".to_string()))?;
		match statement.next() {
			Ok(sqlite::State::Row) => Ok(true),
			Ok(sqlite::State::Done) => Ok(false),
			Err(_) => Ok(false),
		}
	}

	/// Provided a 'from' as prefix, returns a vector of Serializable enums
	pub fn iter(&self, from: &[u8]) -> Vec<Serializable> {
		let query = r#"
			SELECT
				data
			FROM
				data
			WHERE
				prefix = ?;
		"#;
		match self.db.prepare(query) {
			Ok(mut statement) => {
				let prefix_str = String::from_utf8(from.to_vec()).unwrap_or_else(|_| "".to_string());
				if statement.bind((1, prefix_str.as_str())).is_ok() {
					statement
						.into_iter()
						.filter_map(|row| {
							match row {
								Ok(r) => {
									let data: &str = r.read("data");
									ser::deserialize(data).ok()
								}
								Err(_) => None,
							}
						})
						.collect()
				} else {
					Vec::new()
				}
			}
			Err(_) => Vec::new(),
		}
	}

	/// Builds a new batch to be used with this store
	pub fn batch(&self) -> Batch {
		Batch { store: self }
	}

	/// Executes an SQLite statement
	/// If the database is locked due to another writing process,
	/// The code will retry the same statement after 100 milliseconds
	pub fn execute(&self, statement: String) -> Result<(), sqlite::Error> {
		let mut retries = 0;
		loop {
			match self.db.execute(statement.to_string()) {
				Ok(()) => break,
				Err(e) => {
					// e.code follows SQLite error types
					// Full documentation for error types can be found on https://www.sqlite.org/rescode.html
					// Error 5 is SQLITE_BUSY
					if e.code.unwrap() != 5 {
						return Err(e);
					}
					retries = retries + 1;

					if retries > SQLITE_MAX_RETRIES {
						return Err(e);
					}
					thread::sleep(Duration::from_millis(100));
				}
			}
		}

		Ok(())
	}
}

/// Batch to write multiple Writeables to db in an atomic manner
pub struct Batch<'a> {
	store: &'a Store,
}

impl<'a> Batch<'_> {
	/// Writes a single value to the db, given a key and a Serializable enum
	/// Specialized queries are used for TxLogEntry and OutputData to make best use of queriable columns
	pub fn put(&self, key: &[u8], value: Serializable) -> Result<(), Error> {
		// serialize value to json
		let value_s = ser::serialize(&value).map_err(|_| Error::GenericError("Failed to serialize value".to_string()))?;
		let prefix = key[0] as char;

		// Check if key exists first.
		let key_exists = self.exists(&key)?;

		// Prepare the appropriate query based on whether it's an insert or update.
		let (query, params): (String, Vec<sqlite::Value>) = if key_exists {
			// Update existing record.
			match &value {
				Serializable::TxLogEntry(t) => (
					"UPDATE data SET data = ?, q_tx_id = ?, q_confirmed = ?, q_tx_status = ? WHERE key = ?".to_string(),
					vec![
						sqlite::Value::String(value_s),
						sqlite::Value::Integer(t.id as i64),
						sqlite::Value::Integer(t.confirmed as i64),
						sqlite::Value::String(t.tx_type.to_string()),
						sqlite::Value::Binary(key.to_vec()),
					]
				),
				Serializable::OutputData(o) => (
					"UPDATE data SET data = ?, q_tx_id = ?, q_tx_status = ? WHERE key = ?".to_string(),
					vec![
						sqlite::Value::String(value_s),
						sqlite::Value::String(match o.tx_log_entry {
							Some(entry) => entry.to_string(),
							None => "".to_string(),
						}),
						sqlite::Value::String(o.status.to_string()),
						sqlite::Value::Binary(key.to_vec()),
					]
				),
				_ => (
					"UPDATE data SET data = ? WHERE key = ?".to_string(),
					vec![
						sqlite::Value::String(value_s),
						sqlite::Value::Binary(key.to_vec()),
					]
				),
			}
		} else {
			// Insert new record.
			match &value {
				Serializable::TxLogEntry(t) => (
					"INSERT INTO data (key, data, prefix, q_tx_id, q_confirmed, q_tx_status) VALUES (?, ?, ?, ?, ?, ?)".to_string(),
					vec![
						sqlite::Value::Binary(key.to_vec()),
						sqlite::Value::String(value_s),
						sqlite::Value::String(prefix.to_string()),
						sqlite::Value::Integer(t.id as i64),
						sqlite::Value::Integer(t.confirmed as i64),
						sqlite::Value::String(t.tx_type.to_string()),
					]
				),
				Serializable::OutputData(o) => (
					"INSERT INTO data (key, data, prefix, q_tx_id, q_tx_status) VALUES (?, ?, ?, ?, ?)".to_string(),
					vec![
						sqlite::Value::Binary(key.to_vec()),
						sqlite::Value::String(value_s),
						sqlite::Value::String(prefix.to_string()),
						sqlite::Value::String(match o.tx_log_entry {
							Some(entry) => entry.to_string(),
							None => "".to_string(),
						}),
						sqlite::Value::String(o.status.to_string()),
					]
				),
				_ => (
					"INSERT INTO data (key, data, prefix) VALUES (?, ?, ?)".to_string(),
					vec![
						sqlite::Value::Binary(key.to_vec()),
						sqlite::Value::String(value_s),
						sqlite::Value::String(prefix.to_string()),
					]
				),
			}
		};

		// Execute the prepared statement.
		let mut statement = self.store.db.prepare(query).map_err(|_| Error::GenericError("Failed to prepare statement".to_string()))?;
		for (i, param) in params.iter().enumerate() {
			statement.bind((i + 1, param)).map_err(|_| Error::GenericError("Failed to bind parameter".to_string()))?;
		}
		
		statement.next().map_err(|_| Error::GenericError("Failed to execute statement".to_string()))?;
		Ok(())
	}

	/// Writes a single value to the db, given a key and a Serializable enum
	/// Encapsulation for the store put function
	pub fn put_ser(&self, key: &[u8], value: Serializable) -> Result<(), Error> {
		self.put(key, value)
	}

	/// Check if a key exists on the database
	/// Encapsulation for the store exists function
	pub fn exists(&self, key: &[u8]) -> Result<bool, Error> {
		self.store.exists(key)
	}

	/// Provided a 'from' as prefix, returns a vector of Serializable enums
	/// Encapsulation for the store iter function
	pub fn iter(&self, from: &[u8]) -> Vec<Serializable> {
		self.store.iter(from)
	}

	/// Deletes a key from the db
	pub fn delete(&self, key: &[u8]) -> Result<(), Error> {
		let query = "DELETE FROM data WHERE key = ?";
		let mut statement = self.store.db.prepare(query).map_err(|_| Error::GenericError("Failed to prepare delete statement".to_string()))?;
		statement.bind((1, key)).map_err(|_| Error::GenericError("Failed to bind key parameter".to_string()))?;
		statement.next().map_err(|_| Error::GenericError("Failed to execute delete".to_string()))?;
		Ok(())
	}

	/// Returns a single value of the database
	/// Encapsulation for the store get_ser function
	pub fn get_ser(&self, key: &[u8]) -> Option<Serializable> {
		self.store.get_ser(key)
	}
}

unsafe impl Sync for Store {}
unsafe impl Send for Store {}
