//! Internal SPV service.

use bitcoin::BlockHash;
use ckb_bitcoin_spv_verifier::types::{
    core::{Hash, Header},
    prelude::{Pack as VPack, Unpack as VUnpack},
};
use ckb_sdk::rpc::CkbRpcClient;
use ckb_types::prelude::*;

use crate::{
    components::{BitcoinClient, SpvClientCell, SpvInfoCell, SpvInstance, Storage},
    prelude::*,
    result::{Error, Result},
};

#[derive(Clone)]
pub struct SpvService {
    pub(crate) ckb_cli: CkbRpcClient,
    pub(crate) btc_cli: BitcoinClient,
    pub(crate) storage: Storage,
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
    Reset(SpvReorgInput),
}

impl SpvService {
    pub(crate) fn select_operation(&self) -> Result<SpvOperation> {
        let spv_type_script = self.storage.spv_contract_type_script()?;
        let ins = self.ckb_cli.find_spv_cells(spv_type_script)?;
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
            if input.info.clients_count as usize == input.stale.len() {
                log::warn!("[onchain] all SPV clients are stale, resetting");
                return Ok(SpvOperation::Reset(input));
            } else {
                return Ok(SpvOperation::Reorg(input));
            }
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
                if stale.len() > 1 {
                    let input = SpvReorgInput {
                        info,
                        curr: cell.clone(),
                        stale,
                    };
                    return Ok(input);
                } else {
                    log::warn!(
                        "[TODO::KnownIssue] this is a dirty patch to fix an issue in the contract: \
                        update and reorg only 1 block are indistinguishable, \
                        let's just reorg 1 more client"
                    );
                }
            }

            log::trace!("[onchain] header#{spv_height}; mmr-root {spv_header_root}");
            let stg_header_root = packed_stg_header_root.unpack();
            log::trace!("[storage] header#{spv_height}; mmr-root {stg_header_root}");

            stale.push(cell.clone());
            info.info.tip_client_id = info.prev_tip_client_id();
        }
        log::warn!("failed to reorg since no common parent between SPV instance and storage");
        let input = SpvReorgInput {
            info: info.clone(),
            curr: clients.get(&info.info.tip_client_id).unwrap().clone(),
            stale,
        };
        Ok(input)
    }

    pub(crate) fn sync_storage(&self, batch_size: u32) -> Result<bool> {
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
            let headers_opt = self.sync_storage_internal(
                batch_size,
                stg_tip_height + 1,
                btc_tip_height,
                stg_tip_hash,
            )?;
            return Ok(headers_opt.is_some());
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
                break;
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

        let headers_opt = self.sync_storage_internal(
            batch_size,
            fork_height + 1,
            btc_tip_height,
            fork_hash.into(),
        )?;
        Ok(headers_opt.is_some())
    }

    fn sync_storage_internal(
        &self,
        batch_size: u32,
        mut start_height: u32,
        end_height: u32,
        mut start_hash: BlockHash,
    ) -> Result<Option<Vec<Header>>> {
        let spv = self;
        let mut headers = Vec::new();
        while start_height <= end_height {
            let mut next_height = start_height + batch_size;
            if next_height > end_height {
                next_height = end_height;
            }

            let tmp_headers = if let Some(headers) =
                spv.btc_cli
                    .get_headers(start_height, next_height, start_hash)?
            {
                headers
            } else {
                return Ok(None);
            };

            start_height = next_height + 1;
            if let Some(header) = tmp_headers.last() {
                start_hash = header.block_hash();
            } else {
                return Ok(None);
            }
            headers.extend_from_slice(&tmp_headers);
            let _ = spv.storage.append_headers(tmp_headers)?;
        }
        Ok(Some(headers))
    }
}
