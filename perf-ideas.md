# Performance ideas

Working branch: `perf/first-moves`. Run benches with the `just` recipes (`just bench`,
`just bench-all`, `just profile`).

## What benchmarks exist

No `benches/` dirs and no criterion harness — the convention here is **ignored timing
tests** compiled as integration tests, designed to stay comparable across revisions:

| Harness | What it measures | Run |
|---|---|---|
| `lore-revision/tests/filter_bench.rs` | Filter build cost, resident size, query throughput | `just bench filter` |
| `lore-revision/tests/filter_descend_bench.rs` | Filter descent over large/deep/fanout trees, cold/warm, concurrent | `just bench filter_descend` |
| `lore-revision/tests/filter_walk_bench.rs` | Child exclusion-state throughput | `just bench filter_walk` |
| `lore-revision/tests/diff_walk_bench.rs` | Diff walk across views | `just bench diff_walk` |
| `lore-storage/tests/oodle_migration_bench.rs` | Oodle→Zstd store migration | **gated**: requires proprietary `OODLE_LIB_DIR` -- out of scope for public work |
| `lore-io/examples/bench.rs` | IO driver throughput | hand-run |
| `lore-revision/tests/path_allocations.rs`, `node_add_parallel.rs`, `parallel_reads.rs` | allocation / parallelism checks under `unit` tests | cargo test |

Baseline (this machine, 2026-10-25, release):
- `filter` build: 100 authored rules 45.7 us/build, ~46 KiB/filter resident
- `excludes` 1126 ns/call, `targets/file` 2124 ns/call, `excludes_subtree` 4717 ns/call
- `diff_walk`: 0.04–0.08 ms/walk for subtree-local diffs; **2.04 ms/walk** when an
  unanchored exclusion forces walking all 217 directories (3932 verdicts for 1 change)

## Verified facts from the code (not from the LEP)

1. **The whole-content compare path (`write.rs::whole_content_hash`) only ever sees
   content ≤ 256 KiB.** `file_matches` routes to it only when
   `file_size <= FRAGMENT_SIZE_MINIMUM` (32 KiB) or `file_size <= FRAGMENT_SIZE_THRESHOLD`
   (256 KiB) and the stored fragment is a single one. So the LEP's "hashing a
   multi-gigabyte file in one call" does **not** live here.
2. Where files **are** read wholly into memory today:
   - `lore-revision/src/merge.rs` `merge3_text_in_operation` reads base/mine/theirs
     entirely into memory (three residents at once, spawned concurrently).
   - `lore-revision/src/file/diff.rs` reads the working side entirely into memory at two
     sites (working-tree diff content).
   These are the real "whole-file resident" stalls for large text files.
3. The small-file compare hot path (status/stage/commit scans): a whole-content compare
   used to pay `read_file_bytes` = open + stat + read + close; reading through an open
   handle drops the stat and close.
4. Thread model (LEP 2026-07-24, approved, unlanded): three overlapping pools
   (core workers + blocking ≤128 + rayon compute), network shares the core runtime,
   file I/O parallelism = blocking-thread count.

## Applied on this branch

- **`lore-storage/src/write.rs::whole_content_hash`** — reads through the open handle in
  `WHOLE_HASH_WINDOW_SIZE` windows into an incremental `blake3::Hasher` instead of
  `read_all` + one-shot hash. Saves open+stat+read+close → open+read per compared file
  and bounds residency to one window. Behavior-preserving: files on this path are
  ≤ 256 KiB, so today the whole content is one window either way.
- **`lore-base/src/types/mod.rs`** — added `Hash::from_bytes` so an incremental hasher can
  compose a `Hash` without reaching into the private field.
- `Justfile` recipes: `bench`, `bench-all`, `profile`.

## Ranked next moves

1. **Measure the syscall win.** Microbench `file_matches` over N files (fixtures exist in
   `lore-storage/tests/unit/store_fixture.rs`) to confirm the compare-path saving.
2. **Stream the merge and diff reads.** `merge.rs` and `diff.rs` whole-file `read_all`s:
   stream or window them so a large text file is never fully resident (the LEP's
   "double-buffered read while hashing" treatment, applied to diff/merge).
3. **land LEP phase 1**: dedicated net runtime + delete the compute pool (FastCDC/zstd
   inline on core workers); fix the two `block_in_place` hazards (JWT interceptor,
   replication client `Drop`) first.
4. **Diff walk with unanchored exclusions** (2.04 ms vs 0.08 ms): the walk visits all 217
   directories for 1 change; prune subtrees that cannot contain matches.
5. **Phase 2 I/O engine** (io_uring/IOCP/psync) — largest change, already speced; do after
   phase 1.
6. Housekeeping: retire the SDK-gated Oodle bench from the public bench story or move it
   behind an explicit `contrib` fixture.
