use client::traits::EngineClient;
use engines::signer::EngineSigner;
use hbbft::{
    crypto::{PublicKey, Signature},
    honey_badger::{self, HoneyBadgerBuilder},
    Epoched, NetworkInfo,
};
use parking_lot::RwLock;
use std::{collections::BTreeMap, sync::Arc};
use types::{header::Header, ids::BlockId};

use super::{
    contracts::{
        keygen_history::{initialize_synckeygen, synckeygen_to_network_info},
        staking::{get_posdao_epoch, get_posdao_epoch_start},
        validator_set::ValidatorType,
    },
    contribution::Contribution,
    NodeId,
};

pub type HbMessage = honey_badger::Message<NodeId>;
pub(crate) type HoneyBadger = honey_badger::HoneyBadger<Contribution, NodeId>;
pub(crate) type Batch = honey_badger::Batch<Contribution, NodeId>;
pub(crate) type HoneyBadgerStep = honey_badger::Step<Contribution, NodeId>;
pub(crate) type HoneyBadgerResult = honey_badger::Result<HoneyBadgerStep>;

pub(crate) struct HbbftState {
    network_info: Option<NetworkInfo<NodeId>>,
    honey_badger: Option<HoneyBadger>,
    public_master_key: Option<PublicKey>,
    current_posdao_epoch: u64,
    future_messages_cache: BTreeMap<u64, Vec<(NodeId, HbMessage)>>,
}

impl HbbftState {
    pub fn new() -> Self {
        HbbftState {
            network_info: None,
            honey_badger: None,
            public_master_key: None,
            current_posdao_epoch: 0,
            future_messages_cache: BTreeMap::new(),
        }
    }

    fn new_honey_badger(&self, network_info: NetworkInfo<NodeId>) -> Option<HoneyBadger> {
        let mut builder: HoneyBadgerBuilder<Contribution, _> =
            HoneyBadger::builder(Arc::new(network_info));
        return Some(builder.build());
    }

    // should honey badger be updated because the epoch is behind ?
    // returns Some(target_posdao_epoch) if so, and None if not.
    pub fn should_update_honeybadger(
        &self,
        client: Arc<dyn EngineClient>,
        block_id: BlockId,
        force: bool,
    ) -> Option<u64> {
        match get_posdao_epoch(&*client, block_id) {
            Ok(target_posdao_epoch_u256) => {
                let target_posdao_epoch = target_posdao_epoch_u256.low_u64();

                if force {
                    return Some(target_posdao_epoch);
                } else if self.current_posdao_epoch == target_posdao_epoch {
                    // hbbft state is already up to date.
                    return None;
                } else {
                    return Some(target_posdao_epoch);
                }
            }
            Err(call_error) => {
                error!(target: "engine", "Could not read posdao epoch. for block_id {:?} {:?}", block_id, call_error);
                None
            }
        }
    }

    // assumes that should_update_honeybadger has been called, what only requires a readlock instead of a write lock.
    pub fn update_honeybadger(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
        block_id: BlockId,
        target_posdao_epoch: u64,
    ) -> Option<()> {
        let posdao_epoch_start: ethereum_types::U256;
        
        //= get_posdao_epoch_start(&*client, block_id).ok()?;
        //if let posdao

        match get_posdao_epoch_start(&*client, block_id) {
            Ok(value) => posdao_epoch_start = value,
            Err(err) => {
                error!(target: "engine", "could not get posdao epoch start at block {:?}. Error: {:?}", block_id, err);
                return None;
            },
        }
        
        let synckeygen = initialize_synckeygen(
            &*client,
            signer,
            BlockId::Number(posdao_epoch_start.low_u64()),
            ValidatorType::Current,
        )
        .ok()?;
        assert!(synckeygen.is_ready());

        let (pks, sks) = synckeygen.generate().ok()?;
        self.public_master_key = Some(pks.public_key());
        // Clear network info and honey badger instance, since we may not be in this POSDAO epoch any more.
        info!(target: "engine", "public master key: {:?}", pks.public_key());

        self.network_info = None;
        self.honey_badger = None;
        // Set the current POSDAO epoch #
        self.current_posdao_epoch = target_posdao_epoch;
        trace!(target: "engine", "Switched hbbft state to epoch {}.", self.current_posdao_epoch);
        if sks.is_none() {
            trace!(target: "engine", "We are not part of the HoneyBadger validator set - running as regular node.");
            return Some(());
        }

        let network_info = synckeygen_to_network_info(&synckeygen, pks, sks)?;
        self.network_info = Some(network_info.clone());
        self.honey_badger = Some(self.new_honey_badger(network_info)?);

        trace!(target: "engine", "HoneyBadger Algorithm initialized! Running as validator node.");
        Some(())
    }

    // Call periodically to assure cached messages will eventually be delivered.
    pub fn replay_cached_messages(
        &mut self,
        client: Arc<dyn EngineClient>,
    ) -> Option<(Vec<HoneyBadgerResult>, NetworkInfo<NodeId>)> {
        let honey_badger = self.honey_badger.as_mut()?;

        if honey_badger.epoch() == 0 {
            // honey_badger not initialized yet, wait to be called after initialization.
            return None;
        }

        // Caveat:
        // If all necessary honey badger processing for an hbbft epoch is done the HoneyBadger
        // implementation automatically jumps to the next hbbft epoch.
        // This means hbbft may already be on the next epoch while the current epoch/block is not
        // imported yet.
        // The Validator Set may actually change, so we do not know to whom to send these messages yet.
        // We have to attempt to switch to the newest block, and then check if the hbbft epoch's parent
        // block is already imported. If not we have to wait until that block is available.
        let parent_block = honey_badger.epoch() - 1;
        match get_posdao_epoch(&*client, BlockId::Number(parent_block)) {
            Ok(epoch) => {
                if epoch.low_u64() != self.current_posdao_epoch {
                    trace!(target: "engine", "replay_cached_messages: Parent block(#{}) imported, but hbbft state not updated yet, re-trying later.", parent_block);
                    return None;
                }
            }
            Err(e) => {
                trace!(target: "engine", "replay_cached_messages: Could not query posdao epoch at parent block#{}, re-trying later. Probably due to the block not being imported yet. {:?}", parent_block, e);
                return None;
            }
        }

        let messages = self.future_messages_cache.get(&honey_badger.epoch())?;
        if messages.is_empty() {
            return None;
        }

        let network_info = self.network_info.as_ref()?.clone();

        let all_steps: Vec<_> = messages
			.iter()
			.map(|m| {
				trace!(target: "engine", "Replaying cached consensus message {:?} from {}", m.1, m.0);
				honey_badger.handle_message(&m.0, m.1.clone())
			})
			.collect();

        // Delete current epoch and all previous messages
        self.future_messages_cache = self
            .future_messages_cache
            .split_off(&(honey_badger.epoch() + 1));

        Some((all_steps, network_info))
    }

    fn skip_to_current_epoch(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
    ) -> Option<()> {
        // Ensure we evaluate at the same block # in the entire upward call graph to avoid inconsistent state.
        let latest_block_number = client.block_number(BlockId::Latest)?;

        let target_posdao_epoch = self
            .should_update_honeybadger(client.clone(), BlockId::Number(latest_block_number), false)
            .expect("Expected should_update_honeybadger to succeed");

        // Update honey_badger *before* trying to use it to make sure we use the data
        // structures matching the current epoch.
        self.update_honeybadger(
            client.clone(),
            signer,
            BlockId::Number(latest_block_number),
            target_posdao_epoch,
        );

        // If honey_badger is None we are not a validator, nothing to do.
        let honey_badger = self.honey_badger.as_mut()?;

        let next_block = latest_block_number + 1;
        // if next_block != honey_badger.epoch() {
        //trace!(target: "consensus", "Skipping honey_badger forward to epoch(block) {}, was at epoch(block) {}.", next_block, honey_badger.epoch());
        // }
        honey_badger.skip_to_epoch(next_block);

        Some(())
    }

    pub fn process_message(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
        sender_id: NodeId,
        message: HbMessage,
    ) -> Option<(HoneyBadgerStep, NetworkInfo<NodeId>)> {
        self.skip_to_current_epoch(client, signer)?;

        // If honey_badger is None we are not a validator, nothing to do.
        let honey_badger = self.honey_badger.as_mut()?;

        let message_epoch = message.epoch();
        // Note that if the message is for a future epoch we do not know if the current honey_badger
        // instance is the correct one to use. Tt may change if the the POSDAO epoch changes, causing
        // consensus messages to get lost.
        if message_epoch > honey_badger.epoch() {
            trace!(target: "consensus", "Message from future epoch, caching it for handling it in when the epoch is current. Current hbbft epoch is: {}", honey_badger.epoch());
            self.future_messages_cache
                .entry(message.epoch())
                .or_default()
                .push((sender_id, message));
            return None;
        }

        let network_info = self.network_info.as_ref()?.clone();

        match honey_badger.handle_message(&sender_id, message) {
            Ok(step) => Some((step, network_info)),
            Err(err) => {
                // TODO: Report consensus step errors
                // maybe we are not part of the HBBFT Set anymore ?
                // maybe the sender is not Part of the hbbft set ?
                // maybe we have the wrong hbbft for decryption ?
                error!(target: "consensus", "Error on handling HoneyBadger message from {} in epoch {} error: {:?}",sender_id, message_epoch, err);
                None
            }
        }
    }

    pub fn contribute_if_contribution_threshold_reached(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
    ) -> Option<(HoneyBadgerStep, NetworkInfo<NodeId>)> {
        // If honey_badger is None we are not a validator, nothing to do.
        let honey_badger = self.honey_badger.as_mut()?;
        let network_info = self.network_info.as_ref()?;

        if honey_badger.received_proposals() > network_info.num_faulty() {
            return self.try_send_contribution(client, signer);
        }
        None
    }

    pub fn try_send_contribution(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
    ) -> Option<(HoneyBadgerStep, NetworkInfo<NodeId>)> {
        // Make sure we are in the most current epoch.
        self.skip_to_current_epoch(client.clone(), signer)?;

        let honey_badger = self.honey_badger.as_mut()?;

        // If we already sent a contribution for this epoch, there is nothing to do.
        if honey_badger.has_input() {
            return None;
        }

        // If the parent block of the block we would contribute to is not in the hbbft state's
        // epoch we cannot start to contribute, since we would write into a hbbft instance
        // which will be destroyed.
        let posdao_epoch = get_posdao_epoch(&*client, BlockId::Number(honey_badger.epoch() - 1))
            .ok()?
            .low_u64();
        if self.current_posdao_epoch != posdao_epoch {
            trace!(target: "consensus", "hbbft_state epoch mismatch: hbbft_state epoch is {}, honey badger instance epoch is: {}.",
				   self.current_posdao_epoch, posdao_epoch);
            return None;
        }

        let network_info = self.network_info.as_ref()?.clone();

        trace!(target: "consensus", "Writing contribution for hbbft epoch(block) {}.", honey_badger.epoch());

        // Now we can select the transactions to include in our contribution.
        // TODO: Select a random *subset* of transactions to propose
        let input_contribution = Contribution::new(
            &client
                .queued_transactions()
                .iter()
                .map(|txn| txn.signed().clone())
                .collect(),
        );

        let mut rng = rand_065::thread_rng();
        let step = honey_badger.propose(&input_contribution, &mut rng);
        match step {
            Ok(step) => Some((step, network_info)),
            _ => {
                // TODO: Report detailed consensus step errors
                error!(target: "consensus", "Error on proposing Contribution.");
                None
            }
        }
    }

    pub fn verify_seal(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
        signature: &Signature,
        header: &Header,
    ) -> bool {
        self.skip_to_current_epoch(client.clone(), signer);

        // Check if posdao epoch fits the parent block of the header seal to verify.
        let parent_block_nr = header.number() - 1;
        let target_posdao_epoch = match get_posdao_epoch(&*client, BlockId::Number(parent_block_nr))
        {
            Ok(number) => number.low_u64(),
            Err(e) => {
                error!(target: "consensus", "Failed to verify seal - reading POSDAO epoch from contract failed! Error: {:?}", e);
                return false;
            }
        };
        if self.current_posdao_epoch != target_posdao_epoch {
            trace!(target: "consensus", "verify_seal - hbbft state epoch does not match epoch at the header's parent, attempting to reconstruct the appropriate public key share from scratch.");
            // If the requested block nr is already imported we try to generate the public master key from scratch.
            let posdao_epoch_start = match get_posdao_epoch_start(
                &*client,
                BlockId::Number(parent_block_nr),
            ) {
                Ok(epoch_start) => epoch_start,
                Err(e) => {
                    error!(target: "consensus", "Querying epoch start block failed with error: {:?}", e);
                    return false;
                }
            };

            let synckeygen = match initialize_synckeygen(
                &*client,
                &Arc::new(RwLock::new(Option::None)),
                BlockId::Number(posdao_epoch_start.low_u64()),
                ValidatorType::Current,
            ) {
                Ok(synckeygen) => synckeygen,
                Err(e) => {
                    error!(target: "consensus", "Synckeygen failed with error: {:?}", e);
                    return false;
                }
            };

            if !synckeygen.is_ready() {
                error!(target: "consensus", "Synckeygen not ready when it sohuld be!");
                return false;
            }

            let pks = match synckeygen.generate() {
                Ok((pks, _)) => pks,
                Err(e) => {
                    error!(target: "consensus", "Generating of public key share failed with error: {:?}", e);
                    return false;
                }
            };

            trace!(target: "consensus", "verify_seal - successfully reconstructed public key share of past posdao epoch.");
            return pks.public_key().verify(signature, header.bare_hash());
        }

        match self.public_master_key {
            Some(key) => key.verify(signature, header.bare_hash()),
            None => {
                error!(target: "consensus", "Failed to verify seal - public master key not available!");
                false
            }
        }
    }

    pub fn network_info_for(
        &mut self,
        client: Arc<dyn EngineClient>,
        signer: &Arc<RwLock<Option<Box<dyn EngineSigner>>>>,
        block_nr: u64,
    ) -> Option<NetworkInfo<NodeId>> {
        self.skip_to_current_epoch(client.clone(), signer);

        let posdao_epoch = get_posdao_epoch(&*client, BlockId::Number(block_nr - 1))
            .ok()?
            .low_u64();

        if self.current_posdao_epoch != posdao_epoch {
            error!(target: "consensus", "Trying to get the network info from a different epoch. Current epoch: {}, Requested epoch: {}",
				   self.current_posdao_epoch, posdao_epoch);
            return None;
        }

        self.network_info.clone()
    }
}
