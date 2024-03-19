//! Implement reading data from the storage.

use bitcoin::consensus::deserialize;
use ckb_bitcoin_spv_verifier::types::{core::Header, packed, prelude::*};
use ckb_types::packed::{CellDep, CellDepReader, Script, ScriptReader};

use crate::components::storage::{
    prelude::StorageReader,
    result::{Error, Result},
    schemas::{columns, keys},
    Storage,
};

impl StorageReader for Storage {
    fn get_base_bitcoin_height(&self) -> Result<Option<u32>> {
        let height_opt = *self
            .cache
            .base_bitcoin_height
            .read()
            .map_err(Error::storage)?;
        if let Some(height) = height_opt {
            Ok(Some(height))
        } else {
            let height_opt = self
                .get(keys::BASE_BITCOIN_HEIGHT)?
                .map(|raw| packed::Uint32Reader::from_slice(&raw).map(|reader| reader.unpack()))
                .transpose()?;
            if let Some(height) = height_opt {
                *self
                    .cache
                    .base_bitcoin_height
                    .write()
                    .map_err(Error::storage)? = Some(height);
            }
            Ok(height_opt)
        }
    }

    fn get_tip_bitcoin_height(&self) -> Result<u32> {
        self.get(keys::TIP_BITCOIN_HEIGHT)?
            .map(|raw| packed::Uint32Reader::from_slice(&raw).map(|reader| reader.unpack()))
            .transpose()
            .map_err(Into::into)
            .and_then(|opt| opt.ok_or_else(|| Error::not_found("tip bitcoin height")))
    }

    fn get_bitcoin_header(&self, height: u32) -> Result<Header> {
        let key = height.to_be_bytes();
        self.get_cf(columns::COLUMN_BITCOIN_HEADERS, key)?
            .map(|raw| {
                deserialize(&raw).map_err(|err| {
                    let msg =
                        format!("failed to decode the header#{height} from storage since {err}");
                    Error::data(msg)
                })
            })
            .transpose()
            .and_then(|opt| opt.ok_or_else(|| Error::not_found(format!("header#{height}"))))
    }

    fn get_bitcoin_header_digest(&self, position: u64) -> Result<Option<packed::HeaderDigest>> {
        let key = position.to_be_bytes();
        self.get_cf(columns::COLUMN_BITCOIN_HEADER_MMR, key)?
            .map(|raw| {
                packed::HeaderDigestReader::from_slice(&raw)
                    .map(|reader| reader.to_entity())
                    .map_err(Into::into)
            })
            .transpose()
    }

    fn get_spv_contract_type_script(&self) -> Result<Script> {
        self.get(keys::SPV_CONTRACT_TYPE_SCRIPT)?
            .map(|raw| ScriptReader::from_slice(&raw).map(|reader| reader.to_entity()))
            .transpose()
            .map_err(Into::into)
            .and_then(|opt| opt.ok_or_else(|| Error::not_found("the SPV script type script")))
    }

    fn get_spv_contract_cell_dep(&self) -> Result<CellDep> {
        self.get(keys::SPV_CONTRACT_CELL_DEP)?
            .map(|raw| CellDepReader::from_slice(&raw).map(|reader| reader.to_entity()))
            .transpose()
            .map_err(Into::into)
            .and_then(|opt| opt.ok_or_else(|| Error::not_found("the SPV script cell dep")))
    }

    fn get_lock_contract_cell_dep(&self) -> Result<CellDep> {
        self.get(keys::LOCK_CONTRACT_CELL_DEP)?
            .map(|raw| CellDepReader::from_slice(&raw).map(|reader| reader.to_entity()))
            .transpose()
            .map_err(Into::into)
            .and_then(|opt| opt.ok_or_else(|| Error::not_found("the lock script cell dep")))
    }
}
