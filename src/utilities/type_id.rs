use ckb_hash::{new_blake2b, BLAKE2B_LEN};
use ckb_types::{packed, prelude::*};

pub fn calculate_type_id(input: packed::CellInput, outputs_count: usize) -> [u8; BLAKE2B_LEN] {
    let mut blake2b = new_blake2b();
    blake2b.update(input.as_slice());
    blake2b.update(&(outputs_count as u64).to_le_bytes());
    let mut ret = [0; BLAKE2B_LEN];
    blake2b.finalize(&mut ret);
    ret
}
