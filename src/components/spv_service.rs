//! Internal SPV service.

use std::collections::HashMap;

use ckb_bitcoin_spv_verifier::types::{
    core::{SpvClient, SpvInfo},
    packed,
    prelude::Unpack as VUnpack,
};
use ckb_sdk::{
    rpc::{
        ckb_indexer::{Order, SearchKey},
        CkbRpcClient,
    },
    traits::{CellQueryOptions, LiveCell, PrimaryScriptType},
};
use ckb_types::prelude::*;

use crate::{
    components::{BitcoinClient, Storage},
    prelude::*,
    result::{Error, Result},
};

#[derive(Clone)]
pub struct SpvService {
    pub(crate) ckb_cli: CkbRpcClient,
    pub(crate) btc_cli: BitcoinClient,
    pub(crate) storage: Storage,
}

#[derive(Clone)]
pub struct SpvInfoCell {
    pub(crate) info: SpvInfo,
    pub(crate) cell: LiveCell,
    pub(crate) clients_count: u8,
}

#[derive(Clone)]
pub struct SpvClientCell {
    pub(crate) client: SpvClient,
    pub(crate) cell: LiveCell,
}

impl SpvInfoCell {
    pub(crate) fn prev_tip_client_id(&self) -> u8 {
        let current = self.info.tip_client_id;
        if current == 0 {
            self.clients_count - 1
        } else {
            current - 1
        }
    }

    pub(crate) fn next_tip_client_id(&self) -> u8 {
        let next = self.info.tip_client_id + 1;
        if next < self.clients_count {
            next
        } else {
            0
        }
    }
}

impl SpvService {
    pub(crate) fn find_spv_cells_for_update(
        &self,
    ) -> Result<(SpvInfoCell, SpvClientCell, SpvClientCell)> {
        let (spv_info, spv_clients) = self.find_spv_cells()?;
        let spv_client_curr = spv_clients
            .get(&spv_info.info.tip_client_id)
            .ok_or_else(|| {
                let msg = format!(
                    "the current tip SPV client (id={}) is not found",
                    spv_info.info.tip_client_id
                );
                Error::other(msg)
            })?
            .to_owned();
        let next_tip_client_id = spv_info.next_tip_client_id();
        let spv_client_next = spv_clients
            .get(&next_tip_client_id)
            .ok_or_else(|| {
                let msg =
                    format!("the next tip SPV client (id={next_tip_client_id}) is not found",);
                Error::other(msg)
            })?
            .to_owned();
        Ok((spv_info, spv_client_curr, spv_client_next))
    }

    pub(crate) fn find_best_spv_client(&self, height: u32) -> Result<SpvClientCell> {
        let (mut spv_info, spv_clients) = self.find_spv_cells()?;
        for _ in 0..spv_clients.len() {
            let spv_client = spv_clients
                .get(&spv_info.info.tip_client_id)
                .ok_or_else(|| {
                    let msg = format!(
                        "the current tip SPV client (id={}) is not found",
                        spv_info.info.tip_client_id
                    );
                    Error::other(msg)
                })?;
            if spv_client.client.headers_mmr_root.max_height <= height {
                return Ok(spv_client.to_owned());
            }
            spv_info.info.tip_client_id = spv_info.prev_tip_client_id();
        }
        let msg = format!("all SPV clients have better heights than server has (height: {height})");
        Err(Error::other(msg))
    }

    pub(crate) fn find_spv_cells(&self) -> Result<(SpvInfoCell, HashMap<u8, SpvClientCell>)> {
        let cells = self.find_raw_spv_cells()?;
        parse_raw_spv_cells(cells)
    }

    pub(crate) fn find_raw_spv_cells(&self) -> Result<Vec<LiveCell>> {
        let spv_type_script = self.storage.spv_contract_type_script()?;
        let args_data = spv_type_script.args().raw_data();
        let args = packed::SpvTypeArgsReader::from_slice(&args_data)
            .map_err(|err| {
                let msg = format!("the args of the SPV type script is invalid since {err}");
                Error::other(msg)
            })?
            .unpack();

        let query = CellQueryOptions::new(spv_type_script, PrimaryScriptType::Type);
        let order = Order::Desc;
        let search_key = SearchKey::from(query);

        self.ckb_cli
            .get_cells(search_key, order, u32::MAX.into(), None)
            .map_err(Into::into)
            .map(|res| res.objects)
            .and_then(|cells| {
                let actual = cells.len();
                let expected = usize::from(args.clients_count) + 1;
                if actual == expected {
                    Ok(cells.into_iter().map(Into::into).collect())
                } else {
                    let msg = format!(
                        "the count of SPV cells is incorrect, expect {expected} but got {actual}"
                    );
                    Err(Error::other(msg))
                }
            })
    }
}

fn parse_raw_spv_cells(cells: Vec<LiveCell>) -> Result<(SpvInfoCell, HashMap<u8, SpvClientCell>)> {
    let mut spv_info_opt = None;
    let mut spv_clients = HashMap::new();
    let clients_count = (cells.len() - 1) as u8; // Checked when fetch SPV cells.
    for cell in cells.into_iter() {
        let data = &cell.output_data;
        if let Ok(client) = packed::SpvClientReader::from_slice(data) {
            let spv_cell = SpvClientCell {
                client: client.unpack(),
                cell,
            };
            spv_clients.insert(spv_cell.client.id, spv_cell);
        } else if let Ok(info) = packed::SpvInfoReader::from_slice(data) {
            if spv_info_opt.is_some() {
                let msg = "the SPV info cell should be unique";
                return Err(Error::other(msg));
            }
            let spv_cell = SpvInfoCell {
                info: info.unpack(),
                cell,
                clients_count,
            };
            spv_info_opt = Some(spv_cell);
        } else {
            let msg = "the data of the SPV cell is unexpected";
            return Err(Error::other(msg));
        }
    }
    if let Some(spv_info) = spv_info_opt {
        Ok((spv_info, spv_clients))
    } else {
        let msg = "the SPV info cell is missing";
        Err(Error::other(msg))
    }
}
