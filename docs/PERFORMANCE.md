# Performance measurements

PlusPlus treats performance claims as test results, not adjectives. The benchmark suite uses
deterministic inputs and records the exact commit, toolchain, OS, architecture, and kernel next
to Criterion's raw estimates.

## Run the suite

From the repository root:

```bash
scripts/benchmark.sh
```

Artifacts are written to `target/performance/<UTC timestamp>/`:

- `environment.txt` identifies the code and machine.
- `criterion.txt` is the human-readable run log.
- `criterion/` contains Criterion's estimates and HTML report.

For a quick compile-only check that does not measure performance:

```bash
cargo test -p plusplus-core --bench core_hot_paths --no-run --locked
```

## Current benchmark contract

| Group | Deterministic workload | Why it matters |
| --- | --- | --- |
| `safety_analysis` | 1, 10, and 100 mixed read/write statements | Production Guardian must not make the editor feel slow. |
| `page_sql` | A simple table query at offset 1,000,000 in all five SQL dialects | Pagination rewrites run on every page transition. |
| `page_sql/composite_keyset_cursor` | A deterministic two-column primary-key cursor | Deep scrolling must avoid work proportional to OFFSET. |
| `duckdb_analytics` | Grouped aggregate over one million deterministic rows in memory and in Parquet | Verifies both embedded OLAP and offline columnar-scan paths without network variance. |
| `result_processing` | Sort 1k, 10k, and 100k text values | Captures the current materialized-grid cost before memory budgeting. |
| `json_import` | Preview 50 rows from a deterministic 100k-row JSON array | Preview time should depend on preview size, not file length. |

The generated JSON fixture contains only a monotonically increasing id, a fixed-width name, and
a boolean. There is no randomness, wall-clock field, locale-sensitive formatting, or network I/O.

## Comparing changes

Use the same idle machine and save a baseline from the target branch:

```bash
cargo bench -p plusplus-core --bench core_hot_paths -- --save-baseline main
cargo bench -p plusplus-core --bench core_hot_paths -- --baseline main
```

Do not compare timings produced by different machines. Shared GitHub-hosted runners only verify
that benchmarks build; the scheduled workflow publishes results as artifacts but does not block a
change on noisy wall-clock measurements. A blocking regression threshold should only be enabled
after a fixed runner has at least 20 successful baseline runs.

## Planned end-to-end metrics

The microbenchmarks establish the CPU baseline. The next performance milestones add black-box
measurements for startup-to-first-frame, idle/peak RSS, streaming export throughput,
cancellation latency, and release binary size. Multi-tab eviction, stream byte
ceilings, keyset SQL generation, and DuckDB's in-process analytical scan are already covered by
unit or Criterion tests.
