//! Internal SPV service.

use std::collections::HashMap;

use ckb_bitcoin_spv_verifier::types::{
    core::{Hash, SpvClient, SpvInfo},
    packed,
    prelude::{Pack as VPack, Unpack as VUnpack},
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

pub struct SpvInstance {
    pub(crate) info: SpvInfoCell,
    pub(crate) clients: HashMap<u8, SpvClientCell>,
}

pub struct SpvUpdateInput {
    pub(crate) info: SpvInfoCell,
    pub(crate) curr: SpvClientCell,
    pub(crate) next: SpvClientCell,
}

pub struct SpvReorgInput {
    pub(crate) info: SpvInfoCell,
    pub(crate) curr: SpvClientCell,
    pub(crate) stale: Vec<SpvClientCell>,
}

pub enum SpvOperation {
    Update(SpvUpdateInput),
    Reorg(SpvReorgInput),
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
    pub(crate) fn find_best_spv_client(&self, height: u32) -> Result<SpvClientCell> {
        let SpvInstance { mut info, clients } = self.find_spv_cells()?;
        for _ in 0..clients.len() {
            let cell = clients.get(&info.info.tip_client_id).ok_or_else(|| {
                let msg = format!(
                    "the SPV client (id={}) is not found",
                    info.info.tip_client_id
                );
                Error::other(msg)
            })?;
            if cell.client.headers_mmr_root.max_height <= height {
                return Ok(cell.to_owned());
            }
            info.info.tip_client_id = info.prev_tip_client_id();
        }
        let msg = format!("all SPV clients have better heights than server has (height: {height})");
        Err(Error::other(msg))
    }

    pub(crate) fn select_operation(&self) -> Result<SpvOperation> {
        let ins = self.find_spv_cells()?;
        let spv_client_curr = ins
            .clients
            .get(&ins.info.info.tip_client_id)
            .ok_or_else(|| {
                let msg = format!(
                    "the current tip SPV client (id={}) is not found",
                    ins.info.info.tip_client_id
                );
                Error::other(msg)
            })?
            .to_owned();
        log::info!("[onchain] tip SPV client {}", spv_client_curr.client);

        let spv_header_root_curr = &spv_client_curr.client.headers_mmr_root;
        let spv_height_curr = spv_header_root_curr.max_height;
        let packed_stg_header_root_curr = self.storage.generate_headers_root(spv_height_curr)?;
        let packed_spv_header_root_curr = spv_header_root_curr.pack();

        if packed_stg_header_root_curr.as_slice() != packed_spv_header_root_curr.as_slice() {
            log::warn!("[onchain] header#{spv_height_curr}; mmr-root {spv_header_root_curr}");
            let stg_header_root_curr = packed_stg_header_root_curr.unpack();
            log::warn!("[storage] header#{spv_height_curr}; mmr-root {stg_header_root_curr}");
            let input = self.prepare_reorg_input(ins)?;
            return Ok(SpvOperation::Reorg(input));
        }

        let next_tip_client_id = ins.info.next_tip_client_id();
        let spv_client_next = ins
            .clients
            .get(&next_tip_client_id)
            .ok_or_else(|| {
                let msg = format!("the next tip SPV client (id={next_tip_client_id}) is not found");
                Error::other(msg)
            })?
            .to_owned();
        log::trace!(
            "[onchain] old SPV client {} (will be next)",
            spv_client_next.client
        );
        let input = SpvUpdateInput {
            info: ins.info,
            curr: spv_client_curr,
            next: spv_client_next,
        };
        Ok(SpvOperation::Update(input))
    }

    pub(crate) fn prepare_reorg_input(&self, ins: SpvInstance) -> Result<SpvReorgInput> {
        let SpvInstance { mut info, clients } = ins;
        let mut stale = Vec::new();
        for _ in 0..clients.len() {
            let cell = clients.get(&info.info.tip_client_id).ok_or_else(|| {
                let msg = format!(
                    "the SPV client (id={}) is not found",
                    info.info.tip_client_id
                );
                Error::other(msg)
            })?;

            let spv_header_root = &cell.client.headers_mmr_root;
            let spv_height = spv_header_root.max_height;
            let packed_stg_header_root = self.storage.generate_headers_root(spv_height)?;
            let packed_spv_header_root = spv_header_root.pack();

            if packed_stg_header_root.as_slice() == packed_spv_header_root.as_slice() {
                let input = SpvReorgInput {
                    info,
                    curr: cell.clone(),
                    stale,
                };
                return Ok(input);
            }

            log::trace!("[onchain] header#{spv_height}; mmr-root {spv_header_root}");
            let stg_header_root = packed_stg_header_root.unpack();
            log::trace!("[storage] header#{spv_height}; mmr-root {stg_header_root}");

            stale.push(cell.clone());
            info.info.tip_client_id = info.prev_tip_client_id();
        }
        let msg = "failed to reorg since no common parent between SPV instance and storage";
        Err(Error::other(msg))
    }

    pub(crate) fn find_spv_cells(&self) -> Result<SpvInstance> {
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

    pub(crate) fn sync_storage(&self) -> Result<bool> {
        let spv = &self;
        let (stg_tip_height, stg_tip_header) = spv.storage.tip_state()?;
        let stg_tip_hash = stg_tip_header.block_hash();
        log::info!("[storage] header#{stg_tip_height:07}, {stg_tip_hash:#x}; tip");

        let (btc_tip_height, btc_tip_header) = spv.btc_cli.get_tip_state()?;
        log::info!(
            "[bitcoin] header#{btc_tip_height:07}, {:#x}; tip; prev {:#x}",
            btc_tip_header.block_hash(),
            btc_tip_header.prev_blockhash
        );

        if stg_tip_height >= btc_tip_height {
            return Ok(true);
        }

        let btc_header = spv.btc_cli.get_block_header_by_height(stg_tip_height)?;
        let btc_hash = btc_header.block_hash();
        if stg_tip_hash == btc_hash {
            let headers = if let Some(headers) =
                spv.btc_cli
                    .get_headers(stg_tip_height + 1, btc_tip_height, stg_tip_hash)?
            {
                headers
            } else {
                return Ok(false);
            };
            let _ = spv.storage.append_headers(headers)?;
            return Ok(true);
        }

        log::info!("Try to find the height when fork happened");
        let (stg_base_height, _) = spv.storage.base_state()?;
        let mut fork_point = None;

        for height in (stg_base_height..stg_tip_height).rev() {
            let stg_hash = spv.storage.bitcoin_header_hash(height)?;
            log::debug!("[storage] header#{height:07}, {stg_hash:#x}");
            let btc_header = spv.btc_cli.get_block_header_by_height(height)?;
            let btc_hash: Hash = btc_header.block_hash().into();
            log::debug!("[bitcoin] header#{height:07}, {btc_hash:#x}");

            if stg_hash == btc_hash {
                log::info!("Fork happened at height {height}");
                fork_point = Some((height, btc_hash));
            }
        }

        if fork_point.is_none() {
            let msg = format!(
                "reorg failed since the fork point is ahead than \
                local start height {stg_base_height}"
            );
            return Err(Error::other(msg));
        }
        let (fork_height, fork_hash) = fork_point.unwrap();

        log::warn!("The chain in storage rollback to header#{fork_height:07}, {fork_hash:#x}");
        spv.storage.rollback_to(Some(fork_height))?;

        let headers = if let Some(headers) =
            spv.btc_cli
                .get_headers(fork_height + 1, btc_tip_height, fork_hash.into())?
        {
            headers
        } else {
            return Ok(false);
        };
        let _ = spv.storage.append_headers(headers)?;

        Ok(true)
    }
}

fn parse_raw_spv_cells(cells: Vec<LiveCell>) -> Result<SpvInstance> {
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
        let instance = SpvInstance {
            info: spv_info,
            clients: spv_clients,
        };
        Ok(instance)
    } else {
        let msg = "the SPV info cell is missing";
        Err(Error::other(msg))
    }
}
