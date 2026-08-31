# Plan Review Log: font-bytes memory implementation plan

Started 2026-08-09. MAX_ROUNDS=5. MODEL=gpt-5.6-sol, EFFORT=xhigh, sandbox=read-only.

Subject: `docs/superpowers/plans/2026-08-09-font-bytes-memory.md`
Upstream spec (already reviewed over 5 rounds):
`docs/superpowers/specs/2026-08-09-font-bytes-memory-design.md`
Spec review log: `docs/superpowers/specs/2026-08-09-font-bytes-memory-review-log.md`

This loop reviews the *plan*, not the spec: whether the task decomposition,
the exact code in each step, and the RED/GREEN claims actually implement the
approved design against the tree at `3617fe53`.

## Round 1 — Codex

Thread `019fe66e-cf36-7240-9829-6d9984f9d451`. `VERDICT: REVISE`.

> The plan is not executable as written. The Rust design is mostly sound, but the RED commands, Linux warning gate, edit anchors, and acceptance harness have material defects.
>
> 1. Tasks 3 and 4 pass multiple positional test filters; Cargo rejects the second with `unexpected argument`, so no RED state runs. Fix: run each test separately or use one shared filter such as `color_glyph::tests::`.
> 2. Task 4 fails Linux `-D warnings`: `RefCell` is imported unconditionally but used only under `#[cfg(not(unix))]`, while `is_mapped` is compiled under `#[cfg(test)]` but has no Linux consumer. Fix: gate the `RefCell` import with `#[cfg(not(unix))]` and `is_mapped` with `#[cfg(all(test, not(unix)))]`.
> 3. The plan's own scaffolding invalidates later edit ranges: Task 3 inserts seven lines before the claimed `112–121` range, and Task 4 Step 7 calls `std::fs::read` the "body's first line" after a counter has become the first statement. Fix: replace exact source text or symbol-delimited blocks.
> 4. Baseline duplicate extraction selects allocations having both `Priv > 0` and `Mapped > 0` under one `AllocationBase`; the heap `Vec` from `std::fs::read` and the file mapping are independent allocations, so `baseline-dupes.txt` can be empty and criterion 1 becomes vacuous. Fix: correlate separate private and mapped-font rows by size, then assert the captured set is nonempty and has the expected six entries.
> 5. `baseline-mapped.txt` records every `MEM_MAPPED` path, including unrelated or unnamed mappings, despite the criterion claiming to cover font mappings only. Fix: record only nonempty canonical font paths from confirmed `.ttf`, `.ttc`, `.otf`, or `.otc` mappings.
> 6. The six memory and twelve startup samples are never persisted: `$allocs` is overwritten per run, timing is only printed, and four median-based criteria are never calculated. Fix: store every labeled sample, define spread as min–max, calculate medians, and make all six criteria throw on failure.
> 7. The plan says artifacts go to a scratchpad, but every `Set-Content` and `Get-Content` uses a relative path, potentially dirtying the worktree. Fix: define one concrete external `$scratch` directory and use `Join-Path` for every artifact.
> 8. Both readiness loops are unbounded and lack cleanup on exceptions, so a crashed or hung process can block forever or leak an instance into later samples. Fix: add a deadline and `$p.HasExited` check, with termination in `finally`.
> 9. Task 5 has no commit, contradicting the requirement that every task end in one. Fix: reclassify Task 5 as an unnumbered final-verification appendix.
> 10. The worktree command fetches and branches from moving `upstream/master`, even though the plan pins `3617fe53`. Fix: create the worktree directly from `3617fe53` and separately verify it is the intended upstream base.
> 11. The Task 2–4 subjects are 63, 61, and 61 characters, violating the repository's 50-character subject rule. Fix: shorten.
> 12. Task 2 claims five pre-existing color-glyph tests, but the module has six. Fix: change the expected count to six pre-existing, seven after Task 2.

### Claude's response

All twelve accepted; each verified against the tree before applying. None touches the Rust design — they are RED commands, the Linux warning gate, edit anchors, and the acceptance harness.

- **1** — confirmed: `cargo test` takes one positional `TESTNAME`; a second is rejected by cargo's own parser before libtest sees it. Both RED steps now list one invocation per test.
- **2** — confirmed both halves. `RefCell` now enters via a separate `#[cfg(not(unix))] use std::cell::RefCell;`; `is_mapped` is now `#[cfg(all(test, not(unix)))]`, and the Deviations section records the narrower gate.
- **3** — confirmed. Task 3 Step 4 and Task 4 Step 7 now quote the exact source text to be replaced instead of citing a line range, and each states which scaffolding sits above it.
- **4** — the strongest finding. The private copy and the mmap have different `AllocationBase` values, so the old filter would have produced an empty `baseline-dupes.txt` and a criterion that passes by measuring nothing. Rewritten to correlate a font mapping with a same-sized private allocation (`Mapped -eq 0`, within 64 KiB) and to `throw` when fewer than six pairs are found.
- **5** — accepted; `$FontExt` restricts `baseline-mapped.txt` to `.ttf|.ttc|.otf|.otc`, and `Canonical` now returns `""` for an unnamed mapping instead of failing on `$null`.
- **6** — accepted. `Measure-Memory` and `Measure-Startup` are functions returning labelled records; every memory sample also lands in `<label>-allocs.csv`. `Median` and `Spread` are defined, and all six criteria now `throw`.
- **7** — accepted; `$scratch` is `$env:TEMP\alacritree-font-bytes-measurement`, and every artefact path goes through `Join-Path`.
- **8** — accepted; `Wait-Ready` bounds itself at 120 s, fails on `$proc.HasExited`, and both harnesses terminate the process in `finally`.
- **9** — accepted; Task 5 is now the unnumbered "Verification Appendix", stated to change no source and produce no commit.
- **10** — accepted with a tightening: the worktree is cut from `3617fe53` directly, and the step first prints `upstream/master` and instructs a **stop** if it is not `3617fe53`, since every anchor in the plan was derived against that tree.
- **11** — accepted. Subjects are now `refactor(fonts): widen map_font_file visibility` (50), `fix(color_glyph): borrow mapped font bytes` (42), `fix(color_glyph): memoize the chain lookup` (42), `perf(fonts): stop re-reading the fallback seed` (46). The global rule is ≤50 preferred / 72 hard, so the old 61–63 were within the hard limit but over the preference.
- **12** — confirmed by counting the module: six tests at `color_glyph.rs:361, 388, 413, 432, 445, 462`. The spec's "five" referred to the five that gate on `chain_with_color_fonts`; the plan had misread it as the total.

Found independently, before Codex reported: Task 5's gate ran `cargo clippy -p alacritree --all-targets -- -D warnings`, but CI runs `--all-targets --locked --no-deps` (`ci.yml:41`). Without `--no-deps`, clippy lints the vendored `alacritty_terminal`, which is `#![deny(clippy::all)]` — it would have failed on toolchain drift rather than on this change.

## Round 2 — Codex

`VERDICT: REVISE`. The Rust tasks and every questioned PowerShell construct
(`$LASTEXITCODE` after a pipeline into `Out-Null`, `Start-Process -PassThru`
and `HasExited`, `Export-Csv` of a nested object, `[ordered]@{}` with pipeline
values, `-match` against an empty string, integer division in `Median`) checked
out. Six remaining findings, all in the acceptance harness or in prose.

> 1. Duplicate extraction is not one-to-one: `Select-Object -First 1` can reuse one private allocation for multiple same-sized mapped fonts; a probe with five mappings and one private allocation reported five "pairs". Fix: preserve each allocation's base address and consume each private allocation after matching it once.
> 2. Criteria 1 and 3 inspect only `$revMem[0]`; samples 2 and 3 can retain duplicate buffers or lose mappings without failing acceptance. Fix: evaluate against every revised sample, and require the baseline artefacts to be consistent across all baseline samples.
> 3. `VM.Dump` returns `"open failed"`, but `Measure-Memory` accepts missing `TOTALS` as zero totals; one failed sample can be hidden by the median. Fix: throw unless `Dump` returns exactly one `TOTALS` row, at least one `ALLOC` row, and nonzero committed totals.
> 4. Cold-cache deletion suppresses every removal error, while neither warm series is explicitly primed by its own binary; the cache-state labels are not proven. Fix: prime each warm series after a verified deletion, and make cold deletion fail unless the cache is confirmed absent before launch.
> 5. The proposed `is_mapped` comment says "its only caller is — an ungated `#[cfg(test)]` item," which is malformed and omits that the caller is Windows-gated.
> 6. Open question 1 still says Task 5 remains untouched, but Task 5 no longer exists.

### Claude's response

All six accepted.

- **1** — the strongest finding of the round, and the same failure mode as round 1's finding 4 one level deeper. The five CJK fallbacks are the same size, so one private allocation could answer for all five and the `Count -ge 6` gate would pass with a single copy present. `Measure-Memory` now keeps each allocation's `Base`, and `Get-TwinSizes` consumes a matched private allocation through a `HashSet[string]` so pairing is one-to-one.
- **2** — already applied before this critique arrived, at the user's instruction, and independently identified in the previous turn. Criteria 1 and 3 now loop over every revised sample; the baseline artefacts are derived from all three baseline samples, `baseline-mapped.txt` keeps only paths mapped in every one, and the per-sample duplicate counts must be identical and at least six.
- **3** — accepted. `Measure-Memory` now throws unless there is exactly one `TOTALS` row, at least one `ALLOC` row, and both committed totals are positive. `[long]$null` is `0`, so the old code turned a refused `OpenProcess` into a plausible-looking zero.
- **4** — accepted. `Initialize-WarmCache` runs a discarded launch of the binary under test before each warm series; cold runs assert the cache is absent after deletion instead of suppressing the error; warm runs refuse to proceed when no cache exists.
- **5** — accepted; the comment now reads "Gated on `not(unix)` because its only caller is Windows-gated; otherwise this test-only helper is dead code on Linux."
- **6** — accepted; "Tasks 1, 3, 4 and the Verification Appendix."

Note on ordering: finding 2's fix was written to the plan while this round was
still running, so Codex critiqued the pre-fix file for that item. Its stated
fix and the applied fix agree, including the baseline-consistency requirement.

## Round 3 — Codex

`VERDICT: REVISE`. Four findings, all in the verification appendix. Codex
explicitly cleared the Rust side this round: "the cfg gates, borrow lifetimes,
`Default` derivation, exact Rust replacements, RED/GREEN claims, script-scope
variables, generic `HashSet[string]`, closure captures, ordered-dictionary
extension, array wrapping, median calculation, and `$LASTEXITCODE` behavior
showed no additional blocker."

> 1. `Initialize-WarmCache` suppresses cache-deletion failure, so it can accept a stale cache from the other binary. Fix: assert `$CachePath` is absent immediately after removal, using the same verified-deletion helper as cold runs.
> 2. The baseline check compares only duplicate *counts*, then writes sizes solely from sample 1; equal counts can conceal different size multisets and make criterion 1 miss surviving copies. Fix: compare all three sorted twin-size multisets within the stated tolerance before writing their common values.
> 3. The prose says any intermittent mapping invalidates the baseline, while the code silently retains the intersection. Fix: state that the intersection defines the stable baseline, or require all three path sets to match.
> 4. `$rows[0]` — an empty pipeline makes `$rows = $null` and throws `NullArray` before the labelled validation, while a scalar `"open failed"` reports only `o`. Fix: use `$(@($rows)[0])` or `Select-Object -First 1`.

### Claude's response

All four accepted.

- **1** — accepted, and factored rather than duplicated: a `Clear-CoverageCache` helper does the removal and asserts absence, and both `Measure-Startup`'s cold branch and `Initialize-WarmCache` call it. The old code would have primed on the other binary's cache and still passed its own final `Test-Path`.
- **2** — accepted. `$twinSets` now holds the three sorted size lists; each is compared element-wise against sample 1 within the same 64 KiB tolerance before `baseline-dupes.txt` is written from it. `Get-TwinSizes` is also called once per sample instead of twice.
- **3** — accepted as a prose fix, taking the first of the two offered options. The two properties genuinely want different treatment, and the text now says so: mappings take the intersection (criterion 3 must not demand of the revised binary what the baseline did not do reproducibly, with the `-lt 6` floor guarding against collapse), while duplicate sizes must match across all three.
- **4** — accepted. `$rows` is wrapped at assignment — `@([VM]::Dump($p.Id) -split ... | Where-Object { $_ })` — which fixes both halves: `$null` becomes `@()` and a one-line scalar becomes a one-element array, so `$rows[0]` reports `open failed` rather than `o`.

Applied in the same pass, found independently: the Step 9 report iterated an
unordered `@{}`, so the four memory series printed in arbitrary order run to
run. Now `[ordered]@{}`.

## Round 4 — Codex

`VERDICT: REVISE`. One finding. Codex cleared everything else it had asked for
in round 3 — "the `$twinSets` wrapping, numeric sorting, empty/singleton
indexing, 64 KiB comparison, single invocation per sample, `Clear-CoverageCache`
factoring, wrapped `$rows`, and ordered report block are sound" — and restated
that "the Rust tasks are unchanged and retain the previously verified cfg,
lifetime, RED/GREEN, source-anchor, and parallel-test properties."

> Lines 940–957 count allocation rows, not unique canonical paths. Six mappings of one font produce `$mappedAlways.Count == 6`; the PowerShell probe confirmed `rawCount=6`, `uniqueCount=1`, contradicting the claim that the floor prevents collapse. Fix: apply `Sort-Object -Unique` before counting and writing `$mappedAlways`, and make any per-sample floor count distinct paths.

### Claude's response

Accepted. The finding is sharper than it first reads: it does not just weaken
the floor, it contradicts the sentence added in round 3 that claimed the floor
guards against collapse. A single file with six mapped views would have cleared
a floor of six on its own, and `baseline-mapped.txt` would have carried six
copies of one path.

`$baseFontSets` stays as rows — that is what `Get-TwinSizes` pairs against, one
row per mapping. A new `$baseFontPaths` holds `Sort-Object -Unique` paths per
sample; the per-sample floor, the intersection, and the written file all use it.
The prose bullet now says the floor counts distinct files and why.

Applied in the same pass, held back from round 3 to avoid a second mid-read
edit: the `$twinSets[0].Count -lt 6` check now runs *before* the multiset
comparison. With it after, a baseline that produced no duplicate pairs at all
threw "baseline duplicate sizes differ" when the three samples in fact agreed —
at zero. Acceptance was correct either way; the diagnosis was not. The
`-and $a.Count -gt 0` guard in `$same` is gone, since the floor now precedes it.

## Round 5 — Codex

`VERDICT: APPROVED`.

> No material problem remains; the plan is sound enough to implement.
>
> - The row/path split is consistent. A twice-mapped font with one private copy produces one twin; the `HashSet` prevents reuse. Distinct-path floors separately prevent repeated mappings from satisfying the six-font requirement.
> - `$baseFontSets` feeds only twin pairing; `$baseFontPaths` feeds both mapping floors, the intersection, and `baseline-mapped.txt`.
> - PowerShell array wrapping, numeric sorting/indexing, cache clearing, and function ordering are valid.
> - Rust anchors still match `3617fe53`; lifetimes, `Default`, cfg gates, and test-only imports/helpers check out on Linux and Windows.
> - All stated RED failures and GREEN outcomes follow from the baseline and revised control flow. Existing tests retain their behavior, and test paths avoid `FONT_MAPS` interference.
> - No stale Task 5 reference, dangling variable, missing spec behavior, or task-boundary dependency remains.
>
> This was a static read-only review; Cargo commands were not run because they would write build artifacts.

## Outcome

Converged on round 5 of 5 with `VERDICT: APPROVED`. Twenty-three findings across
four rounds of critique: 12, 6, 4, 1 — a clean descent. Every one was accepted;
none was rejected, which is itself worth recording, because in the spec review
that preceded this one four of twenty-one were rejected with cause. The
difference is what the two loops were reviewing: the spec argued about design,
where Claude had positions to defend, while this loop argued about whether the
plan's mechanics were correct, where there is a fact of the matter.

Three findings were independently identified by Claude rather than Codex: the
`clippy --no-deps` mismatch with CI (round 1), the sample-1 evaluation of
criteria 1 and 3 (round 2, fixed at the user's instruction before Codex
reported it), and the misleading "samples differ" diagnosis on an empty
baseline (round 4).

Every defect of consequence was in the acceptance harness, not the Rust. Four
of them shared one failure mode — a check that would have reported a pass while
measuring nothing:

1. Duplicate pairs filtered on a single allocation carrying both private and
   mapped bytes, which never happens; `baseline-dupes.txt` would have been
   empty and criterion 1 vacuous (round 1, finding 4).
2. Pairing that reused one private allocation for all five same-sized CJK
   fonts, reporting five copies where one existed (round 2, finding 1).
3. A refused `OpenProcess` returning `"open failed"`, parsed into zero totals
   and averaged away by the median (round 2, finding 3).
4. A distinct-font floor that counted allocation rows, which six views of one
   file would clear alone (round 4).

Caveat: the review was static. Codex did not run `cargo test`, `cargo clippy`,
or the PowerShell harness, because doing so writes build artifacts and the
sandbox is read-only. The RED/GREEN claims and the compile claims are reasoned
from the source at `3617fe53`, not executed. One claim *was* executed, by
Claude: `cargo test -p alacritree --no-run foo bar` returns
`error: unexpected argument 'bar' found`, confirming round 1's finding 1.
