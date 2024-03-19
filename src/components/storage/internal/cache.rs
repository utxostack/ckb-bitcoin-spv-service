//! Memory cache for the storage.

use std::sync::RwLock;

#[derive(Default)]
pub(crate) struct Cache {
    pub(crate) base_bitcoin_height: RwLock<Option<u32>>,
    // TODO Cache headers by their heights.
}
