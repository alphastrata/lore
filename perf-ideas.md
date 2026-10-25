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

## Landed on this branch (every row bench-arbitrated or unit-gated)

- `lore-revision/src/state/diff.rs` — walk sides pair by rule content, not pointer identity;
  a second verdict per paired node is asked only where the rules actually differ (ed5104ee).
- `lore-storage/src/content.rs` + `write.rs` — `size_and_head` / `read_small_with_retry`
  carry size and head in one `open_read_head` dispatch; the compare, the small-store write
  and the resolved publish each route through it, with their open-handle flows kept as
  fallbacks (c9eef9fe, 53e76f42, d6462492).
- Benches: `file_matches_bench`, `hash_file_bench`, `resolved_publish_bench`; `just bench`,
  `bench-all`, `profile`. The earlier windowed-hash and per-op-SIMD attempts were
  bench-arbitrated OUT and reverted with their mechanisms recorded below.

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

## fearless_simd experiment (measured, reverted)

Wired `fearless_simd` 1.1.0 behind the literal-glob compare (`filter.rs`, commit 914d2b50,
reverted 914d2b50's revert): per-call `dispatch!` + `simd_into`/`into` round-trips on <=32-byte
strings. AVX-2 host, filter_bench (post-change vs. baseline):

| metric | baseline | with kernel |
|---|---|---|
| excludes | 1126 ns/call | 1300 ns/call |
| excludes/target | 2124 | 2260 |
| excludes_subtree | 4717 | 5152 |
| build (100 rules) | 45.7 us | 43.4 us (noise) |

The loss is per-op overhead (dispatch + array<->lane round-trips + panic-able try_into), not
lane-width: short-string equality is already one memcmp with no call-frame for short inputs.
**Conclusion: per-op vectorization of the literal compare is the wrong grain.**

The SIMD-shaped opportunity at the right grain is **batched**: one literal line against every
child name of a directory (the descend walk asks per child today), or all literal lines against
one name, both amenable to a real `simd_eq` mask pass. AVX-512 host available via
`ssh framebox` (cargo needs `source ~/.cargo/env` there); rsync a target-free tree and pull
numbers back.  for that follow-up if/when it
earns its place.

(The revert took the dep back out of the tree; re-add it only when the batched kernel is being built.)

## file_matches bench: first hard numbers, and a regression reversed

`lore-storage/tests/file_matches_bench.rs` (committed): 2000 files of 8 KiB, 3 rounds,
answers asserted. Baseline (session start, 5bca99d6) vs the branch:

| state | per call |
|---|---|
| baseline `read_all` (one composite dispatch) | 25.1 us |
| branch: handle + windows read (open, read, close split out) | 42-43 us |
| after revert of 9203a154 | restored to baseline |

The whole-content compare's cost is not the bytes -- it is the per-dispatch round trip into
the IO driver (~10-20 us each, warm). Splitting one composite dispatch into three tribled the
round trips. Lessons, now facts: (1) never split an existing composite read to save a syscall
inside it; (2) the per-file compare costs ~25 us where the I/O work is worth ~2, so the real
levers are fewer dispatches per file (composite open+read+close operations, batching, the
LEP's I/O engine) -- which moves "phase 2 I/O engine" up the list on measured, not asserted,
grounds.

## One-dispatch compare: the first kept perf gain

`ContentSource::size_and_head` (committed): the compare's routing size and the head that
answers it at or under the minimum cut travel in ONE dispatch via the driver's existing
`open_read_head` composite (open, stat, read), replacing the two the compare paid before
(`metadata`, then `read_file_bytes`). file_matches' whole-content route hashes the head
directly; anything larger falls to the unchanged streaming routes.

| state | per call (2000 x 8 KiB) |
|---|---|
| baseline (two dispatches) | 25.1 us |
| one dispatch, this branch | see bench output in this run |

### Decomposition (branch committed): dispatch vs hash, three sizes

| size | per call |
|---|---|
| 1 KiB | see run |
| 8 KiB | see run |
| 32 KiB (the cap) | see run |

The 1 KiB row is the dispatch floor: one pool handoff plus permits and plumbing. The
difference to the 32 KiB row is what the compare's hashing of the head actually costs. What
remains below the floor is the LEP phase-2 driver work: queue-depth submission instead of a
per-op pool handoff.

### Small-store write route: same lesson applied (hash_file bench added)

`write_from_file` (and so `hash_file`, the hash a stage/status scan computes per small file)
paid open+metadata inside `source.open()` and then `read_all`: three dispatches. Content at
or under one fragment now reads whole, size and head together, in one dispatch through
`read_small_with_retry` -- the open path's retry semantics carried over verbatim; streaming
files and the rare head/size race fall through to the untouched open-handle flow.

### Resolved publishes: the third site, same shape

`write_resolved_content`'s read paid the same three dispatches as the small-store write did.
It now reads whole, head and size together, in one dispatch, under the open path's retry;
streaming and head/size races fall through untouched. Measured on its twin route at
32.1 -> 15.9 us; a dedicated mutable-store bench can pin this one identically when wanted.

### Resolved publishes: pinned

`resolved_publish_bench.rs` added. 1000 files of 8 KiB, three rounds, publishes asserted:

| state | per call |
|---|---|
| baseline `source.open()` + `read_all` (three dispatches) | 36.9 us |
| branch: one dispatch via `read_small_with_retry` | 17.9 us |

## Handoff snapshot (final state of perf/first-moves)

| route | baseline | branch | bench |
|---|---|---|---|
| file compare | 25.1 us | 15.3 us | file_matches_bench |
| small-store read (hash_file) | 32.1 us | 15.9 us | hash_file_bench |
| resolved publish | 36.9 us | 17.9 us | resolved_publish_bench |
| diff walk, equal-rule sides | second verdict per paired node | none | diff_views + unit pair |
| dispatch floor (1 KiB vs 8 KiB) | -- | 15.6 us flat | file_matches_bench decomposition |
| hashing at the 32 KiB cap | -- | ~6.5 us over floor | -- |

First concrete moves for the three open items (each a dedicated session):

1. **Unanchored-exclusion walk prune.** Design: suffix-delta fold. When `DiffFlags::between`
   additionally detects `to.lines` extending `from.lines`, carry the suffix; the walk answers
   the to-side child verdict by folding only the suffix lines continuing the from-side parent
   state (the machinery is `FilterState::ROOT` + `decided_at` flooring + `min_depth`). Ceiling:
   ~2x verdict reduction in the uncovered case; the 25x cases are anchored and already pruned.
   Cost: new filter API, walk seeding plumbing, and count-contract updates in diff_views.

   **Progress: atom 1 landed (`3eb60bc9`).** `Filter::view_suffix(prefix)` detects the
   extension slot-wise (ignore lines equal; prefix's view lines an exact run; returns the
   suffix lines), unit-tested four ways beside the `between` pair. The detection's tests
   caught two draft bugs before any walk plumbing existed -- the prefix span must cover
   `prefix`'s full run, and an empty rule list is a prefix of everything.

   **Atom 2, machinery map.** The fold chain is `child_excludes_tree` ->
   `child_exclude_reason` -> `FilterStates::child_exclusion_states` (diff.rs 1338) ->
   `child_exclusion_state` (filter.rs 1097) -> `step` (1037), flooring at `decided_at` and
   gated by `min_depth`, with `FilterStates` carrying one state per side. The to-side ask
   under TwoViews (`to_subtree_verdict`, called at ~1073 and ~1282) resumes the to-side
   fold from `parent`. The slice: when the two filters pair by suffix, seed the to-side
   child fold from the FROM-side child state and fold only the suffix lines (suffix line
   indices exceed any `decided_at` floor the from side can set, so they are never floored;
   from-side lines resume exactly as the from ask ran them). Requires reading
   `FilterStates`/`step` bodies (~150 lines), threading `view_suffix`'s output from the
   seeding into `to_subtree_verdict`, and reconciling the ask counts `diff_views` asserts
   -- the to-side asks halve for suffix-paired configurations, exactly as they did when
   `between`'s pointer comparison retired.
2. **Dispatch floor.** The 15.6 us flat cost is one psync-pool handoff. Phase-2 lever: batch
   several files' open+stat+read into one dispatch (a multi-file composite op), or submit
   through the uring engine so data-plane parallelism is queue depth, not pool size. The
   three benches arbitrate any slice.
3. **LEP phase 1.** `net_runtime()` accessor + `lore_spawn_net!` variant in
   `lore-base/src/runtime.rs`, then migrate transport construction per the LEP's site list,
   then delete the compute pool. Phased, per the LEP's own plan.

## Pairing read, second pass (atom 2b scoping)

`FilterStates` is slots (ignore/view), not sides; each SIDE walks with its own
`DiffStates { from: FilterStates, to: FilterStates }` in `DiffCursor`. The to-side ask
(`to_subtree_verdict`, diff.rs ~1073 and ~1282) resumes `states.to` with the to filter, and
`from_node_states: FilterStates` is a threaded parameter of the pairing functions (bindings
at diff.rs 615, parameters at 689 and 961) -- the from-side child states are in scope where
the slice needs them. Plumbing shape: when `states.from == states.to` slot-wise and the
filters pair by suffix, the to-side child view state is
`step_suffix(from_child.view, base = from_len, suffix)` and the ignore slot carries verbatim
(`view_suffix` guards ignore-list equality, so both sides fold the identical ignore list).

The hard part, now named precisely: the verdict is not only the state. `child_exclude_reason`
-> `descend_reason` consults the reinclude machinery (a `!rule` beneath forces descent into
an excluded subtree). Under the suffix split, "could anything below be re-included" is an OR
across (a) the shared run beyond the from-side `decided_at` and (b) the suffix lines -- and
that split is where silent over-pruning would hide. Next atom: re-derive the to-side descend
reason from the reinclude indexes over (shared-run tail, suffix), prove it against
`descend_reason` unit-wise the way `folding_a_suffix_composes` proved the fold, and only
then thread it into `to_subtree_verdict` and reconcile `diff_views`' ask counts.
