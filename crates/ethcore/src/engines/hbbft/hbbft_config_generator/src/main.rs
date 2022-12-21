extern crate bincode;
#[macro_use]
extern crate clap;
extern crate ethcore;
extern crate ethereum_types;
extern crate ethkey;
extern crate ethstore;
extern crate hbbft;
extern crate parity_crypto;
extern crate rand;
extern crate rustc_hex;
extern crate serde;
extern crate serde_json;
extern crate toml;

mod keygen_history_helpers;

use clap::{App, Arg};
use ethstore::{KeyFile, SafeAccount};
use keygen_history_helpers::{enodes_to_pub_keys, generate_keygens, key_sync_history_data};
use parity_crypto::publickey::{Address, Generator, KeyPair, Public, Random, Secret};
use std::{
    collections::BTreeMap, convert::TryInto, fmt::Write, fs, num::NonZeroU32, str::FromStr,
    sync::Arc,
};
use toml::{map::Map, Value};

pub fn create_account() -> (Secret, Public, Address) {
    let acc = Random.generate();
    (
        acc.secret().clone(),
        acc.public().clone(),
        acc.address().clone(),
    )
}

pub struct Enode {
    secret: Secret,
    public: Public,
    address: Address,
    idx: usize,
    ip: String,
}

impl ToString for Enode {
    fn to_string(&self) -> String {
        // Example:
        // enode://30ccdeb8c31972f570e4eea0673cd08cbe7cefc5de1d70119b39c63b1cba33b48e494e9916c0d1eab7d296774f3573da46025d1accdef2f3690bc9e6659a34b4@192.168.0.101:30300
        let port = 30300usize + self.idx;
        format!("enode://{:x}@{}:{}", self.public, self.ip, port)
    }
}

fn generate_enodes(
    num_nodes: usize,
    private_keys: Vec<Secret>,
    external_ip: Option<&str>,
) -> BTreeMap<Public, Enode> {
    let mut map = BTreeMap::new();
    for i in 0..num_nodes {
        // Note: node 0 is a regular full node (not a validator) in the testnet setup, so we start at index 1.
        let idx = i + 1;
        let ip = match external_ip {
            Some(ip) => ip,
            None => "127.0.0.1",
        };
        let (secret, public, address) = if private_keys.len() > i {
            let acc = KeyPair::from_secret(private_keys[i].clone())
                .expect("Supplied secret must be valid!");
            (
                acc.secret().clone(),
                acc.public().clone(),
                acc.address().clone(),
            )
        } else {
            create_account()
        };
        println!("Debug, Secret: {:?}", secret);
        map.insert(
            public,
            Enode {
                secret,
                public,
                address,
                idx,
                ip: ip.into(),
            },
        );
    }
    // the map has the element order by their public key.
    // we reassign the idx here, so the index of the nodes follows
    // the same order like everything else.
    let mut new_index = 1;
    for public in map.iter_mut() {
        public.1.idx = new_index;
        new_index = new_index + 1;
    }
    map
}

fn to_toml_array(vec: Vec<&str>) -> Value {
    Value::Array(vec.iter().map(|s| Value::String(s.to_string())).collect())
}

fn to_toml(
    i: usize,
    config_type: &ConfigType,
    external_ip: Option<&str>,
    signer_address: &Address,
    total_num_of_nodes: usize,
    tx_queue_per_sender: Option<i64>,
) -> Value {
    let base_port = 30300i64;
    let base_rpc_port = 8540i64;
    let base_ws_port = 9540i64;

    let mut parity = Map::new();
    match config_type {
        ConfigType::PosdaoSetup => {
            parity.insert("chain".into(), Value::String("./spec/spec.json".into()));
            parity.insert("chain".into(), Value::String("./spec/spec.json".into()));
            let node_data_path = format!("parity-data/node{}", i);
            parity.insert("base_path".into(), Value::String(node_data_path));
        }
        _ => {
            parity.insert("chain".into(), Value::String("spec.json".into()));
            parity.insert("chain".into(), Value::String("spec.json".into()));
            let node_data_path = "data".to_string();
            parity.insert("base_path".into(), Value::String(node_data_path));
        }
    }

    let mut network = Map::new();
    network.insert("port".into(), Value::Integer(base_port + i as i64));
    match config_type {
        ConfigType::PosdaoSetup => {
            network.insert(
                "reserved_peers".into(),
                Value::String("parity-data/reserved-peers".into()),
            );
        }
        _ => {
            network.insert(
                "reserved_peers".into(),
                Value::String("reserved-peers".into()),
            );
        }
    }

    network.insert(
        "min_peers".into(),
        Value::Integer(total_num_of_nodes.try_into().unwrap()),
    );
    network.insert("max_peers".into(), Value::Integer(50));

    match external_ip {
        Some(extip) => {
            network.insert("allow_ips".into(), Value::String("public".into()));
            network.insert("nat".into(), Value::String(format!("extip:{}", extip)));
        }
        None => {
            network.insert("nat".into(), Value::String("none".into()));
            network.insert("interface".into(), Value::String("all".into()));
        }
    }

    let mut rpc = Map::new();
    rpc.insert("interface".into(), Value::String("all".into()));
    rpc.insert("cors".into(), to_toml_array(vec!["all"]));
    rpc.insert("hosts".into(), to_toml_array(vec!["all"]));
    let apis = to_toml_array(vec![
        "web3",
        "eth",
        "pubsub",
        "net",
        "parity",
        "parity_set",
        "parity_pubsub",
        "personal",
        "traces",
    ]);
    rpc.insert("apis".into(), apis);
    rpc.insert("port".into(), Value::Integer(base_rpc_port + i as i64));

    let mut websockets = Map::new();
    websockets.insert("interface".into(), Value::String("all".into()));
    websockets.insert("origins".into(), to_toml_array(vec!["all"]));
    websockets.insert("port".into(), Value::Integer(base_ws_port + i as i64));

    let mut ipc = Map::new();
    ipc.insert("disable".into(), Value::Boolean(true));

    let mut secretstore = Map::new();
    secretstore.insert("disable".into(), Value::Boolean(true));

    let signer_address = format!("{:?}", signer_address);

    let mut account = Map::new();
    match config_type {
        ConfigType::PosdaoSetup => {
            account.insert(
                "unlock".into(),
                to_toml_array(vec![
                    "0xbbcaa8d48289bb1ffcf9808d9aa4b1d215054c78",
                    "0x32e4e4c7c5d1cea5db5f9202a9e4d99e56c91a24",
                ]),
            );
            account.insert("password".into(), to_toml_array(vec!["config/password"]));
        }
        ConfigType::Docker => {
            account.insert("unlock".into(), to_toml_array(vec![&signer_address]));
            account.insert("password".into(), to_toml_array(vec!["password.txt"]));
        }
        _ => (),
    }

    let mut mining = Map::new();

    if config_type != &ConfigType::Rpc {
        mining.insert("engine_signer".into(), Value::String(signer_address));
    }

    mining.insert("force_sealing".into(), Value::Boolean(true));
    mining.insert("min_gas_price".into(), Value::Integer(1000000000));
    mining.insert(
        "gas_floor_target".into(),
        Value::String("1000000000".into()),
    );
    mining.insert("reseal_on_txs".into(), Value::String("none".into()));
    mining.insert("extra_data".into(), Value::String("Parity".into()));
    mining.insert("reseal_min_period".into(), Value::Integer(0));

    if let Some(tx_queue_per_sender_) = tx_queue_per_sender {
        mining.insert(
            "tx_queue_per_sender".into(),
            Value::Integer(tx_queue_per_sender_),
        );
    }

    let mut misc = Map::new();
    misc.insert(
        "logging".into(),
        Value::String("txqueue=trace,consensus=trace,engine=trace".into()),
    );
    misc.insert("log_file".into(), Value::String("parity.log".into()));

    let mut map = Map::new();
    map.insert("parity".into(), Value::Table(parity));
    map.insert("network".into(), Value::Table(network));
    map.insert("rpc".into(), Value::Table(rpc));
    map.insert("websockets".into(), Value::Table(websockets));
    map.insert("ipc".into(), Value::Table(ipc));
    map.insert("secretstore".into(), Value::Table(secretstore));
    map.insert("account".into(), Value::Table(account));
    map.insert("mining".into(), Value::Table(mining));
    map.insert("misc".into(), Value::Table(misc));
    Value::Table(map)
}

arg_enum! {
    #[derive(Debug, PartialEq)]
    enum ConfigType {
        PosdaoSetup,
        Docker,
        Rpc
    }
}

fn write_json_for_secret(secret: Secret, filename: String) {
    let json_key: KeyFile = SafeAccount::create(
        &KeyPair::from_secret(secret).unwrap(),
        [0u8; 16],
        &"test".into(),
        NonZeroU32::new(10240).expect("We know 10240 is not zero."),
        "Test".to_owned(),
        "{}".to_owned(),
    )
    .expect("json key object creation should succeed")
    .into();

    let serialized_json_key =
        serde_json::to_string(&json_key).expect("json key object serialization should succeed");
    fs::write(filename, serialized_json_key).expect("Unable to write json key file");
}

fn main() {
    let matches = App::new("hbbft parity config generator")
        .version("1.0")
        .author("David Forstenlechner <dforsten@gmail.com>, Thomas Haller <thomashaller@gmx.at>")
        .about("Generates n toml files for running a hbbft validator node network")
        .arg(
            Arg::with_name("validator_nodes")
                .help("The number of initial validators to generate")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("total_nodes")
                .help("The number of total validators to generate")
                .required(true)
                .index(2),
        )
        .arg(
            Arg::from_usage("<configtype> 'The ConfigType to use'")
                .possible_values(&ConfigType::variants())
                .index(3),
        )
        .arg(
            Arg::with_name("private_keys")
                .long("private_keys")
                .required(false)
                .takes_value(true)
                .multiple(true),
        )
        .arg(
            Arg::with_name("extip")
                .long("extip")
                .required(false)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("tx_queue_per_sender")
                .long("tx_queue_per_sender")
                .required(false)
                .takes_value(true),
        )
        .get_matches();

    let num_nodes_validators: usize = matches
        .value_of("validator_nodes")
        .expect("Number of validators input required")
        .parse()
        .expect("Validators must be of integer type");

    let num_nodes_total: usize = matches
        .value_of("total_nodes")
        .expect("Number of max_nodes input required")
        .parse()
        .expect("total_nodes must be of integer type");

    let tx_queue_per_sender: Option<i64> =
        matches.value_of("tx_queue_per_sender").map_or(None, |v| {
            Some(
                v.parse::<i64>()
                    .expect("tx_queue_per_sender need to be of integer type"),
            )
        });

    assert!(
        num_nodes_total >= num_nodes_validators,
        "max_nodes must be greater than nodes"
    );

    println!("generating config files for {} nodes in total, with the first {} nodes as initial validator", num_nodes_total, num_nodes_validators);

    let config_type =
        value_t!(matches.value_of("configtype"), ConfigType).unwrap_or(ConfigType::PosdaoSetup);

    let external_ip = matches.value_of("extip");

    let private_keys = matches
        .values_of("private_keys")
        .map_or(Vec::new(), |values| {
            values
                .map(|v| Secret::copy_from_str(v).expect("Secret key format must be correct!"))
                .collect()
        });

    // If private keys are specified we expect as many as there are nodes.
    if private_keys.len() != 0 {
        assert!(private_keys.len() == num_nodes_total);
    };

    let enodes_map = generate_enodes(num_nodes_total, private_keys, external_ip);
    let mut rng = rand::thread_rng();

    let pub_keys = enodes_to_pub_keys(&enodes_map);

    // we only need the first x pub_keys
    let pub_keys_for_key_gen_btree = pub_keys
        .iter()
        .take(num_nodes_validators)
        .map(|x| (x.0.clone(), x.1.clone()))
        .collect();

    let (_sync_keygen, parts, acks) = generate_keygens(
        Arc::new(pub_keys_for_key_gen_btree),
        &mut rng,
        (num_nodes_validators - 1) / 3,
    );

    let mut reserved_peers = String::new();

    for pub_key in pub_keys.iter() {
        let our_id = pub_key.0;

        let enode = enodes_map.get(our_id).expect("validator id must be mapped");
        writeln!(&mut reserved_peers, "{}", enode.to_string())
            .expect("enode should be written to the reserved peers string");
        let i = enode.idx;
        let file_name = format!("hbbft_validator_{}.toml", i);
        let toml_string = toml::to_string(&to_toml(
            i,
            &config_type,
            external_ip,
            &enode.address,
            num_nodes_total,
            tx_queue_per_sender.clone(),
        ))
        .expect("TOML string generation should succeed");
        fs::write(file_name, toml_string).expect("Unable to write config file");

        let file_name = format!("hbbft_validator_key_{}", i);
        fs::write(file_name, enode.secret.to_hex()).expect("Unable to write key file");
        fs::write(
            format!("hbbft_validator_public_{}.txt", i),
            format!("{:?}", enode.public),
        )
        .expect("Unable to write public key file");
        fs::write(
            format!("hbbft_validator_address_{}.txt", i),
            format!("{:?}", enode.address),
        )
        .expect("Unable to write address file");

        write_json_for_secret(
            enode.secret.clone(),
            format!("hbbft_validator_key_{}.json", i),
        );
    }
    // Write rpc node config
    let rpc_string = toml::to_string(&to_toml(
        0,
        &ConfigType::Rpc,
        external_ip,
        &Address::default(), // todo: insert HBBFT Contracts pot here.
        num_nodes_total,
        tx_queue_per_sender.clone(),
    ))
    .expect("TOML string generation should succeed");
    fs::write("rpc_node.toml", rpc_string).expect("Unable to write rpc config file");

    // Write reserved peers file
    fs::write("reserved-peers", reserved_peers).expect("Unable to write reserved_peers file");

    // Write the password file
    fs::write("password.txt", "test").expect("Unable to write password.txt file");

    // only pass over enodes in the enodes_map that are also available for acks and parts.

    fs::write(
        "keygen_history.json",
        key_sync_history_data(&parts, &acks, &enodes_map, true),
    )
    .expect("Unable to write keygen history data file");

    fs::write(
        "nodes_info.json",
        key_sync_history_data(&parts, &acks, &enodes_map, false),
    )
    .expect("Unable to write nodes_info data file");
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbbft::sync_key_gen::{AckOutcome, PartOutcome, SyncKeyGen};
    use keygen_history_helpers::KeyPairWrapper;
    use rand;
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn test_threshold_encryption_single() {
        let (secret, public, _) = crate::create_account();
        let keypair = KeyPairWrapper { public, secret };
        let mut pub_keys: BTreeMap<Public, KeyPairWrapper> = BTreeMap::new();
        pub_keys.insert(public, keypair.clone());
        let mut rng = rand::thread_rng();
        let mut key_gen =
            SyncKeyGen::new(public, keypair, Arc::new(pub_keys), 0, &mut rng).unwrap();
        let part = key_gen.1.unwrap();
        let outcome = key_gen.0.handle_part(&public, part, &mut rng);
        assert!(outcome.is_ok());
        match outcome.unwrap() {
            PartOutcome::Valid(ack) => {
                assert!(ack.is_some());
                let ack_outcome = key_gen.0.handle_ack(&public, ack.unwrap());
                assert!(ack_outcome.is_ok());
                match ack_outcome.unwrap() {
                    AckOutcome::Valid => {
                        assert!(key_gen.0.is_ready());
                        let key_shares = key_gen.0.generate();
                        assert!(key_shares.is_ok());
                        assert!(key_shares.unwrap().1.is_some());
                    }
                    AckOutcome::Invalid(_) => assert!(false),
                }
            }
            PartOutcome::Invalid(_) => assert!(false),
        }
    }

    #[test]
    fn test_threshold_encryption_multiple() {
        let num_nodes = 4;
        let t = 1;

        let enodes = generate_enodes(num_nodes, Vec::new(), None);
        let pub_keys = enodes_to_pub_keys(&enodes);
        let mut rng = rand::thread_rng();

        let (sync_keygen, _, _) = generate_keygens(pub_keys, &mut rng, t);

        let compare_to = sync_keygen.iter().nth(0).unwrap().generate().unwrap().0;

        // Check key generation
        for s in sync_keygen {
            assert!(s.is_ready());
            assert!(s.generate().is_ok());
            assert_eq!(s.generate().unwrap().0, compare_to);
        }
    }
}
