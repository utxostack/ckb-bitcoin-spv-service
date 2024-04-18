//! Constants.

use ckb_types::{core::DepType, H256};

pub(crate) mod mainnet;
pub(crate) mod testnet;

// (code_hash, cell_dep.tx_hash, cell_dep.tx_index, cell_dep.dep_type)
pub(crate) type CodeHashAndItsCellDep = (H256, H256, u32, DepType);
