"use client";
import Link from "next/link";
import { useParams } from "next/navigation";
import { useCallback, useEffect, useState } from "react";
import { getAddress, isConnected } from "@stellar/freighter-api";
import {
  cancelStream,
  describeError,
  enrollBeneficiary,
  isNativeXlm,
  readAvailable,
  readBeneficiaryKey,
  readRefundTarget,
  readSponsor,
  readStream,
  RELAY_URL,
  SPONSOR_ADDRESS,
  STREAMING_CONTRACT_ID,
  withdrawFromStream,
  withdrawMaxFromStream,
  fetchAssetMeta,
  type AssetMeta,
  type SponsorView,
  type StreamView,
} from "@/lib/stellar";
import { useClaimScheduler } from "@/lib/use-claim-scheduler";
import ClaimSchedulerSettings from "@/components/ClaimSchedulerSettings";
import { computeAvailable, StreamSnapshot } from "@/lib/accrual";
import {
  consentPublicKeyHex,
  DEFAULT_INTENT_TTL_SECONDS,
  intentDeadline,
  loadOrCreateConsentKey,
  nextNonce,
  requestSponsoredClaim,
  signClaim,
  type ClaimIntent,
} from "@/lib/sponsor";
import { useToast } from "@/components/toast";
import { validateWithdrawal } from "@/lib/withdrawal";
import { formatAmount } from "@/lib/format";
import DiscoveryConsentModal from "@/components/DiscoveryConsentModal";

export default function StreamDetail() {
  const toast = useToast();
  const params = useParams<{ id: string }>();
  const id = params?.id ?? "";

  const [stream, setStream] = useState<StreamView | null>(null);
  const [assetMeta, setAssetMeta] = useState<AssetMeta | null>(null);
  const [available, setAvailable] = useState<bigint | null>(null);
  const [displayAvailable, setDisplayAvailable] = useState<bigint | null>(null);
  const [snapshot, setSnapshot] = useState<StreamSnapshot | null>(null);
  const [amount, setAmount] = useState<string>("");
  const [isAmountDirty, setIsAmountDirty] = useState(false);
  const [address, setAddress] = useState<string | null>(null);
  const [status, setStatus] = useState<string>("");
  const [error, setError] = useState<string>("");
  const [sponsor, setSponsor] = useState<SponsorView | null>(null);
  const [consentPub, setConsentPub] = useState<string>("");
  const [enrolledKey, setEnrolledKey] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState<string>("");
  const [refreshing, setRefreshing] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [discoveryModalOpen, setDiscoveryModalOpen] = useState<boolean>(false);
  const [refundTarget, setRefundTarget] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (!id) return;
    setRefreshing(true);
    try {
      const [s, a] = await Promise.all([readStream(id), readAvailable(id)]);
      setStream(s);
      setAvailable(a);
      // Separate, non-fatal read: the routing only annotates the cancel card.
      setRefundTarget(await readRefundTarget(id));
      fetchAssetMeta(s.token).then((meta) => setAssetMeta(meta));
      setSnapshot({
        deposit: BigInt(s.deposit),
        startTime: s.startTime,
        endTime: s.endTime,
        withdrawn: BigInt(s.withdrawn),
        status: s.status,
      });

      // Auto-prefill the input with accrued balance on initial load if not modified by user
      if (!isAmountDirty && a !== null && a > 0n) {
        setAmount(a.toString());
      }
      setLoadError("");
    } catch (e) {
      const msg = describeError(e);
      setLoadError(msg);
      toast.error(msg);
    } finally {
      setRefreshing(false);
    }
  }, [id, isAmountDirty, toast]);

  useEffect(() => {
    void (async () => {
      await reload();
    })();
    const t = setInterval(() => void reload(), 15_000); // live accrual poll
    return () => clearInterval(t);
  }, [reload]);

  // Cosmetic linear interpolation: updates the displayed counter every 1s for visual smoothness.
  // CRITICAL NOTE: displayAvailable and this 1-second interpolator are purely cosmetic.
  // Submission decisions and client-side guards are strictly validated against the canonical
  // on-chain `available` value. A client-side guard can only ever prevent, never guarantee,
  // a successful on-chain claim (e.g. near end_time or right after a concurrent cancel).
  useEffect(() => {
    if (!snapshot) return;
    const timer = setInterval(() => {
      const now = BigInt(Math.floor(Date.now() / 1000));
      const next = computeAvailable(
        snapshot.deposit,
        snapshot.startTime,
        snapshot.endTime,
        snapshot.withdrawn,
        snapshot.status,
        now
      );
      setDisplayAvailable(next);
    }, 1000);
    return () => clearInterval(timer);
  }, [snapshot]);

  useEffect(() => {
    void (async () => {
      try {
        if (await isConnected()) {
          const { address } = await getAddress();
          setAddress(address);
        }
      } catch {
        // Detail view stays read-only without a wallet.
      }
    })();
  }, []);

  const loadSponsorship = useCallback(async (beneficiary: string) => {
    if (!SPONSOR_ADDRESS) return;
    try {
      const [cfg, enrolled] = await Promise.all([
        readSponsor(SPONSOR_ADDRESS),
        readBeneficiaryKey(beneficiary),
      ]);
      setSponsor(cfg);
      setEnrolledKey(enrolled);
    } catch {
      // Sponsorship is an optional path; the self-paid claim still works.
      setSponsor(null);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      // The consent keypair lives in localStorage — an external system,
      // loaded once on mount like the wallet address above.
      setConsentPub(consentPublicKeyHex(loadOrCreateConsentKey()));
    })();
  }, []);

  useEffect(() => {
    if (!address) return;
    void (async () => {
      await loadSponsorship(address);
    })();
  }, [address, loadSponsorship]);

  // Fee-aware claim scheduling is scoped to the connected wallet; the hook is
  // inert until an address is available.
  const scheduler = useClaimScheduler(address);

  if (loadError && !stream) {
    return (
      <main className="wrap">
        <h1>Stream #{id}</h1>
        <p className="err">{loadError}</p>
        <Link href="/">← All streams</Link>
      </main>
    );
  }
  if (!stream) {
    return (
      <main className="wrap">
        <h1>Stream #{id}</h1>
        <p className="muted">Loading…</p>
      </main>
    );
  }

  const deposit = BigInt(stream.deposit);
  const withdrawn = BigInt(stream.withdrawn);
  const progress =
    deposit > 0n ? Math.min(100, Number((withdrawn * 100n) / deposit)) : 0;
  const isRecipient = address !== null && address === stream.recipient;
  const isSender = address !== null && address === stream.sender;
  const consentEnrolled =
    consentPub !== "" && enrolledKey !== null && enrolledKey === consentPub;
  const sponsorFundsThisToken =
    sponsor !== null && sponsor.active && sponsor.token === stream.token;
  const sponsorBudgetLeft = sponsor !== null && BigInt(sponsor.remaining) > 0n;

  // Validation is executed strictly against on-chain available balance, NOT cosmetic interpolator
  const validation = validateWithdrawal(amount, available);

  async function doWithdraw(max: boolean) {
    if (!address) return;
    setSubmitting(true);
    try {
      if (max) {
        const paid = await withdrawMaxFromStream(address, stream!.id);
        const paidStr = assetMeta
          ? `${formatAmount(paid, assetMeta.decimals)} ${assetMeta.code}`
          : `${paid} units`;
        toast.success(`Claimed ${paidStr}.`);
        setAmount("");
        setIsAmountDirty(false);
      } else {
        if (!validation.valid) {
          const availStr =
            available !== null && assetMeta
              ? `${formatAmount(available, assetMeta.decimals)} ${assetMeta.code}`
              : `${available?.toString()} units`;
          toast.error(
            validation.error ||
              `Amount exceeds the accrued balance (${availStr}). Try “Withdraw max”.`
          );
          return;
        }
        await withdrawFromStream(address, stream!.id, validation.amount.toString());
        const withdrewStr = assetMeta
          ? `${formatAmount(validation.amount, assetMeta.decimals)} ${assetMeta.code}`
          : `${validation.amount} units`;
        toast.success(`Successfully withdrew ${withdrewStr}.`);
        setAmount("");
        setIsAmountDirty(false);
      }
      await reload();
    } catch (e) {
      toast.error(describeError(e));
    } finally {
      setSubmitting(false);
    }
  }

  async function doEnableSponsorship() {
    if (!address || consentPub === "") return;
    setError("");
    setStatus("");
    setBusy(true);
    try {
      await enrollBeneficiary(address, consentPub);
      setStatus("Consent key enrolled — you can now claim without paying gas.");
      await loadSponsorship(address);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  /**
   * Sponsored claim: sign the intent locally, hand it to the sponsor's
   * relayer, and let the operator pay the transaction fee.
   */
  async function doClaimForFree() {
    if (!address || !sponsor) return;
    const claimAmount =
      amount !== "" ? BigInt(amount) : (displayAvailable ?? available ?? 0n);
    if (claimAmount <= 0n) {
      setError("Nothing has accrued yet.");
      return;
    }
    setError("");
    setStatus("");
    setBusy(true);
    try {
      const intent: ClaimIntent = {
        contractId: STREAMING_CONTRACT_ID,
        sponsor: SPONSOR_ADDRESS,
        beneficiary: address,
        token: stream!.token,
        streamId: stream!.id,
        amount: claimAmount.toString(),
        nonce: nextNonce(`${SPONSOR_ADDRESS}:${address}`).toString(),
        deadline: intentDeadline(),
      };
      const beneficiarySig = await signClaim(
        loadOrCreateConsentKey(),
        intent
      );
      const res = await requestSponsoredClaim(RELAY_URL, {
        intent,
        beneficiarySig,
      });
      setStatus(
        `Claimed ${claimAmount.toString()} units — gas paid by the sponsor. tx ${res.txHash}`
      );
      setAmount("");
      await Promise.all([reload(), loadSponsorship(address)]);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  async function doCancel() {
    if (!address) return;
    setSubmitting(true);
    try {
      await cancelStream(address, stream!.id);
      toast.success("Stream cancelled.");
      await reload();
    } catch (e) {
      toast.error(describeError(e));
    } finally {
      setSubmitting(false);
    }
  }

  const handlePrefillMax = () => {
    if (available !== null && available > 0n) {
      setAmount(available.toString());
      setIsAmountDirty(true);
    }
  };

  return (
    <main className="wrap">
      <p className="kicker mono">STREAM #{stream.id}</p>
      <h1>
        {stream.status === "Active" ? "⚡ Streaming" : `⏹ ${stream.status}`}
      </h1>
      {error && (
        <p className="err" role="alert">
          {error}
        </p>
      )}
      {status && (
        <p className="muted" role="status">
          {status}
        </p>
      )}

      <section className="card">
        <div className="row" style={{ alignItems: "center" }}>
          <progress value={progress} max={100} style={{ flex: 1 }} />
          <span className="mono">{progress}% withdrawn</span>
        </div>
        <table>
          <tbody>
            <tr>
              <th>Sender</th>
              <td className="mono">{stream.sender}</td>
            </tr>
            <tr>
              <th>Recipient</th>
              <td className="mono">{stream.recipient}</td>
            </tr>
            <tr>
              <th>Token</th>
              <td className="mono" style={{ wordBreak: "break-all" }}>
                {isNativeXlm(stream.token)
                  ? `Native XLM — ${stream.token}`
                  : assetMeta
                  ? `${assetMeta.code} — ${stream.token}`
                  : stream.token}
              </td>
            </tr>
            <tr>
              <th>Deposit</th>
              <td className="mono" title={`${stream.deposit} base units`}>
                {assetMeta
                  ? `${formatAmount(deposit, assetMeta.decimals)} ${assetMeta.code}`
                  : stream.deposit}
              </td>
            </tr>
            <tr>
              <th>Withdrawn</th>
              <td className="mono" title={`${stream.withdrawn} base units`}>
                {assetMeta
                  ? `${formatAmount(withdrawn, assetMeta.decimals)} ${assetMeta.code}`
                  : stream.withdrawn}
              </td>
            </tr>
            <tr>
              <th>Available now</th>
              <td className="mono">
                {(() => {
                  const availVal =
                    snapshot?.status === "Active"
                      ? (displayAvailable ?? available)
                      : available;
                  if (availVal === null) return "…";
                  if (!assetMeta) return availVal.toString();
                  return `${formatAmount(availVal, assetMeta.decimals)} ${assetMeta.code}`;
                })()}
                {stream.status === "Active" && (
                  <button
                    onClick={() => void reload()}
                    disabled={refreshing}
                    style={{
                      marginLeft: "0.75rem",
                      padding: "0.2rem 0.5rem",
                      fontSize: "0.8rem",
                    }}
                    title="Refresh on-chain accrual balance"
                  >
                    {refreshing ? "↻ Fetching…" : "↻ Refresh"}
                  </button>
                )}
              </td>
            </tr>
            <tr>
              <th>Window</th>
              <td className="mono">
                {new Date(stream.startTime * 1000).toISOString()} →{" "}
                {new Date(stream.endTime * 1000).toISOString()}
              </td>
            </tr>
            <tr>
              <th>Cancelable</th>
              <td>{String(stream.cancelable)}</td>
            </tr>
            <tr>
              <th>Status</th>
              <td>{stream.status}</td>
            </tr>
          </tbody>
        </table>
      </section>

      {isRecipient && stream.status === "Active" && (
        <section className="card">
          <h2>Withdraw accrued funds</h2>
          <div className="row">
            <input
              type="number"
              min="0"
              placeholder="amount (defaults to available)"
              value={amount}
              onChange={(e) => {
                setAmount(e.target.value);
                setIsAmountDirty(true);
              }}
              disabled={submitting}
            />
            {amount && !isNaN(Number(amount)) && BigInt(amount || "0") > 0n && assetMeta && (
              <span
                className="mono muted"
                style={{ fontSize: "0.85rem", alignSelf: "center", whiteSpace: "nowrap" }}
              >
                ≈ {formatAmount(BigInt(amount), assetMeta.decimals)} {assetMeta.code}
              </span>
            )}
            {available !== null && available > 0n && (
              <button
                type="button"
                onClick={handlePrefillMax}
                disabled={submitting}
                title="Prefill input with the exact current on-chain available balance"
              >
                Prefill Max
              </button>
            )}
            <button
              onClick={() => void doWithdraw(false)}
              disabled={!validation.valid || submitting}
            >
              {submitting ? "Withdrawing…" : "Withdraw"}
            </button>
            <button
              onClick={() => void doWithdraw(true)}
              disabled={submitting || available === 0n}
              title="withdraw_max: atomic claim of the full accrued balance"
            >
              Withdraw max
            </button>
          </div>
          {!validation.valid && validation.error && (
            <p className="err" role="alert">
              {validation.error}
            </p>
          )}
        </section>
      )}

      {isRecipient && stream.status === "Active" && SPONSOR_ADDRESS !== "" && (
        <section className="card">
          <h2>Claim for free</h2>
          {sponsor === null && (
            <p className="muted">
              No sponsor is registered at{" "}
              <span className="mono">{SPONSOR_ADDRESS}</span>, so claims pay
              their own network fee.
            </p>
          )}
          {sponsor !== null && !sponsorFundsThisToken && (
            <p className="muted">
              <span className="mono">{SPONSOR_ADDRESS}</span> does not fund this
              stream&apos;s token (or is paused), so claims pay their own network fee.
            </p>
          )}
          {sponsor !== null && sponsorFundsThisToken && !sponsorBudgetLeft && (
            <p className="muted">
              This sponsor has used its budget for the current window (
              {sponsor.spend} of {sponsor.perPeriodLimit} units). It resets every{" "}
              {sponsor.windowSeconds}s; until then claims pay their own fee.
            </p>
          )}
          {sponsorFundsThisToken && sponsorBudgetLeft && (
            <>
              <p className="muted">
                <strong>You pay nothing.</strong>{" "}
                <span className="mono">{SPONSOR_ADDRESS}</span> is the sponsor:
                its operator builds and submits the transaction and pays the
                Soroban network fee out of this budget —{" "}
                {sponsor?.remaining} of {sponsor?.perPeriodLimit} units still
                available this window. You sign a single claim intent with your
                consent key, bound to this stream, amount, a one-time nonce and
                a {DEFAULT_INTENT_TTL_SECONDS}s expiry, so nobody else can spend
                it or reuse it.
              </p>
              {!consentEnrolled && (
                <>
                  <button
                    onClick={() => void doEnableSponsorship()}
                    disabled={busy}
                  >
                    Enable sponsored claims
                  </button>
                  <p className="muted">
                    One-time setup: registers your consent key on-chain. This
                    setup call is signed and paid by you; the claims it unlocks
                    are not.
                  </p>
                </>
              )}
              {consentEnrolled && RELAY_URL === "" && (
                <p className="muted">
                  Consent key is enrolled, but no sponsor relay is configured
                  (NEXT_PUBLIC_SPONSOR_RELAY_URL), so there is no operator to
                  submit the claim. Use the self-paid withdrawal above.
                </p>
              )}
              {consentEnrolled && RELAY_URL !== "" && (
                <button
                  onClick={() => void doClaimForFree()}
                  disabled={busy}
                  title="sponsored_withdraw: the sponsor's operator pays the fee"
                >
                  {busy ? "Relaying…" : "Claim for free"}
                </button>
              )}
            </>
          )}
        </section>
      )}
      {isRecipient && stream.status === "Active" && (
        <ClaimSchedulerSettings stream={stream} scheduler={scheduler} />
      )}

      {(isSender || (isRecipient && stream.cancelable)) &&
        stream.status === "Active" && (
          <section className="card">
            <h2>Cancel stream</h2>
            <p className="muted">
              Pays the recipient everything accrued and refunds the sender the
              rest, atomically. This is irreversible.
            </p>
            {refundTarget !== null && refundTarget !== stream.sender && (
              <p className="warn" role="note">
                <span aria-hidden="true">⚠</span>
                <span>
                  Warning: the refund of the remainder is routed to{" "}
                  <code>{refundTarget}</code>, not to the current sender. The
                  sender role and who can cancel are unaffected.
                </span>
              </p>
            )}
            <button onClick={() => void doCancel()} disabled={submitting}>
              {submitting ? "Cancelling…" : "Cancel stream"}
            </button>
          </section>
        )}

      {(isSender || isRecipient) && (
        <section className="card">
          <h2>Anonymized Discovery Listing</h2>
          <p className="muted">
            Opt into publishing an anonymized, verifiable listing for this stream to the public directory.
            Requires cryptographic consent from both parties. No addresses are shared by default.
          </p>
          <button
            type="button"
            onClick={() => setDiscoveryModalOpen(true)}
          >
            Manage Discovery Listing
          </button>
        </section>
      )}

      {discoveryModalOpen && address && stream && (
        <DiscoveryConsentModal
          stream={stream}
          currentUserAddress={address}
          isOpen={discoveryModalOpen}
          onClose={() => setDiscoveryModalOpen(false)}
        />
      )}

      <section className="card">
        <h2>Shareable Payment Link</h2>
        <p className="muted">
          Generate a deterministic, privacy-blinded claim URL for this stream. The recipient can claim without logging in first.
        </p>
        <button
          type="button"
          onClick={async () => {
            try {
              const { encodePaymentLinkRef } = await import("@/lib/payment-link");
              const ref = await encodePaymentLinkRef({
                streamId: stream.id,
                recipient: stream.recipient,
              });
              const url = `${window.location.origin}/pay/${ref}`;
              await navigator.clipboard.writeText(url);
              toast.success("Payment link copied to clipboard!");
            } catch (err: unknown) {
              toast.error(`Failed to copy payment link: ${String(err)}`);
            }
          }}
        >
          Copy Shareable Payment Link
        </button>
      </section>

      <p style={{ display: "flex", gap: "16px" }}>
        <Link href="/">← All streams</Link>
        <Link href="/listings" style={{ color: "var(--accent2)" }}>
          Browse Discovery Directory →
        </Link>
      </p>
    </main>
  );
}