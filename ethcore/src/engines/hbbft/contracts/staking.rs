use client::EngineClient;
use engines::hbbft::utils::bound_contract::{BoundContract, CallError};
use ethereum_types::{Address, U256};
use std::str::FromStr;
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

#[cfg(test)]
pub mod tests {
    use super::*;
    use engines::hbbft::utils::test_helpers::HbbftTestClient;
    use ethkey::{Generator, KeyPair, Public, Random};

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
            mining_public_key.0,
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
        miner: &HbbftTestClient,
        extra_funds: U256,
    ) -> KeyPair {
        let min_staking_amount =
            min_staking(moc.client.as_ref()).expect("Query for minimum staking must succeed.");
        let amount_to_transfer = min_staking_amount + extra_funds;

        let staker: KeyPair = Random
            .generate()
            .expect("Random Key Generation should never fail.");
        moc.transfer_to(&staker.address(), &amount_to_transfer);

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
