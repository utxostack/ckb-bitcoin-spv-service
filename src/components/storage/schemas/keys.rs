//! Keys for special values.

// TODO Tracks the current database version.
// pub const MIGRATION_VERSION_KEY: &[u8] = b"db-version";

/// The height of the first Bitcoin header.
pub const BASE_BITCOIN_HEIGHT: &[u8] = b"base-bitcoin-height";
/// The height of the tip Bitcoin header.
pub const TIP_BITCOIN_HEIGHT: &[u8] = b"tip-bitcoin-height";

/// The type script of the Bitcoin SPV contract.
pub const SPV_CONTRACT_TYPE_SCRIPT: &[u8] = b"spv-contract-type-script";
/// The cell dep of the Bitcoin SPV contract.
pub const SPV_CONTRACT_CELL_DEP: &[u8] = b"spv-contract-cell-dep";
/// The cell dep of the lock contract.
pub const LOCK_CONTRACT_CELL_DEP: &[u8] = b"lock-contract-cell-dep";
