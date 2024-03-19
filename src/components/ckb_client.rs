//! Expand the functionality of the original CKB RPC client.

use ckb_jsonrpc_types::TransactionView;
use ckb_sdk::rpc::CkbRpcClient;
use ckb_types::{packed, prelude::*, H256};

use crate::result::Result;

pub trait CkbRpcClientExtension {
    fn send_transaction_ext(&self, tx_json: TransactionView, dry_run: bool) -> Result<H256>;
}

impl CkbRpcClientExtension for CkbRpcClient {
    fn send_transaction_ext(&self, tx_json: TransactionView, dry_run: bool) -> Result<H256> {
        if log::log_enabled!(log::Level::Trace) {
            match serde_json::to_string_pretty(&tx_json) {
                Ok(tx_json_str) => {
                    log::trace!("transaction: {tx_json_str}")
                }
                Err(err) => {
                    log::warn!("failed to convert the transaction into json string since {err}")
                }
            }
        }

        let tx: packed::Transaction = tx_json.inner.clone().into();
        let tx_hash = tx.calc_tx_hash().unpack();

        if log::log_enabled!(log::Level::Debug) {
            let cycles: u64 = self.estimate_cycles(tx_json.inner.clone())?.cycles.into();
            log::debug!("Estimated cycles for {tx_hash:#x}: {cycles}");
        }

        if !dry_run {
            let tx_hash = self.send_transaction(tx_json.inner, None)?;
            log::info!("Transaction hash: {tx_hash:#x}.");
            println!("Send transaction: {tx_hash:#x}");
        }

        Ok(tx_hash)
    }
}
