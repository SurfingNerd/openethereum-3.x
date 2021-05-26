use client::traits::{EngineClient, TransactionRequest};
use crypto::{self, publickey::Public};
use engines::{
    hbbft::{
        contracts::{
            staking::get_posdao_epoch,
            validator_set::{get_validator_pubkeys, ValidatorType},
        },
        utils::bound_contract::{BoundContract, CallError},
        NodeId,
    },
    signer::EngineSigner,
};
use ethereum_types::{Address, H512, U256};
use hbbft::{
    crypto::{PublicKeySet, SecretKeyShare},
    sync_key_gen::{
        Ack, AckOutcome, Error, Part, PartOutcome, PubKeyMap, PublicKey, SecretKey, SyncKeyGen,
    },
    util::max_faulty,
    NetworkInfo,
};
use itertools::Itertools;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};
use types::ids::BlockId;

use_contract!(
    key_history_contract,
    "res/contracts/key_history_contract.json"
);

lazy_static! {
    static ref KEYGEN_HISTORY_ADDRESS: Address =
        Address::from_str("7000000000000000000000000000000000000001").unwrap();
}

macro_rules! call_const_key_history {
	($c:ident, $x:ident $(, $a:expr )*) => {
		$c.call_const(key_history_contract::functions::$x::call($($a),*))
	};
}

pub fn engine_signer_to_synckeygen<'a>(
    signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
    pub_keys: PubKeyMap<Public, PublicWrapper>,
) -> Result<(SyncKeyGen<Public, PublicWrapper>, Option<Part>), Error> {
    let wrapper = KeyPairWrapper {
        inner: signer.clone(),
    };
    let public = match signer.read().as_ref() {
        Some(signer) => signer
            .public()
            .expect("Signer's public key must be available!"),
        None => Public::from(H512::from_low_u64_be(0)),
    };
    let mut rng = rand_065::thread_rng();
    let num_nodes = pub_keys.len();
    SyncKeyGen::new(public, wrapper, pub_keys, max_faulty(num_nodes), &mut rng)
}

pub fn synckeygen_to_network_info(
    synckeygen: &SyncKeyGen<Public, PublicWrapper>,
    pks: PublicKeySet,
    sks: Option<SecretKeyShare>,
) -> Option<NetworkInfo<NodeId>> {
    let pub_keys = synckeygen
        .public_keys()
        .keys()
        .map(|p| NodeId(*p))
        .collect::<Vec<_>>();
    println!("Creating Network Info");
    println!("pub_keys: {:?}", pub_keys);
    println!(
        "pks: {:?}",
        (0..(pub_keys.len()))
            .map(|i| pks.public_key_share(i))
            .collect::<Vec<_>>()
    );
    let sks = sks.unwrap();
    println!("sks.public_key_share: {:?}", sks.public_key_share());
    println!("sks.reveal: {:?}", sks.reveal());

    Some(NetworkInfo::new(
        NodeId(synckeygen.our_id().clone()),
        sks,
        pks,
        pub_keys,
    ))
}

pub fn has_part_of_address_data(
    client: &dyn EngineClient,
    address: Address,
) -> Result<bool, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *KEYGEN_HISTORY_ADDRESS);
    let serialized_part = call_const_key_history!(c, parts, address)?;
    //println!("Part for address {}: {:?}", address, serialized_part);
    Ok(!serialized_part.is_empty())
}

pub fn part_of_address(
    client: &dyn EngineClient,
    address: Address,
    vmap: &BTreeMap<Address, Public>,
    skg: &mut SyncKeyGen<Public, PublicWrapper>,
    block_id: BlockId,
) -> Result<Option<Ack>, CallError> {
    let c = BoundContract::bind(client, block_id, *KEYGEN_HISTORY_ADDRESS);
    let serialized_part = call_const_key_history!(c, parts, address)?;
    //println!("Part for address {}: {:?}", address, serialized_part);
    if serialized_part.is_empty() {
        return Err(CallError::ReturnValueInvalid);
    }
    let deserialized_part: Part = bincode::deserialize(&serialized_part).unwrap();
    let mut rng = rand_065::thread_rng();
    let outcome = skg
        .handle_part(vmap.get(&address).unwrap(), deserialized_part, &mut rng)
        .unwrap();

    match outcome {
        PartOutcome::Invalid(_) => Err(CallError::ReturnValueInvalid),
        PartOutcome::Valid(ack) => Ok(ack),
    }
}

pub fn has_acks_of_address_data(
    client: &dyn EngineClient,
    address: Address,
) -> Result<bool, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *KEYGEN_HISTORY_ADDRESS);
    let serialized_length = call_const_key_history!(c, get_acks_length, address)?;
    Ok(serialized_length.low_u64() != 0)
}

pub fn acks_of_address(
    client: &dyn EngineClient,
    address: Address,
    vmap: &BTreeMap<Address, Public>,
    skg: &mut SyncKeyGen<Public, PublicWrapper>,
    block_id: BlockId,
) -> Result<(), CallError> {
    let c = BoundContract::bind(client, block_id, *KEYGEN_HISTORY_ADDRESS);
    let serialized_length = call_const_key_history!(c, get_acks_length, address)?;

    // println!(
    // 	"Acks for address {} is of size: {:?}",
    // 	address, serialized_length
    // );
    for n in 0..serialized_length.low_u64() {
        let serialized_ack = call_const_key_history!(c, acks, address, n)?;
        //println!("Ack #{} for address {}: {:?}", n, address, serialized_ack);
        if serialized_ack.is_empty() {
            return Err(CallError::ReturnValueInvalid);
        }
        let deserialized_ack: Ack = bincode::deserialize(&serialized_ack).unwrap();
        let outcome = skg
            .handle_ack(vmap.get(&address).unwrap(), deserialized_ack)
            .unwrap();
        if let AckOutcome::Invalid(fault) = outcome {
            panic!("Expected Ack Outcome to be valid. {}", fault);
        }
    }

    Ok(())
}

#[derive(Clone)]
pub struct PublicWrapper {
    pub inner: Public,
}

#[derive(Clone)]
pub struct KeyPairWrapper {
    pub inner: Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
}

impl<'a> PublicKey for PublicWrapper {
    type Error = crypto::publickey::Error;
    type SecretKey = KeyPairWrapper;
    fn encrypt<M: AsRef<[u8]>, R: rand_065::Rng>(
        &self,
        msg: M,
        _rng: &mut R,
    ) -> Result<Vec<u8>, Self::Error> {
        crypto::publickey::ecies::encrypt(&self.inner, b"", msg.as_ref())
    }
}

impl<'a> SecretKey for KeyPairWrapper {
    type Error = crypto::publickey::Error;
    fn decrypt(&self, ct: &[u8]) -> Result<Vec<u8>, Self::Error> {
        self.inner
            .read()
            .as_ref()
            .ok_or(parity_crypto::publickey::Error::InvalidSecretKey)
            .expect("Signer must be set!")
            .decrypt(b"", ct)
    }
}

/// Read available keygen data from the blockchain and initialize a SyncKeyGen instance with it.
pub fn initialize_synckeygen(
    client: &dyn EngineClient,
    signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
    block_id: BlockId,
    validator_type: ValidatorType,
) -> Result<SyncKeyGen<Public, PublicWrapper>, CallError> {
    let vmap = get_validator_pubkeys(&*client, block_id, validator_type)?;
    let pub_keys: BTreeMap<_, _> = vmap
        .values()
        .map(|p| (*p, PublicWrapper { inner: p.clone() }))
        .collect();

    // if synckeygen creation fails then either signer or validator pub keys are problematic.
    // Todo: We should expect up to f clients to write invalid pub keys. Report and re-start pending validator set selection.
    let (mut synckeygen, _) = engine_signer_to_synckeygen(signer, Arc::new(pub_keys))
        .map_err(|_| CallError::ReturnValueInvalid)?;

    for v in vmap.keys().sorted() {
        part_of_address(&*client, *v, &vmap, &mut synckeygen, block_id)?;
    }
    for v in vmap.keys().sorted() {
        acks_of_address(&*client, *v, &vmap, &mut synckeygen, block_id)?;
    }

    Ok(synckeygen)
}

/// Returns a collection of transactions the pending validator has to submit in order to
/// complete the keygen history contract data necessary to generate the next key and switch to the new validator set.
pub fn send_keygen_transactions(
    client: &dyn EngineClient,
    signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
) -> Result<(), CallError> {
    static LAST_PART_SENT: AtomicU64 = AtomicU64::new(0);
    static LAST_ACKS_SENT: AtomicU64 = AtomicU64::new(0);

    // If we have no signer there is nothing for us to send.
    let address = match signer.read().as_ref() {
        Some(signer) => signer.address(),
        None => return Err(CallError::ReturnValueInvalid),
    };

    let full_client = client.as_full_client().ok_or(CallError::NotFullClient)?;

    // If the chain is still syncing, do not send Parts or Acks.
    if full_client.is_major_syncing() {
        return Ok(());
    }

    let vmap = get_validator_pubkeys(&*client, BlockId::Latest, ValidatorType::Pending)?;
    let pub_keys: BTreeMap<_, _> = vmap
        .values()
        .map(|p| (*p, PublicWrapper { inner: p.clone() }))
        .collect();

    // if synckeygen creation fails then either signer or validator pub keys are problematic.
    // Todo: We should expect up to f clients to write invalid pub keys. Report and re-start pending validator set selection.
    let (mut synckeygen, part) = engine_signer_to_synckeygen(signer, Arc::new(pub_keys))
        .map_err(|_| CallError::ReturnValueInvalid)?;

    // If there is no part then we are not part of the pending validator set and there is nothing for us to do.
    let part_data = match part {
        Some(part) => part,
        None => return Err(CallError::ReturnValueInvalid),
    };

    let upcoming_epoch = get_posdao_epoch(client, BlockId::Latest)? + 1;
    let cur_block = client
        .block_number(BlockId::Latest)
        .ok_or(CallError::ReturnValueInvalid)?;

    // Check if we already sent our part.
    if (LAST_PART_SENT.load(Ordering::SeqCst) + 10 < cur_block)
        && !has_part_of_address_data(client, address)?
    {
        let serialized_part = match bincode::serialize(&part_data) {
            Ok(part) => part,
            Err(_) => return Err(CallError::ReturnValueInvalid),
        };
        let serialized_part_len = serialized_part.len();
        let write_part_data =
            key_history_contract::functions::write_part::call(upcoming_epoch, serialized_part);

        // the required gas values have been approximated by
        // experimenting and it's a very rough estimation.
        // it can be further fine tuned to be just above the real consumption.
        // ACKs require much more gas,
        // and usually run into the gas limit problems.
        let gas: usize = serialized_part_len * 750 + 100_000;

        trace!(target: "engine", "Hbbft part transaction gas: part-len: {} gas: {}", serialized_part_len, gas);

        let part_transaction = TransactionRequest::call(*KEYGEN_HISTORY_ADDRESS, write_part_data.0)
            .gas(U256::from(gas))
            .nonce(full_client.next_nonce(&address))
            .gas_price(U256::from(10000000000u64));
        full_client
            .transact_silently(part_transaction)
            .map_err(|_| CallError::ReturnValueInvalid)?;
        LAST_PART_SENT.store(cur_block, Ordering::SeqCst);
    }

    // Return if any Part is missing.
    let mut acks = Vec::new();
    for v in vmap.keys().sorted() {
        acks.push(
            match part_of_address(&*client, *v, &vmap, &mut synckeygen, BlockId::Latest)? {
                Some(ack) => ack,
                None => return Err(CallError::ReturnValueInvalid),
            },
        );
    }

    // Now we are sure all parts are ready, let's check if we sent our Acks.
    if (LAST_ACKS_SENT.load(Ordering::SeqCst) + 10 < cur_block)
        && !has_acks_of_address_data(client, address)?
    {
        let mut serialized_acks = Vec::new();
        let mut total_bytes_for_acks = 0;

        for ack in acks {
            let ack_to_push = match bincode::serialize(&ack) {
                Ok(serialized_ack) => serialized_ack,
                Err(_) => return Err(CallError::ReturnValueInvalid),
            };
            total_bytes_for_acks += ack_to_push.len();
            serialized_acks.push(ack_to_push);
        }

        let write_acks_data =
            key_history_contract::functions::write_acks::call(upcoming_epoch, serialized_acks);

        // the required gas values have been approximated by
        // experimenting and it's a very rough estimation.
        // it can be further fine tuned to be just above the real consumption.
        let gas = total_bytes_for_acks * 800 + 200_000;
        trace!(target: "engine","acks-len: {} gas: {}", total_bytes_for_acks, gas);

        let acks_transaction = TransactionRequest::call(*KEYGEN_HISTORY_ADDRESS, write_acks_data.0)
            .gas(U256::from(gas))
            .nonce(full_client.next_nonce(&address))
            .gas_price(U256::from(10000000000u64));
        full_client
            .transact_silently(acks_transaction)
            .map_err(|_| CallError::ReturnValueInvalid)?;
        LAST_ACKS_SENT.store(cur_block, Ordering::SeqCst);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::publickey::{KeyPair, Secret};
    use engines::signer::{from_keypair, EngineSigner};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn test_synckeygen_initialization() {
        // Create a keypair
        let secret =
            Secret::from_str("49c437676c600660905204e5f3710a6db5d3f46e3da9ba5168b9d34b0b787317")
                .unwrap();
        let keypair = KeyPair::from_secret(secret).expect("KeyPair generation must succeed");
        let public = keypair.public().clone();
        let wrapper = PublicWrapper {
            inner: public.clone(),
        };

        // Convert it to a EngineSigner trait object
        let signer: Arc<RwLock<Option<Box<dyn EngineSigner>>>> =
            Arc::new(RwLock::new(Some(from_keypair(keypair))));

        // Initialize SyncKeyGen with the EngineSigner wrapper
        let mut pub_keys: BTreeMap<Public, PublicWrapper> = BTreeMap::new();
        pub_keys.insert(public, wrapper);

        assert!(engine_signer_to_synckeygen(&signer, Arc::new(pub_keys)).is_ok());
    }
}
