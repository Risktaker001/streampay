"use client";

import Link from "next/link";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import {
  readAvailableAction,
  readRefundTargetAction,
  readStreamAction,
} from "@/app/actions";
import { describeError } from "@/lib/errors";
import { computeAvailable, type StreamSnapshot } from "@/lib/accrual";
import type { StreamView } from "@/lib/stream-types";
import { formatAmount } from "@/lib/format";
import type { AssetMeta } from "@/lib/stellar";

function fmtUnits(value: bigint, meta?: AssetMeta | null): string {
  return meta ? `${formatAmount(value, meta.decimals)} ${meta.code}` : value.toString();
}

function snapshotFromStream(stream: StreamView): StreamSnapshot {
  return {
    deposit: BigInt(stream.deposit),
    startTime: stream.startTime,
    endTime: stream.endTime,
    withdrawn: BigInt(stream.withdrawn),
    status: stream.status,
  };
}

export default function StreamDetailClient({
  initialStream,
}: {
  initialStream: StreamView;
}) {
  const id = initialStream.id;
  const [stream, setStream] = useState<StreamView>(initialStream);
  const [assetMeta, setAssetMeta] = useState<AssetMeta | null>(null);
  const [available, setAvailable] = useState<bigint | null>(null);
  const [displayAvailable, setDisplayAvailable] = useState<bigint | null>(null);
  const [snapshot, setSnapshot] = useState<StreamSnapshot>(() =>
    snapshotFromStream(initialStream)
  );
  const [amount, setAmount] = useState("");
  const [address, setAddress] = useState<string | null>(null);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [refundTarget, setRefundTarget] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    import("@/lib/stellar").then(({ fetchAssetMeta }) => {
      fetchAssetMeta(stream.token).then((meta) => {
        if (active) setAssetMeta(meta);
      });
    });
    return () => {
      active = false;
    };
  }, [stream.token]);

  // -------------------------------------------------------------------------
  // Live-region throttle: announce the accrued balance to screen readers at
  // most once every 5 seconds (WCAG 1.4.13 / live-region best practice).
  // A polite aria-live region is used so announcements do not interrupt speech.
  // -------------------------------------------------------------------------
  const announceRef = useRef<HTMLSpanElement | null>(null);
  const lastAnnouncedRef = useRef<bigint | null>(null);
  const lastAnnounceTimeRef = useRef<number>(0);
  const ANNOUNCE_THROTTLE_MS = 5_000;

  // Unique IDs for ARIA label associations
  const withdrawHeadingId = useId();
  const withdrawAmountId = useId();
  const withdrawHintId = useId();
  const withdrawMaxHintId = useId();
  const cancelHeadingId = useId();
  const cancelWarningId = useId();
  const refundWarningId = useId();
  const tableCapId = useId();
  const progressId = useId();

  const applyStream = useCallback((next: StreamView) => {
    setStream(next);
    setSnapshot(snapshotFromStream(next));
  }, []);

  const reload = useCallback(async () => {
    try {
      const [nextStream, nextAvailable, nextRefundTarget] = await Promise.all([
        readStreamAction(id),
        readAvailableAction(id),
        readRefundTargetAction(id),
      ]);
      applyStream(nextStream);
      setAvailable(BigInt(nextAvailable));
      setRefundTarget(nextRefundTarget);
      setError("");
    } catch (e) {
      setError(describeError(e));
    }
  }, [applyStream, id]);

  useEffect(() => {
    void readAvailableAction(id)
      .then((value) => setAvailable(BigInt(value)))
      .catch((e: unknown) => setError(describeError(e)));
    void readRefundTargetAction(id)
      .then(setRefundTarget)
      .catch(() => setRefundTarget(null));
    const timer = setInterval(() => void reload(), 15_000);
    return () => clearInterval(timer);
  }, [id, reload]);

  // 1-second local interpolation of accrued balance
  useEffect(() => {
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

      // Throttled screen-reader announcement (polite, every 5 s)
      if (
        announceRef.current &&
        snapshot.status === "Active" &&
        (lastAnnouncedRef.current === null || lastAnnouncedRef.current !== next) &&
        Date.now() - lastAnnounceTimeRef.current >= ANNOUNCE_THROTTLE_MS
      ) {
        lastAnnouncedRef.current = next;
        lastAnnounceTimeRef.current = Date.now();
        // Clear first so the mutation fires even if text is identical
        announceRef.current.textContent = "";
        // Use rAF to let the clear flush before setting the new text
        requestAnimationFrame(() => {
          if (announceRef.current) {
            announceRef.current.textContent = `Available balance: ${next.toLocaleString()} units`;
          }
        });
      }
    }, 1000);
    return () => clearInterval(timer);
  }, [snapshot]);

  useEffect(() => {
    void (async () => {
      try {
        const { getAddress, isConnected } = await import("@stellar/freighter-api");
        if (await isConnected()) {
          const { address: walletAddress } = await getAddress();
          setAddress(walletAddress);
        }
      } catch {
        // Detail view stays read-only without a wallet.
      }
    })();
  }, []);

  const deposit = BigInt(stream.deposit);
  const withdrawn = BigInt(stream.withdrawn);
  const progress =
    deposit > 0n ? Math.min(100, Number((withdrawn * 100n) / deposit)) : 0;
  const isRecipient = address !== null && address === stream.recipient;
  const isSender = address !== null && address === stream.sender;
  const overWithdrawal =
    available !== null && amount !== "" && BigInt(amount || "0") > available;

  // Current displayed value for the live counter
  const displayRaw =
    snapshot?.status === "Active"
      ? (displayAvailable ?? available)
      : available;
  const displayValue =
    displayRaw === null
      ? "…"
      : assetMeta
      ? `${formatAmount(displayRaw, assetMeta.decimals)} ${assetMeta.code}`
      : displayRaw.toString();

  async function doWithdraw(max: boolean) {
    if (!address) return;
    setError("");
    setStatus("");
    try {
      if (max) {
        const { withdrawMaxFromStream } = await import("@/lib/stellar");
        const paid = await withdrawMaxFromStream(address, stream.id);
        setStatus(`Claimed ${fmtUnits(paid, assetMeta)}.`);
      } else {
        if (overWithdrawal) {
          setError(
            `Amount exceeds the accrued balance (${displayValue}). Try "Withdraw max".`
          );
          return;
        }
        const { withdrawFromStream } = await import("@/lib/stellar");
        await withdrawFromStream(address, stream.id, amount || "0");
        setStatus("Withdrawal sent.");
      }
      setAmount("");
      await reload();
    } catch (e) {
      setError(describeError(e));
    }
  }

  async function doCancel() {
    if (!address) return;
    setError("");
    try {
      const { cancelStream } = await import("@/lib/stellar");
      await cancelStream(address, stream.id);
      setStatus("Stream cancelled.");
      await reload();
    } catch (e) {
      setError(describeError(e));
    }
  }

  const isStreaming = stream.status === "Active";
  // Issue #204: the contract's `refund_target` view answers with the sender for
  // an unrouted stream, so "differs from sender" is the only signal that a
  // routing exists. A failed read (null) says nothing and shows nothing.
  const refundRouted =
    refundTarget !== null && refundTarget !== stream.sender;

  return (
    <main className="wrap">
      <p className="kicker mono">STREAM #{stream.id}</p>

      <h1>
        {/* Emoji decorative; screen reader gets the text node next to it */}
        <span aria-hidden="true">{isStreaming ? "⚡ " : "⏹ "}</span>
        {isStreaming ? "Streaming" : stream.status}
      </h1>

      {/* Status / error announcements — role="status" for non-urgent, "alert" for errors */}
      {status && (
        <p className="ok" role="status" aria-live="polite">
          {status}
        </p>
      )}
      {error && (
        <p className="err" role="alert" aria-live="assertive">
          {error}
        </p>
      )}

      {/* Hidden polite live-region for throttled accrual announcements */}
      <span
        ref={announceRef}
        className="sr-only"
        aria-live="polite"
        aria-atomic="true"
      />

      <section className="card" aria-labelledby={tableCapId}>
        <h2 id={tableCapId} className="sr-only">
          Stream details for stream #{stream.id}
        </h2>

        {/* Progress bar — role=progressbar carries the semantic value */}
        <div className="row" style={{ alignItems: "center" }}>
          <div
            id={progressId}
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progress}
            aria-label={`Withdrawal progress: ${progress}% withdrawn`}
            style={{
              flex: 1,
              height: 8,
              background: "var(--line)",
              borderRadius: 999,
              overflow: "hidden",
            }}
          >
            <div
              style={{
                width: `${progress}%`,
                height: "100%",
                background: "linear-gradient(90deg, var(--accent), var(--accent2))",
                borderRadius: 999,
                transition: "width 0.3s ease",
              }}
            />
          </div>
          <span className="mono" aria-hidden="true">{progress}% withdrawn</span>
        </div>

        <table aria-labelledby={tableCapId}>
          <tbody>
            <tr>
              <th scope="row">Sender</th>
              <td className="mono">{stream.sender}</td>
            </tr>
            <tr>
              <th scope="row">Recipient</th>
              <td className="mono">{stream.recipient}</td>
            </tr>
            <tr>
              <th scope="row">Token</th>
              <td className="mono">
                {assetMeta ? `${assetMeta.code} — ${stream.token}` : stream.token}
              </td>
            </tr>
            <tr>
              <th scope="row">Deposit</th>
              <td className="mono" title={`${stream.deposit} base units`}>
                {assetMeta
                  ? `${formatAmount(BigInt(stream.deposit), assetMeta.decimals)} ${assetMeta.code}`
                  : stream.deposit}
              </td>
            </tr>
            <tr>
              <th scope="row">Withdrawn</th>
              <td className="mono" title={`${stream.withdrawn} base units`}>
                {assetMeta
                  ? `${formatAmount(BigInt(stream.withdrawn), assetMeta.decimals)} ${assetMeta.code}`
                  : stream.withdrawn}
              </td>
            </tr>
            <tr>
              <th scope="row">Available now</th>
              <td>
                {/* Visible numeric display — updated every second by JS */}
                <span
                  className="mono"
                  aria-label={`Available balance: ${displayValue}`}
                >
                  {displayValue}
                </span>
              </td>
            </tr>
            <tr>
              <th scope="row">Window</th>
              <td className="mono">
                {new Date(stream.startTime * 1000).toISOString()} →{" "}
                {new Date(stream.endTime * 1000).toISOString()}
              </td>
            </tr>
            <tr>
              <th scope="row">Cancelable</th>
              <td>{String(stream.cancelable)}</td>
            </tr>
            <tr>
              <th scope="row">Status</th>
              <td>{stream.status}</td>
            </tr>
          </tbody>
        </table>
      </section>

      {isRecipient && isStreaming && (
        <section className="card" aria-labelledby={withdrawHeadingId}>
          <h2 id={withdrawHeadingId}>Withdraw accrued funds</h2>
          <div
            className="row"
            role="group"
            aria-labelledby={withdrawHeadingId}
          >
            {/* Explicit label + aria-describedby for the number input */}
            <label htmlFor={withdrawAmountId} className="sr-only">
              Withdrawal amount in smallest token units
            </label>
            <input
              id={withdrawAmountId}
              type="number"
              min="0"
              placeholder="amount (defaults to available)"
              value={amount}
              onChange={(event) => setAmount(event.target.value)}
              aria-describedby={withdrawHintId}
              aria-invalid={overWithdrawal || undefined}
            />
            <span id={withdrawHintId} className="sr-only">
              Leave blank to use &ldquo;Withdraw max&rdquo;. Current available
              balance: {available?.toString() ?? "loading"} units.
            </span>

            <button type="button" onClick={() => void doWithdraw(false)}>
              Withdraw
            </button>
            <button
              type="button"
              onClick={() => void doWithdraw(true)}
              aria-describedby={withdrawMaxHintId}
            >
              Withdraw max
            </button>
            <span id={withdrawMaxHintId} className="sr-only">
              Atomically claims the full accrued balance in one transaction
              (withdraw_max).
            </span>
          </div>

          {overWithdrawal && (
            <p className="err" role="alert">
              That exceeds the currently accrued {available?.toString()} units —
              the contract would reject it with AmountExceedsAvailable.
            </p>
          )}
        </section>
      )}

      {(isSender || (isRecipient && stream.cancelable)) && isStreaming && (
        <section className="card" aria-labelledby={cancelHeadingId}>
          <h2 id={cancelHeadingId}>Cancel stream</h2>
          <p className="muted">
            Pays the recipient everything accrued and refunds the sender the
            rest, atomically. This is irreversible.
          </p>
          {refundRouted && (
            <p id={refundWarningId} className="warn" role="note">
              <span aria-hidden="true">⚠</span>
              <span>
                Warning: the refund of the remainder is routed to{" "}
                <code>{refundTarget}</code>, not to the current sender. The
                sender role and who can cancel are unaffected.
              </span>
            </p>
          )}
          <button
            type="button"
            onClick={() => void doCancel()}
            aria-describedby={
              refundRouted
                ? `${cancelWarningId} ${refundWarningId}`
                : cancelWarningId
            }
          >
            Cancel stream
          </button>
          <p id={cancelWarningId} className="sr-only">
            Warning: cancellation is permanent and cannot be undone.
          </p>
        </section>
      )}

      <p>
        <Link href="/">← All streams</Link>
      </p>
    </main>
  );
}
