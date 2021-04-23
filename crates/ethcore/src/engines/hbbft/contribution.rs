use rand_065::{self, distributions::Standard, Rng};
use rlp::RlpStream;
use std::time::UNIX_EPOCH;
use types::transaction::SignedTransaction;

#[derive(Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
pub(crate) struct Contribution {
    pub transactions: Vec<Vec<u8>>,
    pub timestamp: u64,
    /// Random data for on-chain randomness.
    ///
    /// The invariant of `random_data.len()` == RANDOM_BYTES_PER_EPOCH **must** hold true.
    pub random_data: Vec<u8>,
}

/// Number of random bytes to generate per epoch.
///
/// Currently, we want twenty u32s worth of random data to generated on each epoch.
// TODO: Make this configurable somewhere.
const RANDOM_BYTES_PER_EPOCH: usize = 4 * 20;

/// Returns the current UNIX Epoch time, in seconds.
pub fn unix_now_secs() -> u64 {
    UNIX_EPOCH.elapsed().expect("Time not available").as_secs()
}

/// Returns the current UNIX Epoch time, in milliseconds.
pub fn unix_now_millis() -> u128 {
    UNIX_EPOCH
        .elapsed()
        .expect("Time not available")
        .as_millis()
}

impl Contribution {
    pub fn new(txns: &Vec<SignedTransaction>, timestamp: u64) -> Self {
        let ser_txns: Vec<_> = txns
            .iter()
            .map(|txn| {
                let mut s = RlpStream::new();
                txn.rlp_append(&mut s);
                s.drain()
            })
            .collect();
        let mut rng = rand_065::thread_rng();

        //debug!("creating a contribution with TS {}", timestamp);
        println!("creating a contribution with TS {}", timestamp);

        Contribution {
            transactions: ser_txns,
            timestamp: timestamp,
            random_data: rng
                .sample_iter(&Standard)
                .take(RANDOM_BYTES_PER_EPOCH)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crypto::publickey::{Generator, Random};
    use engines::hbbft::test::test_helpers::create_transaction;
    use ethereum_types::U256;
    use types::transaction::{SignedTransaction, TypedTransaction};
    use engines::hbbft::contribution::unix_now_secs;

    #[test]
    fn test_contribution_serialization() {
        let mut pending: Vec<SignedTransaction> = Vec::new();
        let keypair = Random.generate();
        pending.push(create_transaction(&keypair, &U256::from(1)));
        let contribution = super::Contribution::new(&pending, unix_now_secs());

        let deser_txns: Vec<_> = contribution
            .transactions
            .iter()
            .filter_map(|ser_txn| TypedTransaction::decode(ser_txn).ok())
            .filter_map(|txn| SignedTransaction::new(txn).ok())
            .collect();

        assert_eq!(pending.len(), deser_txns.len());
        assert_eq!(
            pending.iter().nth(0).unwrap(),
            deser_txns.iter().nth(0).unwrap()
        );
    }
}
