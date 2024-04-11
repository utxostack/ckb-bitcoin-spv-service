//! The `watch` sub-command.

use std::{net::SocketAddr, path::PathBuf, thread, time};

use clap::Parser;

use crate::{
    components::{ApiServiceConfig, SpvService, Storage},
    prelude::*,
    result::{Error, Result},
    utilities::try_raise_fd_limit,
};

#[derive(Parser)]
pub struct Args {
    #[clap(flatten)]
    pub(crate) common: super::CommonArgs,

    /// The directory, which stores all cached data.
    #[arg(long)]
    pub(crate) data_dir: PathBuf,

    #[clap(flatten)]
    pub(crate) ckb: super::CkbRoArgs,

    #[clap(flatten)]
    pub(crate) bitcoin: super::BitcoinArgs,

    /// The JSON-RPC server's listen address.
    #[arg(long)]
    pub(crate) listen_address: SocketAddr,

    /// A interval in seconds.
    ///
    /// - When no better bitcoin blocks, waiting for several seconds.
    /// - After a CKB transaction is sent, waiting for several seconds.
    #[arg(long, default_value = "30")]
    pub(crate) interval: u64,

    /// The batch size that how many Bitcoin headers will be downloaded at once.
    #[arg(long, default_value = "30")]
    pub(crate) bitcoin_headers_download_batch_size: u32,
}

impl Args {
    pub fn execute(&self) -> Result<()> {
        log::info!("Starting the Bitcoin SPV service (readonly)");

        try_raise_fd_limit();

        let storage = Storage::new(&self.data_dir)?;
        if !storage.is_initialized()? {
            let msg = format!(
                "user-provided data directory \"{}\" is empty, please initialize it",
                self.data_dir.display()
            );
            return Err(Error::other(msg));
        }
        let ckb_cli = self.ckb.client();
        let btc_cli = self.bitcoin.client();

        let spv_service = SpvService {
            ckb_cli: ckb_cli.clone(),
            btc_cli: btc_cli.clone(),
            storage: storage.clone(),
        };

        let _api_service = ApiServiceConfig::new(self.listen_address).start(spv_service.clone());

        loop {
            if !spv_service.sync_storage(self.bitcoin_headers_download_batch_size)? {
                continue;
            }
            self.take_a_break();
        }

        // TODO Handle Ctrl-C and clean resources before exit.
    }

    fn take_a_break(&self) {
        thread::sleep(time::Duration::from_secs(self.interval));
    }
}
