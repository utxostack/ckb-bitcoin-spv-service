use std::fmt;

use thiserror::Error;

pub use crate::components::StorageError;

#[derive(Error, Debug)]
pub enum Error {
    #[error("cli error: {0}")]
    Cli(String),

    #[error("btc rpc error: {0}")]
    BitRpc(#[from] BtcRpcError),
    #[error("ckb rpc error: {0}")]
    CkbRpc(#[from] ckb_sdk::RpcError),
    #[error("ckb error: {0}")]
    CkbTx(#[from] ckb_sdk::tx_builder::TxBuilderError),
    #[error("ckb error: {0}")]
    CkbUnlock(#[from] ckb_sdk::unlock::UnlockError),
    #[error("secp256k1 error: {0}")]
    Secp256k1(#[from] secp256k1::Error),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("error: {0}")]
    Other(String),
}

pub type Result<T> = ::std::result::Result<T, Error>;

impl Error {
    pub fn cli<T: fmt::Display>(inner: T) -> Self {
        Self::Cli(inner.to_string())
    }

    pub fn other<T: fmt::Display>(inner: T) -> Self {
        Self::Other(inner.to_string())
    }
}

#[derive(Error, Debug)]
pub enum BtcRpcError {
    #[error("parse json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("jsonrpc error: {0}")]
    Rpc(#[from] jsonrpc_core::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type BtcRpcResult<T> = ::std::result::Result<T, BtcRpcError>;
