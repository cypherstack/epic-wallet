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

//! Types specific to the wallet api, mostly argument serialization

use crate::epic_core::libtx::secp_ser;
use crate::epic_keychain::Identifier;
use crate::epic_util::secp::pedersen;
use crate::slate_versions::ser as dalek_ser;
use crate::slate_versions::SlateVersion;
use crate::types::OutputData;

use ed25519_dalek::PublicKey as DalekPublicKey;
use ed25519_dalek::Signature as DalekSignature;

pub use crate::epic_core::core::block_fees::BlockFees;
/// Send TX API Args
// TODO: This is here to ensure the legacy V1 API remains intact
// remove this when v1 api is removed
#[derive(Clone, Serialize, Deserialize)]
pub struct SendTXArgs {
	/// amount to send
	pub amount: u64,
	/// minimum confirmations
	pub minimum_confirmations: u64,
	/// payment method
	pub method: String,
	/// destination url
	pub dest: String,
	/// Max number of outputs
	pub max_outputs: usize,
	/// Number of change outputs to generate
	pub num_change_outputs: usize,
	/// whether to use all outputs (combine)
	pub selection_strategy_is_use_all: bool,
	/// Optional message, that will be signed
	pub message: Option<String>,
	/// Optional slate version to target when sending
	pub target_slate_version: Option<u16>,
}

/// V2 Init / Send TX API Args
#[derive(Clone, Serialize, Deserialize)]
pub struct InitTxArgs {
	/// The human readable account name from which to draw outputs
	/// for the transaction, overriding whatever the active account is as set via the
	/// [`set_active_account`](../epic_wallet_api/owner/struct.Owner.html#method.set_active_account) method.
	pub src_acct_name: Option<String>,
	#[serde(with = "secp_ser::string_or_u64")]
	/// The amount to send, in nanoepics. (`1 G = 1_000_000_000nG`)
	pub amount: u64,
	#[serde(with = "secp_ser::string_or_u64")]
	/// The minimum number of confirmations an output
	/// should have in order to be included in the transaction.
	pub minimum_confirmations: u64,
	/// By default, the wallet selects as many inputs as possible in a
	/// transaction, to reduce the Output set and the fees. The wallet will attempt to spend
	/// include up to `max_outputs` in a transaction, however if this is not enough to cover
	/// the whole amount, the wallet will include more outputs. This parameter should be considered
	/// a soft limit.
	pub max_outputs: u32,
	/// The target number of change outputs to create in the transaction.
	/// The actual number created will be `num_change_outputs` + whatever remainder is needed.
	pub num_change_outputs: u32,
	/// If `true`, attempt to use up as many outputs as
	/// possible to create the transaction, up the 'soft limit' of `max_outputs`. This helps
	/// to reduce the size of the UTXO set and the amount of data stored in the wallet, and
	/// minimizes fees. This will generally result in many inputs and a large change output(s),
	/// usually much larger than the amount being sent. If `false`, the transaction will include
	/// as many outputs as are needed to meet the amount, (and no more) starting with the smallest
	/// value outputs.
	pub selection_strategy_is_use_all: bool,
	/// An optional participant message to include alongside the sender's public
	/// ParticipantData within the slate. This message will include a signature created with the
	/// sender's private excess value, and will be publically verifiable. Note this message is for
	/// the convenience of the participants during the exchange; it is not included in the final
	/// transaction sent to the chain. The message will be truncated to 256 characters.
	pub message: Option<String>,
	/// Optionally set the output target slate version (acceptable
	/// down to the minimum slate version compatible with the current. If `None` the slate
	/// is generated with the latest version.
	pub target_slate_version: Option<u16>,
	/// Number of blocks from current after which TX should be ignored
	#[serde(with = "secp_ser::opt_string_or_u64")]
	pub ttl_blocks: Option<u64>,
	/// If set, require a payment proof for the particular recipient
	#[serde(with = "dalek_ser::option_dalek_pubkey_serde")]
	pub payment_proof_recipient_address: Option<DalekPublicKey>,
	/// If true, just return an estimate of the resulting slate, containing fees and amounts
	/// locked without actually locking outputs or creating the transaction. Note if this is set to
	/// 'true', the amount field in the slate will contain the total amount locked, not the provided
	/// transaction amount
	pub estimate_only: Option<bool>,
	/// Sender arguments. If present, the underlying function will also attempt to send the
	/// transaction to a destination and optionally finalize the result
	pub send_args: Option<InitTxSendArgs>,
}

/// Send TX API Args, for convenience functionality that inits the transaction and sends
/// in one go
#[derive(Clone, Serialize, Deserialize)]
pub struct InitTxSendArgs {
	/// The transaction method. Can currently be 'http' or 'keybase'.
	pub method: String,
	/// The destination, contents will depend on the particular method
	pub dest: String,
	/// Whether to finalize the result immediately if the send was successful
	pub finalize: bool,
	/// Whether to post the transasction if the send and finalize were successful
	pub post_tx: bool,
	/// Whether to use dandelion when posting. If false, skip the dandelion relay
	pub fluff: bool,
}

impl Default for InitTxArgs {
	fn default() -> InitTxArgs {
		InitTxArgs {
			src_acct_name: None,
			amount: 0,
			minimum_confirmations: 10,
			max_outputs: 500,
			num_change_outputs: 1,
			selection_strategy_is_use_all: true,
			message: None,
			target_slate_version: None,
			ttl_blocks: None,
			estimate_only: Some(false),
			payment_proof_recipient_address: None,
			send_args: None,
		}
	}
}

/// V2 Issue Invoice Tx Args
#[derive(Clone, Serialize, Deserialize)]
pub struct IssueInvoiceTxArgs {
	/// The human readable account name to which the received funds should be added
	/// overriding whatever the active account is as set via the
	/// [`set_active_account`](../epic_wallet_api/owner/struct.Owner.html#method.set_active_account) method.
	pub dest_acct_name: Option<String>,
	/// The invoice amount in nanoepics. (`1 G = 1_000_000_000nG`)
	#[serde(with = "secp_ser::string_or_u64")]
	pub amount: u64,
	/// Optional message, that will be signed
	pub message: Option<String>,
	/// Optionally set the output target slate version (acceptable
	/// down to the minimum slate version compatible with the current. If `None` the slate
	/// is generated with the latest version.
	pub target_slate_version: Option<u16>,
}

impl Default for IssueInvoiceTxArgs {
	fn default() -> IssueInvoiceTxArgs {
		IssueInvoiceTxArgs {
			dest_acct_name: None,
			amount: 0,
			message: None,
			target_slate_version: None,
		}
	}
}

/// Map Outputdata to commits
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OutputCommitMapping {
	/// Output Data
	pub output: OutputData,
	/// The commit
	#[serde(
		serialize_with = "secp_ser::as_hex",
		deserialize_with = "secp_ser::commitment_from_hex"
	)]
	pub commit: pedersen::Commitment,
}

/// Node height result
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NodeHeightResult {
	/// Last known height
	#[serde(with = "secp_ser::string_or_u64")]
	pub height: u64,
	/// Hash
	pub header_hash: String,
	/// Whether this height was updated from the node
	pub updated_from_node: bool,
}

/// Version request result
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VersionInfo {
	/// API version
	pub foreign_api_version: u16,
	/// Slate version
	pub supported_slate_versions: Vec<SlateVersion>,
}

/// Packaged Payment Proof
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PaymentProof {
	/// Amount
	#[serde(with = "secp_ser::string_or_u64")]
	pub amount: u64,
	/// Kernel Excess
	#[serde(
		serialize_with = "secp_ser::as_hex",
		deserialize_with = "secp_ser::commitment_from_hex"
	)]
	pub excess: pedersen::Commitment,
	/// Recipient Wallet Address (Onion V3)
	pub recipient_address: String,
	/// Recipient Signature
	#[serde(with = "dalek_ser::dalek_sig_serde")]
	pub recipient_sig: DalekSignature,
	/// Sender Wallet Address (Onion V3)
	pub sender_address: String,
	/// Sender Signature
	#[serde(with = "dalek_ser::dalek_sig_serde")]
	pub sender_sig: DalekSignature,
}
