//! A bitcoin client to communicate with a Bitcoin chain.

use std::sync::atomic::{AtomicU64, Ordering};

use bitcoin::{consensus::deserialize, BlockHash, MerkleBlock, Txid};
use ckb_bitcoin_spv_verifier::types::core::Header;
use faster_hex::hex_decode;
use jsonrpc_core::{Error as RpcError, ErrorCode as RpcErrorCode, Id as RpcId, Value as RpcValue};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::result::{BtcRpcError, BtcRpcResult, Error, Result};

pub struct BitcoinClient {
    client: Client,
    endpoint: Url,
    username: Option<String>,
    password: Option<String>,
    id: AtomicU64,
}

impl Clone for BitcoinClient {
    fn clone(&self) -> Self {
        Self::new(
            self.endpoint.clone(),
            self.username.clone(),
            self.password.clone(),
        )
    }
}

#[derive(Serialize, Clone, Copy)]
struct ZeroElemTuple();

// ### Warning
//
// If parameters contain only one parameter:
// - `serde_json::to_value(($($arg,)+))`
// - `serde_json::to_value(($($arg),+))`
// are different.
//
// Ref: https://github.com/serde-rs/serde/issues/1309
macro_rules! serialize_parameters {
    () => ( serde_json::to_value(ZeroElemTuple())?);
    ($($arg:ident),+) => ( serde_json::to_value(($($arg,)+))?)
}

/// JSON-RPC 1.0 compatible response.
///
/// ### Warning
///
/// Do NOT use `jsonrpc_core::types::Output` directly.
///
/// Differences with jsonrpc_core::types::Output`:
/// - JSON-RPC version.
/// - Fields `result` and `error` could be returned in one response.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// Protocol version: 1.0 or 2.0? Don't care about it. Just ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonrpc: Option<String>,
    /// Result
    pub result: Option<RpcValue>,
    /// Error
    pub error: Option<RpcError>,
    /// Correlation id
    pub id: RpcId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockHeight {
    height: u32,
}

/// Implement simple JSON-RPC methods.
impl BitcoinClient {
    pub fn new(endpoint: Url, username: Option<String>, password: Option<String>) -> Self {
        let client = Client::new();
        Self {
            client,
            endpoint,
            username,
            password,
            id: 0.into(),
        }
    }

    pub fn post<PARAM, RET>(&self, method: &str, params: PARAM) -> BtcRpcResult<RET>
    where
        PARAM: serde::ser::Serialize,
        RET: serde::de::DeserializeOwned,
    {
        let params = serde_json::to_value(params)?;
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        log::trace!("params \"{params}\", id: {id}");

        let mut req_json = serde_json::Map::new();
        req_json.insert("id".to_owned(), serde_json::json!(id));
        req_json.insert("jsonrpc".to_owned(), serde_json::json!("1.0"));
        req_json.insert("method".to_owned(), serde_json::json!(method));
        req_json.insert("params".to_owned(), params);
        log::trace!("request data \"{:?}\"", serde_json::to_string(&req_json));

        log::trace!(
            "username \"{:?}\", have-password: {}",
            self.username,
            self.password.is_some()
        );

        let req = self.client.post(self.endpoint.clone());
        let req = match (&self.username, &self.password) {
            (Some(ref username), password) => req.basic_auth(username, password.clone()),
            (None, Some(ref password)) => req.basic_auth("", Some(password)),
            (None, None) => req,
        }
        .header(reqwest::header::CONTENT_TYPE, "text/plain")
        .json(&req_json);
        log::trace!("request: {req:?}");
        let resp = req.send()?;
        log::trace!("response: {resp:?}");

        let output = resp.error_for_status()?.json::<Output>()?;
        match (output.result, output.error) {
            (_, Some(error)) => Err(error.into()),
            (Some(result), None) => serde_json::from_value(result).map_err(Into::into),
            (None, None) => {
                let error = RpcError {
                    code: RpcErrorCode::InternalError,
                    message: "result is empty withtout errors".to_owned(),
                    data: None,
                };
                Err(error.into())
            }
        }
    }

    pub fn get_best_block_hash(&self) -> BtcRpcResult<BlockHash> {
        let params = serialize_parameters!();
        self.post("getbestblockhash", params)
    }

    pub fn get_tip_height(&self) -> BtcRpcResult<u32> {
        // Two way to get the tip height:
        // - getblockcount
        // - getbestblockhash -> getblockstats(height)
        self.get_best_block_hash()
            .and_then(|hash| self.get_block_height(hash))
    }

    pub fn get_block_hash(&self, height: u32) -> BtcRpcResult<BlockHash> {
        let params = serialize_parameters!(height);
        self.post("getblockhash", params)
    }

    pub fn get_block_height(&self, hash: BlockHash) -> BtcRpcResult<u32> {
        let stats = &["height"];
        let params = serialize_parameters!(hash, stats);
        let height: BlockHeight = self.post("getblockstats", params)?;
        Ok(height.height)
    }

    pub fn get_raw_block_header(&self, hash: BlockHash) -> BtcRpcResult<Vec<u8>> {
        let params = serialize_parameters!(hash, false);
        self.post("getblockheader", params).and_then(|hex: String| {
            let mut bin = vec![0; hex.len() / 2];
            hex_decode(hex.as_bytes(), &mut bin).map_err(|err| {
                let error = RpcError {
                    code: RpcErrorCode::ParseError,
                    message: format!("failed to decode the hex string \"{hex}\" since {err}"),
                    data: None,
                };
                <RpcError as Into<BtcRpcError>>::into(error)
            })?;
            Ok(bin)
        })
    }

    pub fn get_block_header(&self, hash: BlockHash) -> BtcRpcResult<Header> {
        self.get_raw_block_header(hash).and_then(|bin| {
            deserialize(&bin).map_err(|err| {
                let error = RpcError {
                    code: RpcErrorCode::ParseError,
                    message: format!("failed to deserialize header from hex string since {err}"),
                    data: None,
                };
                error.into()
            })
        })
    }

    pub fn get_block_header_by_height(&self, height: u32) -> BtcRpcResult<Header> {
        self.get_block_hash(height)
            .and_then(|hash| self.get_block_header(hash))
    }

    pub fn get_raw_tx_out_proof(&self, txid: Txid) -> BtcRpcResult<Vec<u8>> {
        let txids = vec![txid];
        let params = serialize_parameters!(txids);
        self.post("gettxoutproof", params).and_then(|hex: String| {
            let mut bin = vec![0; hex.len() / 2];
            hex_decode(hex.as_bytes(), &mut bin).map_err(|err| {
                let error = RpcError {
                    code: RpcErrorCode::ParseError,
                    message: format!("failed to decode the hex string \"{hex}\" since {err}"),
                    data: None,
                };
                <RpcError as Into<BtcRpcError>>::into(error)
            })?;
            Ok(bin)
        })
    }

    pub fn get_tx_out_proof(&self, txid: Txid) -> BtcRpcResult<(MerkleBlock, Vec<u8>)> {
        self.get_raw_tx_out_proof(txid).and_then(|bin| {
            deserialize(&bin)
                .map_err(|err| {
                    let error = RpcError {
                        code: RpcErrorCode::ParseError,
                        message: format!(
                            "failed to deserialize tx out proof from hex string since {err}"
                        ),
                        data: None,
                    };
                    error.into()
                })
                .map(|mb| (mb, bin))
        })
    }
}

/// Implement combined methods.
impl BitcoinClient {
    pub fn check_then_fetch_header(&self, height: u32) -> Result<Header> {
        let tip_height = self.get_tip_height()?;
        log::debug!("The height of the best bitcoin block is {tip_height}");
        if height > tip_height {
            let msg = format!(
                "the tip height of bitcoin ({tip_height}) is less than
                the required height {height}"
            );
            return Err(Error::other(msg));
        }
        let header = self.get_block_header_by_height(height)?;
        log::debug!("The bitcoin header#{height} is {header:?}");
        Ok(header)
    }

    pub fn get_headers(&self, start: u32, end: u32) -> Result<Vec<Header>> {
        log::trace!("Download headers from {start} to {end}");
        let mut headers = Vec::new();
        for height in start..=end {
            let header = self.get_block_header_by_height(height)?;
            headers.push(header);
        }
        Ok(headers)
    }
}
