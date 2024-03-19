//! Implement MMR-related traits for the storage.

use ckb_bitcoin_spv_verifier::{
    types::packed,
    utilities::mmr::lib::{
        Error as MMRError, MMRStoreReadOps, MMRStoreWriteOps, Result as MMRResult,
    },
};

use crate::components::storage::{prelude::*, Storage};

impl MMRStoreReadOps<packed::HeaderDigest> for Storage {
    fn get_elem(&self, pos: u64) -> MMRResult<Option<packed::HeaderDigest>> {
        self.get_bitcoin_header_digest(pos).map_err(|err| {
            MMRError::StoreError(format!(
                "Failed to read position {} from MMR, DB error {}",
                pos, err
            ))
        })
    }
}

impl MMRStoreWriteOps<packed::HeaderDigest> for Storage {
    fn append(&mut self, pos: u64, elems: Vec<packed::HeaderDigest>) -> MMRResult<()> {
        for (offset, elem) in elems.iter().enumerate() {
            let pos: u64 = pos + (offset as u64);
            self.put_bitcoin_header_digest(pos, elem).map_err(|err| {
                MMRError::StoreError(format!("Failed to append to MMR, DB error {}", err))
            })?;
        }
        Ok(())
    }
}
