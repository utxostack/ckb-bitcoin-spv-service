//! Implement writing data into the storage.

use bitcoin::consensus::serialize;
use ckb_bitcoin_spv_verifier::types::{core::Header, packed, prelude::*};
use ckb_types::packed::{CellDep, Script};

use crate::components::storage::{
    prelude::StorageWriter,
    result::{Error, Result},
    schemas::{columns, keys},
    Storage,
};

impl StorageWriter for Storage {
    fn put_base_bitcoin_height(&self, height: u32) -> Result<()> {
        let value: packed::Uint32 = height.pack();
        let mut writer = self
            .cache
            .base_bitcoin_height
            .write()
            .map_err(Error::storage)?;
        self.put(keys::BASE_BITCOIN_HEIGHT, value.as_slice())?;
        *writer = Some(height);
        Ok(())
    }

    fn put_tip_bitcoin_height(&self, height: u32) -> Result<()> {
        let value: packed::Uint32 = height.pack();
        self.put(keys::TIP_BITCOIN_HEIGHT, value.as_slice())
    }

    fn put_bitcoin_header(&self, height: u32, header: &Header) -> Result<()> {
        let key = height.to_be_bytes();
        let value = serialize(header);
        self.put_cf(columns::COLUMN_BITCOIN_HEADERS, key, value)
    }

    fn put_bitcoin_header_digest(
        &self,
        position: u64,
        digest: &packed::HeaderDigest,
    ) -> Result<()> {
        let key = position.to_be_bytes();
        self.put_cf(columns::COLUMN_BITCOIN_HEADER_MMR, key, digest.as_slice())
    }

    fn put_spv_contract_type_script(&self, type_script: Script) -> Result<()> {
        self.put(keys::SPV_CONTRACT_TYPE_SCRIPT, type_script.as_slice())
    }

    fn put_spv_contract_cell_dep(&self, cell_dep: CellDep) -> Result<()> {
        self.put(keys::SPV_CONTRACT_CELL_DEP, cell_dep.as_slice())
    }

    fn put_lock_contract_cell_dep(&self, cell_dep: CellDep) -> Result<()> {
        self.put(keys::LOCK_CONTRACT_CELL_DEP, cell_dep.as_slice())
    }
}
