use std::{collections::HashMap, sync::OnceLock};

use ckb_types::{core::DepType, h256, packed, prelude::*};

use crate::constants::CodeHashAndItsCellDep;

const CODE_HASH_WITH_CELL_DEP_INFO: &[CodeHashAndItsCellDep] = &[(
    h256!("0xd483925160e4232b2cb29f012e8380b7b612d71cf4e79991476b6bcf610735f6"),
    h256!("0x81e22f4bb39080b112e5efb18e3fad65ebea735eac2f9c495b7f4d3b4faa377d"),
    0,
    DepType::Code,
)];

pub(crate) fn known_cell_dep(code_hash: &packed::Byte32) -> Option<packed::CellDep> {
    static MAP: OnceLock<HashMap<packed::Byte32, packed::CellDep>> = OnceLock::new();
    MAP.get_or_init(|| {
        #[allow(clippy::mutable_key_type)]
        let mut map = HashMap::new();
        for (code_hash, tx_hash, tx_index, dep_type) in CODE_HASH_WITH_CELL_DEP_INFO {
            let out_point = packed::OutPoint::new_builder()
                .tx_hash(tx_hash.pack())
                .index(tx_index.pack())
                .build();
            let cell_dep = packed::CellDep::new_builder()
                .out_point(out_point)
                .dep_type((*dep_type).into())
                .build();
            map.insert(code_hash.pack(), cell_dep);
        }
        map
    })
    .get(code_hash)
    .cloned()
}
