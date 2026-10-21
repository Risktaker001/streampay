//! Error types for StreamPay.
//!
//! Every expected failure is expressed as a [`StreamError`] variant with a
//! stable numeric code, so clients can branch on failures without parsing
//! strings. The contract never panics on expected failure paths: all public
//! entrypoints return `Result<_, StreamError>` and all arithmetic is checked.
//!
//! Codes are part of the public interface — never renumber or reuse a variant;
//! only append new ones at the bottom with the next free code. A variant that
//! turns out to be unreachable is *retired* (kept, documented, and excluded
//! from coverage in `surface.rs`) rather than removed or reused, so the wire
//! codes of every existing client stay valid forever.
//!
//! Token-side failures are always translated into one of the `*TransferFailed`
//! variants below. A token contract's own error numbers live in the same
//! `Error(Contract, #N)` namespace as this enum's codes, so letting a raw
//! token failure surface would let — say — a token's `#5` masquerade as
//! [`StreamError::StreamNotActive`] in clients. The contract therefore invokes
//! every token transfer through the SDK's fallible (`try_`) client and maps
//! any failure to a distinct StreamPay code; the original token error is
//! intentionally not propagated.
//!
//! [`StreamError::ALL`] mirrors the variant set for the generated error-surface
//! coverage test. The exhaustive match in
//! `error_surface_test::stream_error_variant_list_is_exhaustive` fails to
//! compile when a variant is added, forcing the author to extend `ALL` (and the
//! `Branch` registry) in the same change.

use soroban_sdk::contracterror;

/// Errors returned by the StreamPay contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum StreamError {
    /// No stream exists with the provided id.
    StreamNotFound = 1,
    /// **Retired — never produced.** Reserved role check; retained so the
    /// numeric code is never reused. See `docs/ERROR_SURFACE.md`.
    NotStreamSender = 2,
    /// **Retired — never produced.** Reserved role check; retained so the
    /// numeric code is never reused. See `docs/ERROR_SURFACE.md`.
    NotStreamRecipient = 3,
    /// The stream is not `cancelable`, so the recipient cannot cancel it.
    StreamNotCancellable = 4,
    /// The stream is cancelled or depleted; the operation is not permitted.
    StreamNotActive = 5,
    /// `start_time` is before the current ledger timestamp.
    StartTimeInPast = 6,
    /// `end_time` is not strictly after `start_time`.
    InvalidTimeRange = 7,
    /// The stream duration exceeds the maximum supported duration.
    DurationTooLong = 8,
    /// A provided amount (deposit, top-up, or withdrawal) is zero or negative.
    ZeroAmount = 9,
    /// The requested withdrawal exceeds the accrued, unwithdrawn balance.
    AmountExceedsAvailable = 10,
    /// An intermediate computation would overflow; all math is checked.
    MathOverflow = 11,
    /// The caller is not permitted to perform this action.
    NotAuthorized = 12,
    /// `withdraw_max` found nothing accrued-but-unwithdrawn to pay out.
    NothingToWithdraw = 13,
    /// The requested fee rate exceeds the protocol cap (`MAX_FEE_BPS`).
    FeeRateTooHigh = 14,
    /// The caller is not the treasury governor.
    NotGovernor = 15,
    /// `accept_governor` was called but no governor nomination is pending.
    NoPendingGovernor = 16,
    /// A treasury withdrawal exceeds the fees collected minus fees already
    /// withdrawn for that token.
    InsufficientTreasuryBalance = 17,
    /// No sponsor budget is registered for the provided sponsor address.
    SponsorNotFound = 49,
    /// The sponsor's budget exists but is currently paused (inactive).
    SponsorInactive = 50,
    /// The beneficiary has no registered ed25519 consent key; they must
    /// enroll once before a claim can be sponsored on their behalf.
    BeneficiaryNotEnrolled = 51,
    /// A sponsored claim was relayed after its signed `deadline`.
    ClaimExpired = 52,
    /// The claim's nonce was already spent by this sponsor for this
    /// beneficiary — a replay of a previously sponsored call.
    ClaimNonceReplay = 53,
    /// Paying this claim would exceed the sponsor's per-period budget cap.
    SponsorBudgetExceeded = 54,
    /// Paying this claim would exceed the per-beneficiary cap within the
    /// sponsor's current budget window.
    BeneficiaryBudgetExceeded = 55,
    /// No scheduled intent exists with the provided id.
    IntentNotFound = 18,
    /// The scheduled intent is not pending (already executed, cancelled, or
    /// expired), so the operation is not permitted.
    IntentNotPending = 19,
    /// Execution was attempted before the intent's `execute_after` (for the
    /// next outstanding occurrence).
    IntentNotYetExecutable = 20,
    /// Execution or cancellation was attempted at/after the intent's
    /// `expires_at`; the funds are reclaimable via `reclaim_expired`.
    IntentExpired = 21,
    /// The sender tried to cancel an intent whose execution window has already
    /// opened; cancellation is only allowed before `execute_after`.
    IntentNotCancellable = 22,
    /// The scheduling parameters are inconsistent (bad occurrence/interval
    /// combination, negative fee budget, ordering of `execute_after` /
    /// `start_time` / `expires_at`, or an overflowing recurrence window).
    InvalidScheduleParameters = 23,
    /// A defensive integrity check failed: the stored intent's parameters no
    /// longer hash to the commitment recorded when it was scheduled.
    IntentParamsMismatch = 24,
    /// `reclaim_expired` was called before the intent's `expires_at`.
    IntentNotExpired = 25,
    /// The token pull in `create_stream` failed: the sender's balance or
    /// allowance was insufficient (the pull covers `deposit_amount + fee`),
    /// or the asset refused the transfer (frozen/unauthorized/deauthorized
    /// holder, or any non-compliant token). Nothing is persisted when it
    /// fails, so no stream id is consumed.
    DepositTransferFailed = 26,
    /// The token pull in `top_up` failed (see
    /// [`StreamError::DepositTransferFailed`]; the stream's deposit is left
    /// untouched).
    TopUpTransferFailed = 27,
    /// The payout of accrued funds to the stream recipient failed — from
    /// `withdraw`, `withdraw_max`, or the recipient leg of `cancel_stream`.
    /// State changes made before the transfer are rolled back with the error.
    RecipientPayoutFailed = 28,
    /// The refund of unaccrued funds back to the sender failed during
    /// `cancel_stream` (the recipient leg, if any, was already paid).
    /// The cancellation is rolled back in full, so the stream stays active.
    SenderRefundFailed = 29,
    /// The payout of collected protocol fees failed in `withdraw_treasury`.
    /// The recorded withdrawal and the `FeeWithdrawn` event are rolled back
    /// with it, so the fee ledger stays unchanged.
    TreasuryPayoutFailed = 30,
    /// The asset is not permitted by the policy module allowlist.
    PolicyAssetNotAllowed = 31,
    /// The timelock delay has not elapsed for this queued policy change.
    PolicyTimelockNotElapsed = 32,
    /// No policy change is queued for the specified asset.
    PolicyNoQueuedChange = 33,
    /// The policy module has not been initialized.
    PolicyNotInitialized = 34,
    /// No admin handoff proposal is pending.
    PolicyNoPendingHandoff = 35,
    /// The contract is currently frozen for maintenance or upgrade.
    ContractFrozen = 36,
    /// The operation requires the contract to be frozen.
    ContractNotFrozen = 37,
    /// An upgrade operation was attempted but no upgrade is pending.
    NoPendingUpgrade = 38,
    /// The upgrade timelock delay has not yet expired.
    TimelockNotExpired = 39,
    /// The proposed upgrade delay is shorter than the minimum timelock.
    UpgradeDelayTooShort = 40,
    /// The storage migration has already been completed.
    AlreadyMigrated = 41,
    /// Stream rate is below the governance policy floor.
    PolicyRateBelowFloor = 42,
    /// Stream rate exceeds the governance policy ceiling.
    PolicyRateAboveCeiling = 43,
    /// Stream duration is below the governance policy floor.
    PolicyDurationBelowFloor = 44,
    /// Stream duration exceeds the governance policy ceiling.
    PolicyDurationAboveCeiling = 45,
    /// Stream deposit is below the governance policy floor.
    PolicyDepositBelowFloor = 46,
    /// Batch creation was requested with zero entries.
    BatchEmpty = 47,
    /// Batch creation was requested with more entries than MAX_BATCH.
    BatchTooLarge = 48,
    /// Oracle threshold is invalid (e.g., zero or exceeds publisher count).
    InvalidOracleThreshold = 56,
    /// Oracle publishers are not configured.
    OraclePublishersNotConfigured = 57,
    /// Oracle report timestamp is too far in the past or future.
    OracleReportTooOld = 58,
    /// The caller is not an authorized oracle publisher.
    NotOraclePublisher = 59,
    /// No two-step ownership proposal exists for this stream.
    NoPendingOwnership = 60,
    /// The pending ownership proposal's acceptance window has elapsed; the
    /// target must be proposed again by the current role holder.
    OwnershipProposalExpired = 61,
    /// The proposed new owner is already the current holder of that role.
    InvalidTransferTarget = 62,
    /// An unexpired ownership proposal already exists for this stream; cancel
    /// or accept it before proposing another.
    OwnershipProposalPending = 63,
    /// A stored record uses a layout this build does not understand: its
    /// version tag is from the future, or its shape decodes as neither the
    /// current versioned layout nor the legacy untagged one. Reads fail closed
    /// rather than guessing — see `stream::get_stream_record`.
    SchemaVersionUnsupported = 64,
    /// No sender-transfer proposal is stored for the stream, or the one that
    /// was there has already been executed, cancelled or replaced.
    SenderTransferNotFound = 65,
    /// The sender-transfer proposal's acceptance window has elapsed; the role
    /// did not move. Reclaim the stale entry with `expire_sender_transfer`.
    SenderTransferExpired = 66,
    /// `expire_sender_transfer` refused to reclaim a proposal that is still
    /// inside its acceptance window — only the recipient's consent or the
    /// window ending may end a proposal.
    SenderTransferNotExpired = 67,
    /// The nonce presented with the consent does not match the nonce stored
    /// with the proposal, so the consent is aimed at a different (or already
    /// replaced) proposal.
    SenderTransferNonceMismatch = 68,
    /// An unexpired sender-transfer proposal already exists for this stream;
    /// let it expire, execute or reclaim it before proposing another.
    SenderTransferPending = 69,
    /// The caller is not the stream's recipient, so their consent cannot
    /// execute a sender-role transfer.
    SenderTransferNotConsenter = 70,
    /// Only the current sender may route the refund of their deposit. The
    /// refund claim is the counterpart of the money that funded the stream, so
    /// a stranger cannot redirect it by calling `set_refund_target`.
    RefundTargetNotSender = 71,
    /// The requested refund target already holds the claim, so the amendment
    /// would rewrite the same address and emit an event that reports no
    /// change. Rejected so the log only ever records real routing changes.
    RefundTargetUnchanged = 72,
    /// `withdraw_many` was called with an empty list of stream ids. Rejected
    /// as its own code rather than `BatchEmpty` (47), which is documented as a
    /// *creation* batch precondition, so callers can tell the two apart.
    WithdrawBatchEmpty = 73,
    /// `withdraw_many` was called with more stream ids than
    /// `MAX_WITHDRAW_BATCH`. The cap is what keeps a single claim call inside
    /// the transaction budget; split larger sets across several calls.
    WithdrawBatchTooLarge = 74,
    /// `withdraw_many` was given the same stream id more than once. The second
    /// entry would claim against state the first entry already mutated (and,
    /// for `withdraw_max`-style amounts, would silently double-count the
    /// available balance), so the whole call is rejected instead of
    /// de-duplicated.
    WithdrawDuplicateStreamIds = 75,
}

impl StreamError {
    /// Every variant, in code order. The error-surface coverage test iterates
    /// this list, so a code no entrypoint can produce is a test failure unless
    /// it is listed as retired in `crate::surface::RETIRED_CODES`.
    pub const ALL: &'static [StreamError] = &[
        StreamError::StreamNotFound,
        StreamError::NotStreamSender,
        StreamError::NotStreamRecipient,
        StreamError::StreamNotCancellable,
        StreamError::StreamNotActive,
        StreamError::StartTimeInPast,
        StreamError::InvalidTimeRange,
        StreamError::DurationTooLong,
        StreamError::ZeroAmount,
        StreamError::AmountExceedsAvailable,
        StreamError::MathOverflow,
        StreamError::NotAuthorized,
        StreamError::NothingToWithdraw,
        StreamError::FeeRateTooHigh,
        StreamError::NotGovernor,
        StreamError::NoPendingGovernor,
        StreamError::InsufficientTreasuryBalance,
        StreamError::IntentNotFound,
        StreamError::IntentNotPending,
        StreamError::IntentNotYetExecutable,
        StreamError::IntentExpired,
        StreamError::IntentNotCancellable,
        StreamError::InvalidScheduleParameters,
        StreamError::IntentParamsMismatch,
        StreamError::IntentNotExpired,
        StreamError::DepositTransferFailed,
        StreamError::TopUpTransferFailed,
        StreamError::RecipientPayoutFailed,
        StreamError::SenderRefundFailed,
        StreamError::TreasuryPayoutFailed,
        StreamError::PolicyAssetNotAllowed,
        StreamError::PolicyTimelockNotElapsed,
        StreamError::PolicyNoQueuedChange,
        StreamError::PolicyNotInitialized,
        StreamError::PolicyNoPendingHandoff,
        StreamError::ContractFrozen,
        StreamError::ContractNotFrozen,
        StreamError::NoPendingUpgrade,
        StreamError::TimelockNotExpired,
        StreamError::UpgradeDelayTooShort,
        StreamError::AlreadyMigrated,
        StreamError::PolicyRateBelowFloor,
        StreamError::PolicyRateAboveCeiling,
        StreamError::PolicyDurationBelowFloor,
        StreamError::PolicyDurationAboveCeiling,
        StreamError::PolicyDepositBelowFloor,
        StreamError::BatchEmpty,
        StreamError::BatchTooLarge,
        StreamError::SponsorNotFound,
        StreamError::SponsorInactive,
        StreamError::BeneficiaryNotEnrolled,
        StreamError::ClaimExpired,
        StreamError::ClaimNonceReplay,
        StreamError::SponsorBudgetExceeded,
        StreamError::BeneficiaryBudgetExceeded,
        StreamError::InvalidOracleThreshold,
        StreamError::OraclePublishersNotConfigured,
        StreamError::OracleReportTooOld,
        StreamError::NotOraclePublisher,
        StreamError::NoPendingOwnership,
        StreamError::OwnershipProposalExpired,
        StreamError::InvalidTransferTarget,
        StreamError::OwnershipProposalPending,
        StreamError::SchemaVersionUnsupported,
        StreamError::SenderTransferNotFound,
        StreamError::SenderTransferExpired,
        StreamError::SenderTransferNotExpired,
        StreamError::SenderTransferNonceMismatch,
        StreamError::SenderTransferPending,
        StreamError::SenderTransferNotConsenter,
        StreamError::RefundTargetNotSender,
        StreamError::RefundTargetUnchanged,
        StreamError::WithdrawBatchEmpty,
        StreamError::WithdrawBatchTooLarge,
        StreamError::WithdrawDuplicateStreamIds,
    ];

    /// The stable numeric value carried across the contract boundary
    /// (`Error(Contract, #code)`), i.e. the enum discriminant.
    pub const fn code(self) -> u32 {
        self as u32
    }
}
