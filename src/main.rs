use prometheus::{
    core::AtomicU64,
    core::{AtomicF64, GenericCounter, GenericCounterVec, GenericGauge},
    register_gauge_with_registry, register_histogram_with_registry,
    register_int_counter_vec_with_registry, register_int_counter_with_registry, Encoder, Histogram,
    Registry, TextEncoder,
};
use rouille::Response;
use sp_core::crypto::Pair;
use std::{collections::HashMap, error::Error, fmt, sync::Arc, thread};
use tfchain_client::{
    events::{
        KVEvent, SmartContractEvent, TFGridEvent, TfchainEvent, TftBridgeEvent, TftPriceEvent,
    },
    types::ContractData,
    Client,
};

fn main() {
    pretty_env_logger::init();
    //let dev_client = Arc::new(ChainClient::new(Network::Devnet));
    let test_client = Arc::new(ChainClient::new(Network::Testnet));
    let main_client = Arc::new(ChainClient::new(Network::Mainnet));

    let mut handles = Vec::with_capacity(3);
    // let client = dev_client.clone();
    // handles.push(thread::spawn(move || loop {
    //     if let Err(e) = client.process_finalized_blocks() {
    //         eprintln!("Dev client failed: {}", e);
    //     }
    // }));
    let client = test_client.clone();
    handles.push(thread::spawn(move || loop {
        if let Err(e) = client.process_finalized_blocks() {
            eprintln!("Test client failed: {}", e);
        }
    }));
    let client = main_client.clone();
    handles.push(thread::spawn(move || loop {
        if let Err(e) = client.process_finalized_blocks() {
            eprintln!("Main client failed: {}", e);
        }
    }));

    rouille::start_server("[::]:9101", move |request| {
        if request.url() != "/metrics" {
            return Response::text("Not Found").with_status_code(404);
        }
        if request.method() != "GET" {
            return Response::text("Method Not Allowed").with_status_code(405);
        }

        let mut buffer = vec![];
        let encoder = TextEncoder::new();
        // let metric_families = dev_client.registry.gather();
        // encoder.encode(&metric_families, &mut buffer).unwrap();
        let metric_families = test_client.registry.gather();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        let metric_families = main_client.registry.gather();
        encoder.encode(&metric_families, &mut buffer).unwrap();

        Response::text(String::from_utf8(buffer).unwrap())
    });
}

pub struct ChainClient {
    client: Client<sp_core::sr25519::Pair, tfchain_client::runtimes::runtime::Event>,
    registry: Registry,
    metrics: Metrics,
    network: Network,
}

impl ChainClient {
    pub fn new(network: Network) -> Self {
        let (registry, metrics) = ChainClient::setup_metrics(network).unwrap();
        Self {
            client: Client::new(network.url().to_string(), None),
            registry,
            metrics,
            network,
        }
    }

    pub fn process_finalized_blocks(
        &self,
    ) -> Result<std::convert::Infallible, Box<dyn Error + 'static>> {
        loop {
            for finalized_header in self.client.finalized_block_headers()? {
                for event in self
                    .client
                    .get_block_events(Some(finalized_header.hash()))?
                {
                    match event {
                        TfchainEvent::SmartContract(sce) => self.process_smart_contract_event(sce),
                        TfchainEvent::TFGrid(ge) => self.process_grid_event(*ge),
                        TfchainEvent::TftPriceEvent(pe) => self.process_price_event(pe),
                        TfchainEvent::KVStore(kve) => self.process_key_value_event(kve),
                        TfchainEvent::TftBridgeEvent(tbe) => self.process_bridge_event(tbe),
                        _ => {}
                    }
                }
                self.metrics.blocks_finalized.inc();
            }
        }
    }

    fn process_smart_contract_event(&self, sce: SmartContractEvent) {
        match sce {
            SmartContractEvent::TokensBurned(_, amount) => {
                self.metrics.smart_contract_events.tokens_burned.inc();
                self.metrics.contract_tft_burned.inc_by(amount.as_u64());
            }
            SmartContractEvent::ContractBilled(bill) => {
                self.metrics.smart_contract_events.contract_bill.inc();
                self.metrics
                    .contract_tft_billed
                    .inc_by(bill.amount_billed as u64);
                self.metrics
                    .bill_cost_historgram
                    .observe(bill.amount_billed as f64 / 10_000_000.);
            }
            SmartContractEvent::ConsumptionReportReceived(report) => {
                self.metrics
                    .smart_contract_events
                    .consumption_report_received
                    .inc();

                self.metrics
                    .capacity_consumption
                    .with_label_values(&["cru"])
                    .inc_by(report.cru);
                self.metrics
                    .capacity_consumption
                    .with_label_values(&["mru"])
                    .inc_by(report.mru);
                self.metrics
                    .capacity_consumption
                    .with_label_values(&["sru"])
                    .inc_by(report.sru);
                self.metrics
                    .capacity_consumption
                    .with_label_values(&["hru"])
                    .inc_by(report.hru);
                self.metrics
                    .capacity_consumption
                    .with_label_values(&["nru"])
                    .inc_by(report.nru);
            }
            SmartContractEvent::ContractCreated(contract) => {
                // TODO: public ips
                match contract.contract_type {
                    ContractData::NodeContract(_) => {
                        &self.metrics.smart_contract_events.node_contract_created
                    }
                    ContractData::NameContract(_) => {
                        &self.metrics.smart_contract_events.name_contract_created
                    }
                    ContractData::RentContract(_) => {
                        &self.metrics.smart_contract_events.rent_contract_created
                    }
                }
                .inc();
            }
            SmartContractEvent::ContractUpdated(contract) => {
                match contract.contract_type {
                    ContractData::NodeContract(_) => {
                        &self.metrics.smart_contract_events.node_contract_updated
                    }
                    ContractData::NameContract(_) => {
                        &self.metrics.smart_contract_events.name_contract_updated
                    }
                    ContractData::RentContract(_) => {
                        &self.metrics.smart_contract_events.rent_contract_updated
                    }
                }
                .inc();
            }
            SmartContractEvent::ContractDeployed(_, _) => {
                // TODO: used?
                self.metrics.smart_contract_events.contract_deployed.inc();
            }
            SmartContractEvent::NodeContractCanceled(_, _, _) => {
                self.metrics
                    .smart_contract_events
                    .node_contract_cancelled
                    .inc();
            }
            SmartContractEvent::NameContractCanceled(_) => {
                self.metrics
                    .smart_contract_events
                    .name_contract_cancelled
                    .inc();
            }
            SmartContractEvent::RentContractCancelled(_) => {
                self.metrics
                    .smart_contract_events
                    .rent_contract_cancelled
                    .inc();
            }
            SmartContractEvent::IPsFreed(_, ip_list) => {
                // TODO: used?
                self.metrics.smart_contract_events.ips_freed.inc();
                self.metrics
                    .ip_events
                    .with_label_values(&["freed"])
                    .inc_by(ip_list.len() as u64);
            }
            SmartContractEvent::IPsReserved(_, ip_list) => {
                // TODO: used?
                self.metrics.smart_contract_events.ips_reserved.inc();
                self.metrics
                    .ip_events
                    .with_label_values(&["reserved"])
                    .inc_by(ip_list.len() as u64);
            }
            SmartContractEvent::NruConsumption(_, _, _, nru) => {
                self.metrics.smart_contract_events.nru_consumption.inc();
                self.metrics
                    .capacity_consumption
                    .with_label_values(&["nru"])
                    .inc_by(nru);
            }
            SmartContractEvent::UpdatedUsedResources(contract, resources) => {
                self.metrics
                    .smart_contract_events
                    .update_used_resouces
                    .inc();
                // TODO: set used resources somehow
            }
            SmartContractEvent::ContractGracePeriodStarted(_, _, _, _) => {
                self.metrics
                    .smart_contract_events
                    .contract_grace_period_started
                    .inc();
            }
            SmartContractEvent::ContractGracePeriodEnded(_, _, _) => {
                self.metrics
                    .smart_contract_events
                    .contract_grace_period_ended
                    .inc();
            }
        };
    }

    fn process_grid_event(&self, ge: TFGridEvent) {
        match ge {
            TFGridEvent::FarmStored(_) => {
                // TODO: keep track of farm ID?
                self.metrics.grid_events.new_farm.inc();
            }
            TFGridEvent::FarmUpdated(_) => {
                self.metrics.grid_events.farm_updated.inc();
            }
            TFGridEvent::FarmDeleted(_) => {
                self.metrics.grid_events.farm_deleted.inc();
            }
            TFGridEvent::FarmPayoutV2AddressRegistered(_, _) => {
                self.metrics.grid_events.farm_payout_address_set.inc();
            }
            TFGridEvent::NodeStored(node) => {
                self.metrics.grid_events.new_node.inc();

                self.metrics
                    .new_node_capacity
                    .with_label_values(&["cru"])
                    .inc_by(node.resources.cru);
                self.metrics
                    .new_node_capacity
                    .with_label_values(&["mru"])
                    .inc_by(node.resources.mru);
                self.metrics
                    .new_node_capacity
                    .with_label_values(&["sru"])
                    .inc_by(node.resources.sru);
                self.metrics
                    .new_node_capacity
                    .with_label_values(&["hru"])
                    .inc_by(node.resources.hru);
            }
            TFGridEvent::NodeUpdated(_) => {
                self.metrics.grid_events.node_updated.inc();
            }
            TFGridEvent::NodeDeleted(_) => {
                self.metrics.grid_events.node_deleted.inc();
            }
            TFGridEvent::NodeUptimeReported(_, _, uptime) => {
                self.metrics.grid_events.node_uptime_reported.inc();

                self.metrics.uptime_histogram.observe(uptime as f64);
            }
            TFGridEvent::NodePublicConfigStored(_, _) => {
                self.metrics.grid_events.public_config_updated.inc();
            }
            TFGridEvent::TwinStored(_) => {
                // TODO: keep track of twin ID?
                self.metrics.grid_events.new_twin.inc();
            }
            TFGridEvent::TwinUpdated(_) => {
                self.metrics.grid_events.twin_updated.inc();
            }
            TFGridEvent::TwinDeleted(_) => {
                self.metrics.grid_events.twin_deleted.inc();
            }
            TFGridEvent::EntityStored(_) => {
                // TODO: keep track of entity ID?
                self.metrics.grid_events.new_entity.inc();
            }
            TFGridEvent::EntityUpdated(_) => {
                self.metrics.grid_events.entity_updated.inc();
            }
            TFGridEvent::EntityDeleted(_) => {
                self.metrics.grid_events.entity_deleted.inc();
            }
            TFGridEvent::TwinEntityStored(_, _, _) => {
                self.metrics.grid_events.twin_entity_link_created.inc();
            }
            TFGridEvent::TwinEntityRemoved(_, _) => {
                self.metrics.grid_events.twin_entity_link_deleted.inc();
            }
            TFGridEvent::PricingPolicyStored(_) => {
                self.metrics.grid_events.pricing_policy_updated.inc();
            }
            TFGridEvent::FarmingPolicyStored(_) => {
                self.metrics.grid_events.farming_policy_updated.inc();
            }
            TFGridEvent::CertificationCodeStored(_) => {
                self.metrics.grid_events.certificaton_code_updated.inc();
            }
            TFGridEvent::FarmMarkedAsDedicated(_) => {
                self.metrics.grid_events.farm_marked_as_dedicated.inc();
            }
            TFGridEvent::ConnectionPriceSet(_) => {
                self.metrics.grid_events.connection_price_set.inc();
            }
            TFGridEvent::NodeCertificationSet(_, _) => {
                self.metrics.grid_events.node_certification_set.inc();
            }
            TFGridEvent::NodeCertifierAdded(_) => {
                self.metrics.grid_events.node_certifier_added.inc();
            }
            TFGridEvent::NodeCertifierRemoved(_) => {
                self.metrics.grid_events.node_certifier_removed.inc();
            }
            TFGridEvent::FarmingPolicyUpdated(_) => {
                self.metrics.grid_events.farming_policy_updated.inc();
            }
            TFGridEvent::FarmingPolicySet(_, _) => {
                self.metrics.grid_events.farming_policy_set.inc();
            }
            TFGridEvent::FarmCertificationSet(_, _) => {
                self.metrics.grid_events.farm_certification_set.inc();
            }
        }
    }

    fn process_price_event(&self, pe: TftPriceEvent) {
        match pe {
            TftPriceEvent::PriceStored(price) => {
                self.metrics.tft_price.set(price);
            }
            TftPriceEvent::OffchainWorkerExecuted(_) => {}
        }
    }

    fn process_key_value_event(&self, kve: KVEvent) {
        match kve {
            KVEvent::EntrySet(_, key, value) => {
                self.metrics.key_value_events.entry_set.inc();

                self.metrics.key_size_histogram.observe(key.len() as f64);
                self.metrics
                    .value_size_histogram
                    .observe(value.len() as f64);
            }
            KVEvent::EntryGot(_, _, _) => {
                self.metrics.key_value_events.entry_read.inc();
            }
            KVEvent::EntryTaken(_, _, _) => {
                self.metrics.key_value_events.entry_deleted.inc();
            }
        }
    }

    fn process_bridge_event(&self, tbe: TftBridgeEvent) {
        match tbe {
            TftBridgeEvent::MintTransactionProposed(_, _, amount) => {
                self.metrics.bridge_events.mint_tx_proposed.inc();
                self.metrics
                    .mint_amount_histogram
                    .observe(amount.as_u64() as f64 / 10_000_000.);
            }
            TftBridgeEvent::MintTransactionVoted(_) => {
                self.metrics.bridge_events.mint_tx_voted.inc();
            }
            TftBridgeEvent::MintCompleted(_) => {
                self.metrics.bridge_events.mint_tx_completed.inc();
            }
            TftBridgeEvent::MintTransactionExpired(_, _, _) => {
                self.metrics.bridge_events.mint_tx_expired.inc();
            }
            TftBridgeEvent::BurnTransactionCreated(_, _, amount) => {
                self.metrics.bridge_events.burn_tx_created.inc();
                self.metrics
                    .burn_amount_histogram
                    .observe(amount.as_u64() as f64 / 10_000_000.);
            }
            TftBridgeEvent::BurnTransactionProposed(_, _, _) => {
                self.metrics.bridge_events.burn_tx_proposed.inc();
            }
            TftBridgeEvent::BurnTransactionSignatureAdded(_, _) => {
                self.metrics.bridge_events.burn_tx_sig_added.inc();
            }
            TftBridgeEvent::BurnTransactionReady(_) => {
                self.metrics.bridge_events.burn_tx_ready.inc();
            }
            TftBridgeEvent::BurnTransactionProcessed(_) => {
                self.metrics.bridge_events.burn_tx_processed.inc();
            }
            TftBridgeEvent::BurnTransactionExpired(_, _, _) => {
                self.metrics.bridge_events.burn_tx_expired.inc();
            }
            TftBridgeEvent::RefundTransactionCreated(_, _, amount) => {
                self.metrics.bridge_events.refund_tx_created.inc();
                self.metrics
                    .refund_amount_histogram
                    .observe(amount.as_u64() as f64 / 10_000_000.);
            }
            TftBridgeEvent::RefundTransactionsignatureAdded(_, _) => {
                self.metrics.bridge_events.refund_tx_sig_added.inc();
            }
            TftBridgeEvent::RefundTransactionReady(_) => {
                self.metrics.bridge_events.refund_tx_ready.inc();
            }
            TftBridgeEvent::RefundTransactionProcessed(_) => {
                self.metrics.bridge_events.refund_tx_processed.inc();
            }
            TftBridgeEvent::RefundTransactionExpired(_, _, _) => {
                self.metrics.bridge_events.refund_tx_expired.inc();
            }
        }
    }

    fn setup_metrics(network: Network) -> Result<(Registry, Metrics), Box<dyn Error + 'static>> {
        let mut labels = HashMap::new();
        labels.insert("network".to_string(), network.to_string());
        let r = Registry::new_custom(Some("tfchain".to_string()), Some(labels))?;

        let grid_events_vec = register_int_counter_vec_with_registry!(
            "grid_events",
            "Amount of contract events that happened on chain regarding the grid module",
            &["event"],
            r
        )
        .unwrap();

        let grid_events = GridEvents {
            new_farm: grid_events_vec.with_label_values(&["new farm"]),
            farm_updated: grid_events_vec.with_label_values(&["farm updated"]),
            farm_deleted: grid_events_vec.with_label_values(&["farm deleted"]),
            farm_payout_address_set: grid_events_vec
                .with_label_values(&["farm stellar address set"]),
            new_node: grid_events_vec.with_label_values(&["new node"]),
            node_updated: grid_events_vec.with_label_values(&["node updated"]),
            node_deleted: grid_events_vec.with_label_values(&["node deleted"]),
            node_uptime_reported: grid_events_vec.with_label_values(&["node uptime reported"]),
            public_config_updated: grid_events_vec.with_label_values(&["public config updated"]),
            new_twin: grid_events_vec.with_label_values(&["new twin"]),
            twin_updated: grid_events_vec.with_label_values(&["twin updated"]),
            twin_deleted: grid_events_vec.with_label_values(&["twin deleted"]),
            new_entity: grid_events_vec.with_label_values(&["new entity"]),
            entity_updated: grid_events_vec.with_label_values(&["entity updated"]),
            entity_deleted: grid_events_vec.with_label_values(&["entity deleted"]),
            twin_entity_link_created: grid_events_vec
                .with_label_values(&["twin-entity link created"]),
            twin_entity_link_deleted: grid_events_vec
                .with_label_values(&["twin-entity link removed"]),
            pricing_policy_updated: grid_events_vec.with_label_values(&["pricing policy updated"]),
            farming_policy_updated: grid_events_vec.with_label_values(&["farming policy updated"]),
            certificaton_code_updated: grid_events_vec
                .with_label_values(&["certification code updated"]),
            farm_marked_as_dedicated: grid_events_vec
                .with_label_values(&["farm marked as dedicated"]),
            connection_price_set: grid_events_vec.with_label_values(&["connection price set"]),
            node_certification_set: grid_events_vec.with_label_values(&["node certification set"]),
            node_certifier_added: grid_events_vec.with_label_values(&["node certifier added"]),
            node_certifier_removed: grid_events_vec.with_label_values(&["node certifier removed"]),
            farming_policy_set: grid_events_vec.with_label_values(&["farming policy set"]),
            farm_certification_set: grid_events_vec.with_label_values(&["farm certification set"]),
        };

        let smart_contract_events_vec = register_int_counter_vec_with_registry!(
            "smart_contract_events",
            "Amount of contract events that happened on chain regarding the smart contract module",
            &["event"],
            r
        )
        .unwrap();

        let smart_contract_events = SmartContractEvents {
            tokens_burned: smart_contract_events_vec.with_label_values(&["tokens burned"]),
            contract_bill: smart_contract_events_vec.with_label_values(&["contract bill"]),
            consumption_report_received: smart_contract_events_vec
                .with_label_values(&["consumption report received"]),
            node_contract_created: smart_contract_events_vec
                .with_label_values(&["node contract created"]),
            name_contract_created: smart_contract_events_vec
                .with_label_values(&["name contract created"]),
            rent_contract_created: smart_contract_events_vec
                .with_label_values(&["rent contract created"]),
            node_contract_updated: smart_contract_events_vec
                .with_label_values(&["node contract updated"]),
            name_contract_updated: smart_contract_events_vec
                .with_label_values(&["name contract updated"]),
            rent_contract_updated: smart_contract_events_vec
                .with_label_values(&["rent contract updated"]),
            node_contract_cancelled: smart_contract_events_vec
                .with_label_values(&["node contract cancelled"]),
            name_contract_cancelled: smart_contract_events_vec
                .with_label_values(&["name contract cancelled"]),
            rent_contract_cancelled: smart_contract_events_vec
                .with_label_values(&["rent contract cancelled"]),
            contract_deployed: smart_contract_events_vec.with_label_values(&["contract deployed"]),
            contract_grace_period_started: smart_contract_events_vec
                .with_label_values(&["contract grace period started"]),
            contract_grace_period_ended: smart_contract_events_vec
                .with_label_values(&["contract grace period ended"]),
            ips_freed: smart_contract_events_vec.with_label_values(&["IPs freed"]),
            ips_reserved: smart_contract_events_vec.with_label_values(&["IPs reserved"]),
            nru_consumption: smart_contract_events_vec.with_label_values(&["NRU consumption"]),
            update_used_resouces: smart_contract_events_vec
                .with_label_values(&["updated used resources"]),
        };

        let key_value_events_vec = register_int_counter_vec_with_registry!(
            "key_value_store_events",
            "Amount of events that happened on chain regarding the key value storage  module",
            &["event"],
            r
        )
        .unwrap();

        let key_value_events = KeyValueEvents {
            entry_set: key_value_events_vec.with_label_values(&["entry set"]),
            entry_read: key_value_events_vec.with_label_values(&["entry read"]),
            entry_deleted: key_value_events_vec.with_label_values(&["entry deleted"]),
        };

        let bridge_events_vec = register_int_counter_vec_with_registry!(
            "bridge_events",
            "Amount of events that happened on chain regarding the key bridge module",
            &["event"],
            r
        )
        .unwrap();

        let bridge_events = BridgeEvents {
            mint_tx_proposed: bridge_events_vec.with_label_values(&["mint transaction proposed"]),
            mint_tx_voted: bridge_events_vec.with_label_values(&["mint transaction voted"]),
            mint_tx_completed: bridge_events_vec.with_label_values(&["mint transaction completed"]),
            mint_tx_expired: bridge_events_vec.with_label_values(&["mint transaction expired"]),
            burn_tx_created: bridge_events_vec.with_label_values(&["burn transaction created"]),
            burn_tx_proposed: bridge_events_vec.with_label_values(&["burn transaction proposed"]),
            burn_tx_ready: bridge_events_vec.with_label_values(&["burn transaction ready"]),
            burn_tx_sig_added: bridge_events_vec
                .with_label_values(&["burn transaction signature added"]),
            burn_tx_processed: bridge_events_vec.with_label_values(&["burn transaction processed"]),
            burn_tx_expired: bridge_events_vec.with_label_values(&["burn transaction expired"]),
            refund_tx_created: bridge_events_vec.with_label_values(&["refund transaction created"]),
            refund_tx_sig_added: bridge_events_vec
                .with_label_values(&["refund transaction signature added"]),
            refund_tx_ready: bridge_events_vec.with_label_values(&["refund transaction ready"]),
            refund_tx_processed: bridge_events_vec
                .with_label_values(&["refund transaction processed"]),
            refund_tx_expired: bridge_events_vec.with_label_values(&["refund transaction expired"]),
        };

        let ip_events = register_int_counter_vec_with_registry!(
            "ip_events",
            "Public ipv4 addresses being comissioned or decomissioned on chain",
            &["action"],
            r
        )
        .unwrap();

        let capacity_consumption = register_int_counter_vec_with_registry!(
            "capacity_consumption",
            "Capacity consumption reported by nodes",
            &["capacity_type"],
            r
        )
        .unwrap();

        let new_node_capacity = register_int_counter_vec_with_registry!(
            "new_node_capacity",
            "Capacity reported by new nodes on the grid (nodes which were never seen before)",
            &["capacity_type"],
            r
        )
        .unwrap();

        let contract_tft_billed = register_int_counter_with_registry!(
            "contract_tft_billed",
            "Amount of TFT billed for capacity payment",
            r
        )
        .unwrap();

        let contract_tft_burned = register_int_counter_with_registry!(
            "contract_tft_burned",
            "Amount of TFT burned as result of capacity payment",
            r
        )
        .unwrap();

        let uptime_histogram = register_histogram_with_registry!(
            "uptime_distribution",
            "The uptimes reported by nodes on chain",
            vec![3600., 21600., 86400., 604800., 2592000., 7776000., 31536000.],
            r
        )
        .unwrap();

        let bill_cost_historgram = register_histogram_with_registry!(
            "bill_cost_distribution",
            "The amount of tokens paid for capacity per bill",
            vec![0.1, 0.25, 0.5, 1., 2., 5., 10., 50.],
            r
        )
        .unwrap();

        let key_size_histogram = register_histogram_with_registry!(
            "key_size_distribution",
            "The size of keys set in the key value store",
            vec![10., 25., 50., 100., 200., 500., 1000.],
            r
        )
        .unwrap();
        let value_size_histogram = register_histogram_with_registry!(
            "value_size_distribution",
            "The size of values set in the key value store",
            vec![10., 25., 50., 100., 200., 500., 1000., 2000.],
            r
        )
        .unwrap();

        let mint_amount_histogram = register_histogram_with_registry!(
            "mint_amount_distribution",
            "The amount of tokens minted by the bridge. These are tokens transfered from stellar to tfchain",
            vec![1., 10., 50., 100., 500., 1000., 5_000., 25_000.],
            r,
        )
        .unwrap();
        let burn_amount_histogram = register_histogram_with_registry!(
            "burn_amount_distribution",
            "The amount of tokens burned by the bridge. These are tokens transfered from tfchain to stellar",
            vec![1., 10., 50., 100., 500., 1000., 5_000., 25_000.],
            r,
        )
        .unwrap();
        let refund_amount_histogram = register_histogram_with_registry!(
            "refund_amount_distribution",
            "The amount of tokens refunded by the bridge. These are atempted transfered from stellar to tfchain, which failed due to an invalid memo",
            vec![1., 10., 50., 100., 500., 1000., 5_000., 25_000.],
            r,
        )
        .unwrap();

        let tft_price =
            register_gauge_with_registry!("tft_price", "TFT price as set on chain", r).unwrap();

        let blocks_finalized = register_int_counter_with_registry!(
            "blocks_finalized",
            "Amount of blocks finalized",
            r
        )
        .unwrap();

        let metrics = Metrics {
            grid_events,
            smart_contract_events,
            key_value_events,
            bridge_events,
            ip_events,
            capacity_consumption,
            new_node_capacity,
            contract_tft_billed,
            contract_tft_burned,
            tft_price,
            blocks_finalized,
            uptime_histogram,
            bill_cost_historgram,
            key_size_histogram,
            value_size_histogram,
            mint_amount_histogram,
            burn_amount_histogram,
            refund_amount_histogram,
        };

        Ok((r, metrics))
    }
}

struct Metrics {
    grid_events: GridEvents,
    smart_contract_events: SmartContractEvents,
    key_value_events: KeyValueEvents,
    bridge_events: BridgeEvents,
    ip_events: GenericCounterVec<AtomicU64>,
    capacity_consumption: GenericCounterVec<AtomicU64>,
    new_node_capacity: GenericCounterVec<AtomicU64>,
    contract_tft_billed: GenericCounter<AtomicU64>,
    contract_tft_burned: GenericCounter<AtomicU64>,
    tft_price: GenericGauge<AtomicF64>,
    blocks_finalized: GenericCounter<AtomicU64>,
    uptime_histogram: Histogram,
    bill_cost_historgram: Histogram,
    key_size_histogram: Histogram,
    value_size_histogram: Histogram,
    mint_amount_histogram: Histogram,
    burn_amount_histogram: Histogram,
    refund_amount_histogram: Histogram,
}

struct GridEvents {
    new_farm: GenericCounter<AtomicU64>,
    farm_updated: GenericCounter<AtomicU64>,
    farm_deleted: GenericCounter<AtomicU64>,
    farm_payout_address_set: GenericCounter<AtomicU64>,
    new_node: GenericCounter<AtomicU64>,
    node_updated: GenericCounter<AtomicU64>,
    node_deleted: GenericCounter<AtomicU64>,
    node_uptime_reported: GenericCounter<AtomicU64>,
    public_config_updated: GenericCounter<AtomicU64>,
    new_twin: GenericCounter<AtomicU64>,
    twin_updated: GenericCounter<AtomicU64>,
    twin_deleted: GenericCounter<AtomicU64>,
    new_entity: GenericCounter<AtomicU64>,
    entity_updated: GenericCounter<AtomicU64>,
    entity_deleted: GenericCounter<AtomicU64>,
    twin_entity_link_created: GenericCounter<AtomicU64>,
    twin_entity_link_deleted: GenericCounter<AtomicU64>,
    pricing_policy_updated: GenericCounter<AtomicU64>,
    farming_policy_updated: GenericCounter<AtomicU64>,
    certificaton_code_updated: GenericCounter<AtomicU64>,
    farm_marked_as_dedicated: GenericCounter<AtomicU64>,
    connection_price_set: GenericCounter<AtomicU64>,
    node_certification_set: GenericCounter<AtomicU64>,
    node_certifier_added: GenericCounter<AtomicU64>,
    node_certifier_removed: GenericCounter<AtomicU64>,
    farming_policy_set: GenericCounter<AtomicU64>,
    farm_certification_set: GenericCounter<AtomicU64>,
}

struct SmartContractEvents {
    tokens_burned: GenericCounter<AtomicU64>,
    contract_bill: GenericCounter<AtomicU64>,
    consumption_report_received: GenericCounter<AtomicU64>,
    node_contract_created: GenericCounter<AtomicU64>,
    name_contract_created: GenericCounter<AtomicU64>,
    rent_contract_created: GenericCounter<AtomicU64>,
    node_contract_updated: GenericCounter<AtomicU64>,
    name_contract_updated: GenericCounter<AtomicU64>,
    rent_contract_updated: GenericCounter<AtomicU64>,
    node_contract_cancelled: GenericCounter<AtomicU64>,
    name_contract_cancelled: GenericCounter<AtomicU64>,
    rent_contract_cancelled: GenericCounter<AtomicU64>,
    contract_deployed: GenericCounter<AtomicU64>,
    contract_grace_period_started: GenericCounter<AtomicU64>,
    contract_grace_period_ended: GenericCounter<AtomicU64>,
    ips_freed: GenericCounter<AtomicU64>,
    ips_reserved: GenericCounter<AtomicU64>,
    nru_consumption: GenericCounter<AtomicU64>,
    update_used_resouces: GenericCounter<AtomicU64>,
}

struct KeyValueEvents {
    entry_set: GenericCounter<AtomicU64>,
    entry_read: GenericCounter<AtomicU64>,
    entry_deleted: GenericCounter<AtomicU64>,
}

struct BridgeEvents {
    mint_tx_proposed: GenericCounter<AtomicU64>,
    mint_tx_voted: GenericCounter<AtomicU64>,
    mint_tx_completed: GenericCounter<AtomicU64>,
    mint_tx_expired: GenericCounter<AtomicU64>,
    burn_tx_created: GenericCounter<AtomicU64>,
    burn_tx_proposed: GenericCounter<AtomicU64>,
    burn_tx_sig_added: GenericCounter<AtomicU64>,
    burn_tx_ready: GenericCounter<AtomicU64>,
    burn_tx_processed: GenericCounter<AtomicU64>,
    burn_tx_expired: GenericCounter<AtomicU64>,
    refund_tx_created: GenericCounter<AtomicU64>,
    refund_tx_sig_added: GenericCounter<AtomicU64>,
    refund_tx_ready: GenericCounter<AtomicU64>,
    refund_tx_processed: GenericCounter<AtomicU64>,
    refund_tx_expired: GenericCounter<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
pub enum Network {
    Mainnet,
    Testnet,
    Devnet,
}

impl Network {
    pub fn url(&self) -> &'static str {
        match self {
            &Network::Devnet => "wss://tfchain.dev.grid.tf",
            &Network::Testnet => "wss://tfchain.test.grid.tf",
            &Network::Mainnet => "wss://tfchain.grid.tf",
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Network::Devnet => "devnet",
            Network::Testnet => "testnet",
            Network::Mainnet => "mainnet",
        })
    }
}
