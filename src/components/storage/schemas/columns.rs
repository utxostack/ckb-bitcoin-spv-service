//! Constants which define low-level database column families.

/// Column families alias type
pub type Column = &'static str;

/// Total column number
pub const COUNT: usize = 2;

/// Column to store MMR for Bitcoin headers
pub const COLUMN_BITCOIN_HEADER_MMR: Column = "bitcoin-header-mmr";

/// Column to store Bitcoin headers
pub const COLUMN_BITCOIN_HEADERS: Column = "bitcoin-headers";
