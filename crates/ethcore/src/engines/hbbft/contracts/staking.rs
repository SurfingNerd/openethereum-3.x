use client::EngineClient;
use engines::hbbft::utils::bound_contract::{BoundContract, CallError};
use ethereum_types::{Address, U256};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
};
use types::ids::BlockId;

use_contract!(staking_contract, "res/contracts/staking_contract.json");

lazy_static! {
    static ref STAKING_CONTRACT_ADDRESS: Address =
        Address::from_str("1100000000000000000000000000000000000001").unwrap();
}

macro_rules! call_const_staking {
		($c:ident, $x:ident $(, $a:expr )*) => {
			$c.call_const(staking_contract::functions::$x::call($($a),*))
		};
	}

pub fn get_posdao_epoch(client: &dyn EngineClient, block_id: BlockId) -> Result<U256, CallError> {
    let c = BoundContract::bind(client, block_id, *STAKING_CONTRACT_ADDRESS);
    call_const_staking!(c, staking_epoch)
}

pub fn get_posdao_epoch_start(
    client: &dyn EngineClient,
    block_id: BlockId,
) -> Result<U256, CallError> {
    let c = BoundContract::bind(client, block_id, *STAKING_CONTRACT_ADDRESS);
    call_const_staking!(c, staking_epoch_start_block)
}

pub fn start_time_of_next_phase_transition(client: &dyn EngineClient) -> Result<U256, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *STAKING_CONTRACT_ADDRESS);
    call_const_staking!(c, start_time_of_next_phase_transition)
}

pub fn candidate_min_stake(client: &dyn EngineClient) -> Result<U256, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *STAKING_CONTRACT_ADDRESS);
    call_const_staking!(c, candidate_min_stake)
}

pub fn get_validator_internet_address(
    client: &dyn EngineClient,
    staking_address: &Address,
) -> Result<SocketAddrV4, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *STAKING_CONTRACT_ADDRESS);
    let result = call_const_staking!(c, get_pool_internet_address, staking_address.clone());
    //staking_contract::functions::get_pool_internet_address::call()

    match result {
        Ok((ip, port)) => {
            //byteorder::new();
            return Ok(SocketAddrV4::new(
                Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]),
                port[0] as u16 * 256 + port[1] as u16,
            ));
        }
        Err(e) => return Err(e),
    }
}

pub fn stake_amount(
    client: &dyn EngineClient,
    staking_address: &Address,
    owner_address: &Address,
) -> Result<U256, CallError> {
    let c = BoundContract::bind(client, BlockId::Latest, *STAKING_CONTRACT_ADDRESS);
    call_const_staking!(
        c,
        stake_amount,
        staking_address.clone(),
        owner_address.clone()
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crypto::publickey::{Generator, KeyPair, Public, Random};
    use engines::hbbft::test::hbbft_test_client::HbbftTestClient;

    pub fn min_staking(client: &dyn EngineClient) -> Result<U256, CallError> {
        let c = BoundContract::bind(client, BlockId::Latest, *STAKING_CONTRACT_ADDRESS);
        call_const_staking!(c, candidate_min_stake)
    }

    pub fn is_pool_active(
        client: &dyn EngineClient,
        staking_address: Address,
    ) -> Result<bool, CallError> {
        let c = BoundContract::bind(client, BlockId::Latest, *STAKING_CONTRACT_ADDRESS);
        call_const_staking!(c, is_pool_active, staking_address)
    }

    pub fn add_pool(mining_address: Address, mining_public_key: Public) -> ethabi::Bytes {
        let (abi_bytes, _) = staking_contract::functions::add_pool::call(
            mining_address,
            mining_public_key.as_bytes(),
            [0; 16],
        );
        abi_bytes
    }

    /// Creates a staking address and registers it as a pool with the staking contract.
    ///
    /// # Arguments
    ///
    /// * `moc` - A client with sufficient balance to fund the new staker.
    /// * `validator` - The validator to be registered with the new staking address.
    /// * `extra_funds` - Should be sufficiently high to pay for transactions necessary to create the staking pool.  
    pub fn create_staker(
        moc: &mut HbbftTestClient,
        funder: &KeyPair,
        miner: &HbbftTestClient,
        extra_funds: U256,
    ) -> KeyPair {
        let min_staking_amount =
            min_staking(moc.client.as_ref()).expect("Query for minimum staking must succeed.");
        let amount_to_transfer = min_staking_amount + extra_funds;

        let staker: KeyPair = Random.generate();
        moc.transfer(funder, &staker.address(), &amount_to_transfer);

        // Generate call data.
        let abi_bytes = add_pool(miner.address(), miner.keypair.public().clone());

        // Register the staker
        moc.call_as(
            &staker,
            &STAKING_CONTRACT_ADDRESS,
            abi_bytes,
            &min_staking_amount,
        );

        staker
    }
}
