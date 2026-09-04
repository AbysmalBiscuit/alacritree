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

| Variant | 1 thread | 4 threads | 16 threads |
| --- | --- | --- | --- |
| today | 1393 ms | 541 ms | 260 ms |
| ranges built during the walk | 367 ms | 124 ms | 91 ms |
| the raw cmap walk, no range building at all | 319 ms | 111 ms | 108 ms |

The second row lands within about 15% of the third. The sort was nearly all the overhead, and after removing it there is little left to win short of not reading the tables.

Thread count sweep on the second row, from a separate run: 397 ms at 1, 212 at 2, 164 at 3, 121 at 4, 99 at 6, 130 at 8, 86 at 12, 94 at 16. The four-thread figure differs from the table above by 3 ms across runs, which is the scale of the noise. The knee is at 4. Past it the curve flattens into run-to-run noise, since the work is memory-bound rather than compute-bound.

## 1. Build ranges during the walk

`cmap_coverage` stops collecting codepoints and starts collecting ranges.

For each unicode subtable it folds the walk directly into a `Vec<(u32, u32)>`: extend the last range when the next codepoint is one higher than its end, otherwise push a new one. cmap subtables enumerate in ascending order, so this produces a sorted, disjoint range list with no sort at all. The per-subtable lists are then merged pairwise into the face's coverage.

**The merge treats overlapping and adjacent ranges as one.** A font's format-4 and format-12 subtables overlap heavily, and a range ending at `n` followed by one starting at `n + 1` has to become a single range or the output stops matching today's. Merging two sorted lists is linear in their combined length, against a sort of their union that is not.

**A subtable that does not enumerate in ascending order falls back**, for that subtable alone, to collecting its codepoints and calling `from_codepoints`. Detection is free: the fold already compares each codepoint against the last range's end, so a codepoint that goes backwards is visible where it happens. No face across the 928 on the measurement machine triggered this, but the cmap specification does not require ascending enumeration and a malformed font must not produce silently wrong coverage.

`Coverage::from_codepoints` stays. `face_coverage` still calls it for a single seed face, the fallback above uses it, and the disk cache's `from_stored_ranges` path is untouched.

### Why not select a single subtable

Walking only the widest unicode subtable per face is faster still, and it is wrong. Six of 928 faces here lose 304 codepoints under that rule, all Nerd Fonts whose format-4 subtable carries codepoints their format-12 subtable lacks: U+01F7, U+02CA, U+037B and others. Restricting to format 12 and 13 loses the same 304. The union across subtables has to be preserved. Only the way it is accumulated changes.

## 2. Fan out over faces

`scan_coverage` splits into three phases.

**The stat pass runs first, serially, over distinct paths.** It is 14 ms, and hoisting it is what lets the parallel phase read a finished map instead of contending on one. This is also why `stat_memo` exists today: a `.ttc` collection holds several faces behind one path, and each would otherwise stat it again.

**The parallel phase hands out face indices from an `AtomicUsize`** to scoped threads, each writing into a preallocated `Vec<Option<_>>` at its own index. Writing by position rather than pushing is what keeps the output order bit-identical to today's, so the change cannot perturb tie-breaking in `order_candidates` or `trim_by_coverage`.

**Each worker accumulates locally and the merge phase folds.** A worker keeps its own `fresh_files` fragment, its own `hits`, and its own `any_fresh`, so the hot loop touches no shared state beyond the atomic index and its own slot. After `thread::scope` returns, the main thread folds the fragments, writes the cache, and logs, all exactly as now.

The thread count is:

```rust
std::thread::available_parallelism()
    .map_or(1, |n| n.get())
    .saturating_sub(1)
    .clamp(1, 4)
```

One core is left for the UI thread and the first shell spawn. The ceiling of 4 is the measured knee: a 36-core machine gains nothing measurable from 36 threads on memory-bound work, and `saturating_sub` plus the lower clamp means a restricted container reporting one CPU scans serially rather than panicking or spawning zero threads.

### No thread pool

The fan-out spawns threads directly rather than reusing a pool. Windows thread creation is genuinely slower than Linux's, measured here at about 33 µs per thread against single-digit µs, but the scan pays it once: four scoped threads cost 131 µs typically and 409 µs at the worst of 200 runs, against a scan of roughly 120 ms. That is 0.1% to 0.3%, below the scan's own run-to-run variance.

A pool amortises repeated spawns, and there is exactly one fan-out per process here. A reusable pool does already exist, in `jobs.rs` on `perf/nonblocking-ui`, and using it would put this branch behind the pool stack, the same dependency that has #27 waiting. Writing a second pool would duplicate it. Either way the structural cost far exceeds 131 µs.

This changes if the scan ever becomes repeated, per session or on a font-install event. It is not today, and #27's scope note rules mid-session font installation out.

## Error handling

Nothing new. An unparseable face is skipped with a debug log, unchanged, now inside a worker. A face whose source is `Source::Binary` is skipped as before. A panic in a worker propagates out of `thread::scope` on the UI thread during `AlacritreeApp::new`, which is where a panic in font setup already lands. The cache write, its failure path, and the summary log are untouched.

## Testing

Equivalence, which is the load-bearing property:

- Coverage for every installed face is identical computed both ways, over the real system font set rather than a fixture. A fixture cannot cover the subtable shapes real fonts carry, and the Nerd Font case above is exactly the kind a hand-written fixture would miss.
- A parallel scan returns the same `Vec` as a serial scan, element for element and in the same order, not merely the same set.

The range builder:

- Overlapping ranges from two subtables merge into one.
- Ranges that are adjacent but not overlapping merge into one.
- A subtable enumerating out of ascending order still produces correct ranges, against a synthetic subtable since no real font on the measurement machine does it.

The thread count:

- 1 when `available_parallelism` reports 1, 3 when it reports 4, 4 when it reports 36.

Existing tests should need no changes. They assert on coverage content, and this design's whole claim is that the content does not change.

## What this does not do

Everything in #27: starting from the cache, moving the scan to a background job, swapping the chain mid-session, the scan overlay, the config option. The last acceptance criterion on #55 is to record the cold scan time after this lands, so #27 can be re-scoped against a number rather than the 2860 ms it was written against.

Change the disk cache format, its keying, or its validity rules. A cache written before this change stays valid after it, because the ranges it holds are unchanged.

Touch the Unix path. There is no coverage scan there.

## Unresolved questions

1. The cold scan time after the change, on the machine that reported 2860 ms. Everything above is a warm-filesystem measurement of the parse and range building; the first launch after a reboot also pays page faults on every font file, which no measurement here covers.
2. Whether the 14 ms stat pass is still 14 ms cold. It is unmeasured after a reboot, and once the scan is around 120 ms it is a tenth of the total rather than a hundredth.
3. Whether the equivalence test over the real system font set is acceptable in CI, where the font set differs from any developer machine and is much smaller. The test proves equivalence against whatever fonts are present, which is weaker on a CI runner than locally but is still a real assertion.
