//! The `serve` sub-command.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    net::SocketAddr,
    num::NonZeroU32,
    path::PathBuf,
    thread, time,
};

use ckb_bitcoin_spv_verifier::types::{
    core::{BitcoinChainType, SpvClient},
    packed,
    prelude::Pack as VPack,
};
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
        Address as CkbAddress, AddressPayload as CkbAddressPayload, NetworkInfo, NetworkType,
        ScriptGroup, TransactionWithScriptGroups,
    },
    SECP256K1,
};
use ckb_types::{
    core::{Capacity, DepType},
    packed::{
        Byte32, Bytes as PackedBytes, BytesOpt, CellDep, CellInput, CellOutput, OutPoint,
        WitnessArgs,
    },
    prelude::*,
    H256,
};
use clap::Parser;
use secp256k1::SecretKey;

use crate::{
    components::{
        ApiServiceConfig, SpvOperation, SpvReorgInput, SpvService, SpvUpdateInput, Storage,
    },
    constants,
    prelude::*,
    result::{Error, Result},
    utilities::{try_raise_fd_limit, value_parsers},
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

    /// Don't update all headers in one CKB transaction,
    /// to avoid size limit or cycles limit.
    #[arg(long, default_value = "10")]
    pub(crate) spv_headers_update_limit: NonZeroU32,

    /// The out point of the Bitcoin SPV contract.
    ///
    /// This parameter will override the value in the storage.
    /// If this parameter is not provided, the value in the storage will be used.
    #[arg(long, value_parser = value_parsers::OutPointValueParser)]
    pub(crate) spv_contract_out_point: Option<OutPoint>,

    /// The batch size that how many Bitcoin headers will be downloaded at once.
    #[arg(long, default_value = "30")]
    pub(crate) bitcoin_headers_download_batch_size: u32,

    #[clap(flatten)]
    pub(crate) spv_owner_opt: super::SpvOwnerOpt,

    /// Perform all steps without sending.
    #[arg(long, hide = true)]
    pub(crate) dry_run: bool,
}

impl Args {
    pub fn execute(&self) -> Result<()> {
        log::info!("Starting the Bitcoin SPV service");

        self.spv_owner_opt.check_network(self.ckb.network)?;

        try_raise_fd_limit();

        let storage = Storage::new(&self.data_dir)?;
        if !storage.is_initialized()? {
            let msg = format!(
                "user-provided data directory \"{}\" is empty, please initialize it",
                self.data_dir.display()
            );
            return Err(Error::other(msg));
        }

        if let Some(ref spv_contract_out_point) = self.spv_contract_out_point {
            let spv_contract_cell_dep = CellDep::new_builder()
                .out_point(spv_contract_out_point.clone())
                .dep_type(DepType::Code.into())
                .build();
            let spv_type_script = storage.spv_contract_type_script()?;
            storage.save_cells_state(spv_type_script, spv_contract_cell_dep)?;
        }

        let ckb_cli = self.ckb.client();
        let btc_cli = self.bitcoin.client();

        let spv_service = SpvService {
            ckb_cli: ckb_cli.clone(),
            btc_cli: btc_cli.clone(),
            storage: storage.clone(),
        };

        let _api_service = ApiServiceConfig::new(self.listen_address).start(spv_service.clone());

        let mut prev_tx_hash: Option<H256> = None;

        loop {
            if !spv_service.sync_storage(self.bitcoin_headers_download_batch_size)? {
                continue;
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

            let (stg_tip_height, stg_tip_header) = spv_service.storage.tip_state()?;
            let stg_tip_hash = stg_tip_header.block_hash();
            log::info!("[storage] header#{stg_tip_height:07}, {stg_tip_hash:#x}; tip");

            match spv_service.select_operation()? {
                SpvOperation::Update(input) => {
                    let spv_tip_height = input.curr.client.headers_mmr_root.max_height;

                    match stg_tip_height.cmp(&spv_tip_height) {
                        Ordering::Less | Ordering::Equal => {
                            log::info!("No updates, sleep for a while");
                            self.take_a_break();
                            continue;
                        }
                        Ordering::Greater => {}
                    }

                    log::info!("Try to update SPV instance");

                    let (spv_client, spv_update) = storage.generate_spv_client_and_spv_update(
                        spv_tip_height,
                        self.spv_headers_update_limit,
                        input.info.get_flags()?,
                    )?;

                    let tx_hash =
                        self.update_spv_cells(&spv_service, input, spv_client, spv_update)?;

                    prev_tx_hash = Some(tx_hash);
                }
                SpvOperation::Reorg(input) => {
                    log::info!("Try to reorg SPV instance");

                    let spv_tip_height = input.curr.client.headers_mmr_root.max_height;

                    let flags = input.info.get_flags()?;
                    let limit = match flags.into() {
                        BitcoinChainType::Testnet => self.spv_headers_update_limit,
                        _ => NonZeroU32::MAX,
                    };
                    let (spv_client, spv_update) =
                        storage.generate_spv_client_and_spv_update(spv_tip_height, limit, flags)?;

                    let tx_hash =
                        self.reorg_spv_cells(&spv_service, input, spv_client, spv_update)?;

                    prev_tx_hash = Some(tx_hash);
                }
            }
        }

        // TODO Handle Ctrl-C and clean resources before exit.
    }

    pub(crate) fn update_spv_cells(
        &self,
        spv: &SpvService,
        update_input: SpvUpdateInput,
        mut spv_client: SpvClient,
        spv_update: packed::SpvUpdate,
    ) -> Result<H256> {
        let network_info =
            NetworkInfo::new(self.ckb.network, self.ckb.ckb_endpoint.as_str().to_owned());
        let configuration = {
            let mut tmp = TransactionBuilderConfiguration::new_with_network(network_info.clone())?;
            tmp.fee_rate = self.ckb.fee_rate()?;
            tmp
        };

        let (deployer, deployer_key) = SecretKey::from_slice(&self.ckb.private_key.as_ref()[..])
            .map(|sk| {
                let pk = sk.public_key(&SECP256K1);
                let payload = CkbAddressPayload::from_pubkey(&pk);
                let address = CkbAddress::new(self.ckb.network, payload, true);
                (address, sk)
            })?;
        log::debug!("The SPV cells will be updated by {deployer}");

        let iterator = InputIterator::new_with_address(&[deployer.clone()], &network_info);
        let mut tx_builder = TransactionBuilder::default();

        let spv_inputs = {
            let spv_info_input = CellInput::new_builder()
                .previous_output(update_input.info.cell.out_point.clone())
                .build();
            let spv_client_input = CellInput::new_builder()
                .previous_output(update_input.next.cell.out_point.clone())
                .build();
            vec![spv_info_input, spv_client_input]
        };
        tx_builder.inputs(spv_inputs);

        let spv_contract_cell_dep = spv.storage.spv_contract_cell_dep()?;
        tx_builder.cell_dep(spv_contract_cell_dep);
        let spv_client_curr_cell_dep = CellDep::new_builder()
            .out_point(update_input.curr.cell.out_point)
            .dep_type(DepType::Code.into())
            .build();
        tx_builder.cell_dep(spv_client_curr_cell_dep);

        // Try to insert cell deps for the lock scripts.
        match self.ckb.network {
            NetworkType::Mainnet | NetworkType::Testnet => {
                let known_cell_dep = if self.ckb.network == NetworkType::Mainnet {
                    constants::mainnet::known_cell_dep
                } else {
                    constants::testnet::known_cell_dep
                };
                #[allow(clippy::mutable_key_type)]
                let mut handled_code_hashes = HashSet::new();
                for cell in [&update_input.info.cell, &update_input.next.cell] {
                    let code_hash = cell.output.lock().code_hash();
                    if handled_code_hashes.insert(code_hash.clone()) {
                        if let Some(cell_dep) = known_cell_dep(&code_hash) {
                            tx_builder.cell_dep(cell_dep);
                        }
                    }
                }
            }
            _ => {
                log::warn!("Unsupport CKB network \"{}\"", self.ckb.network);
            }
        }

        let spv_outputs_data = {
            spv_client.id = update_input.next.client.id;
            let mut spv_info = update_input.info.info;
            spv_info.tip_client_id = spv_client.id;
            let packed_spv_info: packed::SpvInfo = spv_info.pack();
            let packed_spv_client: packed::SpvClient = spv_client.pack();
            vec![packed_spv_info.as_bytes(), packed_spv_client.as_bytes()]
        };
        let spv_outputs: Vec<CellOutput> = if let Some(lock_script) =
            self.spv_owner_opt.lock_script()
        {
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
            let info_output = update_input
                .info
                .cell
                .output
                .clone()
                .as_builder()
                .lock(lock_script.clone())
                .build_exact_capacity(spv_info_capacity)
                .map_err(|err| {
                    let msg = format!(
                        "failed to sum the total capacity for Bitcoin SPV info cell since {err}"
                    );
                    Error::other(msg)
                })?;
            let client_output = update_input
                .next
                .cell
                .output
                .clone()
                .as_builder()
                .lock(lock_script)
                .build_exact_capacity(spv_client_capacity)
                .map_err(|err| {
                    let msg = format!(
                        "failed to sum the total capacity for Bitcoin SPV client cell since {err}"
                    );
                    Error::other(msg)
                })?;
            vec![info_output, client_output]
        } else {
            vec![
                update_input.info.cell.output.clone(),
                update_input.next.cell.output.clone(),
            ]
        };
        tx_builder.outputs(spv_outputs);
        tx_builder.outputs_data(spv_outputs_data.iter().map(Pack::pack));

        #[allow(clippy::mutable_key_type)]
        let mut lock_groups: HashMap<Byte32, ScriptGroup> = HashMap::default();
        #[allow(clippy::mutable_key_type)]
        let mut type_groups: HashMap<Byte32, ScriptGroup> = HashMap::default();

        {
            let lock_script = update_input.info.cell.output.lock();
            lock_groups
                .entry(lock_script.calc_script_hash())
                .or_insert_with(|| ScriptGroup::from_lock_script(&lock_script))
                .input_indices
                .push(0);
            let lock_script = update_input.next.cell.output.lock();
            lock_groups
                .entry(lock_script.calc_script_hash())
                .or_insert_with(|| ScriptGroup::from_lock_script(&lock_script))
                .input_indices
                .push(1);
        }

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
                live_cell: update_input.info.cell.clone(),
                since: 0,
            };
            let spv_client_input = TransactionInput {
                live_cell: update_input.next.cell.clone(),
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
                log::debug!("Try to find the {input_index}-th live cell for {deployer}");
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
            let msg = format!("{deployer}'s live cells are not enough");
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

    pub(crate) fn reorg_spv_cells(
        &self,
        spv: &SpvService,
        reorg_input: SpvReorgInput,
        mut spv_client: SpvClient,
        spv_update: packed::SpvUpdate,
    ) -> Result<H256> {
        let network_info =
            NetworkInfo::new(self.ckb.network, self.ckb.ckb_endpoint.as_str().to_owned());
        let configuration = {
            let mut tmp = TransactionBuilderConfiguration::new_with_network(network_info.clone())?;
            tmp.fee_rate = self.ckb.fee_rate()?;
            tmp
        };

        let (deployer, deployer_key) = SecretKey::from_slice(&self.ckb.private_key.as_ref()[..])
            .map(|sk| {
                let pk = sk.public_key(&SECP256K1);
                let payload = CkbAddressPayload::from_pubkey(&pk);
                let address = CkbAddress::new(self.ckb.network, payload, true);
                (address, sk)
            })?;
        log::debug!("The SPV cells will be updated by {deployer}");

        let iterator = InputIterator::new_with_address(&[deployer.clone()], &network_info);
        let mut tx_builder = TransactionBuilder::default();

        let spv_inputs = {
            let spv_info_input = CellInput::new_builder()
                .previous_output(reorg_input.info.cell.out_point.clone())
                .build();
            let mut inputs = vec![spv_info_input];
            for client in &reorg_input.stale {
                let spv_client_input = CellInput::new_builder()
                    .previous_output(client.cell.out_point.clone())
                    .build();
                inputs.push(spv_client_input);
            }
            inputs
        };
        tx_builder.inputs(spv_inputs);

        let spv_contract_cell_dep = spv.storage.spv_contract_cell_dep()?;
        tx_builder.cell_dep(spv_contract_cell_dep);
        let spv_client_curr_cell_dep = CellDep::new_builder()
            .out_point(reorg_input.curr.cell.out_point)
            .dep_type(DepType::Code.into())
            .build();
        tx_builder.cell_dep(spv_client_curr_cell_dep);

        // Try to insert cell deps for the lock scripts.
        match self.ckb.network {
            NetworkType::Mainnet | NetworkType::Testnet => {
                let known_cell_dep = if self.ckb.network == NetworkType::Mainnet {
                    constants::mainnet::known_cell_dep
                } else {
                    constants::testnet::known_cell_dep
                };
                #[allow(clippy::mutable_key_type)]
                let mut handled_code_hashes = HashSet::new();
                let code_hash = reorg_input.info.cell.output.lock().code_hash();
                if handled_code_hashes.insert(code_hash.clone()) {
                    if let Some(cell_dep) = known_cell_dep(&code_hash) {
                        tx_builder.cell_dep(cell_dep);
                    }
                }
                for client in &reorg_input.stale {
                    let code_hash = client.cell.output.lock().code_hash();
                    if handled_code_hashes.insert(code_hash.clone()) {
                        if let Some(cell_dep) = known_cell_dep(&code_hash) {
                            tx_builder.cell_dep(cell_dep);
                        }
                    }
                }
            }
            _ => {
                log::warn!("Unsupport CKB network \"{}\"", self.ckb.network);
            }
        }

        let spv_outputs_data = {
            let mut spv_info = reorg_input.info.info.clone();
            spv_info.tip_client_id = reorg_input.info.next_tip_client_id();
            let packed_spv_info: packed::SpvInfo = spv_info.pack();
            let mut outputs_data = vec![packed_spv_info.as_bytes()];
            for client in &reorg_input.stale {
                spv_client.id = client.client.id;
                let packed_spv_client: packed::SpvClient = spv_client.pack();
                outputs_data.push(packed_spv_client.as_bytes());
            }
            outputs_data
        };
        let spv_outputs = if let Some(lock_script) = self.spv_owner_opt.lock_script() {
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
            let info_output = reorg_input
                .info
                .cell
                .output
                .clone()
                .as_builder()
                .lock(lock_script.clone())
                .build_exact_capacity(spv_info_capacity)
                .map_err(|err| {
                    let msg = format!(
                        "failed to sum the total capacity for Bitcoin SPV info cell since {err}"
                    );
                    Error::other(msg)
                })?;
            let mut outputs = vec![info_output];
            for client in &reorg_input.stale {
                let client_output = client
                    .cell
                    .output
                    .clone()
                    .as_builder()
                    .lock(lock_script.clone())
                    .build_exact_capacity(spv_client_capacity)
                    .map_err(|err| {
                        let msg = format!(
                            "failed to sum the total capacity for Bitcoin SPV client cell since {err}"
                        );
                        Error::other(msg)
                    })?;
                outputs.push(client_output);
            }
            outputs
        } else {
            let mut outputs = vec![reorg_input.info.cell.output.clone()];
            for client in &reorg_input.stale {
                outputs.push(client.cell.output.clone());
            }
            outputs
        };
        tx_builder.outputs(spv_outputs);
        tx_builder.outputs_data(spv_outputs_data.iter().map(Pack::pack));

        #[allow(clippy::mutable_key_type)]
        let mut lock_groups: HashMap<Byte32, ScriptGroup> = HashMap::default();
        #[allow(clippy::mutable_key_type)]
        let mut type_groups: HashMap<Byte32, ScriptGroup> = HashMap::default();

        {
            let lock_script = reorg_input.info.cell.output.lock();
            lock_groups
                .entry(lock_script.calc_script_hash())
                .or_insert_with(|| ScriptGroup::from_lock_script(&lock_script))
                .input_indices
                .push(0);
            for (index, client) in reorg_input.stale.iter().enumerate() {
                let lock_script = client.cell.output.lock();
                lock_groups
                    .entry(lock_script.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_lock_script(&lock_script))
                    .input_indices
                    .push(index + 1);
            }
        }

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
        tx_builder.witnesses(vec![PackedBytes::default(); reorg_input.stale.len()]);

        let mut change_builder =
            DefaultChangeBuilder::new(&configuration, (&deployer).into(), Vec::new());
        change_builder.init(&mut tx_builder);
        {
            let spv_info_input = TransactionInput {
                live_cell: reorg_input.info.cell.clone(),
                since: 0,
            };
            let _ = change_builder.check_balance(spv_info_input, &mut tx_builder);
            for client in &reorg_input.stale {
                let spv_client_input = TransactionInput {
                    live_cell: client.cell.clone(),
                    since: 0,
                };
                let _ = change_builder.check_balance(spv_client_input, &mut tx_builder);
            }
        };
        let contexts = HandlerContexts::default();

        let mut tx_with_groups = {
            let mut check_result = None;
            for (mut input_index, input) in iterator.enumerate() {
                input_index += 1 + reorg_input.stale.len(); // info + stale clients
                log::debug!("Try to find the {input_index}-th live cell for {deployer}");
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
            let msg = format!("{deployer}'s live cells are not enough");
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
