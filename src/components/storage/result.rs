use std::{fmt, result};

use ckb_bitcoin_spv_verifier::{molecule, utilities::mmr};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("low-level db error: {0}")]
    Db(#[from] rocksdb::Error),

    #[error("mmr error: {0}")]
    Mmr(#[from] mmr::lib::Error),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("data error: {0}")]
    Data(String),
}

pub type Result<T> = result::Result<T, Error>;

impl Error {
    pub fn storage<T: fmt::Display>(inner: T) -> Self {
        Self::Storage(inner.to_string())
    }

    pub fn data<T: fmt::Display>(inner: T) -> Self {
        Self::Data(inner.to_string())
    }

    pub fn not_found<T: fmt::Display>(name: T) -> Self {
        let msg = format!("{name} is not found");
        Self::Data(msg)
    }
}

impl From<molecule::error::VerificationError> for Error {
    fn from(error: molecule::error::VerificationError) -> Self {
        Self::data(error)
    }
}
