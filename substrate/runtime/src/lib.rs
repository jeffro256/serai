#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

use sp_core::OpaqueMetadata;
pub use sp_core::sr25519::{Public, Signature};
use sp_runtime::{
  create_runtime_str, generic, impl_opaque_keys, KeyTypeId,
  traits::{Convert, OpaqueKeys, IdentityLookup, BlakeTwo256, Block as BlockT},
  transaction_validity::{TransactionSource, TransactionValidity},
  ApplyExtrinsicResult, Perbill,
};
use sp_std::prelude::*;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

use frame_support::{
  traits::{ConstU8, ConstU32, ConstU64},
  weights::{
    constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
    IdentityFee, Weight,
  },
  dispatch::DispatchClass,
  parameter_types, construct_runtime,
};
pub use frame_system::Call as SystemCall;

pub use pallet_timestamp::Call as TimestampCall;
pub use pallet_balances::Call as BalancesCall;
use pallet_transaction_payment::CurrencyAdapter;

use pallet_session::PeriodicSessions;

/// An index to a block.
pub type BlockNumber = u32;

/// Account ID type, equivalent to a public key
pub type AccountId = Public;

/// Balance of an account.
pub type Balance = u64;

/// Index of a transaction in the chain, for a given account.
pub type Index = u32;

/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;

pub mod opaque {
  use super::*;

  pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

  pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
  pub type Block = generic::Block<Header, UncheckedExtrinsic>;
  pub type BlockId = generic::BlockId<Block>;

  impl_opaque_keys! {
    pub struct SessionKeys {
      pub tendermint: Tendermint,
    }
  }
}

use opaque::SessionKeys;

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
  spec_name: create_runtime_str!("serai"),
  // TODO: "core"?
  impl_name: create_runtime_str!("turoctocrab"),
  authoring_version: 1,
  // TODO: 1? Do we prefer some level of compatibility or our own path?
  spec_version: 100,
  impl_version: 1,
  apis: RUNTIME_API_VERSIONS,
  transaction_version: 1,
  state_version: 1,
};

// 1 MB
pub const BLOCK_SIZE: u32 = 1024 * 1024;
// 6 seconds
pub const TARGET_BLOCK_TIME: u64 = 6000;

/// Measured in blocks.
pub const MINUTES: BlockNumber = 60_000 / (TARGET_BLOCK_TIME as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
  NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

// Unit = the base number of indivisible units for balances
const UNIT: Balance = 1_000_000_000_000;
const MILLIUNIT: Balance = 1_000_000_000;

const fn deposit(items: u32, bytes: u32) -> Balance {
  (items as Balance * UNIT + (bytes as Balance) * (5 * MILLIUNIT / 100)) / 10
}

parameter_types! {
  pub const BlockHashCount: BlockNumber = 2400;
  pub const Version: RuntimeVersion = VERSION;

  pub const SS58Prefix: u8 = 42; // TODO: Remove for Bech32m

  // 1 MB block size limit
  pub BlockLength: frame_system::limits::BlockLength =
    frame_system::limits::BlockLength::max_with_normal_ratio(BLOCK_SIZE, NORMAL_DISPATCH_RATIO);
  pub BlockWeights: frame_system::limits::BlockWeights =
    frame_system::limits::BlockWeights::with_sensible_defaults(
      Weight::from_ref_time(2u64 * WEIGHT_REF_TIME_PER_SECOND).set_proof_size(u64::MAX),
      NORMAL_DISPATCH_RATIO,
    );

  pub const DepositPerItem: Balance = deposit(1, 0);
  pub const DepositPerByte: Balance = deposit(0, 1);
  pub const DeletionQueueDepth: u32 = 128;
  // The lazy deletion runs inside on_initialize.
  pub DeletionWeightLimit: Weight = BlockWeights::get()
    .per_class
    .get(DispatchClass::Normal)
    .max_total
    .unwrap_or(BlockWeights::get().max_block);
}

impl frame_system::Config for Runtime {
  type BaseCallFilter = frame_support::traits::Everything;
  type BlockWeights = BlockWeights;
  type BlockLength = BlockLength;
  type AccountId = AccountId;
  type RuntimeCall = RuntimeCall;
  type Lookup = IdentityLookup<AccountId>;
  type Index = Index;
  type BlockNumber = BlockNumber;
  type Hash = Hash;
  type Hashing = BlakeTwo256;
  type Header = Header;
  type RuntimeOrigin = RuntimeOrigin;
  type RuntimeEvent = RuntimeEvent;
  type BlockHashCount = BlockHashCount;
  type DbWeight = RocksDbWeight;
  type Version = Version;
  type PalletInfo = PalletInfo;

  type OnNewAccount = ();
  type OnKilledAccount = ();
  type OnSetCode = ();

  type AccountData = pallet_balances::AccountData<Balance>;
  type SystemWeightInfo = ();
  type SS58Prefix = SS58Prefix; // TODO: Remove for Bech32m

  type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_timestamp::Config for Runtime {
  type Moment = u64;
  type OnTimestampSet = ();
  type MinimumPeriod = ConstU64<{ TARGET_BLOCK_TIME / 2 }>;
  type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
  type MaxLocks = ConstU32<50>;
  type MaxReserves = ();
  type ReserveIdentifier = [u8; 8];
  type Balance = Balance;
  type RuntimeEvent = RuntimeEvent;
  type DustRemoval = ();
  type ExistentialDeposit = ConstU64<500>;
  type AccountStore = System;
  type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
}

impl pallet_transaction_payment::Config for Runtime {
  type RuntimeEvent = RuntimeEvent;
  type OnChargeTransaction = CurrencyAdapter<Balances, ()>;
  type OperationalFeeMultiplier = ConstU8<5>;
  type WeightToFee = IdentityFee<Balance>;
  type LengthToFee = IdentityFee<Balance>;
  type FeeMultiplierUpdate = ();
}

const SESSION_LENGTH: BlockNumber = 5 * DAYS;
type Sessions = PeriodicSessions<ConstU32<{ SESSION_LENGTH }>, ConstU32<{ SESSION_LENGTH }>>;

pub struct IdentityValidatorIdOf;
impl Convert<Public, Option<Public>> for IdentityValidatorIdOf {
  fn convert(key: Public) -> Option<Public> {
    Some(key)
  }
}

impl validator_sets_pallet::Config for Runtime {
  type RuntimeEvent = RuntimeEvent;
}

impl pallet_session::Config for Runtime {
  type RuntimeEvent = RuntimeEvent;
  type ValidatorId = AccountId;
  type ValidatorIdOf = IdentityValidatorIdOf;
  type ShouldEndSession = Sessions;
  type NextSessionRotation = Sessions;
  type SessionManager = ();
  type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
  type Keys = SessionKeys;
  type WeightInfo = pallet_session::weights::SubstrateWeight<Runtime>;
}

impl pallet_tendermint::Config for Runtime {}

pub type Address = AccountId;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type SignedExtra = (
  frame_system::CheckNonZeroSender<Runtime>,
  frame_system::CheckSpecVersion<Runtime>,
  frame_system::CheckTxVersion<Runtime>,
  frame_system::CheckGenesis<Runtime>,
  frame_system::CheckEra<Runtime>,
  frame_system::CheckNonce<Runtime>,
  frame_system::CheckWeight<Runtime>,
  pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);
pub type UncheckedExtrinsic =
  generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
pub type SignedPayload = generic::SignedPayload<RuntimeCall, SignedExtra>;
pub type Executive = frame_executive::Executive<
  Runtime,
  Block,
  frame_system::ChainContext<Runtime>,
  Runtime,
  AllPalletsWithSystem,
>;

construct_runtime!(
  pub enum Runtime where
    Block = Block,
    NodeBlock = Block,
    UncheckedExtrinsic = UncheckedExtrinsic
  {
    System: frame_system,
    Timestamp: pallet_timestamp,
    Balances: pallet_balances,
    TransactionPayment: pallet_transaction_payment,

    ValidatorSets: validator_sets_pallet,
    Session: pallet_session,
    Tendermint: pallet_tendermint,
  }
);

#[cfg(feature = "runtime-benchmarks")]
#[macro_use]
extern crate frame_benchmarking;

#[cfg(feature = "runtime-benchmarks")]
mod benches {
  define_benchmarks!(
    [frame_benchmarking, BaselineBench::<Runtime>]
    [frame_system, SystemBench::<Runtime>]
    [pallet_balances, Balances]
    [pallet_timestamp, Timestamp]
  );
}

sp_api::impl_runtime_apis! {
  impl sp_api::Core<Block> for Runtime {
    fn version() -> RuntimeVersion {
      VERSION
    }

    fn execute_block(block: Block) {
      Executive::execute_block(block);
    }

    fn initialize_block(header: &<Block as BlockT>::Header) {
      Executive::initialize_block(header)
    }
  }

  impl sp_api::Metadata<Block> for Runtime {
    fn metadata() -> OpaqueMetadata {
      OpaqueMetadata::new(Runtime::metadata().into())
    }
  }

  impl sp_block_builder::BlockBuilder<Block> for Runtime {
    fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
      Executive::apply_extrinsic(extrinsic)
    }

    fn finalize_block() -> <Block as BlockT>::Header {
      Executive::finalize_block()
    }

    fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
      data.create_extrinsics()
    }

    fn check_inherents(
      block: Block,
      data: sp_inherents::InherentData,
    ) -> sp_inherents::CheckInherentsResult {
      data.check_extrinsics(&block)
    }
  }

  impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
    fn validate_transaction(
      source: TransactionSource,
      tx: <Block as BlockT>::Extrinsic,
      block_hash: <Block as BlockT>::Hash,
    ) -> TransactionValidity {
      Executive::validate_transaction(source, tx, block_hash)
    }
  }

  impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
    fn offchain_worker(header: &<Block as BlockT>::Header) {
      Executive::offchain_worker(header)
    }
  }

  impl sp_session::SessionKeys<Block> for Runtime {
    fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
      opaque::SessionKeys::generate(seed)
    }

    fn decode_session_keys(
      encoded: Vec<u8>,
    ) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
      opaque::SessionKeys::decode_into_raw_public_keys(&encoded)
    }
  }

  impl sp_tendermint::TendermintApi<Block> for Runtime {
    fn current_session() -> u32 {
      Tendermint::session()
    }

    fn validators() -> Vec<Public> {
      Session::validators()
    }
  }

  impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Index> for Runtime {
    fn account_nonce(account: AccountId) -> Index {
      System::account_nonce(account)
    }
  }

  impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<
    Block,
    Balance
  > for Runtime {
    fn query_info(
      uxt: <Block as BlockT>::Extrinsic,
      len: u32,
    ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
      TransactionPayment::query_info(uxt, len)
    }
    fn query_fee_details(
      uxt: <Block as BlockT>::Extrinsic,
      len: u32,
    ) -> pallet_transaction_payment::FeeDetails<Balance> {
      TransactionPayment::query_fee_details(uxt, len)
    }
  }
}
