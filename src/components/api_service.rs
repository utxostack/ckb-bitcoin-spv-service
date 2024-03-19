//! JSON-RPC APIs service.

use std::net::SocketAddr;

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
    components::{SpvClientCell, SpvService},
    prelude::*,
    result::{Error, Result},
};

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
        Self { spv_service }
    }
}

impl SpvRpc for SpvRpcImpl {
    fn get_tx_proof(
        &self,
        txid: Txid,
        tx_index: u32,
        confirmations: u32,
    ) -> RpcResult<BitcoinTxProof> {
        log::trace!("Call getTxProof with params [{txid:#x}, {confirmations}]");
        let spv = &self.spv_service;

        let (target_height, target_hash, raw_tx_out_proof) =
            tokio::task::block_in_place(|| -> RpcResult<(u32, Hash, Vec<u8>)> {
                let (merkle_block, raw_tx_out_proof) =
                    spv.btc_cli.get_tx_out_proof(txid).map_err(|err| {
                        let message = format!(
                            "failed to get tx out proof for {txid:#x} from remote since {err}"
                        );
                        RpcError {
                            code: RpcErrorCode::InternalError,
                            message,
                            data: None,
                        }
                    })?;
                let block_hash = merkle_block.header.block_hash();
                log::trace!(">>> the input tx in header {block_hash:#x}");
                let block_height = spv.btc_cli.get_block_height(block_hash).map_err(|err| {
                    let message = format!("failed to get block height from remote since {err}");
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
            let message =
                format!("failed to read tip bitcoin height from local storage since {err}");
            RpcError {
                code: RpcErrorCode::InternalError,
                message,
                data: None,
            }
        })?;
        log::trace!(">>> tip height in local storage is {stg_tip_height}");

        // TODO Define server errors with enum.
        if stg_tip_height < target_height {
            let message = format!(
                "target transaction is in header#{target_height}, \
                but the tip header in server is header#{stg_tip_height}"
            );
            return Err(RpcError {
                code: RpcErrorCode::ServerError(-1),
                message,
                data: None,
            });
        }
        if stg_tip_height < target_height + confirmations {
            let message = format!(
                "target transaction is in header#{target_height} \
                and it requires {confirmations} confirmations, \
                but the tip header in server is header#{stg_tip_height}"
            );
            return Err(RpcError {
                code: RpcErrorCode::ServerError(-2),
                message,
                data: None,
            });
        }
        let stg_target_hash = spv
            .storage
            .bitcoin_header_hash(target_height)
            .map_err(|err| {
                let message = format!("server doesn't have header#{target_height} since {err}");
                RpcError {
                    code: RpcErrorCode::ServerError(-3),
                    message,
                    data: None,
                }
            })?;
        if target_hash != stg_target_hash {
            let message = format!(
                "target transaction is in header#{target_height}, \
                the header hash from remote is {target_hash:#x}, \
                its hash in server is {stg_target_hash:#x}"
            );
            return Err(RpcError {
                code: RpcErrorCode::ServerError(-4),
                message,
                data: None,
            });
        }

        let spv_client_cell = tokio::task::block_in_place(|| -> RpcResult<SpvClientCell> {
            spv.find_best_spv_client(stg_tip_height).map_err(|err| {
                let message = format!("failed to get SPV cell from remote since {err}");
                RpcError {
                    code: RpcErrorCode::InternalError,
                    message,
                    data: None,
                }
            })
        })?;
        log::trace!(">>> the best SPV client is {}", spv_client_cell.client);

        if spv_client_cell.client.headers_mmr_root.max_height < target_height + confirmations {
            let message = format!(
                "target transaction is in header#{target_height} \
                and it requires {confirmations} confirmations, \
                but the best SPV header is header#{}",
                spv_client_cell.client.headers_mmr_root.max_height
            );
            return Err(RpcError {
                code: RpcErrorCode::ServerError(-5),
                message,
                data: None,
            });
        }

        let header_proof = spv
            .storage
            .generate_headers_proof(
                spv_client_cell.client.headers_mmr_root.max_height,
                vec![target_height],
            )
            .map_err(|err| {
                let message = format!("failed to generate headers MMR proof since {err}");
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
