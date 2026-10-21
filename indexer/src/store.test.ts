import assert from "node:assert/strict";
import test from "node:test";
import { StreamStore } from "./store.js";
import type { DecodedEvent } from "./types.js";

test("event upserts are idempotent and project stream lifecycle", () => {
  const store = new StreamStore(":memory:");
  const base: DecodedEvent = { id: "event-1", pagingToken: "1", ledger: 10, ledgerClosedAt: "2026-01-01T00:00:00Z", contractId: "C", name: "stream_created", streamId: "7", data: { sender: "Gsender", recipient: "Grecipient", token: "Ctoken", deposit: 1000n, start_time: 1n, end_time: 10n, cancelable: true } };
  store.ingestPage([base], "1", 10);
  store.ingestPage([base], "1", 10);
  assert.equal(store.getState("cursor"), "1");
  assert.equal(store.getState("lastLedger"), "10");
  assert.equal(store.getStream("7")?.deposit, "1000");
  assert.equal(store.listAddress("Grecipient").length, 1);
  store.ingest({ ...base, id: "event-2", pagingToken: "2", ledger: 11, name: "withdrawn", data: { recipient: "Grecipient", amount: 100n, withdrawn_total: 100n } });
  assert.equal(store.getStream("7")?.withdrawn, "100");
  store.ingest({ ...base, id: "event-3", pagingToken: "3", ledger: 12, name: "stream_topped_up", data: { new_deposit: 1500n } });
  assert.equal(store.getStream("7")?.deposit, "1500");
  store.ingest({ ...base, id: "event-4", pagingToken: "4", ledger: 13, name: "stream_cancelled", data: { sender_amount: 1n, recipient_amount: 2n } });
  assert.equal(store.getStream("7")?.status, "cancelled");
  store.close();
});

// Refund routing is a projection column, not a lifecycle state: the routing must
// land, must survive every later event that does not mention it, and must not be
// rewound by a late delivery (issue #204).
test("refund routing projects onto the stream row and survives later events", () => {
  const store = new StreamStore(":memory:");
  const created: DecodedEvent = { id: "e-1", pagingToken: "1", ledger: 10, ledgerClosedAt: "2026-01-01T00:00:00Z", contractId: "C", name: "stream_created", streamId: "7", data: { sender: "Gsender", recipient: "Grecipient", token: "Ctoken", deposit: 1000n, start_time: 1n, end_time: 10n, cancelable: true } };
  store.ingestPage([created], "1", 10);
  assert.equal(store.getStream("7")?.refund_target ?? null, null);

  store.ingest({ ...created, id: "e-2", pagingToken: "2", ledger: 11, name: "stream_refund_target_changed", data: { previous_target: "Gsender", new_target: "Grefund", at_creation: false } });
  assert.equal(store.getStream("7")?.refund_target, "Grefund");

  store.ingest({ ...created, id: "e-3", pagingToken: "3", ledger: 12, name: "withdrawn", data: { recipient: "Grecipient", amount: 100n, withdrawn_total: 100n } });
  assert.equal(store.getStream("7")?.withdrawn, "100");
  assert.equal(store.getStream("7")?.refund_target, "Grefund");

  store.ingest({ ...created, id: "e-4", pagingToken: "4", ledger: 13, name: "stream_refund_target_changed", data: { previous_target: "Grefund", new_target: "Gother", at_creation: false } });
  assert.equal(store.getStream("7")?.refund_target, "Gother");

  // A stale re-delivery of the older routing must not rewind the row.
  store.ingest({ ...created, id: "e-2", pagingToken: "2", ledger: 11, name: "stream_refund_target_changed", data: { previous_target: "Gsender", new_target: "Grefund", at_creation: false } });
  assert.equal(store.getStream("7")?.refund_target, "Gother");
  // The routing is not a discoverable role, so it never enters the address index.
  assert.equal(store.listAddress("Grefund").length, 0);
  store.close();
});

// A batch claim (#205) publishes one summary across every stream it settled,
// alongside the per-stream `withdrawn` events that stay the authoritative
// projection source: the summary names no single stream, so it is journalled and
// otherwise ignored — it must never create a row or rewind one.
test("a batch withdraw summary is journalled without touching any stream row", () => {
  const store = new StreamStore(":memory:");
  const created: DecodedEvent = { id: "e-1", pagingToken: "1", ledger: 10, ledgerClosedAt: "2026-01-01T00:00:00Z", contractId: "C", name: "stream_created", streamId: "7", data: { sender: "Gsender", recipient: "Grecipient", token: "Ctoken", deposit: 1000n, start_time: 1n, end_time: 10n, cancelable: true } };
  store.ingestPage([created], "1", 10);
  store.ingest({ ...created, id: "e-2", pagingToken: "2", ledger: 11, name: "withdrawn", data: { recipient: "Grecipient", amount: 100n, withdrawn_total: 100n } });

  store.ingest({
    ...created,
    id: "e-3",
    pagingToken: "3",
    ledger: 11,
    name: "batch_withdraw_summary",
    // Keyed by the recipient, exactly as the contract publishes it.
    streamId: "Grecipient",
    data: {
      recipient: "Grecipient",
      settled: [{ stream_id: 7n, amount: 100n }],
      total: 100n,
      requested: 2n,
      skipped: 1n,
    },
  });

  const journalled = store.db
    .prepare("SELECT name, payload FROM events WHERE id = ?")
    .get("e-3") as { name: string; payload: string } | undefined;
  assert.equal(journalled?.name, "batch_withdraw_summary");
  assert.match(journalled?.payload ?? "", /"requested":"2"/);

  // No row for the key it is published under, and the stream's own projection is
  // still the one the `withdrawn` event wrote.
  assert.equal(store.getStream("Grecipient"), undefined);
  assert.equal(store.getStream("7")?.withdrawn, "100");
  assert.deepEqual(
    store.getStreamHistory("7").map((event) => event.event_type),
    ["stream_created", "withdrawn"]
  );
  store.close();
});
