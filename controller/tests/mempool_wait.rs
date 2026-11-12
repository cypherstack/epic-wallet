// Copyright 2019 The Epic Developers
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

//! Test that waiting for a posted tx reports where it was seen, and stops once the tx is mined
#[macro_use]
extern crate log;
extern crate epic_wallet_controller as wallet;
extern crate epic_wallet_impls as impls;

use epic_wallet_libwallet as libwallet;
use impls::test_framework::{self, LocalWalletClient};
use libwallet::api_impl::owner::PostedTxStatus;
use libwallet::{InitTxArgs, Slate, TxLogEntryType};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
#[macro_use]
mod common;
use common::{clean_output_dir, create_wallet_proxy, setup};

fn mempool_wait_test_impl(test_dir: &'static str) -> Result<(), libwallet::Error> {
    let mut wallet_proxy = create_wallet_proxy(test_dir);
    let chain = wallet_proxy.chain.clone();
    let chain_tip_failures = wallet_proxy.chain_tip_failures.clone();

    create_wallet_and_add!(
        client1,
        wallet1,
        mask1_i,
        test_dir,
        "wallet1",
        None,
        &mut wallet_proxy,
        true
    );
    let mask1 = (&mask1_i).as_ref();

    thread::spawn(move || {
        if let Err(e) = wallet_proxy.run() {
            error!("Wallet Proxy error: {}", e);
        }
    });

    let is_node_synced = Arc::new(AtomicBool::new(true));
    wallet::controller::owner_single_use(
        wallet1.clone(),
        mask1,
        |api, m| {
            api.create_account_path(m, "listener")?;
            Ok(())
        },
        is_node_synced.clone(),
    )?;
    let _ = test_framework::award_blocks_to_wallet(&chain, wallet1.clone(), mask1, 10, false);

    // Self send from the default account to "listener"; the test node mines a
    // block for every posted tx and reports the last posted tx as its pool.
    let self_send = |amount: u64| -> Result<Slate, libwallet::Error> {
        let mut slate = Slate::blank(1);
        wallet::controller::owner_single_use(
            wallet1.clone(),
            mask1,
            |api, m| {
                let args = InitTxArgs {
                    src_acct_name: None,
                    amount,
                    minimum_confirmations: 2,
                    max_outputs: 500,
                    num_change_outputs: 1,
                    selection_strategy_is_use_all: false,
                    ..Default::default()
                };
                slate = api.init_send_tx(m, args, is_node_synced.clone())?;
                api.tx_lock_outputs(m, &slate, 0, None)?;
                wallet::controller::foreign_single_use(wallet1.clone(), mask1_i.clone(), |api| {
                    slate = api.receive_tx(&slate, Some("listener"), None, None)?;
                    Ok(())
                })?;
                slate = api.finalize_tx(m, &slate)?;
                api.post_tx(m, &slate.tx, false)?;
                Ok(())
            },
            is_node_synced.clone(),
        )?;
        Ok(slate)
    };

    let mined = self_send(1_000_000_000)?;
    let in_pool = self_send(2_000_000_000)?;

    wallet::controller::owner_single_use(
        wallet1.clone(),
        mask1,
        |api, m| {
            // The sent entry of a slate, as the default account stores it.
            let sent_type = |slate: &Slate| -> Result<TxLogEntryType, libwallet::Error> {
                let txs = api
                    .retrieve_txs(m, false, None, Some(slate.id), None, None, None)?
                    .txs;
                let sent: Vec<_> = txs
                    .iter()
                    .filter(|t| t.amount_debited > 0)
                    .map(|t| t.tx_type.clone())
                    .collect();
                assert_eq!(sent.len(), 1);
                Ok(sent[0].clone())
            };
            let mined_before = sent_type(&mined)?;
            assert_ne!(mined_before, TxLogEntryType::TxSentMempool);
            assert_ne!(sent_type(&in_pool)?, TxLogEntryType::TxSentMempool);

            // The first tx is on chain and no longer in the pool: the wait must
            // say so straight away instead of polling for all 30 attempts.
            let start = Instant::now();
            let status = api.wait_for_posted_tx(m, &mined.id, 1, 30)?;
            assert_eq!(status, PostedTxStatus::OnChain);
            assert!(start.elapsed() < Duration::from_secs(5));
            // It was never seen in the pool, so its entry is left as it was.
            assert_eq!(sent_type(&mined)?, mined_before);

            // One failed chain tip request costs one poll, not the whole wait.
            chain_tip_failures.store(1, Ordering::Relaxed);
            let start = Instant::now();
            let status = api.wait_for_posted_tx(m, &mined.id, 1, 30)?;
            assert_eq!(status, PostedTxStatus::OnChain);
            assert_eq!(chain_tip_failures.load(Ordering::Relaxed), 0);
            assert!(start.elapsed() < Duration::from_secs(5));

            // The older call cannot tell "on chain" from "not found".
            assert!(!api.wait_for_tx_in_mempool(m, &mined.id, 1, 30)?);

            // The second tx is what the test node reports as its pool.
            let status = api.wait_for_posted_tx(m, &in_pool.id, 1, 30)?;
            assert_eq!(status, PostedTxStatus::InMempool);
            assert_eq!(sent_type(&in_pool)?, TxLogEntryType::TxSentMempool);
            assert!(api.wait_for_tx_in_mempool(m, &in_pool.id, 1, 30)?);
            Ok(())
        },
        is_node_synced.clone(),
    )?;

    thread::sleep(Duration::from_millis(200));
    Ok(())
}

#[test]
fn wallet_mempool_wait_stops_when_mined() {
    let test_dir = "test_output/mempool_wait";
    setup(test_dir);
    if let Err(e) = mempool_wait_test_impl(test_dir) {
        panic!("Libwallet Error: {}", e);
    }
    clean_output_dir(test_dir);
}
