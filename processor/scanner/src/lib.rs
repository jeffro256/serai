use core::{marker::PhantomData, fmt::Debug};
use std::{io, collections::HashMap};

use group::GroupEncoding;

use serai_db::{Get, DbTxn, Db};

use serai_primitives::{NetworkId, Coin, Amount};
use serai_in_instructions_primitives::Batch;
use serai_coins_primitives::OutInstructionWithBalance;

use primitives::{task::*, Address, ReceivedOutput, Block};

// Logic for deciding where in its lifetime a multisig is.
mod lifetime;
pub use lifetime::LifetimeStage;

// Database schema definition and associated functions.
mod db;
use db::{ScannerGlobalDb, SubstrateToEventualityDb};
// Task to index the blockchain, ensuring we don't reorganize finalized blocks.
mod index;
// Scans blocks for received coins.
mod scan;
/// Check blocks for transactions expected to eventually occur.
mod eventuality;
/// Task which reports `Batch`s to Substrate.
mod report;

pub(crate) fn sort_outputs<K: GroupEncoding, A: Address, O: ReceivedOutput<K, A>>(
  a: &O,
  b: &O,
) -> core::cmp::Ordering {
  use core::cmp::{Ordering, Ord};
  let res = a.id().as_ref().cmp(b.id().as_ref());
  assert!(res != Ordering::Equal, "two outputs within a collection had the same ID");
  res
}

/// Extension traits around Block.
pub(crate) trait BlockExt: Block {
  fn scan_for_outputs(&self, key: Self::Key) -> Vec<Self::Output>;
}
impl<B: Block> BlockExt for B {
  fn scan_for_outputs(&self, key: Self::Key) -> Vec<Self::Output> {
    let mut outputs = self.scan_for_outputs_unordered(key);
    outputs.sort_by(sort_outputs);
    outputs
  }
}

/// A feed usable to scan a blockchain.
///
/// This defines the primitive types used, along with various getters necessary for indexing.
#[async_trait::async_trait]
pub trait ScannerFeed: 'static + Send + Sync + Clone {
  /// The ID of the network being scanned for.
  const NETWORK: NetworkId;

  /// The amount of confirmations a block must have to be considered finalized.
  ///
  /// This value must be at least `1`.
  const CONFIRMATIONS: u64;

  /// The amount of blocks to process in parallel.
  ///
  /// This must be at least `1`. This must be less than or equal to `CONFIRMATIONS`. This value
  /// should be the worst-case latency to handle a block divided by the expected block time.
  const WINDOW_LENGTH: u64;

  /// The amount of blocks which will occur in 10 minutes (approximate).
  ///
  /// This value must be at least `1`.
  const TEN_MINUTES: u64;

  /// The representation of a block for this blockchain.
  ///
  /// A block is defined as a consensus event associated with a set of transactions. It is not
  /// necessary to literally define it as whatever the external network defines as a block. For
  /// external networks which finalize block(s), this block type should be a representation of all
  /// transactions within a finalization event.
  type Block: Block;

  /// An error encountered when fetching data from the blockchain.
  ///
  /// This MUST be an ephemeral error. Retrying fetching data from the blockchain MUST eventually
  /// resolve without manual intervention/changing the arguments.
  type EphemeralError: Debug;

  /// Fetch the number of the latest finalized block.
  ///
  /// The block number is its zero-indexed position within a linear view of the external network's
  /// consensus. The genesis block accordingly has block number 0.
  async fn latest_finalized_block_number(&self) -> Result<u64, Self::EphemeralError>;

  /// Fetch a block header by its number.
  ///
  /// This does not check the returned BlockHeader is the header for the block we indexed.
  async fn unchecked_block_header_by_number(
    &self,
    number: u64,
  ) -> Result<<Self::Block as Block>::Header, Self::EphemeralError>;

  /// Fetch a block by its number.
  ///
  /// This does not check the returned Block is the block we indexed.
  async fn unchecked_block_by_number(
    &self,
    number: u64,
  ) -> Result<Self::Block, Self::EphemeralError>;

  /// Fetch a block by its number.
  ///
  /// Panics if the block requested wasn't indexed.
  async fn block_by_number(
    &self,
    getter: &(impl Send + Sync + Get),
    number: u64,
  ) -> Result<Self::Block, String> {
    let block = match self.unchecked_block_by_number(number).await {
      Ok(block) => block,
      Err(e) => Err(format!("couldn't fetch block {number}: {e:?}"))?,
    };

    // Check the ID of this block is the expected ID
    {
      let expected = crate::index::block_id(getter, number);
      if block.id() != expected {
        panic!(
          "finalized chain reorganized from {} to {} at {}",
          hex::encode(expected),
          hex::encode(block.id()),
          number,
        );
      }
    }

    Ok(block)
  }

  /// The dust threshold for the specified coin.
  ///
  /// This MUST be constant. Serai MUST NOT create internal outputs worth less than this. This
  /// SHOULD be a value worth handling at a human level.
  fn dust(&self, coin: Coin) -> Amount;

  /// The cost to aggregate an input as of the specified block.
  ///
  /// This is defined as the transaction fee for a 2-input, 1-output transaction.
  async fn cost_to_aggregate(
    &self,
    coin: Coin,
    reference_block: &Self::Block,
  ) -> Result<Amount, Self::EphemeralError>;
}

type KeyFor<S> = <<S as ScannerFeed>::Block as Block>::Key;
type AddressFor<S> = <<S as ScannerFeed>::Block as Block>::Address;
type OutputFor<S> = <<S as ScannerFeed>::Block as Block>::Output;
type EventualityFor<S> = <<S as ScannerFeed>::Block as Block>::Eventuality;

#[async_trait::async_trait]
pub trait BatchPublisher: 'static + Send + Sync {
  /// An error encountered when publishing the Batch.
  ///
  /// This MUST be an ephemeral error. Retrying publication MUST eventually resolve without manual
  /// intervention/changing the arguments.
  type EphemeralError: Debug;

  /// Publish a Batch.
  ///
  /// This function must be safe to call with the same Batch multiple times.
  async fn publish_batch(&mut self, batch: Batch) -> Result<(), Self::EphemeralError>;
}

/// A return to occur.
pub struct Return<S: ScannerFeed> {
  address: AddressFor<S>,
  output: OutputFor<S>,
}

impl<S: ScannerFeed> Return<S> {
  pub(crate) fn write(&self, writer: &mut impl io::Write) -> io::Result<()> {
    self.address.write(writer)?;
    self.output.write(writer)
  }

  pub(crate) fn read(reader: &mut impl io::Read) -> io::Result<Self> {
    let address = AddressFor::<S>::read(reader)?;
    let output = OutputFor::<S>::read(reader)?;
    Ok(Return { address, output })
  }
}

/// An update for the scheduler.
pub struct SchedulerUpdate<S: ScannerFeed> {
  outputs: Vec<OutputFor<S>>,
  forwards: Vec<OutputFor<S>>,
  returns: Vec<Return<S>>,
}

/// The object responsible for accumulating outputs and planning new transactions.
pub trait Scheduler<S: ScannerFeed>: 'static + Send {
  /// Activate a key.
  ///
  /// This SHOULD setup any necessary database structures. This SHOULD NOT cause the new key to
  /// be used as the primary key. The multisig rotation time clearly establishes its steps.
  fn activate_key(&mut self, txn: &mut impl DbTxn, key: KeyFor<S>);

  /// Flush all outputs within a retiring key to the new key.
  ///
  /// When a key is activated, the existing multisig should retain its outputs and utility for a
  /// certain time period. With `flush_key`, all outputs should be directed towards fulfilling some
  /// obligation or the `new_key`. Every output MUST be connected to an Eventuality. If a key no
  /// longer has active Eventualities, it MUST be able to be retired.
  fn flush_key(&mut self, txn: &mut impl DbTxn, retiring_key: KeyFor<S>, new_key: KeyFor<S>);

  /// Retire a key as it'll no longer be used.
  ///
  /// Any key retired MUST NOT still have outputs associated with it. This SHOULD be a NOP other
  /// than any assertions and database cleanup. This MUST NOT be expected to be called in a fashion
  /// ordered to any other calls.
  fn retire_key(&mut self, txn: &mut impl DbTxn, key: KeyFor<S>);

  /// Accumulate outputs into the scheduler, yielding the Eventualities now to be scanned for.
  ///
  /// `active_keys` is the list of active keys, potentially including a key for which we've already
  /// called `retire_key` on. If so, its stage will be `Finishing` and no further operations will
  /// be expected for it. Nonetheless, it may be present.
  ///
  /// The `Vec<u8>` used as the key in the returned HashMap should be the encoded key the
  /// Eventualities are for.
  fn update(
    &mut self,
    txn: &mut impl DbTxn,
    active_keys: &[(KeyFor<S>, LifetimeStage)],
    update: SchedulerUpdate<S>,
  ) -> HashMap<Vec<u8>, Vec<EventualityFor<S>>>;

  /// Fulfill a series of payments, yielding the Eventualities now to be scanned for.
  ///
  /// Any Eventualities returned by this function must include an output-to-Serai (such as a Branch
  /// or Change), unless they descend from a transaction returned by this function which satisfies
  /// that requirement.
  ///
  /// `active_keys` is the list of active keys, potentially including a key for which we've already
  /// called `retire_key` on. If so, its stage will be `Finishing` and no further operations will
  /// be expected for it. Nonetheless, it may be present.
  ///
  /// The `Vec<u8>` used as the key in the returned HashMap should be the encoded key the
  /// Eventualities are for.
  /*
    We need an output-to-Serai so we can detect a block with an Eventuality completion with regards
    to Burns, forcing us to ensure we have accumulated all the Burns we should by the time we
    handle that block. We explicitly don't require children have this requirement as by detecting
    the first resolution, we ensure we'll accumulate the Burns (therefore becoming aware of the
    childrens' Eventualities, enabling recognizing their resolutions).

    This carve out enables the following:

      ------------------  Fulfillment TX  ----------------------
      | Primary Output | ---------------> | New Primary Output |
      ------------------         |        ----------------------
                                 |
                                 |        ------------------------------
                                 |------> | Branching Output for Burns |
                                          ------------------------------

    Without wasting pointless Change outputs on every transaction (as there's a single parent which
    has an output-to-Serai, the new primary output).
  */
  fn fulfill(
    &mut self,
    txn: &mut impl DbTxn,
    active_keys: &[(KeyFor<S>, LifetimeStage)],
    payments: Vec<OutInstructionWithBalance>,
  ) -> HashMap<Vec<u8>, Vec<EventualityFor<S>>>;
}

/// A representation of a scanner.
#[allow(non_snake_case)]
pub struct Scanner<S: ScannerFeed> {
  eventuality_handle: RunNowHandle,
  _S: PhantomData<S>,
}
impl<S: ScannerFeed> Scanner<S> {
  /// Create a new scanner.
  ///
  /// This will begin its execution, spawning several asynchronous tasks.
  pub async fn new(
    db: impl Db,
    feed: S,
    batch_publisher: impl BatchPublisher,
    scheduler: impl Scheduler<S>,
    start_block: u64,
  ) -> Self {
    let index_task = index::IndexTask::new(db.clone(), feed.clone(), start_block).await;
    let scan_task = scan::ScanTask::new(db.clone(), feed.clone(), start_block);
    let report_task = report::ReportTask::<_, S, _>::new(db.clone(), batch_publisher, start_block);
    let eventuality_task = eventuality::EventualityTask::new(db, feed, scheduler, start_block);

    let (_index_handle, index_run) = RunNowHandle::new();
    let (scan_handle, scan_run) = RunNowHandle::new();
    let (report_handle, report_run) = RunNowHandle::new();
    let (eventuality_handle, eventuality_run) = RunNowHandle::new();

    // Upon indexing a new block, scan it
    tokio::spawn(index_task.continually_run(index_run, vec![scan_handle.clone()]));
    // Upon scanning a block, report it
    tokio::spawn(scan_task.continually_run(scan_run, vec![report_handle]));
    // Upon reporting a block, we do nothing
    tokio::spawn(report_task.continually_run(report_run, vec![]));
    // Upon handling the Eventualities in a block, we run the scan task as we've advanced the
    // window its allowed to scan
    tokio::spawn(eventuality_task.continually_run(eventuality_run, vec![scan_handle]));

    Self { eventuality_handle, _S: PhantomData }
  }

  /// Acknowledge a block.
  ///
  /// This means this block was ordered on Serai in relation to `Burn` events, and all validators
  /// have achieved synchrony on it.
  ///
  /// The calls to this function must be ordered with regards to `queue_burns`.
  pub fn acknowledge_block(
    &mut self,
    mut txn: impl DbTxn,
    block_number: u64,
    key_to_activate: Option<KeyFor<S>>,
  ) {
    log::info!("acknowledging block {block_number}");

    assert!(
      ScannerGlobalDb::<S>::is_block_notable(&txn, block_number),
      "acknowledging a block which wasn't notable"
    );
    if let Some(prior_highest_acknowledged_block) =
      ScannerGlobalDb::<S>::highest_acknowledged_block(&txn)
    {
      assert!(block_number > prior_highest_acknowledged_block, "acknowledging blocks out-of-order");
      for b in (prior_highest_acknowledged_block + 1) .. (block_number - 1) {
        assert!(
          !ScannerGlobalDb::<S>::is_block_notable(&txn, b),
          "skipped acknowledging a block which was notable"
        );
      }
    }

    ScannerGlobalDb::<S>::set_highest_acknowledged_block(&mut txn, block_number);
    if let Some(key_to_activate) = key_to_activate {
      ScannerGlobalDb::<S>::queue_key(&mut txn, block_number + S::WINDOW_LENGTH, key_to_activate);
    }

    // Commit the txn
    txn.commit();
    // Run the Eventuality task since we've advanced it
    // We couldn't successfully do this if that txn was still floating around, uncommitted
    // The execution of this task won't actually have more work until the txn is committed
    self.eventuality_handle.run_now();
  }

  /// Queue Burns.
  ///
  /// The scanner only updates the scheduler with new outputs upon acknowledging a block. The
  /// ability to fulfill Burns, and therefore their order, is dependent on the current output
  /// state. This immediately sets a bound that this function is ordered with regards to
  /// `acknowledge_block`.
  /*
    The fact Burns can be queued during any Substrate block is problematic. The scanner is allowed
    to scan anything within the window set by the Eventuality task. The Eventuality task is allowed
    to handle all blocks until it reaches a block needing acknowledgement.

    This means we may queue Burns when the latest acknowledged block is 1, yet we've already
    scanned 101. Such Burns may complete back in block 2, and we simply wouldn't have noticed due
    to not having yet generated the Eventualities.

    We solve this by mandating all transactions made as the result of an Eventuality include a
    output-to-Serai worth at least `DUST`. If that occurs, the scanner will force a consensus
    protocol on block 2. Accordingly, we won't scan all the way to block 101 (missing the
    resolution of the Eventuality) as we'll obtain synchrony on block 2 and all Burns queued prior
    to it.

    Another option would be to re-check historical blocks, yet this would potentially redo an
    unbounded amount of work. It would also not allow us to safely detect if received outputs were
    in fact the result of Eventualities or not.

    Another option would be to schedule Burns after the next-acknowledged block, yet this would add
    latency and likely practically require we add regularly scheduled notable blocks (which may be
    unnecessary).
  */
  pub fn queue_burns(&mut self, txn: &mut impl DbTxn, burns: &Vec<OutInstructionWithBalance>) {
    let queue_as_of = ScannerGlobalDb::<S>::highest_acknowledged_block(txn)
      .expect("queueing Burns yet never acknowledged a block");

    SubstrateToEventualityDb::send_burns(txn, queue_as_of, burns)
  }
}
