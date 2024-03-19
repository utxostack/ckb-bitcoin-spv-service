//! Storage for all cached data.

mod internal;
pub(crate) mod prelude;
mod result;
pub(crate) mod schemas;

pub use internal::Storage;
pub use result::Error;
