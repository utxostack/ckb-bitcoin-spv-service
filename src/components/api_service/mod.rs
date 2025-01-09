//! JSON-RPC APIs service.

use std::{net::SocketAddr, sync::RwLock, time::SystemTime};

use bitcoin::Txid;
use ckb_bitcoin_spv_verifier::types::{
    core::{Bytes, Hash},
    packed,
    prelude::*,
};
use ckb_jsonrpc_types::{JsonBytes, OutPoint};
use jsonrpc_core::{Error as RpcError, ErrorCode as RpcErrorCode, IoHandler, Result as RpcResult};
use jsonrpc_derive::rpc;
use jsonrpc_http_server::{Server, ServerBuilder};
use jsonrpc_server_utils::{cors::AccessControlAllowOrigin, hosts::DomainsValidation};
use serde::Serialize;

use crate::{
    components::{SpvInstance, SpvService},
    prelude::*,
    result::{Error, Result},
};

mod error;

pub use error::ApiErrorCode;

// Bitcoin target block time is 10 minutes.
const SPV_INSTANCE_CACHED_SECS: u64 = 60 * 10;

pub struct ApiServiceConfig {
    listen_address: SocketAddr,
}

#[derive(Serialize, Clone)]
pub struct BitcoinTxProof {
    pub(crate) spv_client: OutPoint,
    pub(crate) proof: JsonBytes,
}

#[rpc(server)]
pub trait SpvRpc {
    #[rpc(name = "getTxProof")]
    fn get_tx_proof(
        &self,
        tx_hash: Txid,
        tx_index: u32,
        confirmations: u32,
    ) -> RpcResult<BitcoinTxProof>;
}

pub struct SpvRpcImpl {
    spv_service: SpvService,
    cached_spv_instance: RwLock<Option<CachedSpvInstance>>,
}

#[derive(Clone)]
struct CachedSpvInstance {
    instance: SpvInstance,
    expired_timestamp: u64,
}

impl ApiServiceConfig {
    pub fn new(listen_address: SocketAddr) -> Self {
        Self { listen_address }
    }

    pub fn start(&self, spv_service: SpvService) -> Result<Server> {
        log::info!("Starting the JSON-RPC service ...");
        let mut io_handler = IoHandler::new();
        let spv_rpc_impl = SpvRpcImpl::new(spv_service);
        io_handler.extend_with(spv_rpc_impl.to_delegate());

        ServerBuilder::new(io_handler)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ]))
            .health_api(("/ping", "ping"))
            .start_http(&self.listen_address)
            .map_err(Error::other)
    }
}

impl SpvRpcImpl {
    pub fn new(spv_service: SpvService) -> Self {
        Self {
            spv_service,
            cached_spv_instance: RwLock::new(None),
        }
    }

    fn load_spv_instance(
        &self,
        stg_tip_height: u32,
        target_confirmed_height: u32,
    ) -> Result<Option<SpvInstance>> {
        let spv_instance = match self.load_cache_spv_instance() {
            Some(instance) => instance,
            None => return Ok(None),
        };

        let spv_client_cell =
            spv_instance.find_best_spv_client_not_greater_than_height(stg_tip_height)?;

        let spv_header_root = &spv_client_cell.client.headers_mmr_root;
        let spv_best_height = spv_header_root.max_height;

        if spv_best_height < target_confirmed_height {
            return Ok(None);
        }

        Ok(Some(spv_instance))
    }

    fn load_cache_spv_instance(&self) -> Option<SpvInstance> {
        if let Some(cached) = self
            .cached_spv_instance
            .read()
            .ok()
            .and_then(|locked| locked.as_ref().cloned())
        {
            if let Ok(dur) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                let current_timestamp = dur.as_secs();
                if current_timestamp > cached.expired_timestamp {
                    log::trace!(
                        "cached SPV instance is expired, expired at {}, now is {current_timestamp}",
                        cached.expired_timestamp
                    );
                    None
                } else {
                    log::trace!(
                        "cached SPV instance is loaded, will be expired at {}, now is {current_timestamp}",
                        cached.expired_timestamp
                    );
                    Some(cached.instance)
                }
            } else {
                log::warn!("failed to read current timestamp for load the cached SPV instance");
                None
            }
        } else {
            log::debug!("failed to load cached SPV instance: not existed or lock error");
            None
        }
    }

    fn update_spv_instance(&self, instance: SpvInstance) {
        match self.cached_spv_instance.write() {
            Ok(mut locked) => {
                if let Ok(dur) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                    let current_timestamp = dur.as_secs();
                    let expired_timestamp = current_timestamp + SPV_INSTANCE_CACHED_SECS;
                    let cache = CachedSpvInstance {
                        instance,
                        expired_timestamp,
                    };
                    *locked = Some(cache);
                    log::debug!(
                        "refreshed the cached SPV instance, it will be expired at {expired_timestamp}, \
                        now is {current_timestamp}",
                    );
                } else {
                    log::warn!(
                        "failed to read current timestamp for update the cached SPV instance"
                    );
                }
            }
            Err(err) => {
                log::debug!("failed to update the cached SPV instance since {err}");
            }
        }
    }
}

impl SpvRpc for SpvRpcImpl {
    fn get_tx_proof(
        &self,
        txid: Txid,
        tx_index: u32,
        confirmations: u32,
    ) -> RpcResult<BitcoinTxProof> {
        log::debug!("Call getTxProof with params [{txid:#x}, {confirmations}]");
        let spv = &self.spv_service;

        let (target_height, target_hash, raw_tx_out_proof) =
            tokio::task::block_in_place(|| -> RpcResult<(u32, Hash, Vec<u8>)> {
                let (merkle_block, raw_tx_out_proof) =
                    spv.btc_cli.get_tx_out_proof(txid).map_err(|err| {
                        let message =
                            format!("failed to get tx out proof for {txid:#x} from remote");
                        log::error!("{message} since {err}");
                        RpcError {
                            code: RpcErrorCode::InternalError,
                            message,
                            data: None,
                        }
                    })?;
                let block_hash = merkle_block.header.block_hash();
                log::trace!(">>> the input tx in header {block_hash:#x}");
                let block_height = spv.btc_cli.get_block_height(block_hash).map_err(|err| {
                    let message =
                        format!("failed to get block height for {block_hash:#x} from remote");
                    log::error!("{message} since {err}");
                    RpcError {
                        code: RpcErrorCode::InternalError,
                        message,
                        data: None,
                    }
                })?;
                log::trace!(">>> the input tx in header {block_height}");
                Ok((block_height, block_hash.into(), raw_tx_out_proof))
            })?;

        let (stg_tip_height, _) = spv.storage.tip_state().map_err(|err| {
            let message = "failed to read tip bitcoin height from local storage".to_owned();
            log::error!("{message} since {err}");
            RpcError {
                code: RpcErrorCode::InternalError,
                message,
                data: None,
            }
        })?;
        log::debug!(">>> tip height in local storage is {stg_tip_height}");

        if stg_tip_height < target_height {
            let desc = format!(
                "target transaction is in header#{target_height}, \
                but the tip header in local storage is header#{stg_tip_height}"
            );
            return Err(ApiErrorCode::StorageTxTooNew.with_desc(desc));
        }
        if stg_tip_height < target_height + confirmations {
            let desc = format!(
                "target transaction is in header#{target_height} \
                and it requires {confirmations} confirmations, \
                but the tip header in local storage is header#{stg_tip_height}"
            );
            return Err(ApiErrorCode::StorageTxUnconfirmed.with_desc(desc));
        }
        let stg_target_hash = spv
            .storage
            .bitcoin_header_hash(target_height)
            .map_err(|err| {
                let desc = format!("local storage doesn't have header#{target_height}");
                log::error!("{desc} since {err}");
                ApiErrorCode::StorageHeaderMissing.with_desc(desc)
            })?;
        if target_hash != stg_target_hash {
            let desc = format!(
                "target transaction is in header#{target_height}, \
                the header hash from remote is {target_hash:#x}, \
                its hash in local storage is {stg_target_hash:#x}"
            );
            return Err(ApiErrorCode::StorageHeaderUnmatched.with_desc(desc));
        }

        let spv_type_script = spv.storage.spv_contract_type_script().map_err(|err| {
            let message = "failed to get SPV contract type script from storage".to_owned();
            log::error!("{message} since {err}");
            RpcError {
                code: RpcErrorCode::InternalError,
                message,
                data: None,
            }
        })?;

        log::debug!(">>> try the cached SPV instance at first");

        let spv_instance = if let Ok(Some(spv_instance)) =
            self.load_spv_instance(stg_tip_height, target_height + confirmations)
        {
            log::debug!(">>> the cached SPV instance is {spv_instance}");
            spv_instance
        } else {
            log::debug!(">>> fetch SPV instance from remote since cached is not satisfied");
            let spv_instance = tokio::task::block_in_place(|| -> RpcResult<SpvInstance> {
                spv.ckb_cli.find_spv_cells(spv_type_script).map_err(|err| {
                    let message = format!(
                        "failed to get SPV cell base on height {stg_tip_height} from chain"
                    );
                    log::error!("{message} since {err}");
                    RpcError {
                        code: RpcErrorCode::InternalError,
                        message,
                        data: None,
                    }
                })
            })?;
            log::debug!(">>> the fetched SPV instance is {spv_instance}");
            self.update_spv_instance(spv_instance.clone());
            spv_instance
        };

        // First Strategy: find the best SPV client not greater than the storage tip height.
        // The spv client found has the longest lifetime and
        // is most likely to cover the height of the block where the bitcoin tx is located.
        // The downside is that it can be affected by reorg.
        let mut spv_client_cell = spv_instance
            .find_best_spv_client_not_greater_than_height(stg_tip_height)
            .map_err(|err| {
                let message = format!(
                    "failed to get SPV cell base on height {stg_tip_height} from fetched data"
                );
                log::error!("{message} since {err}");
                RpcError {
                    code: RpcErrorCode::InternalError,
                    message,
                    data: None,
                }
            })?;

        log::debug!(">>> the best SPV client is {}", spv_client_cell.client);

        let spv_header_root = &spv_client_cell.client.headers_mmr_root;

        let spv_best_height = spv_header_root.max_height;
        if spv_best_height < target_height + confirmations {
            let desc = format!(
                "target transaction is in header#{target_height} \
                and it requires {confirmations} confirmations, \
                but the best SPV header is header#{spv_best_height}",
            );
            return Err(ApiErrorCode::OnchainTxUnconfirmed.with_desc(desc));
        }

        let packed_stg_header_root =
            spv.storage
                .generate_headers_root(spv_best_height)
                .map_err(|err| {
                    let message =
                        format!("failed to generate headers MMR root for height {spv_best_height}");
                    log::error!("{message} since {err}");
                    RpcError {
                        code: RpcErrorCode::InternalError,
                        message,
                        data: None,
                    }
                })?;
        let packed_spv_header_root = spv_header_root.pack();

        if packed_stg_header_root.as_slice() != packed_spv_header_root.as_slice() {
            log::warn!("[onchain] header#{spv_best_height}; mmr-root {spv_header_root}");
            let stg_header_root = packed_stg_header_root.unpack();
            log::warn!("[storage] header#{spv_best_height}; mmr-root {stg_header_root}");
            let desc = "Strategy 1 failed to find a valid SPV client due to reorg, switching to strategy 2 for further lookup";
            log::warn!("{desc}");

            // Second Strategy: Find the Nth (20% of total) spv cell before the tip spv cell.
            // The cell is far enough away from the tip to be less affected by the reorg,
            // and has a relatively long survival period.
            // But it may not be able to cover the height of the block where the newer bitcoin tx is located
            let count = spv_instance.clients.len() / 5;
            spv_client_cell = spv_instance
                .find_spv_client_before_tip(count)
                .map_err(|err| {
                    let message =
                        format!("failed to get the {count}th SPV client before the tip client");
                    log::error!("{message} since {err}");
                    RpcError {
                        code: RpcErrorCode::InternalError,
                        message,
                        data: None,
                    }
                })?;

            log::debug!(
                ">>> the best SPV client is {} found in the {} blocks before tip",
                spv_client_cell.client,
                count
            );

            let spv_header_root = &spv_client_cell.client.headers_mmr_root;

            let spv_best_height = spv_header_root.max_height;
            if spv_best_height < target_height + confirmations {
                let desc = format!(
                    "target transaction is in header#{target_height} \
                    and it requires {confirmations} confirmations, \
                    but the best SPV header is header#{spv_best_height}",
                );
                return Err(ApiErrorCode::OnchainTxUnconfirmed.with_desc(desc));
            }

            let packed_stg_header_root = spv
                .storage
                .generate_headers_root(spv_best_height)
                .map_err(|err| {
                    let message =
                        format!("failed to generate headers MMR root for height {spv_best_height}");
                    log::error!("{message} since {err}");
                    RpcError {
                        code: RpcErrorCode::InternalError,
                        message,
                        data: None,
                    }
                })?;

            let packed_spv_header_root = spv_header_root.pack();

            if packed_stg_header_root.as_slice() != packed_spv_header_root.as_slice() {
                log::warn!("[onchain] header#{spv_best_height}; mmr-root {spv_header_root}");
                let stg_header_root = packed_stg_header_root.unpack();
                log::warn!("[storage] header#{spv_best_height}; mmr-root {stg_header_root}");
                let desc = "the SPV instance on chain is unknown, reorg is required";
                log::warn!("{desc}");
                return Err(ApiErrorCode::OnchainReorgRequired.with_desc(desc));
            }
        }

        let header_proof = spv
            .storage
            .generate_headers_proof(
                spv_client_cell.client.headers_mmr_root.max_height,
                vec![target_height],
            )
            .map_err(|err| {
                let message = "failed to generate headers MMR proof".to_owned();
                log::error!("{message} since {err}");
                RpcError {
                    code: RpcErrorCode::InternalError,
                    message,
                    data: None,
                }
            })?;

        let tx_proof: Bytes = packed::TransactionProof::new_builder()
            .tx_index(tx_index.pack())
            .height(target_height.pack())
            .transaction_proof(Bytes::from(raw_tx_out_proof).pack())
            .header_proof(header_proof.pack())
            .build()
            .as_bytes();

        let btc_tx_proof = BitcoinTxProof {
            spv_client: spv_client_cell.cell.out_point.into(),
            proof: JsonBytes::from_bytes(tx_proof),
        };
        Ok(btc_tx_proof)
    }
}
