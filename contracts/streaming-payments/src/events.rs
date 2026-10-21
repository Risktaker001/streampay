//! Events emitted by StreamPay.
//!
//! Per-stream events are keyed by `stream_id`; `StreamsCreated` is keyed by
//! sender and carries the batch ids, and `BatchWithdrawSummary` is keyed by the
//! recipient of a `withdraw_many` call. Non-topic fields are event data.

use soroban_sdk::{contractevent, Address, BytesN, Vec};

use crate::stream::{OwnershipRole, WithdrawOutcome};

/// Emitted when a new stream is created and its deposit pulled in.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamCreated {
    #[topic]
    pub stream_id: u64,
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    pub deposit: i128,
    pub start_time: u64,
    pub end_time: u64,
    pub cancelable: bool,
}

/// Emitted once after an atomic batch creates its streams.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamsCreated {
    #[topic]
    pub sender: Address,
    pub stream_ids: Vec<u64>,
}

/// Emitted when the recipient withdraws accrued funds.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Withdrawn {
    #[topic]
    pub stream_id: u64,
    pub recipient: Address,
    pub amount: i128,
    /// Total withdrawn from the stream after this withdrawal.
    pub withdrawn_total: i128,
}

/// Emitted once after a `withdraw_many` batch settles, alongside the
/// per-stream [`Withdrawn`] events. The per-stream shape is unchanged so
/// existing consumers keep working; this event is the batch-level view: every
/// entry that paid, the total, and how many of the requested ids were skipped.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchWithdrawSummary {
    #[topic]
    pub recipient: Address,
    /// Entries that settled, in request order. A skipped entry appears only in
    /// the entrypoint's return value, never here — nothing was paid for it.
    pub settled: Vec<WithdrawOutcome>,
    /// Sum of the delivered amounts: the total the recipient received.
    pub total: i128,
    /// Ids the caller asked for.
    pub requested: u32,
    /// `requested - settled.len()`: entries skipped with a per-entry error.
    pub skipped: u32,
}

/// Emitted when a stream is cancelled. The recipient's accrued balance and
/// the sender's refund are both paid atomically in the same transaction.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamCancelled {
    #[topic]
    pub stream_id: u64,
    pub cancelled_by: Address,
    /// Amount paid out to the recipient at cancellation.
    pub recipient_amount: i128,
    /// Amount refunded to the sender at cancellation.
    pub sender_amount: i128,
}

/// Emitted when the sender adds funds to an existing stream.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamToppedUp {
    #[topic]
    pub stream_id: u64,
    pub sender: Address,
    pub amount: i128,
    /// Stream deposit after the top-up.
    pub new_deposit: i128,
}

/// Emitted when a sender pre-authorizes a future (optionally recurring)
/// stream creation and escrows the deposit plus keeper fee budget.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamScheduled {
    #[topic]
    pub intent_id: u64,
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    /// Deposit escrowed for *each* occurrence.
    pub deposit: i128,
    /// Scheduled start of the first occurrence (later occurrences are shifted
    /// by `interval`).
    pub start_time: u64,
    /// Scheduled end of the first occurrence.
    pub end_time: u64,
    pub cancelable: bool,
    /// Seconds between occurrence starts; `0` for a one-shot intent.
    pub interval: u64,
    /// Total number of streams this intent will create.
    pub occurrences: u32,
    /// Keeper fee escrowed for *each* occurrence.
    pub fee_budget: i128,
    /// Earliest time the first occurrence may be executed.
    pub execute_after: u64,
    /// After this time no occurrence may be executed; the escrow is reclaimable.
    pub expires_at: u64,
}

/// Emitted when a keeper executes one occurrence of a scheduled intent,
/// creating the stream and paying itself the escrowed per-occurrence fee.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledStreamExecuted {
    #[topic]
    pub intent_id: u64,
    pub keeper: Address,
    /// The newly created stream id.
    pub stream_id: u64,
    /// Zero-based index of the occurrence just executed.
    pub occurrence_index: u32,
    /// Occurrences still outstanding after this execution.
    pub occurrences_remaining: u32,
    /// Fee paid to the keeper for this execution.
    pub fee_paid: i128,
}

/// Emitted when a sender cancels a not-yet-executable intent and is refunded
/// the escrow for all outstanding occurrences.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledIntentCancelled {
    #[topic]
    pub intent_id: u64,
    pub sender: Address,
    /// Total refunded (deposit + fee budget for every outstanding occurrence).
    pub refund: i128,
}

/// Emitted when an intent passed `expires_at` with occurrences outstanding and
/// its escrow was returned to the sender.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledIntentExpired {
    #[topic]
    pub intent_id: u64,
    pub sender: Address,
    /// Total refunded for the outstanding occurrences.
    pub refund: i128,
}

/// Emitted exactly once per funded stream that owes a protocol fee, in the
/// same transaction as (and immediately after) `StreamCreated`. Keyed by
/// `stream_id` so fee history is auditable per stream; the stream's stored
/// fee (`stream_fee` read) and the token's `treasury_status` totals must
/// always reconcile with the sum of these events — that reconciliation is
/// the treasury invariant, checkable by anyone via `verify_treasury` and
/// `scripts/verify-fees.ts`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeCharged {
    #[topic]
    pub stream_id: u64,
    pub token: Address,
    /// The sender who paid the fee on top of the deposit.
    pub payer: Address,
    /// Fee amount in smallest token units (deposit * fee_bps / 10_000, floored).
    pub amount: i128,
    /// The fee rate in basis points that produced this amount.
    pub fee_bps: u32,
}

/// Emitted when the treasury governor withdraws accumulated protocol fees for
/// one token. Keyed by `token` (treasury accounting is per token);
/// `withdrawn_total` carries the cumulative withdrawn amount so event
/// consumers can reconcile against `treasury_status` without reading state.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeWithdrawn {
    #[topic]
    pub token: Address,
    /// The governor who authorized the withdrawal.
    pub governor: Address,
    /// Where the withdrawn fees were sent.
    pub recipient: Address,
    pub amount: i128,
    /// Total fees withdrawn for this token after this withdrawal.
    pub withdrawn_total: i128,
}

/// Emitted when a sponsor registers (or reconfigures) a gas-sponsorship
/// budget. Keyed by the sponsor address; the `registered` symbol lets indexers
/// separate budget setup from sponsored claims.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SponsorRegistered {
    #[topic]
    pub sponsor: Address,
    #[topic]
    pub kind: soroban_sdk::Symbol,
    pub token: Address,
    pub per_period_limit: i128,
    pub per_beneficiary_limit: i128,
    pub window_seconds: u64,
}

/// Emitted when a sponsor pauses or resumes its budget.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SponsorActiveSet {
    #[topic]
    pub sponsor: Address,
    #[topic]
    pub kind: soroban_sdk::Symbol,
    pub active: bool,
}

/// Emitted when a beneficiary binds an ed25519 consent key, and again whenever
/// they rotate it. Keyed by the beneficiary address.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BeneficiaryEnrolled {
    #[topic]
    pub beneficiary: Address,
    #[topic]
    pub kind: soroban_sdk::Symbol,
    pub public_key: BytesN<32>,
}

/// Emitted when a sponsor rotates its authorized operator (relayer) key. The
/// previous operator key stops validating relay signatures immediately.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorKeyRotated {
    #[topic]
    pub sponsor: Address,
    #[topic]
    pub kind: soroban_sdk::Symbol,
    pub new_operator_key: BytesN<32>,
}

/// Emitted when a claim is sponsored and paid out: the operator relayed the
/// beneficiary's withdrawal and the sponsor's budget absorbed the gas. The
/// payout still flows from the stream, exactly as in an unsponsored withdraw.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SponsorClaim {
    #[topic]
    pub sponsor: Address,
    #[topic]
    pub kind: soroban_sdk::Symbol,
    #[topic]
    pub stream_id: u64,
    pub beneficiary: Address,
    pub amount: i128,
    pub nonce: u64,
}

/// Emitted when the policy module is initialized with its initial admin.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyInitialized {
    pub admin: Address,
}

/// Emitted when an asset allowlist mutation is proposed and queued under timelock.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyAssetQueued {
    #[topic]
    pub asset: Address,
    pub allowed: bool,
    pub executable_after: u64,
}

/// Emitted when a queued asset allowlist change is executed after timelock expiry.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyAssetExecuted {
    #[topic]
    pub asset: Address,
    pub allowed: bool,
}

/// Emitted when the admin proposes a new admin in step 1 of the two-step handoff.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyHandoffProposed {
    #[topic]
    pub current_admin: Address,
    pub proposed_admin: Address,
}

/// Emitted when the proposed admin accepts the handoff in step 2.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyHandoffAccepted {
    #[topic]
    pub new_admin: Address,
}

/// Emitted when a pending admin handoff is cancelled by the current admin.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyHandoffCancelled {
    #[topic]
    pub proposed_admin: Address,
}

/// Emitted when a contract upgrade is proposed with a timelock delay.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposed {
    #[topic]
    pub new_wasm_hash: BytesN<32>,
    pub unlock_time: u64,
    pub proposer: Address,
}

/// Emitted when a pending contract upgrade is cancelled by the governor.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeCancelled {
    #[topic]
    pub new_wasm_hash: BytesN<32>,
    pub cancelled_by: Address,
}

/// Emitted when a contract upgrade is executed after its timelock has elapsed.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractUpgraded {
    #[topic]
    pub new_wasm_hash: BytesN<32>,
    pub executed_by: Address,
}

/// Emitted when the contract freeze state is toggled by the governor.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractFrozenStateChanged {
    #[topic]
    pub frozen: bool,
    pub changed_by: Address,
}

/// Emitted when a batch of streams is migrated to a new storage schema version.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageMigrated {
    #[topic]
    pub from_version: u32,
    #[topic]
    pub to_version: u32,
    pub start_id: u64,
    pub end_id: u64,
    pub is_complete: bool,
}

/// Emitted when a single stream's storage record is re-encoded under a new
/// layout version. Only fired when a rewrite actually happened: an
/// already-current record migrates as a silent no-op, so the absence of this
/// event is what makes repeated `migrate_stream_version` calls idempotent.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRecordMigrated {
    #[topic]
    pub stream_id: u64,
    pub from_version: u32,
    pub to_version: u32,
}

/// Emitted when a stream policy floor/ceiling configuration is updated.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamPolicySet {
    pub token: Option<Address>,
    pub old_policy: Option<crate::policy::StreamPolicy>,
    pub new_policy: crate::policy::StreamPolicy,
}

/// Emitted when the oracle configuration is updated by governance.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleConfigUpdated {
    pub threshold: u32,
    pub max_stale_seconds: u64,
}

/// Emitted when an authorized oracle publisher reports a reference timestamp.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleTimeReported {
    #[topic]
    pub publisher: Address,
    pub reported_time: u64,
    pub ledger_time: u64,
}

/// Emitted when oracle time resolution falls back to native ledger time due to
/// stale feed, insufficient reports, or unconfigured registry.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleStaleFallback {
    #[topic]
    pub stream_id: u64,
    pub ledger_time: u64,
}

/// Emitted when a current sender or recipient proposes handing their role to a
/// new address. The handoff takes effect only when the target accepts.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnershipProposed {
    #[topic]
    pub stream_id: u64,
    /// The current role holder (must be the stream's sender or recipient).
    pub current_owner: Address,
    /// Address that must accept before the role moves.
    pub new_owner: Address,
    pub role: OwnershipRole,
    /// Ledger timestamp at or after which the proposal is stale.
    pub expires_at: u64,
}

/// Emitted when a proposed ownership handoff is accepted by its target. The
/// stream's accounting fields (`deposit`, `withdrawn`, time window) are
/// unchanged; only the role address moves.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnershipAccepted {
    #[topic]
    pub stream_id: u64,
    /// The role holder before the transfer.
    pub previous_owner: Address,
    /// The new role holder (the accepting address).
    pub new_owner: Address,
    pub role: OwnershipRole,
}

/// Emitted when the sender proposes handing the payer role to a new address.
/// Unlike [`OwnershipProposed`], this handoff is not completed by the target's
/// acceptance: the *recipient* consents, because changing who pays them (and
/// who can cancel on them) is their risk to take.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderTransferProposed {
    #[topic]
    pub stream_id: u64,
    pub sender_from: Address,
    pub new_sender: Address,
    /// The proposal's freshness marker, which the consenting call must repeat.
    pub nonce: u64,
    pub expires_at: u64,
    /// Whether consenting also moves the refund claim for the existing deposit.
    pub transfer_refund_rights: bool,
}

/// Emitted when the recipient's consent executes a proposed sender transfer.
/// `sender_from` / `sender_to` name both sides of the hop; the accounting
/// fields of the stream are untouched.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderTransferred {
    #[topic]
    pub stream_id: u64,
    pub sender_from: Address,
    pub sender_to: Address,
    pub nonce: u64,
    /// False when the deposit's refund claim stayed with the original sender —
    /// the default, and the reason this is a field rather than an inference.
    pub refund_rights_transferred: bool,
}

/// Emitted when a proposal that aged out of its acceptance window is reclaimed,
/// freeing its storage. The role never moved.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderTransferExpired {
    #[topic]
    pub stream_id: u64,
    pub sender_from: Address,
    pub new_sender: Address,
    pub nonce: u64,
}

/// Emitted whenever the refund claim of a stream is pinned to a new address —
/// at creation (`create_stream_with_refund`) or later by the sender
/// (`set_refund_target`). The unstreamed remainder of the deposit is paid to
/// `new_target` on cancellation instead of to the sender. Accrual, the
/// recipient's claims and who may cancel are untouched: this is a destination
/// change, not a value change.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRefundTargetChanged {
    #[topic]
    pub stream_id: u64,
    /// The address that held the claim before this change: the previous
    /// `new_target`, or the sender on the first routing of a stream.
    pub previous_target: Address,
    pub new_target: Address,
    /// True when the routing was set by `create_stream_with_refund`, so a
    /// reader can tell a creation-time destination from a later amendment.
    pub at_creation: bool,
}
