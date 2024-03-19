//! The `serve` sub-command.

use std::{cmp::Ordering, collections::HashMap, net::SocketAddr, path::PathBuf, thread, time};

use ckb_bitcoin_spv_verifier::types::{core::SpvClient, packed, prelude::Pack as VPack};
use ckb_jsonrpc_types::{Status, TransactionView};
use ckb_sdk::{
    core::TransactionBuilder,
    transaction::{
        builder::{ChangeBuilder, DefaultChangeBuilder},
        handler::HandlerContexts,
        input::{InputIterator, TransactionInput},
        signer::{SignContexts, TransactionSigner},
        TransactionBuilderConfiguration,
    },
    types::{
        Address as CkbAddress, AddressPayload as CkbAddressPayload, NetworkInfo, ScriptGroup,
        TransactionWithScriptGroups,
    },
    SECP256K1,
};
use ckb_types::{
    core::DepType,
    packed::{Byte32, Bytes as PackedBytes, BytesOpt, CellDep, CellInput, CellOutput, WitnessArgs},
    prelude::*,
    H256,
};
use clap::Parser;
use secp256k1::SecretKey;

use crate::{
    components::{ApiServiceConfig, SpvClientCell, SpvInfoCell, SpvService, Storage},
    prelude::*,
    result::{Error, Result},
};

#[derive(Parser)]
pub struct Args {
    #[clap(flatten)]
    pub(crate) common: super::CommonArgs,

    /// The directory, which stores all cached data.
    #[arg(long)]
    pub(crate) data_dir: PathBuf,

    #[clap(flatten)]
    pub(crate) ckb: super::CkbArgs,

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

    /// Take a break after download some bitcoin headers,
    /// to avoid some API limits.
    #[arg(long, default_value = "10")]
    pub(crate) bitcoin_headers_download_limit: u32,

    /// Don't update all headers in one CKB transaction,
    /// to avoid size limit or cycles limit.
    #[arg(long, default_value = "10")]
    pub(crate) spv_headers_update_limit: u32,

    /// Perform all steps without sending.
    #[arg(long, hide = true)]
    pub(crate) dry_run: bool,
}

impl Args {
    pub fn execute(&self) -> Result<()> {
        log::info!("Starting the Bitcoin SPV service.");

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

        let (mut stg_tip_height, _) = storage.tip_state()?;
        log::info!("Tip height in local storage is {stg_tip_height}");

        let mut prev_tx_hash: Option<H256> = None;

        loop {
            let btc_tip_height = btc_cli.get_tip_height()?;
            log::info!("Tip height from Bitcoin endpoint is {btc_tip_height}");

            let required_download = stg_tip_height < btc_tip_height;
            if required_download {
                let end_height_limit = stg_tip_height + self.bitcoin_headers_download_limit;
                let end_height = if end_height_limit < btc_tip_height {
                    end_height_limit
                } else {
                    btc_tip_height
                };
                let headers = btc_cli.get_headers(stg_tip_height + 1, end_height)?;
                (stg_tip_height, _) = storage.append_headers(headers)?;
            }

            if let Some(ref tx_hash) = prev_tx_hash {
                let tx_status = ckb_cli
                    .get_transaction_status(tx_hash.to_owned())?
                    .tx_status
                    .status;

                match tx_status {
                    Status::Pending | Status::Proposed => {
                        // To avoid PoolRejectedDuplicatedTransaction
                        log::debug!("Waiting for the previous transaction {tx_hash:#x}");
                        self.take_a_break();
                        continue;
                    }
                    Status::Committed | Status::Unknown | Status::Rejected => {}
                }
            }

            let (spv_info, spv_client_curr, spv_client_next) =
                spv_service.find_spv_cells_for_update()?;
            log::info!("Tip SPV client is {}", spv_client_curr.client.id);
            let spv_tip_height = spv_client_curr.client.headers_mmr_root.max_height;
            log::info!("Tip height in Bitcoin SPV instance is {spv_tip_height}");

            match stg_tip_height.cmp(&spv_tip_height) {
                Ordering::Less | Ordering::Equal => {
                    self.take_a_break();
                    continue;
                }
                Ordering::Greater => {}
            }

            let (spv_client, spv_update) = storage.generate_spv_client_and_spv_update(
                spv_tip_height,
                self.spv_headers_update_limit,
            )?;

            let tx_hash = self.update_spv_cells(
                &spv_service,
                (spv_info, spv_client_curr, spv_client_next),
                spv_client,
                spv_update,
            )?;

            prev_tx_hash = Some(tx_hash);
        }

        // TODO Handle Ctrl-C and clean resources before exit.
    }

    pub(crate) fn update_spv_cells(
        &self,
        spv: &SpvService,
        cells: (SpvInfoCell, SpvClientCell, SpvClientCell),
        mut spv_client: SpvClient,
        spv_update: packed::SpvUpdate,
    ) -> Result<H256> {
        let (spv_info_cell, spv_client_curr_cell, spv_client_next_cell) = cells;

        let network_info =
            NetworkInfo::new(self.ckb.network, self.ckb.ckb_endpoint.as_str().to_owned());
        let configuration = {
            let mut tmp = TransactionBuilderConfiguration::new_with_network(network_info.clone())?;
            tmp.fee_rate = self.ckb.fee_rate;
            tmp
        };

        let (deployer, deployer_key) = SecretKey::from_slice(&self.common.private_key.as_ref()[..])
            .map(|sk| {
                let pk = sk.public_key(&SECP256K1);
                let payload = CkbAddressPayload::from_pubkey(&pk);
                let address = CkbAddress::new(self.ckb.network, payload, true);
                (address, sk)
            })?;
        log::debug!("The SPV cells will be updated by {deployer}.");

        let iterator = InputIterator::new_with_address(&[deployer.clone()], &network_info);
        let mut tx_builder = TransactionBuilder::default();

        let spv_inputs = {
            let spv_info_input = CellInput::new_builder()
                .previous_output(spv_info_cell.cell.out_point.clone())
                .build();
            let spv_client_input = CellInput::new_builder()
                .previous_output(spv_client_next_cell.cell.out_point.clone())
                .build();
            vec![spv_info_input, spv_client_input]
        };
        tx_builder.inputs(spv_inputs);

        let spv_contract_cell_dep = spv.storage.spv_contract_cell_dep()?;
        let lock_contract_cell_dep = spv.storage.lock_contract_cell_dep()?;
        tx_builder.cell_dep(spv_contract_cell_dep);
        tx_builder.cell_dep(lock_contract_cell_dep);
        let spv_client_curr_cell_dep = CellDep::new_builder()
            .out_point(spv_client_curr_cell.cell.out_point)
            .dep_type(DepType::Code.into())
            .build();
        tx_builder.cell_dep(spv_client_curr_cell_dep);

        let spv_outputs: Vec<CellOutput> = vec![
            spv_info_cell.cell.output.clone(),
            spv_client_next_cell.cell.output.clone(),
        ];
        let spv_outputs_data = {
            spv_client.id = spv_client_next_cell.client.id;
            let mut spv_info = spv_info_cell.info;
            spv_info.tip_client_id = spv_client.id;
            let packed_spv_info: packed::SpvInfo = spv_info.pack();
            let packed_spv_client: packed::SpvClient = spv_client.pack();
            vec![packed_spv_info.as_bytes(), packed_spv_client.as_bytes()]
        };
        tx_builder.outputs(spv_outputs);
        tx_builder.outputs_data(spv_outputs_data.iter().map(Pack::pack));

        #[allow(clippy::mutable_key_type)]
        let mut lock_groups: HashMap<Byte32, ScriptGroup> = HashMap::default();
        #[allow(clippy::mutable_key_type)]
        let mut type_groups: HashMap<Byte32, ScriptGroup> = HashMap::default();

        for (output_idx, output) in tx_builder.get_outputs().clone().iter().enumerate() {
            if let Some(type_script) = &output.type_().to_opt() {
                type_groups
                    .entry(type_script.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(type_script))
                    .output_indices
                    .push(output_idx);
            }
        }

        let witness = {
            let type_args = BytesOpt::new_builder()
                .set(Some(Pack::pack(spv_update.as_slice())))
                .build();
            let witness_args = WitnessArgs::new_builder().output_type(type_args).build();
            Pack::pack(&witness_args.as_bytes())
        };
        tx_builder.witness(witness);
        tx_builder.witness(PackedBytes::default());

        let mut change_builder =
            DefaultChangeBuilder::new(&configuration, (&deployer).into(), Vec::new());
        change_builder.init(&mut tx_builder);
        {
            let spv_info_input = TransactionInput {
                live_cell: spv_info_cell.cell.clone(),
                since: 0,
            };
            let spv_client_input = TransactionInput {
                live_cell: spv_client_next_cell.cell.clone(),
                since: 0,
            };
            let _ = change_builder.check_balance(spv_info_input, &mut tx_builder);
            let _ = change_builder.check_balance(spv_client_input, &mut tx_builder);
        };
        let contexts = HandlerContexts::default();

        let mut tx_with_groups = {
            let mut check_result = None;
            for (mut input_index, input) in iterator.enumerate() {
                input_index += 2; // The first 2 inputs are SPV cells.
                log::debug!("Try to find the {input_index}-th live cell for {deployer}.");
                let input = input.map_err(|err| {
                    let msg = format!(
                        "failed to find {input_index}-th live cell for {deployer} since {err}"
                    );
                    Error::other(msg)
                })?;
                tx_builder.input(input.cell_input());
                tx_builder.witness(PackedBytes::default());

                let previous_output = input.previous_output();
                let lock_script = previous_output.lock();
                lock_groups
                    .entry(lock_script.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_lock_script(&lock_script))
                    .input_indices
                    .push(input_index);

                if change_builder.check_balance(input, &mut tx_builder) {
                    let mut script_groups: Vec<ScriptGroup> = lock_groups
                        .into_values()
                        .chain(type_groups.into_values())
                        .collect();
                    for script_group in script_groups.iter_mut() {
                        for handler in configuration.get_script_handlers() {
                            for context in &contexts.contexts {
                                if handler.build_transaction(
                                    &mut tx_builder,
                                    script_group,
                                    context.as_ref(),
                                )? {
                                    break;
                                }
                            }
                        }
                    }
                    let tx_view = change_builder.finalize(tx_builder);

                    check_result = Some(TransactionWithScriptGroups::new(tx_view, script_groups));
                    break;
                }
            }
            check_result
        }
        .ok_or_else(|| {
            let msg = format!("{deployer}'s live cells are not enough.");
            Error::other(msg)
        })?;

        TransactionSigner::new(&network_info).sign_transaction(
            &mut tx_with_groups,
            &SignContexts::new_sighash(vec![deployer_key]),
        )?;

        let tx_json = TransactionView::from(tx_with_groups.get_tx_view().clone());
        let tx_hash = self
            .ckb
            .client()
            .send_transaction_ext(tx_json, self.dry_run)?;

        Ok(tx_hash)
    }

    fn take_a_break(&self) {
        thread::sleep(time::Duration::from_secs(self.interval));
    }
}
