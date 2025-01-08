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

mod tests {
    use super::*;
    use ckb_types::packed::CellInput;
    use ckb_types::{h256, H256};

    #[test]
    fn test_calculate_type_id() {
        let previous_output = packed::OutPoint::new_builder()
            .tx_hash(
                h256!("0x806600be4ae8330e8ce3893f208d169684e0b998acf9549c07b9fd5357eb157e").pack(),
            )
            .index(1u32.pack())
            .build();

        let input = CellInput::new_builder()
            .previous_output(previous_output)
            .since(0u64.pack())
            .build();
        let outputs_count = 33;
        let type_id = calculate_type_id(input.clone(), outputs_count);

        // Expected value calculated manually or from a trusted source
        let expected_type_id: H256 =
            h256!("0xac4cd5342895c4861631310f2fd731b8f8c71682577e384b78ca3ee3361ec3a1");

        assert_eq!(type_id, expected_type_id.as_bytes());
    }
}
