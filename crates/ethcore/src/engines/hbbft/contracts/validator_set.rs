use client::{
    traits::{EngineClient, TransactionRequest},
    BlockChainClient,
};
use crypto::publickey::Public;
use engines::hbbft::utils::bound_contract::{BoundContract, CallError};
use ethereum_types::{Address, U256};
use std::{collections::BTreeMap, net::Ipv4Addr, str::FromStr};
use types::{ids::BlockId, transaction::Error};

use_contract!(
    validator_set_hbbft,
    "res/contracts/validator_set_hbbft.json"
);

lazy_static! {
    static ref VALIDATOR_SET_ADDRESS: Address =
        Address::from_str("1000000000000000000000000000000000000001").unwrap();
}

macro_rules! call_const_validator {
	($c:ident, $x:ident $(, $a:expr )*) => {
		$c.call_const(validator_set_hbbft::functions::$x::call($($a),*))
	};
}

pub enum ValidatorType {
    Current,
    Pending,
}

pub fn get_validator_pubkeys(
    client: &dyn EngineClient,
    block_id: BlockId,
    validator_type: ValidatorType,
) -> Result<BTreeMap<Address, Public>, CallError> {
    let c = BoundContract::bind(client, block_id, *VALIDATOR_SET_ADDRESS);
    let validators = match validator_type {
        ValidatorType::Current => call_const_validator!(c, get_validators)?,
        ValidatorType::Pending => call_const_validator!(c, get_pending_validators)?,
    };
    let mut validator_map = BTreeMap::new();
    for v in validators {
        let pubkey = call_const_validator!(c, get_public_key, v)?;

        if pubkey.len() != 64 {
            return Err(CallError::ReturnValueInvalid);
        }
        let pubkey = Public::from_slice(&pubkey);

        //println!("Validator {:?} with public key {}", v, pubkey);
        validator_map.insert(v, pubkey);
    }
    Ok(validator_map)
}

#[cfg(test)]
pub fn mining_by_staking_address(
    client: &dyn EngineClient,
    staking_address: &Address,
) -> Result<Address, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);
    call_const_validator!(c, mining_by_staking_address, staking_address.clone())
}

pub fn staking_by_mining_address(
    client: &dyn EngineClient,
    mining_address: &Address,
) -> Result<Address, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);
    call_const_validator!(c, staking_by_mining_address, mining_address.clone())
}

pub fn is_pending_validator(
    client: &dyn EngineClient,
    staking_address: &Address,
) -> Result<bool, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);
    call_const_validator!(c, is_pending_validator, staking_address.clone())
}

#[derive(PartialEq)]
pub enum KeyGenMode {
    WritePart,
    WriteAck,
    Other,
}

pub fn get_pending_validator_key_generation_mode(
    client: &dyn EngineClient,
    mining_address: &Address,
) -> Result<KeyGenMode, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);
    let key_gen_mode = call_const_validator!(
        c,
        get_pending_validator_key_generation_mode,
        mining_address.clone()
    )?;
    Ok(match key_gen_mode.low_u64() {
        1 => KeyGenMode::WritePart,
        3 => KeyGenMode::WriteAck,
        _ => KeyGenMode::Other,
    })
}

pub fn get_validator_available_since(
    client: &dyn EngineClient,
    address: &Address,
) -> Result<U256, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);
    call_const_validator!(c, validator_available_since, address.clone())
}

pub fn get_pending_validators(client: &dyn EngineClient) -> Result<Vec<Address>, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);
    call_const_validator!(c, get_pending_validators)
}

/// Sets this validators internet address.
/// Can only be called if there is a pool existing for this signer.
pub fn set_validator_internet_address(
    full_client: &dyn BlockChainClient,
    signer_address: &Address,
    ip_address: &Ipv4Addr,
    port: u16,
) -> Result<(), Error> {
    //let c = BoundContract::bind(client, BlockId::Latest, *VALIDATOR_SET_ADDRESS);

    let octects = ip_address.octets();
    let ip_address_array: [u8; 16] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, octects[0], octects[1], octects[2], octects[3],
    ];

    let port_array: [u8; 2] = [(port / 256) as u8, (port - (port / 256)) as u8];

    let send_data = validator_set_hbbft::functions::set_validator_internet_address::call(
        ip_address_array,
        port_array,
    );

    let mut nonce = full_client
        .nonce(signer_address, BlockId::Latest)
        .unwrap_or_default();

    let transaction = TransactionRequest::call(*VALIDATOR_SET_ADDRESS, send_data.0)
        .gas(U256::from(100_000))
        .nonce(nonce);

    info!(target:"consensus", "set_validator_internet_address: ip: {} port: {} none: {}", ip_address, port, nonce);
    full_client.transact_silently(transaction)?;
    Ok(())
}

pub fn send_tx_announce_availability(
    full_client: &dyn BlockChainClient,
    address: &Address,
) -> Result<(), Error> {
    // chain.latest_nonce(address)
    // we need to get the real latest nonce.
    //let nonce_from_full_client =  full_client.nonce(address,BlockId::Latest);

    let mut nonce = full_client.next_nonce(&address);

    match full_client.nonce(address, BlockId::Latest) {
        Some(new_nonce) => {
            if new_nonce != nonce {
                info!(target:"consensus", "got better nonce for announce availability: {} => {}", nonce, new_nonce);
                nonce = new_nonce;
            }
        }
        None => {}
    }

    match full_client.block_number(BlockId::Latest) {
        Some(block_number) => match full_client.block_hash(BlockId::Number(block_number)) {
            None => {
                error!(target:"consensus", "could not announce availability. could not retrieve block hash for block {}", block_number);
            }
            Some(block_hash) => {
                let send_data = validator_set_hbbft::functions::announce_availability::call(
                    block_number,
                    block_hash,
                );
                let transaction = TransactionRequest::call(*VALIDATOR_SET_ADDRESS, send_data.0)
                    .gas(U256::from(1_000_000))
                    .nonce(nonce);

                info!(target:"consensus", "sending announce availability with nonce: {}", nonce);
                full_client.transact_silently(transaction)?;
                return Ok(());
            }
        },
        None => {
            error!(target:"consensus", "could not announce availability. could not retrieve current block number");
        }
    }

    return Err(Error::TransactionTypeNotEnabled);
}
