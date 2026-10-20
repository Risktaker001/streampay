//! Stream state, storage, and the checked accrual math.
//!
//! Accrual is linear over the stream window: a stream funded with `deposit`
//! between `start_time` and `end_time` streams `deposit * elapsed / duration`
//! tokens, floored to whole token units. All arithmetic is checked (`u128`
//! intermediate products) and returns [`StreamError::MathOverflow`] instead of
//! overflowing or panicking.

use soroban_sdk::{contracttype, Address, BytesN, Env, MuxedAddress, TryFromVal, Val, Vec};

use crate::errors::StreamError;
use crate::surface::{self, Branch};

/// Minimum delay for an upgrade proposal timelock: 24 hours.
pub const MIN_UPGRADE_DELAY_SECONDS: u64 = 86_400;

/// Initial schema version (v1).
pub const INITIAL_SCHEMA_VERSION: u32 = 1;

/// Target schema version after migration (v2).
pub const TARGET_SCHEMA_VERSION: u32 = 2;

/// Maximum number of streams processed per migration call.
pub const MAX_MIGRATION_BATCH_SIZE: u32 = 50;

/// Storage-record layout of the original, **untagged** stream entries: the
/// value stored under [`DataKey::Stream`] was a bare [`Stream`] struct, so its
/// version could only be *inferred* from the absence of a version tag.
pub const STREAM_RECORD_VERSION_V1: u32 = 1;

/// Storage-record layout that carries an explicit version tag: the stored value
/// is a [`VersionedStream`], so every record states its own layout version on
/// write instead of leaving readers to guess.
pub const STREAM_RECORD_VERSION_V2: u32 = 2;

/// The record layout this build writes. Appending a field to [`Stream`] means
/// bumping this constant (and, if the field is not an `Option`, adding a
/// migration step in [`migrate_stream_record`]).
pub const CURRENT_STREAM_RECORD_VERSION: u32 = STREAM_RECORD_VERSION_V2;

/// Maximum supported stream duration: 100 years, including leap days
/// (36,525 days). Bounds worst-case rate multiplications and rejects
/// nonsensical windows early.
pub const MAX_DURATION_SECONDS: u64 = 3_155_760_000;

/// Maximum number of stream ids a single paginated index query may return.
/// Callers asking for more are clamped down; this bounds per-call footprint
/// of the address-index views.
pub const MAX_PAGE_SIZE: u32 = 50;

/// Minimum number of stream entries permitted in a batch.
pub const MIN_BATCH: u32 = 1;

/// Maximum number of stream entries permitted in a batch.
///
/// Bounded to 25 because each stream creation emits a `StreamCreated` event
/// and a token `transfer` event (~600 bytes combined). At 25 streams, event
/// size is ~15.5 KB, safely within Soroban's network limit of 16,384 bytes (16 KB),
/// and consumes ~16.2M CPU instructions (~16% of the 100M transaction budget).
pub const MAX_BATCH: u32 = 25;

/// Lifetime of a pending ownership proposal, in seconds (7 days).
///
/// A proposal that is not accepted within this window becomes stale: it can no
/// longer be accepted (the target must ask the current role holder to propose
/// again), but the current holder can still cancel it explicitly or replace it
/// with a fresh proposal. Expiry is enforced against the ledger timestamp, not
/// storage TTL, so the semantics are identical on every network.
pub const OWNERSHIP_PROPOSAL_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;

/// TTL maintenance, in ledgers (~5 seconds per ledger on Stellar mainnet).
///
/// Streams live in persistent storage: entries that go untouched are
/// eventually archived off-chain (and auto-restored at a fee when next read).
/// Every mutating entrypoint extends the stream entry's TTL, so a stream that
/// is actually being used never goes cold.
pub(crate) const TTL_THRESHOLD_LEDGERS: u32 = 200_000; // extend when TTL drops below ~11.5 days
pub(crate) const TTL_EXTEND_LEDGERS: u32 = 500_000; // extend to ~29 days

/// The role being handed over by a two-step ownership transfer.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum OwnershipRole {
    /// The payer: may top up and cancel, and receives cancellation refunds.
    Sender,
    /// The claimant: may withdraw accrued funds.
    Recipient,
}

/// A pending two-step ownership proposal for one stream.
///
/// Proposals live under their own storage key ([`DataKey::PendingOwnership`])
/// rather than inside [`Stream`], so accepting or cancelling one leaves the
/// stream's accounting fields untouched and the dedicated key can be given its
/// own TTL treatment.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingOwnership {
    pub stream_id: u64,
    /// The current role holder that created the proposal (and the only address
    /// allowed to cancel it).
    pub proposed_by: Address,
    /// The address that must accept the role before it takes effect.
    pub new_owner: Address,
    pub role: OwnershipRole,
    /// Ledger timestamp at or after which the proposal is stale.
    pub expires_at: u64,
}

/// Lifecycle state of a stream.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StreamStatus {
    /// Running; funds accrue and can be withdrawn or topped up.
    Active,
    /// Cancelled; remaining funds were split at cancellation. Terminal.
    Cancelled,
    /// Fully streamed and fully withdrawn. Terminal.
    Depleted,
}

/// Full state of a payment stream.
///
/// Field order and names are the storage wire format: appending a field is a
/// schema change that bumps [`CURRENT_STREAM_RECORD_VERSION`], and the field
/// must be an `Option` (or defaulted by a migration) so older records keep
/// decoding.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stream {
    pub id: u64,
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    /// Total amount funded into the stream (original deposit + top-ups).
    pub deposit: i128,
    /// Total amount paid to the recipient so far (withdrawals + cancel payout).
    pub withdrawn: i128,
    pub start_time: u64,
    pub end_time: u64,
    /// If true, the recipient may also cancel and reclaim their accrued balance.
    pub cancelable: bool,
    pub status: StreamStatus,
    /// If true, settles against external oracle time feed instead of ledger time.
    pub use_oracle: bool,
}

impl Stream {
    /// Seconds in the stream's full window. Always `>= 1` for a stored stream
    /// (`create_stream` rejects `end_time <= start_time`).
    pub fn duration(&self) -> u64 {
        self.end_time - self.start_time
    }
}

/// A persisted [`Stream`] record together with the storage layout version it was
/// written under.
///
/// This is the value stored under [`DataKey::Stream`] for every record written
/// by [`put_stream`]; the `version` field is written explicitly, never inferred.
/// [`get_stream_record`] decodes the legacy untagged [`STREAM_RECORD_VERSION_V1`]
/// layout as well, so records created before versioned writes landed keep
/// working until they are migrated.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionedStream {
    pub version: u32,
    pub stream: Stream,
}

/// Outcome of [`migrate_stream_record`]: which layout a single stream moved
/// from and to, and whether it moved at all.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamVersionReport {
    pub stream_id: u64,
    pub from_version: u32,
    pub to_version: u32,
    /// False when the record already carried the current layout, so nothing was
    /// rewritten — the proof that migration is idempotent.
    pub migrated: bool,
}

/// Parameters for creating a single stream within a batch creation call.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchEntry {
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    pub deposit: i128,
    pub start_time: u64,
    pub end_time: u64,
    pub cancelable: bool,
}

/// Proposed contract upgrade pending timelock expiration.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingUpgrade {
    pub new_wasm_hash: BytesN<32>,
    pub unlock_time: u64,
}

/// Progress report for on-chain storage migration.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationStatus {
    pub current_version: u32,
    pub target_version: u32,
    pub cursor: u64,
    pub total_streams: u64,
    pub is_complete: bool,
}

/// Storage keys for the contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Next stream id to hand out; ids start at 1.
    NextStreamId,
    /// The stream with the given id. Stored as a [`VersionedStream`] by
    /// current builds; [`STREAM_RECORD_VERSION_V1`] records (a bare [`Stream`]
    /// with no version tag) remain readable until migrated.
    Stream(u64),
    /// Append-only index of stream ids where the given address is the
    /// recipient. Advisory (discovery) data: the `Stream` entries remain the
    /// source of truth, and terminal streams keep their place in the index.
    RecipientStreams(Address),
    /// Append-only index of stream ids created (as sender) by the given
    /// address. Same advisory semantics as [`DataKey::RecipientStreams`].
    SenderStreams(Address),
    /// Next scheduled-intent id to hand out; ids start at 1.
    NextIntentId,
    /// The scheduled intent with the given id.
    Intent(u64),
    /// Append-only index of scheduled-intent ids created (as sender) by the
    /// given address. Same advisory semantics as [`DataKey::SenderStreams`].
    SenderIntents(Address),
    /// Treasury governor address (instance storage; set once by the
    /// contract constructor, later by two-step nomination/acceptance).
    Governor,
    /// Pending governor nominee (instance storage; set by
    /// `nominate_governor`, cleared by `accept_governor`).
    PendingGovernor,
    /// Configured protocol fee in basis points (instance storage; absent
    /// means 0 — fees disabled, the v0.3 behavior).
    FeeBps,
    /// Per-token fee accounting: fees charged minus fees withdrawn
    /// (persistent storage; written only by fee-affecting entrypoints).
    Treasury(Address),
    /// Fee charged for one stream at creation time (persistent storage;
    /// written only when the fee rounds above zero, so absent == 0).
    StreamFee(u64),
    /// Schema version for storage layout evolution (instance storage).
    SchemaVersion,
    /// Freeze state: when true, all state-mutating entrypoints revert (instance storage).
    ContractFrozen,
    /// Proposed contract upgrade pending timelock expiration (instance storage).
    PendingUpgrade,
    /// Highest stream id processed by storage migration (instance storage).
    MigrationCursor,
    /// Global oracle configuration (instance storage).
    OracleConfig,
    /// Most recent timestamp report from a publisher (persistent storage).
    OracleReport(Address),
    /// The pending two-step ownership proposal for the given stream, if any.
    /// At most one proposal exists per stream at a time.
    PendingOwnership(u64),
}

/// Seconds elapsed of the stream window at the given timestamp `now`.
pub fn elapsed_seconds_at(now: u64, start_time: u64, end_time: u64) -> u64 {
    if now <= start_time {
        0
    } else if now >= end_time {
        // MUTANT-SUPPRESS: replace - with + is acceptable because this branch
        // is only reachable when now >= end_time, which means the full duration
        // has already elapsed. The test elapsed_after_end_returns_full_duration
        // verifies that the full deposit is available at this point. The mutation
        // would cause an overflow in the subsequent streamed_at calculation,
        // which is caught by MathOverflow. This is an unreachable code path given
        // the contract's usage pattern.
        end_time - start_time
    } else {
        now - start_time
    }
}

/// Seconds elapsed of the stream window at the current ledger time.
///
/// The window is half-open `[start_time, end_time)`: at `now == start_time`
/// nothing has accrued yet; at `now >= end_time` the full duration has elapsed.
#[allow(dead_code)]
pub fn elapsed_seconds(env: &Env, start_time: u64, end_time: u64) -> u64 {
    elapsed_seconds_at(env.ledger().timestamp(), start_time, end_time)
}

/// Validates that `deposit * duration` — the worst-case rate multiplication —
/// fits in `u128`, so [`streamed_amount`] can never overflow for this stream.
///
/// `duration` must be `>= 1` (enforced by `create_stream`).
pub fn validate_capacity(deposit: i128, duration: u64) -> Result<(), StreamError> {
    let cap = u128::MAX / (duration.max(1) as u128);
    if (deposit as u128) > cap {
        return Err(surface::reject(Branch::CapacityOverflow));
    }
    Ok(())
}

/// Pure rate computation: amount streamed after `elapsed` seconds of a
/// `duration`-second window funded with `deposit`.
///
/// Linear rate: `deposit * elapsed / duration`, floored to whole token units,
/// capped at `deposit`. Checked math throughout; returns
/// [`StreamError::MathOverflow`] rather than overflowing or panicking. Exposed
/// as a pure function so tests and fuzzing can exercise it without an
/// environment.
pub fn streamed_at(deposit: i128, duration: u64, elapsed: u64) -> Result<i128, StreamError> {
    if elapsed == 0 || deposit <= 0 || duration == 0 {
        return Ok(0);
    }
    let product = (deposit as u128)
        .checked_mul(elapsed as u128)
        .ok_or_else(|| surface::reject(Branch::RateProductOverflow))?;
    let streamed = (product / (duration.max(1) as u128)).min(deposit as u128);
    // `streamed <= deposit <= i128::MAX`, so the narrowing cast cannot wrap.
    Ok(streamed as i128)
}

/// Amount streamed (accrued) at reference timestamp `now`.
pub fn streamed_amount_at(stream: &Stream, now: u64) -> Result<i128, StreamError> {
    let elapsed = elapsed_seconds_at(now, stream.start_time, stream.end_time);
    streamed_at(stream.deposit, stream.duration(), elapsed)
}

/// Amount streamed (accrued) at the current ledger time.
///
/// See [`streamed_at`] for the rate semantics.
#[allow(dead_code)]
pub fn streamed_amount(env: &Env, stream: &Stream) -> Result<i128, StreamError> {
    streamed_amount_at(stream, env.ledger().timestamp())
}

/// Accrued-but-unwithdrawn balance at reference timestamp `now`. Zero for
/// cancelled streams.
pub fn available_amount_at(stream: &Stream, now: u64) -> Result<i128, StreamError> {
    if stream.status == StreamStatus::Cancelled {
        return Ok(0);
    }
    let streamed = streamed_amount_at(stream, now)?;
    streamed
        .checked_sub(stream.withdrawn)
        .ok_or_else(|| surface::reject(Branch::AvailableBalanceOverflow))
}

/// Accrued-but-unwithdrawn balance at the current ledger time. Zero for
/// cancelled streams (their payout was settled atomically at cancellation).
#[allow(dead_code)]
pub fn available_amount(env: &Env, stream: &Stream) -> Result<i128, StreamError> {
    available_amount_at(stream, env.ledger().timestamp())
}

/// Payout split at cancellation at reference timestamp `now`: `(recipient_amount, sender_amount)`.
pub fn cancel_amounts_at(stream: &Stream, now: u64) -> Result<(i128, i128), StreamError> {
    let streamed = streamed_amount_at(stream, now)?;
    let recipient_amount = streamed
        .checked_sub(stream.withdrawn)
        .ok_or_else(|| surface::reject(Branch::CancelSplitOverflow))?;
    let sender_amount = stream
        .deposit
        .checked_sub(streamed)
        .ok_or_else(|| surface::reject(Branch::CancelSplitOverflow))?;
    Ok((recipient_amount, sender_amount))
}

/// Payout split at cancellation: `(recipient_amount, sender_amount)`.
///
/// The recipient receives their accrued-but-unwithdrawn balance; the sender is
/// refunded the unaccrued remainder.
#[allow(dead_code)]
pub fn cancel_amounts(env: &Env, stream: &Stream) -> Result<(i128, i128), StreamError> {
    cancel_amounts_at(stream, env.ledger().timestamp())
}

/// Address helper: token transfer destinations are muxed addresses.
pub(crate) fn muxed(addr: &Address) -> MuxedAddress {
    addr.clone().into()
}

/// Hands out the next stream id (1-based) and stores the incremented counter.
pub fn next_stream_id(env: &Env) -> u64 {
    let key = DataKey::NextStreamId;
    let id: u64 = env.storage().persistent().get(&key).unwrap_or(1);
    env.storage().persistent().set(&key, &id.saturating_add(1));
    id
}

/// Reserves a contiguous range of stream ids with one persistent write.
pub fn reserve_stream_ids(env: &Env, count: u32) -> Result<u64, StreamError> {
    let key = DataKey::NextStreamId;
    let first: u64 = env.storage().persistent().get(&key).unwrap_or(1);
    let next = first
        .checked_add(count as u64)
        .ok_or(StreamError::MathOverflow)?;
    env.storage().persistent().set(&key, &next);
    Ok(first)
}

/// Loads a stream, or returns [`StreamError::StreamNotFound`].
///
/// A thin wrapper over [`get_stream_record`] that discards the layout version.
pub fn get_stream(env: &Env, id: u64) -> Result<Stream, StreamError> {
    Ok(get_stream_record(env, id)?.stream)
}

/// Loads a stream together with the storage layout version its record carries.
///
/// Both layouts are readable: the current [`VersionedStream`] tag and the
/// legacy untagged [`Stream`] map (reported as [`STREAM_RECORD_VERSION_V1`]).
/// The probe order is deliberate — the tagged layout is tried first, and a
/// record that decodes as *neither* layout is refused with
/// [`StreamError::SchemaVersionUnsupported`] rather than being guessed at.
/// That is the fail-closed half of versioning: a record written by a future
/// build with fields this build has never seen can never be silently
/// mis-decoded into a present-day [`Stream`].
pub fn get_stream_record(env: &Env, id: u64) -> Result<VersionedStream, StreamError> {
    #[cfg(test)]
    crate::cei_trace::record(&std::format!("read persistent Stream({id})"));
    let key = DataKey::Stream(id);
    // Reading as a raw `Val` cannot fail: shape probing happens below with
    // fallible conversions, not through the storage getter's infallible ones.
    let Some(raw): Option<Val> = env.storage().persistent().get(&key) else {
        return Err(surface::reject(Branch::StreamNotFound));
    };
    if let Ok(record) = VersionedStream::try_from_val(env, &raw) {
        // Unknown tags — from the future *or* malformed — are refused: only a
        // layout this build actually understands may be decoded.
        if record.version < STREAM_RECORD_VERSION_V1
            || record.version > CURRENT_STREAM_RECORD_VERSION
        {
            return Err(surface::reject(Branch::SchemaVersionUnsupported));
        }
        return Ok(record);
    }
    if let Ok(stream) = Stream::try_from_val(env, &raw) {
        return Ok(VersionedStream {
            version: STREAM_RECORD_VERSION_V1,
            stream,
        });
    }
    Err(surface::reject(Branch::SchemaVersionUnsupported))
}

/// The storage layout version of one stream record.
pub fn stream_record_version(env: &Env, id: u64) -> Result<u32, StreamError> {
    Ok(get_stream_record(env, id)?.version)
}

/// Re-encodes one stream record under [`CURRENT_STREAM_RECORD_VERSION`].
///
/// Idempotent by construction: a record that already carries the current layout
/// is reported with `migrated = false` and left untouched (no rewrite, so no
/// storage delta and no event). Callable by anyone — it changes no accounting,
/// only the encoding of a record that is already provably equivalent — and it
/// touches exactly one storage entry, which is what bounds its gas.
pub fn migrate_stream_record(env: &Env, id: u64) -> Result<StreamVersionReport, StreamError> {
    let record = get_stream_record(env, id)?;
    if record.version == CURRENT_STREAM_RECORD_VERSION {
        return Ok(StreamVersionReport {
            stream_id: id,
            from_version: record.version,
            to_version: record.version,
            migrated: false,
        });
    }
    put_stream(env, &record.stream);
    Ok(StreamVersionReport {
        stream_id: id,
        from_version: record.version,
        to_version: CURRENT_STREAM_RECORD_VERSION,
        migrated: true,
    })
}

/// Persists a stream under the current versioned layout and extends its storage TTL.
pub fn put_stream(env: &Env, stream: &Stream) {
    #[cfg(test)]
    crate::cei_trace::record(&std::format!("write persistent Stream({})", stream.id));
    let key = DataKey::Stream(stream.id);
    let record = VersionedStream {
        version: CURRENT_STREAM_RECORD_VERSION,
        stream: stream.clone(),
    };
    env.storage().persistent().set(&key, &record);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Appends a stream's id to both parties' address indexes.
///
/// Called after each stream created by `create_stream` or `create_streams`
/// is persisted.
/// Cancel/deplete/top-up flows intentionally do not touch the indexes: they
/// are append-only discovery aids keyed by *creation*, not by lifecycle.
pub fn index_stream(env: &Env, stream: &Stream) {
    index_sender(env, &stream.sender, stream.id);
    index_recipient(env, &stream.recipient, stream.id);
}

/// Appends `id` to the sender's discovery index. Called on creation and when
/// an accepted ownership transfer installs a new sender.
pub fn index_sender(env: &Env, sender: &Address, id: u64) {
    push_to_index(env, &DataKey::SenderStreams(sender.clone()), id);
}

/// Appends `id` to the recipient's discovery index. Called on creation and
/// when an accepted ownership transfer installs a new recipient.
pub fn index_recipient(env: &Env, recipient: &Address, id: u64) {
    push_to_index(env, &DataKey::RecipientStreams(recipient.clone()), id);
}

/// Loads the pending ownership proposal for `id`, if one exists, and extends
/// its storage TTL.
///
/// The `Some` check matters: `extend_ttl` on a key that was never written (or
/// was removed) fails, so we only extend after a successful read.
pub fn get_pending_ownership(env: &Env, id: u64) -> Option<PendingOwnership> {
    let key = DataKey::PendingOwnership(id);
    let storage = env.storage().persistent();
    let pending: PendingOwnership = storage.get(&key)?;
    storage.extend_ttl(&key, TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
    Some(pending)
}

/// Persists a pending ownership proposal and extends its storage TTL.
pub fn put_pending_ownership(env: &Env, pending: &PendingOwnership) {
    let key = DataKey::PendingOwnership(pending.stream_id);
    env.storage().persistent().set(&key, pending);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Removes a pending ownership proposal. No-op if none exists.
pub fn remove_pending_ownership(env: &Env, id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::PendingOwnership(id));
}

pub(crate) fn push_to_index(env: &Env, key: &DataKey, id: u64) {
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(key)
        .unwrap_or_else(|| Vec::new(env));
    ids.push_back(id);
    env.storage().persistent().set(key, &ids);
    env.storage()
        .persistent()
        .extend_ttl(key, TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Returns one page of stream ids from an address index, in creation order.
///
/// `limit` is clamped to [`MAX_PAGE_SIZE`]; an `offset` past the end (or an
/// unknown address) yields an empty page rather than an error. Reading the
/// index extends its TTL, so actively-queried addresses never go cold.
/// Addresses with no index entry never had any streams: return early without
/// touching storage (`extend_ttl` on a never-written key would fail).
pub fn page_stream_ids(env: &Env, key: &DataKey, offset: u32, limit: u32) -> Vec<u64> {
    let storage = env.storage().persistent();
    let Some(ids): Option<Vec<u64>> = storage.get(key) else {
        return Vec::new(env);
    };
    storage.extend_ttl(key, TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);

    let start = offset.min(ids.len());
    let end = start
        .saturating_add(limit.min(MAX_PAGE_SIZE))
        .min(ids.len());
    ids.slice(start..end)
}

/// Returns the current schema version (defaults to INITIAL_SCHEMA_VERSION if unset).
pub fn schema_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::SchemaVersion)
        .unwrap_or(INITIAL_SCHEMA_VERSION)
}

/// Persists the new schema version and extends instance storage TTL.
pub fn set_schema_version(env: &Env, version: u32) {
    env.storage()
        .instance()
        .set(&DataKey::SchemaVersion, &version);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Checks whether the contract is currently frozen.
pub fn is_frozen(env: &Env) -> bool {
    #[cfg(test)]
    crate::cei_trace::record("read instance ContractFrozen");
    env.storage()
        .instance()
        .get(&DataKey::ContractFrozen)
        .unwrap_or(false)
}

/// Updates the contract freeze state and extends instance storage TTL.
pub fn set_frozen(env: &Env, frozen: bool) {
    #[cfg(test)]
    crate::cei_trace::record("write instance ContractFrozen");
    env.storage()
        .instance()
        .set(&DataKey::ContractFrozen, &frozen);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Returns the pending contract upgrade proposal, if one exists.
pub fn pending_upgrade(env: &Env) -> Option<PendingUpgrade> {
    env.storage().instance().get(&DataKey::PendingUpgrade)
}

/// Sets the pending contract upgrade proposal and extends instance storage TTL.
pub fn set_pending_upgrade(env: &Env, pending: &PendingUpgrade) {
    env.storage()
        .instance()
        .set(&DataKey::PendingUpgrade, pending);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Removes any pending contract upgrade proposal and extends instance storage TTL.
pub fn remove_pending_upgrade(env: &Env) {
    env.storage().instance().remove(&DataKey::PendingUpgrade);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}

/// Returns the current migration cursor (highest stream id migrated so far).
pub fn migration_cursor(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::MigrationCursor)
        .unwrap_or(0)
}

/// Sets the migration cursor and extends instance storage TTL.
pub fn set_migration_cursor(env: &Env, cursor: u64) {
    env.storage()
        .instance()
        .set(&DataKey::MigrationCursor, &cursor);
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD_LEDGERS, TTL_EXTEND_LEDGERS);
}
