//! Utilities.

mod key;
mod platform;
mod type_id;
pub(crate) mod value_parsers;

pub(crate) use key::Key256Bits;
pub(crate) use platform::try_raise_fd_limit;
pub(crate) use type_id::calculate_type_id;
