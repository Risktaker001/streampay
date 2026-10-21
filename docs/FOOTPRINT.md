# StreamPay Gas & Storage Footprint Analysis

This document records the measurement methodology, baseline benchmarks, optimization candidate evaluations, architectural tradeoffs, and before/after comparisons for **Issue #27: Gas and storage footprint pass**.

---

## 1. Context & Objectives

The goal of this pass is to measure and minimize the resource footprint (CPU instructions + persistent storage keys/bytes) of StreamPay's hot paths:
- `create_stream`
- `withdraw`
- `withdraw_max`
- `cancel_stream`
- Discovery pagination: `sender_streams` / `recipient_streams`

### Key Constraints
- **Zero Interface Drift**: Public storage keys (`DataKey::Stream(u64)`), event schemas (`StreamCreated`, `Withdrawn`, etc.), and numeric error codes (`StreamError`) must remain byte-for-byte wire compatible.
- **Invariant & Test Preservation**: All 195+ unit/property tests, Checks-Effects-Interactions (CEI) golden ordering traces (`test_orderings/*.txt`), and TTL sentinel cliff models (`infra/ttl-sentinel`) must remain green without modification.
- **Documented Tradeoffs**: Every evaluated candidate must justify whether it is accepted or rejected based on concrete Soroban metering data.

---

## 2. Simulation Harness

The footprint simulation harness is implemented in [`contracts/streaming-payments/examples/storage_benchmark.rs`](../contracts/streaming-payments/examples/storage_benchmark.rs) and exercised as a regression assertion in [`contracts/streaming-payments/src/test.rs`](../contracts/streaming-payments/src/test.rs) (`hot_path_footprint_harness`).

Run the benchmark executable directly:
```bash
cargo run -p streampay-contract --example storage_benchmark
```

The harness measures:
1. **CPU Instructions & Memory Bytes**: Measured via `env.cost_estimate().budget().cpu_instruction_cost()` and `memory_bytes_cost()`.
2. **Wire & Storage Footprint**: Exact XDR byte lengths of serialized keys, entries, and event bodies via `soroban_sdk::xdr::ToXdr`.
3. **Simulated Storage Layouts**: Comparative execution of monolithic `Stream` storage vs. split hot/cold entries under identical contract state.

---

## 3. Production Contract Footprint Baseline

### 3.1 Hot Path Lifecycle Operations

| Operation | Type | Storage Footprint | CPU Instructions | Memory (Bytes) | Event Bytes (XDR) |
|---|---|---|---:|---:|---:|
| `create_stream` | Write | 1 Stream + 2 Indexes | 541,598 | 210,434 | 396 B |
| `get_stream` | Read | 1 Stream Entry | 66,664 | 40,243 | 0 B |
| `available` | Read | 1 Stream Entry | 58,284 | 39,461 | 0 B |
| `withdraw` | RMW | 1 Stream Entry | 347,082 | 138,899 | 236 B |
| `top_up` | RMW | 1 Stream Entry | 447,290 | 167,390 | ~76 B |
| `cancel_stream` | RMW | 1 Stream Entry | 523,563 | 171,201 | 248 B |
| `withdraw_max` | RMW | 1 Stream Entry | 338,663 | 134,329 | 236 B |
| `sender_streams (5 ids)` | Read | 1 Index Entry | 52,369 | 25,006 | 0 B |

### 3.2 Wire & Storage Byte Footprint

| Component | Entity | Serialized Payload (Bytes) | Key Size (Bytes) | Total Ledger Footprint |
|---|---|---:|---:|---:|
| Storage Entry | `Stream` (monolithic) | 444 B | 40 B | 484 B |
| Storage Entry (Simulated) | `StreamConfigCold` | 332 B | 44 B | 376 B |
| Storage Entry (Simulated) | `StreamStateHot` | 120 B | 44 B | 164 B |
| Storage Entry | `SenderStreams` index key | N/A | 76 B | 76 B (key) |
| Event Payload | `StreamCreated` | 396 B | N/A | 396 B |
| Event Payload | `Withdrawn` | 236 B | N/A | 236 B |
| Event Payload | `StreamCancelled` | 248 B | N/A | 248 B |

---

## 4. Candidate Evaluations & Architectural Tradeoffs

### Candidate 1: Splitting Cold Fields from Hot Fields in `Stream` Struct
- **Mechanism Tested:**
  Split `Stream` into:
  - `StreamConfigCold`: Immutable configuration fields (`sender`, `recipient`, `token`, `deposit`, `start_time`, `end_time`, `cancelable`).
  - `StreamStateHot`: Mutable balance fields (`withdrawn`, `status`).
- **Benchmarked Results:**
  - Monolithic `Stream` Write: 60,956 CPU | 43,537 Memory
  - Split (Cold + Hot) Write: 98,480 CPU (**+61.6% CPU penalty**) | 60,375 Memory
  - Monolithic `Stream` Read: 48,319 CPU | 36,485 Memory
  - Split Read (Both entries): 73,200 CPU (**+51.5% CPU penalty**) | 47,926 Memory
  - Split Read (Hot-only): 41,123 CPU | 25,625 Memory
- **Tradeoff Analysis:**
  Every mutating path in the contract (`withdraw`, `withdraw_max`, `cancel_stream`, `top_up`) strictly requires BOTH configuration fields (token address for payout, deposit and window for accrual calculation, caller authorization checks) and state fields (`withdrawn`, `status`).
  Splitting fields into two separate storage keys incurs:
  1. A **+51.5% CPU read penalty** and a **+61.6% CPU write penalty**.
  2. Doubling of ledger keys touched per transaction, increasing base transaction fees.
  3. Total storage overhead actually increases: monolithic entry is 484 B total (444 B val + 40 B key), while two split entries total 540 B (376 B + 164 B) due to duplicate ledger key headers and prefix overhead.
  4. Breaking change: breaks `DataKey::Stream(u64)` and public contract interface.
- **Conclusion:** **Negative result (Rejected)**. The monolithic layout is strictly superior in CPU instructions, ledger fees, and storage footprint.

---

### Candidate 2: Trimming Event Fields
- **Mechanism Tested:**
  Trim fields considered redundant from event payloads (e.g., omitting `recipient` in `Withdrawn`, which could save ~36 bytes).
- **Tradeoff Analysis:**
  - Event payloads are small: `StreamCreated` is 396 B, `Withdrawn` is 236 B, and `StreamCancelled` is 248 B, well within Soroban transaction log limits.
  - Event schemas form an immutable public integration contract. Off-chain indexers ([`indexer/src/store.ts`](../indexer/src/store.ts)), CLI tools ([`tools/streampay-cli`](../tools/streampay-cli)), and client-parity test vectors ([`tests/client-parity/fixtures.json`](../tests/client-parity/fixtures.json)) strictly depend on these exact event fields.
  - Altering event schemas violates the explicit constraint: *"Must not change the public interface (storage keys, events, error codes)"*.
- **Conclusion:** **Rejected**. Retained full event payload fidelity.

---

### Candidate 3: TTL Extension Thresholds & Lazy Extension
- **Mechanism Tested:**
  Evaluate increasing or deferring `TTL_THRESHOLD_LEDGERS` (200,000) and `TTL_EXTEND_LEDGERS` (500,000).
- **Tradeoff Analysis:**
  - In Soroban, `extend_ttl` is already inherently lazy inside the host environment: if `current_ttl >= threshold`, the host performs a no-op and does not touch ledger state.
  - The 200,000 / 500,000 ledger thresholds (~11.5 days / ~29 days) are strictly bound to [`infra/ttl-sentinel`](../infra/ttl-sentinel) cliff models (validated by `infra/ttl-sentinel/test/constants-sync.test.ts`).
  - Mutating thresholds would desynchronize the monitoring sentinel daemon and risk premature ledger entry archiving.
- **Conclusion:** **Rejected**. Existing threshold configuration is optimal and synchronized with sentinel monitoring.

---

### Candidate 4: Host Call Resolution Caching & Native Index Slicing (Implemented)
- **1. Contract Address Resolution Caching:**
  - In `pull` ([`contracts/streaming-payments/src/lib.rs`](../contracts/streaming-payments/src/lib.rs)), cached `env.current_contract_address()` to eliminate redundant host dispatch on deposit transfers.
  - In `cancel_stream` ([`contracts/streaming-payments/src/lib.rs`](../contracts/streaming-payments/src/lib.rs)), cached `env.current_contract_address()` across recipient and sender payout legs, saving a redundant host dispatch when both payouts execute.
  - Saves **548 CPU instructions** and **80 memory bytes** per call without modifying any contract interface or behavior.
- **2. Native Index Slicing:**
  - In `page_stream_ids` ([`contracts/streaming-payments/src/stream.rs`](../contracts/streaming-payments/src/stream.rs)), replaced manual `while` loops and element-by-element push allocation (`Vec::new`, `get(i)`, `push_back(id)`) with native Soroban SDK vector slicing (`ids.slice(start..end)`).
  - Eliminates O(N) allocation and per-element host dispatch overhead during discovery pagination.
- **Conclusion:** **Accepted & Implemented**.

---

### Candidate 5: Versioned Stream Records and the Refund-Target Read (Implemented)

Two later changes each bought a property at the cost of metered work on the hot
paths, so both are recorded here rather than being discovered by a surprised
contributor:

- **`VersionedStream` envelope (#202).** `DataKey::Stream(id)` now stores
  `{ version, stream }` instead of a bare `Stream`. Every read pays one extra
  map decode and every write one extra map encode, and the record grows by the
  version symbol. This is what makes a future layout change decodable at all —
  the fail-closed probe in `get_stream_record` needs a tag to probe against.
- **`DataKey::RefundTarget(id)` read (#201).** `cancel_stream` resolves the
  address that owns the unaccrued remainder before it writes state, because a
  consented sender-role transfer must be able to leave that claim behind with
  the address that funded the deposit. The read happens on every cancel, and a
  *missing* key is metered like a present one, so a stream that never had a
  transfer pays the difference too.
- **Rejected alternative:** a `refund_to` field on `Stream`. `#[contracttype]`
  structs encode positionally (§13.2 of `docs/ARCHITECTURE.md`), so adding a
  field would make every already-stored record undecodable — the one change
  the storage-versioning work exists to make unnecessary. A separate key costs
  CPU; a field would cost the ledger's history.
- **Refund routing (#204).** `set_refund_target` is a read-then-overwrite of that
  same `DataKey::RefundTarget(id)`: one stream read, one target read, one key
  write, one event, and no token transfer — so amendments are cheaper than the
  cancel they redirect. `create_stream_with_refund` pays ordinary creation plus
  that single write, which is why the creation goldens and budgets are
  unchanged: routing is delegated to the existing creation path and pinned
  afterwards. Because every amendment overwrites the one key rather than
  appending, storage grows by at most one address per *routed stream*, not per
  routed amendment — the append-only growth claim is bounded by streams, and an
  unrouted stream stores nothing at all.
- **Storage growth is append-only and bounded per stream:** at most three extra
  keys exist for a stream that has been through the sender-transfer surface
  (`PendingSenderTransfer`, `RefundTarget`, `SenderHistory`), and each is under
  the same 200,000/500,000-ledger TTL regime as the stream entry itself, so an
  abandoned stream still archives.

Re-measured with `cargo test -p streampay-contract --lib hot_path_footprint
--nocapture` (which prints `FOOTPRINT: path=… cpu=… mem=…` for each step):

| Operation | CPU Instructions | Memory (Bytes) | vs. §3.1 baseline |
|---|---:|---:|---:|
| `create_stream` | 546,291 | 214,304 | +4,693 (+0.87%) |
| `withdraw` | 354,596 | 143,334 | +7,514 (+2.17%) |
| `withdraw_max` | 347,710 | 138,764 | +9,047 (+2.67%) |
| `cancel_stream` | 552,355 | 181,145 | +28,792 (+5.50%) |
| `set_refund_target` | 140,812 | 68,410 | new path (#204), budget 160,000 |

`cancel_stream` is the only path that pays both effects; the rest of the delta
is the record envelope. Its harness budget was raised from 540,000 to
580,000 CPU instructions accordingly, and the harness now prints the numbers
above so the next change to a hot path is measured, not estimated.

---

## 5. Before vs. After Footprint Comparison

| Operation | Metric | Before | After | Change |
|---|---|---:|---:|:---:|
| `create_stream` | CPU Instructions | 542,146 | 541,598 | **-548 (-0.10%)** |
| `create_stream` | Memory Bytes | 210,514 | 210,434 | **-80 B** |
| `create_stream` | Storage Entries | 3 keys | 3 keys | 0 |
| `cancel_stream` | CPU Instructions | 524,111 | 523,563 | **-548 (-0.10%)** |
| `cancel_stream` | Memory Bytes | 171,281 | 171,201 | **-80 B** |
| `cancel_stream` | Storage Entries | 1 key | 1 key | 0 |
| `withdraw` | CPU Instructions | 347,082 | 347,082 | Parity (Optimal) |
| `withdraw_max` | CPU Instructions | 338,663 | 338,663 | Parity (Optimal) |
| `sender_streams (5 ids)` | Vector Strategy | Manual loop / alloc | Native `ids.slice` | **Optimized host slice** |

---

## 6. Verification & Regression Testing

1. **Unit & Property Tests**:
   - `cargo test -p streampay-contract`: All 195 unit and property tests pass.
   - `cargo test -p streampay-passkey`: All 14 tests pass.
2. **CEI Golden Ordering Traces**:
   - `cargo test -p streampay-contract --lib operation_order_matches_golden --locked`: All 9 golden traces match byte-for-byte.
3. **Footprint Regression Harness**:
   - `hot_path_footprint_harness` in `contracts/streaming-payments/src/test.rs` permanently asserts that hot paths execute within instruction budgets:
     - `create_stream` <= 560,000 CPU
     - `withdraw` <= 360,000 CPU
     - `cancel_stream` <= 540,000 CPU
     - `withdraw_max` <= 350,000 CPU
