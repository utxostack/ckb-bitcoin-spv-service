use bitcoin::constants::DIFFCHANGE_INTERVAL;
use ckb_bitcoin_spv_verifier::{
    types::{
        core::{Hash, Header, HeaderDigest, MmrProof, SpvClient, Target},
        packed,
        prelude::*,
    },
    utilities::{
        bitcoin::calculate_next_target,
        mmr::{self, ClientRootMMR},
    },
};
use ckb_types::packed::{CellDep, Script};

use crate::components::storage::{
    result::{Error, Result},
    Storage,
};

pub(crate) trait StorageReader: Send + Sync + Sized {
    // Initialize DB
    fn get_base_bitcoin_height(&self) -> Result<Option<u32>>;
    // Store Bitcoin state
    fn get_tip_bitcoin_height(&self) -> Result<u32>;
    fn get_bitcoin_header(&self, height: u32) -> Result<Header>;
    // For MMR
    fn get_bitcoin_header_digest(&self, position: u64) -> Result<Option<packed::HeaderDigest>>;
    // For CKB transactions
    fn get_spv_contract_type_script(&self) -> Result<Script>;
    fn get_spv_contract_cell_dep(&self) -> Result<CellDep>;
    fn get_lock_contract_cell_dep(&self) -> Result<CellDep>;
}

pub(crate) trait StorageWriter: Send + Sync + Sized {
    // Initialize DB
    fn put_base_bitcoin_height(&self, height: u32) -> Result<()>;
    // Store Bitcoin state
    fn put_tip_bitcoin_height(&self, height: u32) -> Result<()>;
    fn put_bitcoin_header(&self, height: u32, header: &Header) -> Result<()>;
    // For MMR
    fn put_bitcoin_header_digest(&self, position: u64, digest: &packed::HeaderDigest)
        -> Result<()>;
    // For CKB transactions
    fn put_spv_contract_type_script(&self, type_script: Script) -> Result<()>;
    fn put_spv_contract_cell_dep(&self, cell_dep: CellDep) -> Result<()>;
    fn put_lock_contract_cell_dep(&self, cell_dep: CellDep) -> Result<()>;
}

// Private APIs: for internal use.
// Don't call methods in this trait directly.
pub(crate) trait InternalBitcoinSpvStorage:
    mmr::lib::MMRStoreReadOps<packed::HeaderDigest>
    + mmr::lib::MMRStoreWriteOps<packed::HeaderDigest>
    + StorageReader
    + StorageWriter
    + Clone
{
    /// Returns the chain root MMR for a provided height, with the base height.
    fn chain_root_mmr(&self, current_height: u32) -> Result<(u32, ClientRootMMR<Self>)> {
        let base_height = self
            .get_base_bitcoin_height()
            .and_then(|opt| opt.ok_or_else(|| Error::not_found("base bitcoin height")))?;
        if current_height < base_height {
            let msg = format!("base height {base_height} is larget than input {current_height}");
            return Err(Error::data(msg));
        }
        let index = current_height - base_height;
        let mmr_size = mmr::lib::leaf_index_to_mmr_size(u64::from(index));
        let mmr = ClientRootMMR::new(mmr_size, self.clone());
        Ok((base_height, mmr))
    }
}

pub(crate) trait BitcoinSpvStorage: InternalBitcoinSpvStorage {
    fn is_initialized(&self) -> Result<bool> {
        self.get_base_bitcoin_height().map(|inner| inner.is_some())
    }

    fn initialize_with(&self, height: u32, header: Header) -> Result<SpvClient> {
        if self.is_initialized()? {
            return Err(Error::data("don't initialize a non-empty storage"));
        }
        let block_hash: Hash = header.block_hash().into();
        let digest = HeaderDigest::new_leaf(height, block_hash).pack();

        let mut mmr = ClientRootMMR::new(0, self.clone());

        mmr.push(digest)?;
        let mmr_root = mmr.get_root()?;
        mmr.commit()?;

        self.put_bitcoin_header(height, &header)?;
        self.put_base_bitcoin_height(height)?;
        self.put_tip_bitcoin_height(height)?;

        let target_adjust_info = packed::TargetAdjustInfo::encode(header.time, header.bits);
        let spv_client = SpvClient {
            id: 0,
            tip_block_hash: block_hash,
            headers_mmr_root: mmr_root.unpack(),
            target_adjust_info,
        };

        Ok(spv_client)
    }

    fn save_cells_state(
        &self,
        spv_script: Script,
        spv_cell_dep: CellDep,
        lock_cell_dep: CellDep,
    ) -> Result<()> {
        self.put_spv_contract_type_script(spv_script.clone())?;
        self.put_spv_contract_cell_dep(spv_cell_dep.clone())?;
        self.put_lock_contract_cell_dep(lock_cell_dep.clone())?;
        Ok(())
    }

    fn append_headers(&self, headers: Vec<Header>) -> Result<(u32, Header)> {
        if headers.is_empty() {
            return Err(Error::not_found("input headers"));
        }

        let (mut tip_height, mut tip_header) = self.tip_state()?;
        let mut tip_hash: Hash = tip_header.block_hash().into();
        let (base_height, mut mmr) = self.chain_root_mmr(tip_height)?;
        let mut positions = Vec::new();

        for header in &headers {
            let prev_hash = header.prev_blockhash.into();
            if tip_hash != prev_hash {
                let msg = format!(
                    "input headers are uncontinuous, expect previous hash is \
                    {tip_hash:#x} but got {prev_hash:#x}"
                );
                return Err(Error::data(msg));
            }
            tip_height += 1;
            tip_header = *header;
            tip_hash = header.block_hash().into();

            let index = tip_height - base_height;
            let position = mmr::lib::leaf_index_to_pos(u64::from(index));

            let digest = HeaderDigest::new_leaf(tip_height, tip_hash).pack();

            self.put_bitcoin_header(tip_height, &tip_header)?;
            positions.push(position);
            mmr.push(digest)?;
        }
        mmr.commit()?;

        self.put_tip_bitcoin_height(tip_height)?;

        Ok((tip_height, tip_header))
    }

    fn generate_spv_client_and_spv_update(
        &self,
        prev_height: u32,
        limit: u32,
    ) -> Result<(SpvClient, packed::SpvUpdate)> {
        let mut tip_height = self.get_tip_bitcoin_height()?;
        if tip_height > prev_height + limit {
            tip_height = prev_height + limit;
        }
        let tip_header = self.get_bitcoin_header(tip_height)?;
        let (headers_mmr_root, headers_mmr_proof) = {
            let (base_height, mmr) = self.chain_root_mmr(tip_height)?;
            let positions = (prev_height..tip_height)
                .map(|height| {
                    let index = height + 1 - base_height;
                    mmr::lib::leaf_index_to_pos(u64::from(index))
                })
                .collect::<Vec<_>>();
            let headers_mmr_root = mmr.get_root()?;
            let headers_mmr_proof_items = mmr
                .gen_proof(positions)?
                .proof_items()
                .iter()
                .map(Clone::clone)
                .collect::<Vec<_>>();
            let headers_mmr_proof = packed::MmrProof::new_builder()
                .set(headers_mmr_proof_items)
                .build();
            (headers_mmr_root, headers_mmr_proof)
        };
        let mut headers = Vec::new();
        for height in (prev_height + 1)..=tip_height {
            let header = self.get_bitcoin_header(height)?;
            headers.push(header)
        }

        let flag = (tip_height + 1) % DIFFCHANGE_INTERVAL;

        let target_adjust_info = if flag == 1 {
            packed::TargetAdjustInfo::encode(tip_header.time, tip_header.bits)
        } else {
            let start_height = (tip_height / DIFFCHANGE_INTERVAL) * DIFFCHANGE_INTERVAL;
            let start_header = self.get_bitcoin_header(start_height)?;
            if flag == 0 {
                let curr_target: Target = tip_header.bits.into();
                log::trace!(
                    "height {tip_height}, time: {}, target {curr_target:#x}",
                    tip_header.time
                );
                let next_target =
                    calculate_next_target(curr_target, start_header.time, tip_header.time);
                log::trace!("calculated new target  {next_target:#x}");
                let next_bits = next_target.to_compact_lossy();
                let next_target: Target = next_bits.into();
                log::trace!("after definition lossy {next_target:#x}");
                packed::TargetAdjustInfo::encode(start_header.time, next_bits)
            } else {
                packed::TargetAdjustInfo::encode(start_header.time, start_header.bits)
            }
        };

        let spv_client = SpvClient {
            id: 0,
            tip_block_hash: tip_header.block_hash().into(),
            headers_mmr_root: headers_mmr_root.unpack(),
            target_adjust_info,
        };
        let spv_update = packed::SpvUpdate::new_builder()
            .headers(headers.pack())
            .new_headers_mmr_proof(headers_mmr_proof)
            .build();

        Ok((spv_client, spv_update))
    }

    fn generate_headers_proof(&self, tip_height: u32, heights: Vec<u32>) -> Result<MmrProof> {
        let (base_height, mmr) = self.chain_root_mmr(tip_height)?;
        let positions = heights
            .into_iter()
            .map(|height| {
                let index = height - base_height;
                mmr::lib::leaf_index_to_pos(u64::from(index))
            })
            .collect::<Vec<_>>();
        let proof = mmr
            .gen_proof(positions)?
            .proof_items()
            .iter()
            .map(|item| item.unpack())
            .collect::<Vec<_>>();
        Ok(proof)
    }

    fn rollback_to(&self, height_opt: Option<u32>) -> Result<()> {
        if let Some(height) = height_opt {
            self.put_tip_bitcoin_height(height)?;
        } else if let Some(height) = self.get_base_bitcoin_height()? {
            self.put_tip_bitcoin_height(height)?;
        } else {
            return Err(Error::data("don't rollback on an empty storage"));
        }
        Ok(())
    }

    fn tip_state(&self) -> Result<(u32, Header)> {
        self.get_tip_bitcoin_height().and_then(|height| {
            self.get_bitcoin_header(height)
                .map(|header| (height, header))
        })
    }

    fn bitcoin_header_hash(&self, height: u32) -> Result<Hash> {
        self.get_bitcoin_header(height)
            .map(|header| header.block_hash().into())
    }

    fn spv_contract_type_script(&self) -> Result<Script> {
        self.get_spv_contract_type_script()
    }

    fn spv_contract_cell_dep(&self) -> Result<CellDep> {
        self.get_spv_contract_cell_dep()
    }

    fn lock_contract_cell_dep(&self) -> Result<CellDep> {
        self.get_lock_contract_cell_dep()
    }
}

impl InternalBitcoinSpvStorage for Storage {}
impl BitcoinSpvStorage for Storage {}
