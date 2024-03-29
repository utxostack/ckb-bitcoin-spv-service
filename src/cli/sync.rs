//! The `sync` sub-command.

use std::path::PathBuf;

use ckb_sdk::rpc::ResponseFormatGetter as _;
use ckb_types::{
    core::DepType,
    packed::{CellDep, CellOutput, OutPoint},
    prelude::*,
};
use clap::Parser;

use crate::{
    components::Storage,
    prelude::*,
    result::{Error, Result},
    utilities::value_parsers,
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

    /// The out point of the Bitcoin SPV contract.
    #[arg(long, value_parser = value_parsers::OutPointValueParser)]
    pub(crate) spv_contract_out_point: OutPoint,

    /// The out point of the lock contract.
    ///
    /// The lock contract has to satisfy that:
    /// - If total capacity of cells which use this lock script were not
    ///   decreased, any non-owner users can update them.
    #[arg(long, value_parser = value_parsers::OutPointValueParser)]
    pub(crate) lock_contract_out_point: OutPoint,

    /// An out point of any cell in the target Bitcoin SPV instance.
    #[arg(long, value_parser = value_parsers::OutPointValueParser)]
    pub(crate) spv_cell_out_point: OutPoint,
}

impl Args {
    pub fn execute(&self) -> Result<()> {
        log::info!("Sync data to local storage base on on-chain Bitcoin SPV instance");

        let ckb_cli = self.ckb.client();

        let input_tx_hash = self.spv_cell_out_point.tx_hash();
        let input_cell_index: u32 = self.spv_cell_out_point.index().unpack();
        let input_cell_output: CellOutput = ckb_cli
            .get_transaction(input_tx_hash.unpack())?
            .ok_or_else(|| {
                let msg = format!("CKB transaction {input_tx_hash:#x} is not existed");
                Error::other(msg)
            })?
            .transaction
            .ok_or_else(|| {
                let msg = format!("remote server replied empty for transaction {input_tx_hash:#x}");
                Error::other(msg)
            })?
            .get_value()?
            .inner
            .outputs
            .get(input_cell_index as usize)
            .ok_or_else(|| {
                let msg = format!(
                    "CKB transaction {input_tx_hash:#x} doesn't have \
                    the {input_cell_index}-th output"
                );
                Error::other(msg)
            })?
            .to_owned()
            .into();
        let spv_type_script = input_cell_output.type_().to_opt().ok_or_else(|| {
            let msg = format!(
                "input cell (tx-hash: {input_tx_hash:#x}, index: {input_cell_index}) \
                is not a SPV cell since no type script"
            );
            Error::other(msg)
        })?;

        let tip_spv_client_cell = ckb_cli.find_best_spv_client(spv_type_script.clone(), None)?;
        let start_height = tip_spv_client_cell.client.headers_mmr_root.min_height;

        let btc_cli = self.bitcoin.client();
        let start_header = btc_cli.get_block_header_by_height(start_height)?;

        let storage = Storage::new(&self.data_dir)?;
        let _ = storage.initialize_with(start_height, start_header)?;

        let spv_contract_cell_dep = CellDep::new_builder()
            .out_point(self.spv_contract_out_point.clone())
            .dep_type(DepType::Code.into())
            .build();
        let lock_contract_cell_dep = CellDep::new_builder()
            .out_point(self.lock_contract_out_point.clone())
            .dep_type(DepType::Code.into())
            .build();
        storage.save_cells_state(
            spv_type_script,
            spv_contract_cell_dep,
            lock_contract_cell_dep,
        )?;

        Ok(())
    }
}
