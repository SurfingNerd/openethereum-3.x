mod block_reward_hbbft;
mod contracts;
mod contribution;
mod hbbft_engine;
mod hbbft_state;
mod sealing;
mod utils;

pub use self::hbbft_engine::HoneyBadgerBFT;
use ethkey::Public;
use std::fmt;

#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct NodeId(pub Public);

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:6}", hex_fmt::HexFmt(&self.0))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "NodeId({})", self.0)
    }
}

#[cfg(test)]
mod tests {

    use super::utils::test_helpers::create_hbbft_client;
    use client::traits::BlockInfo;
    use engines::hbbft::contracts::staking::tests::{create_staker, is_pool_active};
    use engines::hbbft::contracts::staking::{
        get_posdao_epoch, start_time_of_next_phase_transition,
    };
    use engines::hbbft::contracts::validator_set::{
        is_pending_validator, mining_by_staking_address,
    };
    use engines::hbbft::contribution::unix_now_secs;
    use ethereum_types::{Address, U256};
    use ethkey::{Generator, KeyPair, Random, Secret};
    use std::str::FromStr;
    use types::ids::BlockId;

    lazy_static! {
        static ref MASTER_OF_CEREMONIES_KEYPAIR: KeyPair = KeyPair::from_secret(
            Secret::from_str("a5c780aaee99c46e09d739405d7ff6566f7958bae2100404fac795083bea26cf")
                .expect("Secret from hex string must succeed")
        )
        .expect("KeyPair generation from secret must succeed");
    }

    #[test]
    fn test_miner_transaction_injection() {
        let mut test_data = create_hbbft_client(MASTER_OF_CEREMONIES_KEYPAIR.clone());

        // Verify that we actually start at block 0.
        assert_eq!(test_data.client.chain().best_block_number(), 0);

        // Inject a transaction, with instant sealing a block will be created right away.
        test_data.create_some_transaction(None);

        // Expect a new block to be created.
        assert_eq!(test_data.client.chain().best_block_number(), 1);

        // Expect one transaction in the block.
        let block = test_data
            .client
            .block(BlockId::Number(1))
            .expect("Block 1 must exist");
        assert_eq!(block.transactions_count(), 1);

        // Inject a transaction, with instant sealing a block will be created right away.
        test_data.create_some_transaction(None);

        // Expect a new block to be created.
        assert_eq!(test_data.client.chain().best_block_number(), 2);
    }

    #[test]
    fn test_staking_account_creation() {
        // Create Master of Ceremonies
        let mut moc = create_hbbft_client(MASTER_OF_CEREMONIES_KEYPAIR.clone());

        // Verify the master of ceremony is funded.
        assert!(moc.balance(&moc.address()) > U256::from(10000000));

        // Create a potential validator.
        let miner_1 =
            create_hbbft_client(Random.generate().expect("KeyPair generation must succeed."));

        // Verify the pending validator is unfunded.
        assert_eq!(moc.balance(&miner_1.address()), U256::from(0));

        // Verify that we actually start at block 0.
        assert_eq!(moc.client.chain().best_block_number(), 0);

        let transaction_funds = U256::from(9000000000000000000u64);

        // Inject a transaction, with instant sealing a block will be created right away.
        moc.transfer_to(&miner_1.address(), &transaction_funds);

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 1);

        // Verify the pending validator is now funded.
        assert_eq!(moc.balance(&miner_1.address()), transaction_funds);

        // Create staking address
        let staker_1 = create_staker(&mut moc, &miner_1, transaction_funds);

        // Expect two new blocks to be created, one for the transfer of staking funds,
        // one for registering the staker as pool.
        assert_eq!(moc.client.chain().best_block_number(), 3);

        let block = moc
            .client
            .block(BlockId::Number(3))
            .expect("Block must exist");
        // Since the epoch duration is set to 1 in the genesis block we expect a Part write
        // transaction to be sent after block 2 in addition to the the transaction for
        // adding the staker.
        // The expected number of transactions in block 3 is therefore 2!
        assert_eq!(block.transactions_count(), 2);

        assert_ne!(
            mining_by_staking_address(moc.client.as_ref(), &staker_1.address())
                .expect("Constant call must succeed."),
            Address::zero()
        );

        // Check if the staking pool is active.
        assert_eq!(
            is_pool_active(moc.client.as_ref(), staker_1.address())
                .expect("Pool active query must succeed."),
            true
        );
    }

    #[test]
    fn test_epoch_transition() {
        // Create Master of Ceremonies
        let mut moc = create_hbbft_client(MASTER_OF_CEREMONIES_KEYPAIR.clone());
        // To avoid performing external transactions with the MoC we create and fund a random address.
        let transactor: KeyPair = Random.generate().expect("KeyPair generation must succeed.");

        let genesis_transition_time = start_time_of_next_phase_transition(moc.client.as_ref())
            .expect("Constant call must succeed");

        // Genesis block is at time 0, current unix time must be much larger.
        assert!(genesis_transition_time.as_u64() < unix_now_secs());

        // We should not be in the pending validator set at the genesis block.
        assert!(!is_pending_validator(moc.client.as_ref(), &moc.address())
            .expect("Constant call must succeed"));

        // Fund the transactor.
        // Also triggers the creation of a block.
        // This implicitly calls the block reward contract, which should trigger a phase transition
        // since we already verified that the genesis transition time threshold has been reached.
        let transaction_funds = U256::from(9000000000000000000u64);
        moc.transfer_to(&transactor.address(), &transaction_funds);

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 1);

        // Now we should be part of the pending validator set.
        assert!(is_pending_validator(moc.client.as_ref(), &moc.address())
            .expect("Constant call must succeed"));

        // Check if we are still in the first epoch.
        assert_eq!(
            get_posdao_epoch(moc.client.as_ref(), BlockId::Latest)
                .expect("Constant call must succeed"),
            U256::from(0)
        );

        // First the validator realizes it is in the next validator set and sends his part.
        moc.create_some_transaction(Some(&transactor));

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 2);

        // The part will be included in the block triggered by this transaction, but not part of the global state yet,
        // so it sends the transaction another time.
        moc.create_some_transaction(Some(&transactor));

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 3);

        // Now the part is part of the global chain state, and we send our acks.
        moc.create_some_transaction(Some(&transactor));

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 4);

        // The acks will be included in the block triggered by this transaction, but not part of the global state yet.
        moc.create_some_transaction(Some(&transactor));

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 5);

        // Now the acks are part of the global block state, and the key generation is complete and the next epoch begins
        moc.create_some_transaction(Some(&transactor));

        // Expect a new block to be created.
        assert_eq!(moc.client.chain().best_block_number(), 6);

        let block = moc
            .client
            .block(BlockId::Number(6))
            .expect("Block must exist");
        assert_eq!(block.transactions_count(), 2);

        // At this point we should be in the new epoch.
        assert_eq!(
            get_posdao_epoch(moc.client.as_ref(), BlockId::Latest)
                .expect("Constant call must succeed"),
            U256::from(1)
        );

        // Let's do another one to check if the transition to the new honey badger and keys works.
        moc.create_some_transaction(Some(&transactor));
    }
}
