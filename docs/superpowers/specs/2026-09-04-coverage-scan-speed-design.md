# Coverage scan speed design

**Goal:** the font coverage scan produces exactly the coverage it produces today, in a fraction of the time, without moving off the UI thread and without depending on the job pool.

**Issue:** [#55](https://github.com/AbysmalBiscuit/alacritree/issues/55), a sub-issue of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22), blocking [#27](https://github.com/AbysmalBiscuit/alacritree/issues/27).

**Branch:** `perf/coverage-scan-speed`, cut from `fix-selecting-text-near-the-left-side-of-the` (PR 206, the current stack tip), marker `[5]`.

**Platform:** Windows only. The whole coverage path is `#[cfg(not(unix))]`; fontconfig answers fallback on Linux and macOS, so there is nothing to scan there.

**Relationship to #27:** #27 moves the scan off the UI thread, and its entire design is priced against a multi-second scan. This makes the scan short enough that the question changes. It deliberately does not do any of #27's work, so that #27 can be re-scoped against a measurement rather than a projection.

## Context

`AlacritreeApp::new` calls `fonts::install_terminal_fonts` before the first frame, which reaches `SystemFonts::scanned_coverage` and from there `scan_coverage` (`fonts.rs:182`). That function walks every system face, stats its file, reuses cached codepoint ranges when the file's size and mtime still match, and parses the cmap otherwise. The window does not paint until it returns.

Two things make it slow, and neither is the file I/O.

`cmap_coverage` (`fonts.rs:1138`) pushes every codepoint of every unicode subtable into one `Vec<u32>`, and `Coverage::from_codepoints` (`fonts.rs:2459`) then sorts it, dedups it and compresses it into ranges. Across the faces on the measurement machine that is a sort over roughly 23.6M elements, and it is where nearly all the time goes.

And the whole walk is serial, though each face is independent of every other.

### Measurements

928 faces, 16 logical CPUs, warm filesystem cache, best of 5, taken with a standalone probe against `fontdb` 0.23 and `ttf-parser` 0.25. Reproduce by timing `scan_coverage` against a scratch cache directory so every face takes the fresh-parse branch.

| Variant | 1 thread | 4 threads |
| --- | --- | --- |
| today | 1393 ms | 541 ms |
| ranges built during the walk | 367 ms | 124 ms |
| the raw cmap walk, no range building at all | 319 ms | 111 ms |

The second row lands within about 15% of the third. The sort was nearly all the overhead, and after removing it there is little left to win short of not reading the tables.

Thread count sweep on the second row, from a separate run: 397 ms at 1, 212 at 2, 164 at 3, 121 at 4, 99 at 6, 130 at 8, 86 at 12, 94 at 16. The four-thread figure differs from the table above by 3 ms across runs, which is the scale of the noise, and past 4 the curve is noise throughout: 8 came in slower than 6, and 16 slower than 12. The knee is at 4. The work is memory-bound rather than compute-bound, which is why more cores stop helping.

## 1. Build ranges during the walk

`cmap_coverage` stops collecting codepoints and starts collecting ranges. It gains a constructor to build them through, since `Coverage`'s `ranges` field is private to `mod coverage` (`fonts.rs:2440`) and `cmap_coverage` sits outside it:

```rust
impl Coverage {
    /// Build from a walk that emits codepoints in ascending order, folding
    /// them into ranges as they arrive.  Falls back to `from_codepoints` for
    /// a walk that turns out not to be ascending.
    pub fn from_ascending_walk(walk: impl Fn(&mut dyn FnMut(u32))) -> Self;
}
```

`walk` takes `Fn` rather than `FnOnce` because the fallback below runs it twice.

**The fold rule is a three-way compare against the last range's end.** Given the last range `(start, end)` and the next codepoint `cp`:

- `cp == end + 1` extends the range to `cp`.
- `cp > end + 1` pushes `(cp, cp)`.
- `cp <= end` means the walk is not ascending.

The `<=` arm catches duplicates, not only regressions. A `cp` equal to `end` would otherwise push `(cp, cp)` after a range already ending at `cp`, producing an overlap. That arm is also what makes `end` the maximum codepoint seen so far, which is what makes the detection total: a rule using `<` would let duplicates through as overlaps that stay correct only if a merge pass runs even over a single-subtable face.

The extend arm is written `cp.checked_sub(1) == Some(end)`, so a subtable emitting a codepoint after one at `u32::MAX` cannot overflow. Today's `from_codepoints` is safe from this by accident, because nothing follows `u32::MAX` after a dedup; the fold has no such guarantee.

**The non-ascending fallback re-walks the subtable.** `Subtable::codepoints` takes `&self`, returns `()`, and has no early exit (`ttf-parser-0.25.1/src/tables/cmap/mod.rs:145`), so a walk cannot be abandoned partway. On detection the fold records the fact and lets the walk finish, discarding what it built; `from_ascending_walk` then calls `walk` a second time, collecting into a `Vec<u32>` and handing it to `from_codepoints`. Walking twice costs nothing that matters, because this path exists only for malformed fonts.

Malformed is the right word: the ordering is required, not merely conventional. OpenType requires format-4 segments sorted by `endCode` and format-12/13 groups sorted by `startCharCode`, and `ttf-parser` binary-searches on exactly that in `glyph_index` (`format4.rs:52-101`, `format12.rs:48-60`), so a font violating it already fails glyph lookups. Formats 0, 6 and 10 are index walks and are ascending by construction. Format 2 can interleave by construction (`format2.rs:126-147`), but no unicode format-2 subtable exists in practice. The fallback costs a well-formed font nothing and keeps a malformed one from producing silently wrong coverage.

**The per-subtable lists go through the existing `Coverage::merge`** (`fonts.rs:2491`), already a two-way union that coalesces overlapping and adjacent ranges. Nothing new is written for the merge itself.

**A face whose cmap has no unicode subtable still yields `Some(Coverage::default())`**, as today (`fonts.rs:1140-1148`). Returning `None` would log the face as unparseable and drop it from the scan. Symbol-encoded fonts hit this on every Windows machine.

`Coverage::from_codepoints` stays. After this change it has two callers: the non-ascending fallback, and the tests.

### Why not select a single subtable

Walking only the widest unicode subtable per face is faster still, and it is wrong. Six of 928 faces here lose 304 codepoints under that rule, all Nerd Fonts whose format-4 subtable carries codepoints their format-12 subtable lacks: U+01F7, U+02CA, U+037B and others. Restricting to formats 12 and 13 loses the same 304. The union across subtables has to be preserved. Only the way it is accumulated changes.

## 2. Fan out over faces

`scan_coverage` splits into three phases.

**The stat pass runs first, serially, over distinct paths.** It is 14 ms, and hoisting it is what lets the parallel phase read a finished map instead of contending on one. This is also why `stat_memo` exists today: a `.ttc` collection holds several faces behind one path, and each would otherwise stat it again.

**The parallel phase hands out face indices from an `AtomicUsize`**, and each worker collects into its own `Vec<(usize, (Candidate, Coverage))>`, carrying the face's position alongside its result. No worker touches a shared slot. The fold concatenates the fragments and sorts by the carried index.

The obvious alternative, writing into a preallocated `Vec<Option<_>>` at each worker's own index, is not expressible in safe Rust: several scoped threads cannot each hold `&mut` into one `Vec`, and a dynamic atomic queue rules out splitting it with `chunks_mut`. A `Vec<OnceLock<_>>` would work, but carrying the index costs less and needs no shared allocation at all.

Preserving position matters for determinism, not for tie-breaking. `order_candidates` sorts on family, then path, then face index (`fonts.rs:2590-2596`), a total order over the scan, so input order cannot reach it. What input order does reach is the tests, which compare whole `Vec`s.

**Each worker accumulates its own bookkeeping and the fold merges it.** A worker keeps its own `fresh_files` fragment, its own `hits`, and its own `any_fresh`, so the hot loop touches no shared state beyond the atomic index.

**The `fresh_files` fold merges per-file face maps rather than replacing them.** Today one `CachedFile` accumulates every face of a collection under one path (`fonts.rs:232-240`). Two workers scanning two faces of the same `.ttc` each build their own `CachedFile` for that path, and a `HashMap::extend` in the fold would keep one and discard the other's faces. The fold is therefore `entry(path).or_insert(file).faces.extend(file.faces)`. Getting it wrong fails silently: the dropped faces reparse on every launch, and `coverage_cache_round_trips_across_scans` (`fonts.rs:2141-2155`) still passes, because a cache miss only means a reparse.

The thread count is:

```rust
fn worker_count(reported: usize) -> usize {
    reported.clamp(1, 4)
}
```

called as `worker_count(std::thread::available_parallelism().map_or(1, |n| n.get()))`. Of those workers, one is the main thread running the loop inline and the rest are spawned, so a machine reporting one CPU spawns nothing and scans serially.

The main thread participates rather than reserving a core for itself. It has nothing else to do: it is inside `AlacritreeApp::new`, and the alternative is to sit blocked in `thread::scope` while a core goes unused. Reserving one would give a four-core machine three workers, which the sweep above prices at 164 ms against 121.

The ceiling of 4 is the measured knee: a 36-core machine gains nothing measurable from 36 threads on memory-bound work. The floor of 1 keeps a restricted container from spawning zero workers. `worker_count` is a free function because that is what makes it testable; `available_parallelism` cannot be mocked.

### No thread pool

The fan-out spawns threads directly rather than reusing a pool. Windows thread creation is genuinely slower than Linux's, measured here at about 33 µs per thread against single-digit µs, but the scan pays it once: four scoped threads cost 131 µs typically and 409 µs at the worst of 200 runs, against a scan of roughly 120 ms. That is 0.1% to 0.3%, below the scan's own run-to-run variance.

A pool amortises repeated spawns, and there is exactly one fan-out per process here. A reusable pool does already exist, in `jobs.rs` on `perf/nonblocking-ui`, and using it would put this branch behind the pool stack, the same dependency that has #27 waiting. Writing a second pool would duplicate it. Either way the structural cost far exceeds 131 µs.

This changes if the scan ever becomes repeated, per session or on a font-install event. It is not today, and #27's scope note rules mid-session font installation out.

## Error handling

Nothing new. An unparseable face is skipped with a debug log, unchanged, now inside a worker. A face whose source is `Source::Binary` is skipped as before, and its position simply produces no entry. A panic in a worker re-panics on the joining thread when `thread::scope` returns, on the UI thread during `AlacritreeApp::new`, which is where a panic in font setup already lands; the crash hook fires once, on the worker. The cache write, its failure path, and the summary log are untouched.

## Testing

Equivalence is the load-bearing property, and both tests below need an oracle. Since `cmap_coverage` is the function being replaced, the tests keep a private copy of today's collect-and-sort body to compare against. Without it the implementer compares the new path against itself.

- Coverage for every installed face is identical computed both ways, over the real system font set rather than a fixture. A fixture cannot cover the subtable shapes real fonts carry, and the Nerd Font case above is exactly what a hand-written fixture would miss.
- A parallel scan returns the same `Vec` as a serial scan, element for element and in order. Both runs pass `cache_path = None`, or distinct scratch paths: the existing pattern at `fonts.rs:2143-2151` writes a cache on the first scan, and a second scan against it takes the `from_stored_ranges` branch and never parses a cmap at all.

That second test needs a worker count it can set. `scan_coverage` keeps its current signature (`fonts.rs:182-185`) and delegates to a new `scan_coverage_with_workers(db, cache_path, workers)`, so the four existing call sites (`fonts.rs:2147, 2151, 2165, 2170`) are untouched.

The range builder, tested through `Coverage::from_ascending_walk` against a synthetic walk over a slice rather than hand-assembled cmap bytes:

- A walk that goes backwards falls back and still produces correct ranges.
- A walk that repeats a codepoint falls back rather than emitting an overlap. This is the `<=` arm, and a `<` implementation fails it.

`Coverage::merge` already has `merge_coalesces_overlapping_and_adjacent_ranges` (`fonts.rs:2719`), so overlap and adjacency need no new test. What needs one is the fold that drives it, which the equivalence test above covers.

The cache fold:

- A warm scan of a `.ttc` collection reports every one of its faces as a cache hit. This is the regression test for the per-file merge; a fold that replaces rather than merges fails it, where `coverage_cache_round_trips_across_scans` does not. It needs `hits` returned from `scan_coverage_with_workers`, or an assertion that the second scan does not rewrite the cache file.

The thread count, against `worker_count` directly: 1 from 1, 4 from 4, 4 from 36.

## What this does not do

Everything in #27: starting from the cache, moving the scan to a background job, swapping the chain mid-session, the scan overlay, the config option. The last acceptance criterion on #55 is to record the cold scan time after this lands, so #27 can be re-scoped against a number rather than the 2860 ms it was written against.

Change the disk cache format, its keying, or its validity rules. A cache written before this change stays valid after it, because the ranges it holds are unchanged.

Touch the Unix path. There is no coverage scan there.

## Unresolved questions

1. The cold scan time after the change, on the machine that reported 2860 ms. Everything above is a warm-filesystem measurement of the parse and range building; the first launch after a reboot also pays page faults on every font file, which no measurement here covers.
2. Whether the 14 ms stat pass is still 14 ms cold. It is unmeasured after a reboot, and once the scan is around 120 ms it is a tenth of the total rather than a hundredth.
3. #55's acceptance criteria say the parallel and serial scans are "compared order-independently". This spec asserts the stronger element-for-element equality, which the fan-out design supports. Update the issue rather than weakening the test.
