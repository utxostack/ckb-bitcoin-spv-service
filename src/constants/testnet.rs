use std::{collections::HashMap, sync::OnceLock};

use ckb_types::{core::DepType, h256, packed, prelude::*};

use crate::constants::CodeHashAndItsCellDep;

const CODE_HASH_WITH_CELL_DEP_INFO: &[CodeHashAndItsCellDep] = &[(
    h256!("0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"),
    h256!("0x9660b35c0a97fc47debb73f68a4868d8108e226a669219b62cc34a8c213c9d57"),
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
