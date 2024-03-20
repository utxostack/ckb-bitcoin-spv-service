//! The `init` sub-command.

use std::{collections::HashMap, path::PathBuf};

use bitcoin::blockdata::constants::DIFFCHANGE_INTERVAL;
use ckb_bitcoin_spv_verifier::{
    constants::FLAG_DISABLE_DIFFICULTY_CHECK,
    types::{core::Hash as BitcoinHash, packed, prelude::Pack as VPack},
};
use ckb_jsonrpc_types::TransactionView;
use ckb_sdk::{
    core::TransactionBuilder,
    transaction::{
        builder::{ChangeBuilder, DefaultChangeBuilder},
        handler::HandlerContexts,
        input::InputIterator,
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
    core::{Capacity, DepType, ScriptHashType},
    packed::{
        Byte32, Bytes as PackedBytes, BytesOpt, CellDep, CellOutput, OutPoint, Script, WitnessArgs,
    },
    prelude::*,
    H256,
};
use clap::Parser;
use secp256k1::SecretKey;

use crate::{
    components::Storage,
    prelude::*,
    result::{Error, Result},
    utilities::{calculate_type_id, value_parsers},
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

    /// The start height of the new Bitcoin SPV instance.
    ///
    /// This height should be multiples of number 2016.
    #[arg(long, required = true)]
    pub(crate) bitcoin_start_height: u32,

    /// How many SPV clients will be created for the new Bitcoin SPV instance.
    #[arg(long, required = true)]
    pub(crate) spv_clients_count: u8,

    /// The data hash of the Bitcoin SPV contract.
    #[arg(long, value_parser = value_parsers::H256ValueParser)]
    pub(crate) spv_contract_data_hash: H256,

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

    /// The owner of Bitcoin SPV cells.
    #[arg(long, value_parser = value_parsers::AddressValueParser)]
    pub(crate) spv_owner: CkbAddress,

    /// Disable the on-chain difficulty check.
    ///
    /// Warning
    ///
    /// For testing purpose only.
    /// Do NOT enable this flag in production environment.
    #[arg(long)]
    pub(crate) disable_difficulty_check: bool,

    /// Perform all steps without sending.
    #[arg(long, hide = true)]
    pub(crate) dry_run: bool,
}

impl Args {
    // TODO Split this method into several smaller methods.
    pub fn execute(&self) -> Result<()> {
        log::info!("Try to initialize a Bitcoin SPV instance on CKB.");

        self.check_inputs()?;
        log::info!("The bitcoin start height is {}", self.bitcoin_start_height);
        self.check_remotes()?;

        let btc_start_header = self
            .bitcoin
            .client()
            .check_then_fetch_header(self.bitcoin_start_height)?;

        let storage = Storage::new(&self.data_dir)?;
        let spv_client = storage.initialize_with(self.bitcoin_start_height, btc_start_header)?;

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
        log::info!("The contract deployer is {deployer}.");

        let spv_outputs_data = {
            let spv_info = packed::SpvInfo::new_builder().build();
            let mut outputs_data = vec![spv_info.as_bytes()];
            let mut spv_client = spv_client.clone();
            for id in 0..self.spv_clients_count {
                spv_client.id = id;
                let packed_client: packed::SpvClient = spv_client.pack();
                outputs_data.push(packed_client.as_bytes());
            }
            outputs_data
        };

        let mut iterator = InputIterator::new_with_address(&[deployer.clone()], &network_info);
        let mut tx_builder = TransactionBuilder::default();

        let spv_contract_cell_dep = CellDep::new_builder()
            .out_point(self.spv_contract_out_point.clone())
            .dep_type(DepType::Code.into())
            .build();
        let lock_contract_cell_dep = CellDep::new_builder()
            .out_point(self.lock_contract_out_point.clone())
            .dep_type(DepType::Code.into())
            .build();
        tx_builder.cell_dep(spv_contract_cell_dep.clone());

        log::debug!("Try to find the first live cell for {deployer}.");
        let input0 = iterator
            .next()
            .transpose()
            .map_err(|err| {
                let msg = format!("failed to find any live cell for {deployer} since {err}");
                Error::other(msg)
            })?
            .ok_or_else(|| {
                let msg = format!("{deployer} has no live cell.");
                Error::other(msg)
            })?;

        let spv_type_script = {
            let cells_count = usize::from(self.spv_clients_count) + 1;
            let type_id_array = calculate_type_id(input0.cell_input(), cells_count);
            let type_id = BitcoinHash::from_bytes_ref(&type_id_array);
            let mut flags = 0u8;
            if self.disable_difficulty_check {
                flags |= FLAG_DISABLE_DIFFICULTY_CHECK;
            }
            let args = packed::SpvTypeArgs::new_builder()
                .type_id(type_id.pack())
                .clients_count(self.spv_clients_count.into())
                .flags(flags.into())
                .build();
            Script::new_builder()
                .code_hash(self.spv_contract_data_hash.pack())
                .hash_type(ScriptHashType::Data1.into())
                .args(Pack::pack(&args.as_bytes()))
                .build()
        };

        storage.save_cells_state(
            spv_type_script.clone(),
            spv_contract_cell_dep,
            lock_contract_cell_dep,
        )?;

        let spv_outputs = {
            let spv_info_capacity = Capacity::bytes(spv_outputs_data[0].len()).map_err(|err| {
                let msg = format!(
                    "failed to calculate the capacity for Bitcoin SPV info cell since {err}"
                );
                Error::other(msg)
            })?;
            let spv_client_capacity =
                Capacity::bytes(spv_outputs_data[1].len()).map_err(|err| {
                    let msg = format!(
                        "failed to calculate the capacity for Bitcoin SPV client cell since {err}"
                    );
                    Error::other(msg)
                })?;
            let spv_cell = CellOutput::new_builder()
                .lock((&self.spv_owner).into())
                .type_(Some(spv_type_script).pack())
                .build();
            let spv_info = spv_cell
                .clone()
                .as_builder()
                .build_exact_capacity(spv_info_capacity)
                .map_err(|err| {
                    let msg = format!(
                        "failed to sum the total capacity for Bitcoin SPV info cell since {err}"
                    );
                    Error::other(msg)
                })?;
            let spv_client = spv_cell
                .as_builder()
                .build_exact_capacity(spv_client_capacity)
                .map_err(|err| {
                    let msg = format!(
                        "failed to sum the total capacity for Bitcoin SPV client cell since {err}"
                    );
                    Error::other(msg)
                })?;
            let mut outputs = vec![spv_client.clone(); usize::from(self.spv_clients_count) + 1];
            outputs[0] = spv_info;
            outputs
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

        let mut change_builder =
            DefaultChangeBuilder::new(&configuration, input0.live_cell.output.lock(), Vec::new());
        change_builder.init(&mut tx_builder);

        tx_builder.input(input0.cell_input());
        let previous_output0 = input0.previous_output();
        let lock_script0 = previous_output0.lock();
        lock_groups
            .entry(lock_script0.calc_script_hash())
            .or_insert_with(|| ScriptGroup::from_lock_script(&lock_script0))
            .input_indices
            .push(0);

        let witness = {
            let bootstrap = packed::SpvBootstrap::new_builder()
                .height(VPack::pack(&self.bitcoin_start_height))
                .header(btc_start_header.pack())
                .build();
            let type_args = BytesOpt::new_builder()
                .set(Some(Pack::pack(bootstrap.as_slice())))
                .build();
            let witness_args = WitnessArgs::new_builder().output_type(type_args).build();
            Pack::pack(&witness_args.as_bytes())
        };
        tx_builder.witness(witness);

        let contexts = HandlerContexts::default();

        let mut tx_with_groups = if change_builder.check_balance(input0, &mut tx_builder) {
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

            Some(TransactionWithScriptGroups::new(tx_view, script_groups))
        } else {
            let mut check_result = None;
            for (mut input_index, input) in iterator.enumerate() {
                input_index += 1; // The first input has been handled.
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
        self.ckb
            .client()
            .send_transaction_ext(tx_json, self.dry_run)?;

        Ok(())
    }

    fn check_inputs(&self) -> Result<()> {
        if self.spv_owner.network() != self.ckb.network {
            let msg = "The input addresses and the selected network are not matched.";
            return Err(Error::cli(msg));
        }

        if self.spv_clients_count < 3 {
            let msg = format!(
                "The Bitcoint SPV clients count should be 3 at least but got {}",
                self.spv_clients_count
            );
            return Err(Error::cli(msg));
        }

        if self.bitcoin_start_height % DIFFCHANGE_INTERVAL != 0 {
            let msg = format!(
                "invalid Bitcoint start height, expected multiples of \
                {DIFFCHANGE_INTERVAL} but got {}",
                self.bitcoin_start_height
            );
            return Err(Error::cli(msg));
        }

        Ok(())
    }

    fn check_remotes(&self) -> Result<()> {
        if self.spv_owner.network() != self.ckb.network {
            let msg = "The input addresses and the selected network are not matched.";
            return Err(Error::cli(msg));
        }

        if self.spv_clients_count < 3 {
            let msg = format!(
                "The Bitcoint SPV clients count should be 3 at least but got {}",
                self.spv_clients_count
            );
            return Err(Error::cli(msg));
        }

        if self.bitcoin_start_height % DIFFCHANGE_INTERVAL != 0 {
            let msg = format!(
                "invalid Bitcoint start height, expected multiples of \
                {DIFFCHANGE_INTERVAL} but got {}",
                self.bitcoin_start_height
            );
            return Err(Error::cli(msg));
        }

        Ok(())
    }
}
