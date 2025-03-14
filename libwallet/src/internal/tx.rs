// Copyright 2019 The Epic Developers
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

//! Transaction building functions

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;
use uuid::Uuid;

use crate::epic_core::consensus::valid_header_version;
use crate::epic_core::core::HeaderVersion;
use crate::epic_keychain::{Identifier, Keychain};
use crate::epic_util::secp::key::SecretKey;
use crate::epic_util::secp::pedersen;
use crate::epic_util::Mutex;
use crate::internal::{selection, updater};
use crate::slate::Slate;
use crate::types::{Context, NodeClient, StoredProofInfo, TxLogEntryType, WalletBackend};
use crate::{address, Error, ErrorKind};
use ed25519_dalek::Keypair as DalekKeypair;
use ed25519_dalek::PublicKey as DalekPublicKey;
use ed25519_dalek::SecretKey as DalekSecretKey;
use ed25519_dalek::Signature as DalekSignature;

// static for incrementing test UUIDs
lazy_static! {
	static ref SLATE_COUNTER: Mutex<u8> = { Mutex::new(0) };
}

/// Creates a new slate for a transaction, can be called by anyone involved in
/// the transaction (sender(s), receiver(s))
pub fn new_tx_slate<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	amount: u64,
	num_participants: usize,
	use_test_rng: bool,
	ttl_blocks: Option<u64>,
) -> Result<Slate, Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let current_height = wallet.w2n_client().get_chain_tip()?.0;
	let mut slate = Slate::blank(num_participants);
	if let Some(b) = ttl_blocks {
		slate.ttl_cutoff_height = Some(current_height + b);
	}
	if use_test_rng {
		{
			let sc = SLATE_COUNTER.lock();
			let bytes = [4, 54, 67, 12, 43, 2, 98, 76, 32, 50, 87, 5, 1, 33, 43, *sc];
			slate.id = Uuid::from_slice(&bytes).unwrap();
		}
		*SLATE_COUNTER.lock() += 1;
	}
	slate.amount = amount;
	slate.height = current_height;

	if valid_header_version(current_height, HeaderVersion(6)) {
		slate.version_info.block_header_version = 6;
	}

	if valid_header_version(current_height, HeaderVersion(7)) {
		slate.version_info.block_header_version = 7;
	}

	// Set the lock_height explicitly to 0 here.
	// This will generate a Plain kernel (rather than a HeightLocked kernel).
	slate.lock_height = 0;

	Ok(slate)
}

/// Estimates locked amount and fee for the transaction without creating one
pub fn estimate_send_tx<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	amount: u64,
	minimum_confirmations: u64,
	max_outputs: usize,
	num_change_outputs: usize,
	selection_strategy_is_use_all: bool,
	parent_key_id: &Identifier,
) -> Result<
	(
		u64, // total
		u64, // fee
	),
	Error,
>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	// Get lock height
	let current_height = wallet.w2n_client().get_chain_tip()?.0;
	// ensure outputs we're selecting are up to date
	updater::refresh_outputs(wallet, keychain_mask, parent_key_id, false)?;

	// Sender selects outputs into a new slate and save our corresponding keys in
	// a transaction context. The secret key in our transaction context will be
	// randomly selected. This returns the public slate, and a closure that locks
	// our inputs and outputs once we're convinced the transaction exchange went
	// according to plan
	// This function is just a big helper to do all of that, in theory
	// this process can be split up in any way
	let (_coins, total, _amount, fee) = selection::select_coins_and_fee(
		wallet,
		amount,
		current_height,
		minimum_confirmations,
		max_outputs,
		num_change_outputs,
		selection_strategy_is_use_all,
		parent_key_id,
	)?;
	Ok((total, fee))
}

/// Add inputs to the slate (effectively becoming the sender)
pub fn add_inputs_to_slate<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	minimum_confirmations: u64,
	max_outputs: usize,
	num_change_outputs: usize,
	selection_strategy_is_use_all: bool,
	parent_key_id: &Identifier,
	participant_id: usize,
	message: Option<String>,
	is_initator: bool,
	use_test_rng: bool,
) -> Result<Context, Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	// sender should always refresh outputs
	updater::refresh_outputs(wallet, keychain_mask, parent_key_id, false)?;

	// Sender selects outputs into a new slate and save our corresponding keys in
	// a transaction context. The secret key in our transaction context will be
	// randomly selected. This returns the public slate, and a closure that locks
	// our inputs and outputs once we're convinced the transaction exchange went
	// according to plan
	// This function is just a big helper to do all of that, in theory
	// this process can be split up in any way
	let mut context = selection::build_send_tx(
		wallet,
		&wallet.keychain(keychain_mask)?,
		keychain_mask,
		slate,
		minimum_confirmations,
		max_outputs,
		num_change_outputs,
		selection_strategy_is_use_all,
		parent_key_id.clone(),
		use_test_rng,
	)?;

	// Generate a kernel offset and subtract from our context's secret key. Store
	// the offset in the slate's transaction kernel, and adds our public key
	// information to the slate
	let _ = slate.fill_round_1(
		&wallet.keychain(keychain_mask)?,
		&mut context.sec_key,
		&context.sec_nonce,
		participant_id,
		message,
		use_test_rng,
	)?;

	if !is_initator {
		// perform partial sig
		let _ = slate.fill_round_2(
			&wallet.keychain(keychain_mask)?,
			&context.sec_key,
			&context.sec_nonce,
			participant_id,
		)?;
	}

	Ok(context)
}

/// Add receiver output to the slate
pub fn add_output_to_slate<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	parent_key_id: &Identifier,
	participant_id: usize,
	message: Option<String>,
	is_initiator: bool,
	use_test_rng: bool,
) -> Result<Context, Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	// create an output using the amount in the slate
	let (_, mut context) = selection::build_recipient_output(
		wallet,
		keychain_mask,
		slate,
		parent_key_id.clone(),
		use_test_rng,
	)?;

	// fill public keys
	let _ = slate.fill_round_1(
		&wallet.keychain(keychain_mask)?,
		&mut context.sec_key,
		&context.sec_nonce,
		1,
		message,
		use_test_rng,
	)?;

	if !is_initiator {
		// perform partial sig
		let _ = slate.fill_round_2(
			&wallet.keychain(keychain_mask)?,
			&context.sec_key,
			&context.sec_nonce,
			participant_id,
		)?;
	}

	Ok(context)
}

/// Complete a transaction
pub fn complete_tx<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	participant_id: usize,
	context: &Context,
) -> Result<(), Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let _ = slate.fill_round_2(
		&wallet.keychain(keychain_mask)?,
		&context.sec_key,
		&context.sec_nonce,
		participant_id,
	)?;

	// Final transaction can be built by anyone at this stage
	slate.finalize(&wallet.keychain(keychain_mask)?)?;
	Ok(())
}

/// Rollback outputs associated with a transaction in the wallet
pub fn cancel_tx<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	parent_key_id: &Identifier,
	tx_id: Option<u32>,
	tx_slate_id: Option<Uuid>,
) -> Result<(), Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let mut tx_id_string = String::new();
	if let Some(tx_id) = tx_id {
		tx_id_string = tx_id.to_string();
	} else if let Some(tx_slate_id) = tx_slate_id {
		tx_id_string = tx_slate_id.to_string();
	}
	let tx_vec = updater::retrieve_txs(wallet, tx_id, tx_slate_id, Some(&parent_key_id), false)?;
	if tx_vec.len() != 1 {
		return Err(ErrorKind::TransactionDoesntExist(tx_id_string))?;
	}
	let tx = tx_vec[0].clone();
	if tx.tx_type != TxLogEntryType::TxSent && tx.tx_type != TxLogEntryType::TxReceived {
		return Err(ErrorKind::TransactionNotCancellable(tx_id_string))?;
	}
	if tx.confirmed == true {
		return Err(ErrorKind::TransactionNotCancellable(tx_id_string))?;
	}
	// get outputs associated with tx
	let res = updater::retrieve_outputs(
		wallet,
		keychain_mask,
		false,
		Some(tx.id),
		Some(&parent_key_id),
	)?;
	let outputs = res.iter().map(|m| m.output.clone()).collect();
	updater::cancel_tx_and_outputs(wallet, keychain_mask, tx, outputs, parent_key_id)?;
	Ok(())
}

/// Update the stored transaction (this update needs to happen when the TX is finalised)
pub fn update_stored_tx<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	context: &Context,
	slate: &Slate,
	is_invoiced: bool,
) -> Result<(), Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	// finalize command
	let tx_vec = updater::retrieve_txs(wallet, None, Some(slate.id), None, false)?;
	let mut tx = None;
	// don't want to assume this is the right tx, in case of self-sending
	for t in tx_vec {
		if t.tx_type == TxLogEntryType::TxSent && !is_invoiced {
			tx = Some(t.clone());
			break;
		}
		if t.tx_type == TxLogEntryType::TxReceived && is_invoiced {
			tx = Some(t.clone());
			break;
		}
	}
	let mut tx = match tx {
		Some(t) => t,
		None => return Err(ErrorKind::TransactionDoesntExist(slate.id.to_string()))?,
	};
	wallet.store_tx(&format!("{}", tx.tx_slate_id.unwrap()), &slate.tx)?;
	let parent_key = tx.parent_key_id.clone();
	tx.kernel_excess = Some(slate.tx.body.kernels[0].excess);

	if let Some(ref p) = slate.payment_proof {
		let derivation_index = match context.payment_proof_derivation_index {
			Some(i) => i,
			None => 0,
		};
		let keychain = wallet.keychain(keychain_mask)?;
		let parent_key_id = wallet.parent_key_id();
		let excess = slate.calc_excess(&keychain)?;
		let sender_key =
			address::address_from_derivation_path(&keychain, &parent_key_id, derivation_index)?;
		let sender_address = address::ed25519_keypair(&sender_key)?.1;
		let sig =
			create_payment_proof_signature(slate.amount, &excess, p.sender_address, sender_key)?;
		tx.payment_proof = Some(StoredProofInfo {
			receiver_address: p.receiver_address,
			receiver_signature: p.receiver_signature,
			sender_address_path: derivation_index,
			sender_address,
			sender_signature: Some(sig),
		})
	}

	let mut batch = wallet.batch(keychain_mask)?;
	batch.save_tx_log_entry(tx, &parent_key)?;
	batch.commit()?;
	Ok(())
}

/// Update the transaction participant messages
pub fn update_message<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	slate: &Slate,
) -> Result<(), Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let tx_vec = updater::retrieve_txs(wallet, None, Some(slate.id), None, false)?;
	if tx_vec.is_empty() {
		return Err(ErrorKind::TransactionDoesntExist(slate.id.to_string()))?;
	}
	let mut batch = wallet.batch(keychain_mask)?;
	for mut tx in tx_vec.into_iter() {
		tx.messages = Some(slate.participant_messages());
		let parent_key = tx.parent_key_id.clone();
		batch.save_tx_log_entry(tx, &parent_key)?;
	}
	batch.commit()?;
	Ok(())
}

pub fn payment_proof_message(
	amount: u64,
	kernel_commitment: &pedersen::Commitment,
	sender_address: DalekPublicKey,
) -> Result<Vec<u8>, Error> {
	let mut msg = Vec::new();
	msg.write_u64::<BigEndian>(amount)?;
	msg.append(&mut kernel_commitment.0.to_vec());
	msg.append(&mut sender_address.to_bytes().to_vec());
	Ok(msg)
}

pub fn _decode_payment_proof_message(
	msg: &Vec<u8>,
) -> Result<(u64, pedersen::Commitment, DalekPublicKey), Error> {
	let mut rdr = Cursor::new(msg);
	let amount = rdr.read_u64::<BigEndian>()?;
	let mut commit_bytes = [0u8; 33];
	for i in 0..33 {
		commit_bytes[i] = rdr.read_u8()?;
	}
	let mut sender_address_bytes = [0u8; 32];
	for i in 0..32 {
		sender_address_bytes[i] = rdr.read_u8()?;
	}

	Ok((
		amount,
		pedersen::Commitment::from_vec(commit_bytes.to_vec()),
		DalekPublicKey::from_bytes(&sender_address_bytes).unwrap(),
	))
}

/// create a payment proof
pub fn create_payment_proof_signature(
	amount: u64,
	kernel_commitment: &pedersen::Commitment,
	sender_address: DalekPublicKey,
	sec_key: SecretKey,
) -> Result<DalekSignature, Error> {
	let msg = payment_proof_message(amount, kernel_commitment, sender_address)?;
	let d_skey = match DalekSecretKey::from_bytes(&sec_key.0) {
		Ok(k) => k,
		Err(e) => {
			return Err(ErrorKind::ED25519Key(format!("{}", e)).to_owned())?;
		}
	};
	let pub_key: DalekPublicKey = (&d_skey).into();
	let keypair = DalekKeypair {
		public: pub_key,
		secret: d_skey,
	};
	Ok(keypair.sign(&msg))
}

/// Verify all aspects of a completed payment proof on the current slate
pub fn verify_slate_payment_proof<'a, T: ?Sized, C, K>(
	wallet: &mut T,
	keychain_mask: Option<&SecretKey>,
	parent_key_id: &Identifier,
	context: &Context,
	slate: &Slate,
) -> Result<(), Error>
where
	T: WalletBackend<'a, C, K>,
	C: NodeClient + 'a,
	K: Keychain + 'a,
{
	let tx_vec = updater::retrieve_txs(wallet, None, Some(slate.id), Some(parent_key_id), false)?;
	if tx_vec.len() == 0 {
		return Err(ErrorKind::PaymentProof(
			"TxLogEntry with original proof info not found (is account correct?)".to_owned(),
		))?;
	}

	let orig_proof_info = tx_vec[0].clone().payment_proof;

	if orig_proof_info.is_some() && slate.payment_proof.is_none() {
		return Err(ErrorKind::PaymentProof(
			"Expected Payment Proof for this Transaction is not present".to_owned(),
		))?;
	}

	if let Some(ref p) = slate.payment_proof {
		let orig_proof_info = match orig_proof_info {
			Some(p) => p,
			None => {
				return Err(ErrorKind::PaymentProof(
					"Original proof info not stored in tx".to_owned(),
				))?;
			}
		};
		let keychain = wallet.keychain(keychain_mask)?;
		let index = match context.payment_proof_derivation_index {
			Some(i) => i,
			None => {
				return Err(ErrorKind::PaymentProof(
					"Payment proof derivation index required".to_owned(),
				))?;
			}
		};
		let orig_sender_sk =
			address::address_from_derivation_path(&keychain, parent_key_id, index)?;
		let orig_sender_address = address::ed25519_keypair(&orig_sender_sk)?.1;
		if p.sender_address != orig_sender_address {
			return Err(ErrorKind::PaymentProof(
				"Sender address on slate does not match original sender address".to_owned(),
			))?;
		}

		if orig_proof_info.receiver_address != p.receiver_address {
			return Err(ErrorKind::PaymentProof(
				"Recipient address on slate does not match original recipient address".to_owned(),
			))?;
		}
		let msg = payment_proof_message(
			slate.amount,
			&slate.calc_excess(&keychain)?,
			orig_sender_address,
		)?;
		let sig = match p.receiver_signature {
			Some(s) => s,
			None => {
				return Err(ErrorKind::PaymentProof(
					"Recipient did not provide requested proof signature".to_owned(),
				))?;
			}
		};

		if let Err(_) = p.receiver_address.verify(&msg, &sig) {
			return Err(ErrorKind::PaymentProof(
				"Invalid proof signature".to_owned(),
			))?;
		};
	}
	Ok(())
}

#[cfg(test)]
mod test {
	use super::*;
	use rand::rngs::mock::StepRng;

	use crate::epic_core::core::KernelFeatures;
	use crate::epic_core::libtx::{build, ProofBuilder};
	use crate::epic_keychain::{
		BlindSum, BlindingFactor, ExtKeychain, ExtKeychainPath, Keychain, SwitchCommitmentType,
	};
	use crate::epic_util::{secp, static_secp_instance};

	#[test]
	// demonstrate that input.commitment == referenced output.commitment
	// based on the public key and amount begin spent
	fn output_commitment_equals_input_commitment_on_spend() {
		let keychain = ExtKeychain::from_random_seed(false).unwrap();
		let builder = ProofBuilder::new(&keychain);
		let key_id1 = ExtKeychainPath::new(1, 1, 0, 0, 0).to_identifier();

		let tx1 = build::transaction(
			KernelFeatures::Plain { fee: 0 },
			vec![build::output(105, key_id1.clone())],
			&keychain,
			&builder,
		)
		.unwrap();
		let tx2 = build::transaction(
			KernelFeatures::Plain { fee: 0 },
			vec![build::input(105, key_id1.clone())],
			&keychain,
			&builder,
		)
		.unwrap();

		assert_eq!(tx1.outputs()[0].features, tx2.inputs()[0].features);
		assert_eq!(tx1.outputs()[0].commitment(), tx2.inputs()[0].commitment());
	}

	#[test]
	fn payment_proof_construction() {
		let secp_inst = static_secp_instance();
		let secp = secp_inst.lock();
		let mut test_rng = StepRng::new(1234567890u64, 1);
		let sec_key = secp::key::SecretKey::new(&secp, &mut test_rng);
		let d_skey = DalekSecretKey::from_bytes(&sec_key.0).unwrap();

		let address: DalekPublicKey = (&d_skey).into();

		let kernel_excess = {
			ExtKeychainPath::new(1, 1, 0, 0, 0).to_identifier();
			let keychain = ExtKeychain::from_random_seed(true).unwrap();
			let switch = &SwitchCommitmentType::Regular;
			let id1 = ExtKeychain::derive_key_id(1, 1, 0, 0, 0);
			let id2 = ExtKeychain::derive_key_id(1, 2, 0, 0, 0);
			let skey1 = keychain.derive_key(0, &id1, switch).unwrap();
			let skey2 = keychain.derive_key(0, &id2, switch).unwrap();
			let blinding_factor = keychain
				.blind_sum(
					&BlindSum::new()
						.sub_blinding_factor(BlindingFactor::from_secret_key(skey1))
						.add_blinding_factor(BlindingFactor::from_secret_key(skey2)),
				)
				.unwrap();
			keychain
				.secp()
				.commit(0, blinding_factor.secret_key(&keychain.secp()).unwrap())
				.unwrap()
		};

		let amount = 12345678u64;
		let msg = payment_proof_message(amount, &kernel_excess, address).unwrap();
		println!("payment proof message is (len {}): {:?}", msg.len(), msg);

		let decoded = _decode_payment_proof_message(&msg).unwrap();
		assert_eq!(decoded.0, amount);
		assert_eq!(decoded.1, kernel_excess);
		assert_eq!(decoded.2, address);

		let sig = create_payment_proof_signature(amount, &kernel_excess, address, sec_key).unwrap();

		assert!(address.verify(&msg, &sig).is_ok());
	}
}
