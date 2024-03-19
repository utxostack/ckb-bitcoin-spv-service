//! The `deploy` sub-command.

use ckb_jsonrpc_types::TransactionView;
use ckb_sdk::{
    transaction::{
        builder::{CkbTransactionBuilder, SimpleTransactionBuilder},
        input::InputIterator,
        signer::{SignContexts, TransactionSigner},
        TransactionBuilderConfiguration,
    },
    types::{
        Address as CkbAddress, AddressPayload as CkbAddressPayload, HumanCapacity, NetworkInfo,
    },
    SECP256K1,
};
use ckb_types::{bytes::Bytes, core::Capacity, packed, prelude::*};
use clap::Parser;
use secp256k1::SecretKey;

use crate::{
    prelude::*,
    result::{Error, Result},
    utilities::value_parsers,
};

#[derive(Parser)]
pub struct Args {
    #[clap(flatten)]
    pub(crate) common: super::CommonArgs,

    #[clap(flatten)]
    pub(crate) ckb: super::CkbArgs,

    /// A binary file, which should contain the Bitcoin SPV contract.
    ///
    /// The repository of the contract source code is
    /// <https://github.com/ckb-cell/ckb-bitcoin-spv-contracts>.
    ///
    /// ### Warnings
    ///
    /// Under the development phase, the compatibility has chance to be broken
    /// without any declaration.
    ///
    /// Please always use the latest versions of both the service and the contract.
    ///
    /// TODO Matched versions of the contracts should be list.
    #[arg(
        long = "contract-file", value_name = "CONTRACT_FILE", required = true,
        value_parser = value_parsers::BinaryFileValueParser
    )]
    pub(crate) contract_data: Bytes,

    /// The contract owner's address.
    #[arg(long="contract-owner", value_parser = value_parsers::AddressValueParser)]
    pub(crate) contract_owner: CkbAddress,

    /// Perform all steps without sending.
    #[arg(long, hide = true)]
    pub(crate) dry_run: bool,
}

impl Args {
    // TODO Deploy the Bitcoin SPV contract as type script.
    pub fn execute(&self) -> Result<()> {
        log::info!("Try to deploy a contract on CKB.");

        if self.contract_owner.network() != self.ckb.network {
            let msg = "The input addresses and the selected network are not matched.";
            return Err(Error::Cli(msg.to_owned()));
        }

        let contract_data_capacity = Capacity::bytes(self.contract_data.len()).map_err(|err| {
            let msg = format!("failed to calculate the capacity for contract data since {err}");
            Error::other(msg)
        })?;
        log::info!(
            "The contract requires {} CKBytes for its data.",
            HumanCapacity::from(contract_data_capacity.as_u64())
        );

        let network_info =
            NetworkInfo::new(self.ckb.network, self.ckb.ckb_endpoint.as_str().to_owned());
        let configuration = {
            let mut tmp = TransactionBuilderConfiguration::new_with_network(network_info.clone())?;
            tmp.fee_rate = self.ckb.fee_rate;
            tmp
        };

        let output = packed::CellOutput::new_builder()
            .lock((&self.contract_owner).into())
            .build_exact_capacity(contract_data_capacity)
            .map_err(|err| {
                let msg = format!("failed to calculate the capacity for the output since {err}");
                Error::other(msg)
            })?;

        let (deployer, deployer_key) = SecretKey::from_slice(&self.common.private_key.as_ref()[..])
            .map(|sk| {
                let pk = sk.public_key(&SECP256K1);
                let payload = CkbAddressPayload::from_pubkey(&pk);
                let address = CkbAddress::new(self.ckb.network, payload, true);
                (address, sk)
            })?;
        log::info!("The contract deployer is {deployer}.");

        let iterator = InputIterator::new_with_address(&[deployer], &network_info);
        let mut builder = SimpleTransactionBuilder::new(configuration, iterator);
        builder.add_output_and_data(output, self.contract_data.pack());
        let data_hash = packed::CellOutput::calc_data_hash(&self.contract_data);
        log::info!("The contract data hash is {data_hash:#x}.");

        let mut tx_with_groups = builder.build(&Default::default())?;

        TransactionSigner::new(&network_info).sign_transaction(
            &mut tx_with_groups,
            &SignContexts::new_sighash(vec![deployer_key]),
        )?;

        let tx_json = TransactionView::from(tx_with_groups.get_tx_view().clone());
        self.ckb
            .client()
            .send_transaction_ext(tx_json, self.dry_run)?;

        Ok(())
    }
}
