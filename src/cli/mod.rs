//! The command line argument.

use ckb_sdk::{rpc::CkbRpcClient, types::NetworkType};
use clap::{Parser, Subcommand};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use url::Url;

use crate::{
    components::BitcoinClient,
    result::Result,
    utilities::{value_parsers, Key256Bits},
};

mod deploy;
mod init;
mod serve;
mod sync;

#[derive(Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Commands {
    /// Deploy a contract on CKB.
    ///
    /// This command can be used to deploy any contract and;
    /// also, users can deploy the contract in their own way.
    Deploy(deploy::Args),
    /// Initialize a new Bitcoin SPV instance on CKB, and initialize local storage.
    Init(init::Args),
    /// Run a service to update a Bitcoin SPV instance base on local storage.
    Serve(serve::Args),
    /// Sync data to rebuild local storage base on an existed Bitcoin SPV instance.
    Sync(sync::Args),
}

#[derive(Parser)]
pub struct CommonArgs {
    #[command(flatten)]
    pub(crate) verbose: Verbosity<InfoLevel>,
}

#[derive(Parser)]
pub struct CkbArgs {
    /// CKB JSON-RPC APIs endpoint.
    #[arg(long)]
    pub(crate) ckb_endpoint: Url,

    /// The network type of the CKB chain which connected.
    #[arg(
        long = "network-type",
        value_parser = value_parsers::NetworkTypeValueParser,
        default_value = "testnet"
    )]
    pub network: NetworkType,

    /// The fee rate for CKB transactions.
    #[arg(long = "ckb-fee-rate", default_value = "1000")]
    pub(crate) fee_rate: u64,

    /// A binary file, which contains a secp256k1 private key.
    /// This private key will be used to provide all CKBytes.
    ///
    /// Tip: After starting the service, this file should be deleted, for safety.
    #[arg(long = "key-file", value_name = "KEY_FILE")]
    pub(crate) private_key: Key256Bits,
}

#[derive(Parser)]
pub struct CkbRoArgs {
    /// CKB JSON-RPC APIs endpoint.
    #[arg(long)]
    pub(crate) ckb_endpoint: Url,

    /// The network type of the CKB chain which connected.
    #[arg(
        long = "network-type",
        value_parser = value_parsers::NetworkTypeValueParser,
        default_value = "testnet"
    )]
    pub network: NetworkType,
}

#[derive(Parser)]
pub struct BitcoinArgs {
    /// Bitcoin JSON-RPC APIs endpoint.
    ///
    /// Required Methods: `getbestblockhash`, `getblockhash`, `getblockstats`, `getblockheader` and `gettxoutproof`.
    ///
    /// Ref: <https://developer.bitcoin.org/reference/rpc/index.html>
    #[arg(long = "bitcoin-endpoint", value_name = "BITCOIN_ENDPOINT")]
    pub(crate) endpoint: Url,
    /// Username for the Bitcoin JSON-RPC APIs endpoint.
    #[arg(
        long = "bitcoin-endpoint-username",
        value_name = "BITCOIN_ENDPOINT_USERNAME"
    )]
    pub(crate) username: Option<String>,
    /// Password for the Bitcoin JSON-RPC APIs endpoint.
    #[arg(
        long = "bitcoin-endpoint-password",
        value_name = "BITCOIN_ENDPOINT_PASSWORD"
    )]
    pub(crate) password: Option<String>,
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }

    pub fn execute(self) -> Result<()> {
        self.configure_logger();
        log::info!("Bitcoin SPV on CKB service is starting ...");
        match self.command {
            Commands::Deploy(args) => args.execute()?,
            Commands::Init(args) => args.execute()?,
            Commands::Serve(args) => args.execute()?,
            Commands::Sync(args) => args.execute()?,
        }
        log::info!("Bitcoin SPV on CKB service is stopped.");
        Ok(())
    }

    pub fn configure_logger(&self) {
        match self.command {
            Commands::Deploy(ref args) => args.common.configure_logger(),
            Commands::Init(ref args) => args.common.configure_logger(),
            Commands::Serve(ref args) => args.common.configure_logger(),
            Commands::Sync(ref args) => args.common.configure_logger(),
        }
    }
}

impl CommonArgs {
    pub fn configure_logger(&self) {
        env_logger::Builder::new()
            .filter_level(self.verbose.log_level_filter())
            .init();
    }
}

impl CkbArgs {
    pub fn client(&self) -> CkbRpcClient {
        CkbRpcClient::new(self.ckb_endpoint.as_str())
    }
}

impl CkbRoArgs {
    pub fn client(&self) -> CkbRpcClient {
        CkbRpcClient::new(self.ckb_endpoint.as_str())
    }
}

impl BitcoinArgs {
    pub fn client(&self) -> BitcoinClient {
        BitcoinClient::new(
            self.endpoint.clone(),
            self.username.clone(),
            self.password.clone(),
        )
    }
}
