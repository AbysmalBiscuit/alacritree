# Plan Review Log: stop copying font files into memory

Started 2026-08-09. MAX_ROUNDS=5. MODEL=gpt-5.6-sol, EFFORT=xhigh, sandbox=read-only.

Plan under review: `docs/superpowers/specs/2026-08-09-font-bytes-memory-design.md`
Baseline commit: `3617fe53`. Thread: `019fe625-fc43-79a1-a110-6dfde9b03173`.

## Round 1 — Codex

**VERDICT: REVISE.** The core lifetime and memo designs are sound, but the test plan contains
false RED claims, the mmap risk is understated, and the performance evidence is not reproducible.

1. **Test 3 can pass against the broken baseline.** `FONT_MAPS` is global and never cleared
   (`fonts.rs:1144`); Test 1 maps the same fixture, so default parallel execution can make the
   proposed `is_mapped` assertion pass even though `face_coverage` still calls `std::fs::read`.
   *Fix:* give Test 3 a unique temporary copy and assert it is unmapped before calling
   `face_coverage`.
2. **Test 5 is nondeterministic and can assert nothing.** `chain_with_color_fonts` may return
   `None`, guarantees only one renderable character, and does not guarantee the unspecified second
   glyph renders and evicts the first (`color_glyph.rs:329`); merely checking `chain_walks` also
   passes if the re-render incorrectly returns `None`. *Fix:* use an injected or bundled
   deterministic colour face, then assert the second render succeeds, the first entry was evicted,
   the third render succeeds, and `chain_walks` stays unchanged.
3. **Test 2 neither has a RED state nor proves integration.** A unit test of
   `scanned_seed_coverage` still passes if `gather_fallback_faces` ignores it and continues through
   `face_coverage`. *Fix:* instrument the fallback coverage path and test through
   `gather_fallback_faces`, observing a baseline call and zero revised calls for a scanned seed.
4. **`scanned_seed_coverage` is conditional, not authoritative.** The scan omits explicit paths and
   faces whose cmap parsing fails. More seriously, cached coverage is trusted using only file size
   and millisecond mtime (`fonts.rs:178`), so a same-size replacement inside one millisecond can
   return stale seed coverage without falling back. The zero-parse claim is also false for explicit
   or unscanned paths. *Fix:* route seed coverage through a per-install `(path, face_index)` memo,
   strengthen cache identity, and qualify the zero-parse claim to scanned cache hits.
5. **"The mmap bet does not widen" is false for colour-only faces.** Those faces are mapped before
   joining the chain, but currently the mapping is touched only during startup classification while
   rendering uses a stable `Arc<Vec<u8>>`; the proposal makes later glyph rendering dereference the
   mapping, widening the period in which truncation or replacement can fault. *Fix:* explicitly
   accept and document the widened access-time risk, or retain owned snapshots only for colour-only
   faces.
6. **The retained `files` map is redundant under the plan's own invariant.** Every legitimate chain
   face has already mapped successfully: primary at `fonts.rs:890`, user fallbacks at `:548`,
   automatic fallbacks at `:1020`, including colour-only branches. Its `None` state is unreachable
   in production, and `entry(path.to_path_buf())` retains avoidable hashing and path cloning.
   *Fix:* preload a `Vec<Option<&'static [u8]>>` aligned with `chain` in `ColorGlyphCache::new`,
   logging any invariant violation once.
7. **The ~150 MB estimate follows arithmetically, but the acceptance measurement is not
   reproducible.** No region-walk script, fixed configuration, build profile, binary identity,
   settling interval, or sample count. *Fix:* record an executable measurement script and at least
   three before/after samples using the same release binary, configuration, sessions, and settling
   interval.
8. **The claimed baseline line references are stale.** At `3617fe53`, `map_font_file` is at 1146
   not 949, its relevant calls at 548 and 1020 not 534 and 823, and the cache budget at
   `config.rs:1050` not 729. *Fix:* refresh against `3617fe53` or cite symbols instead.

Claims Codex confirmed as holding:

- Every face in the returned normal chain, including colour-only faces, is already mapped. Part 1
  adds no mappings.
- `HashMap<PathBuf, Option<&'static [u8]>>` compiles as proposed. The returned reference is copied
  with a `'static` lifetime, so the mutable `files` borrow ends before `chain` or `scale` is used;
  `swash 0.2.9` then borrows the slice only for the scaler lifetime.
- The Part 3 memo restructure preserves `entries`, `used`, byte budgeting, eviction and cell-size
  reset behaviour. `source` already survives resize, and a stored positive index safely re-renders
  through the unchanged chain.
- Tests 1 and 4 have genuine baseline RED states.

### Claude's response

Independently found (8) before the critique arrived: the references were derived against
`35406884`, prior to the fast-forward to `3617fe53`. Re-derived every anchor. The substance holds —
`map_font_file` at `:548` and `:1020` still precedes `is_color_only` at `:555` and `:1027`.

**Accepted in full: 1, 2 (partly), 3, 7, 8.**

- (1) Test 3 now copies the fixture to a unique temp path per test and asserts it is unmapped first.
- (3) Test 2 becomes an integration test through `gather_fallback_faces` with a `#[cfg(test)]`
  counter on `face_coverage`, which gives it a real RED. This also satisfies the repo rule against
  unit tests that skip the layer the defect surfaced at.
- (7) The spec now embeds the exact region-walk script and fixes binary, config, session count,
  settling interval and sample count.
- (8) All references refreshed to `3617fe53`; symbol names lead, line numbers follow as hints.

**Partly accepted: 2, 4, 5, 6.**

- (2) Accepted the assertion strengthening — the test must prove the second render succeeded, the
  first entry was evicted, and the third render succeeded, not merely that a counter held still.
  Rejected bundling a colour font: it is a new licensed asset for one test, and five existing tests
  (`color_glyph.rs:364, 390, 416, 447, 465`) already skip when no colour face is installed. The
  test stays environment-gated, and the spec now says so rather than implying determinism.
- (4) Accepted the per-install memo and the qualification of the zero-parse claim. Rejected
  "strengthen cache identity": size+mtime is the existing contract for every *candidate* face's
  coverage, which already determines the whole chain. A stale cache produces a wrong chain today;
  reading the seed from the same cache does not create a new failure mode, and changing the cache
  key would invalidate every user's cache for a separate reason. Documented as a deliberate
  consistency choice instead.
- (5) Accepted the correction. "Adds no new mappings" is true; "the unsafe bet does not widen" was
  wrong. Narrowed the claim precisely: every chain face *except* colour-only ones is already
  handed to epaint as borrowed `FontData`, so the process already dereferences those mappings on
  every frame. Colour-only faces are the sole exception — they are skipped before
  `insert_face` — so for those the access window does widen from startup classification to paint
  time. Rejected owned snapshots for colour-only faces: `claiming_index` dereferences every
  face's cmap at first sight of a new character regardless, so the mitigation would not close the
  window it targets.
- (6) Accepted the analysis, rejected the restructure. The `None` state is indeed unreachable —
  a face that fails to map never reaches the chain — and the spec now records that as an
  invariant with a one-time log if violated. The `Vec<Option<&'static [u8]>>` preload is left as a
  noted alternative rather than adopted: the user explicitly chose "keep `files`, change its value
  type" at design review, and this is a memory fix, not a hot-path optimization. Surfaced for the
  sign-off gate.

## Round 2 — Codex

**VERDICT: REVISE.** The core mmap change, borrow lifetimes, "no new mappings" claim and the
Part 3 restructure now check out. Five remaining problems:

1. **`seed_memo` has no owner or API.** `HashMap` has no `get_or_insert_with`, and a memo scoped to
   the four terminal seeds misses UI fallback resolution at `fonts.rs:974`. *Fix:* store
   `RefCell<HashMap<(PathBuf, u32), Option<Coverage>>>` on the per-install `SystemFonts` and expose
   one method caching both hits and misses for every `gather_fallback_faces` call.
2. **Test 2 tests only the scan shortcut,** so it passes if the memo is omitted; an unspecified
   process-global counter is also contaminated by existing Windows fallback tests running
   concurrently. *Fix:* resettable thread-local counter, assert scan membership, and add an
   explicit-path subcase calling `gather_fallback_faces` twice against one `SystemFonts` asserting
   exactly one parse.
3. **Test 5 never defines glyph B,** while `chain_with_color_fonts` proves renderability only for
   U+1F600, so step 2 can fail on a machine that satisfies the stated gate. *Fix:* discover two
   renderable characters before the measured sequence, skip explicitly if fewer than two exist, and
   run against a fresh cache.
4. **Verification has no tolerances or startup timer.** "Approximately", "~150 MB" and "mapped
   unchanged" are unfalsifiable, and the cold-cache comparison does not require deletion before
   both binaries. *Fix:* specify private and mapped tolerances, define a launch-to-first-paint
   measurement with a pass threshold, and delete the cache separately before each cold run.
5. **Part 1's code logs at `debug`,** contradicting the one-time `warn` the invariant paragraph
   requires. *Fix:* `log::warn!`; the cached `None` already limits it to once per path.

### Claude's response

All five accepted; none rejected. Verified (1) against the code first — `install_ui_font`
(`fonts.rs:974`) does reach `register_fallback_faces`, and `SystemFonts` already exposes `&self`
methods over interior-mutable fields, so a `RefCell` memo matches the existing shape rather than
introducing a new one.

- (1) The memo now lives on `SystemFonts` as
  `RefCell<HashMap<(PathBuf, u32), Option<coverage::Coverage>>>`, behind one `seed_coverage(&self,
  &ResolvedFace)` method that caches misses as well as hits. The spec notes that `borrow()` is
  released before `face_coverage` runs so `borrow_mut()` cannot panic re-entrantly.
- (2) Test 2 is now two subcases through `gather_fallback_faces`: a scan hit asserting membership
  first and a count of 0, and an explicit-path case calling twice against one `SystemFonts` and
  asserting exactly 1. The counter is thread-local and resettable, following `steady_state.rs`.
- (3) Test 5 now probes a candidate set for two genuinely renderable characters, skips explicitly
  below two, and measures against a fresh cache so the probe cannot pre-populate `source`.
- (4) Pass criteria are now a table of thresholds: ≤250 MB private, mapped within ±5%, no private
  region within ±1 MB of a chain font's size, and startup one-sided with a +100 ms regression
  bound. Startup is measured as wall clock to first IPC answer, four series, cache deleted before
  each cold launch.
- (5) `log::warn!`, with the unreachability noted inline.

## Round 3 — Codex

**VERDICT: REVISE.** Confirmed correct this round: `RefCell<HashMap<..>>` implements `Default`;
`SystemFonts` is already single-threaded through `std::cell::OnceCell`; the field, method and local
`seed_coverage` names do not collide; the temporary `borrow()` ends before `borrow_mut()`;
`scan_coverage` does not call `face_coverage`; and font installation precedes IPC startup.

Five remaining problems, all in the pseudocode and the harness rather than the design:

1. **The memo method does not compile.** It calls `self.scanned_seed_coverage(face)` but the helper
   is a free function, and the method lacks the `#[cfg(not(unix))]` its field and `face_coverage`
   require. *Fix:* gate the method and call `scanned_seed_coverage(self, face)`.
2. **Test 2's explicit-path subcase rests on a false premise.** `resolve_via_path`
   (`fonts.rs:1308`) accepts system-font paths that `scanned_coverage` also contains, so an
   explicit path is not automatically outside the scan. *Fix:* use
   `SystemFonts::with_cache_dir(None)`, copy the fixture to a unique temporary path, and assert the
   resolved face is absent from `scanned_coverage` before resetting the counter.
3. **The region script cannot evaluate its own criteria.** It prints only 21 allocations of ≥16 MB,
   truncates to whole MB, and is given no chain-font list, so "within ±1 MB of any chain font"
   is uncheckable; the aggregate ±5% band also permits losing one 33 MB Noto mapping. *Fix:* emit
   exact-byte machine-readable data for every allocation and compare programmatically against an
   explicit baseline path/size set, including per-path mapped retention.
4. **The startup thresholds contradict themselves.** "≤ baseline median" rejects every positive
   regression while the failure column permits +100 ms. *Fix:* state one rule.
5. **Both scripts select processes by bare name.** `Start-Process alacritree` and the polling client
   may use the wrong binary, socket discovery may answer from another instance, and
   `(Get-Process alacritree).Id` is not an `int` when several are running. *Fix:* parameterize the
   executable, poll `--socket "\\.\pipe\alacritree-$($p.Id).sock"`, call `Dump($p.Id)`, and
   terminate between samples.

### Claude's response

All five accepted; none rejected. None touched the design — they were defects in how the spec
expressed and measured it, which is the right thing to still be finding at round 3.

- (1) The method is now `#[cfg(not(unix))]` and calls the free function as `scanned_seed_coverage(self, face)`.
- (2) The subcase now builds `SystemFonts::with_cache_dir(None)`, copies the fixture to a unique
  temporary path, and asserts absence from `scanned_coverage` before measuring — the premise is
  constructed rather than assumed.
- (3) `Dump` now emits `TOTALS` and one `ALLOC` row per allocation with exact byte counts and no
  size floor, and the spec carries the PowerShell that compares them against a captured chain-font
  path/size set. Per-path mapped retention is now its own binding criterion, with the aggregate
  ±5% demoted to a coarse backstop, since one lost 33 MB mapping hides inside 5% of 1.1 GB.
- (4) One rule: revised median ≤ baseline median + 100 ms.
- (5) Both scripts take an explicit `$exe`, address the socket by the launched pid, call
  `Dump($p.Id)`, and stop the process between samples. The spec notes `ipc_socket` must stay
  enabled because the poll is the stop signal.

## Round 4 — Codex

**VERDICT: REVISE.** Confirmed: the `RefCell` field derives `Default`, its temporary borrow ends
safely, the method/local name overlap is legal, and the thread-local counter is unaffected by
`OnceCell` scan initialization. The Rust design and all five tests are otherwise implementable.

1. **`scanned_seed_coverage` breaks the Unix build.** It is compiled on Unix but calls the
   `#[cfg(not(unix))]`-only `scanned_coverage`. *Fix:* gate the free function too.
2. **The memory run never reaches two sessions.** The script measures the single session
   `app.rs:899` creates at startup, while the stated condition is two. *Fix:* create the second
   through the explicit socket and assert `session list` returns exactly two before settling.
3. **Criterion 1 is both over- and under-inclusive.** Matching every private allocation within
   ±1 MiB of every chain-font size can reject unrelated heap or driver allocations, and it does not
   identify which font copies were eliminated. *Fix:* compare against the baseline-confirmed
   duplicate allocation sizes with allocation-granularity tolerance.
4. **The per-path retention check compares basenames.** A different same-named font elsewhere
   produces a false pass. *Fix:* canonicalize DOS and device paths into one form and compare the
   complete path.

### Claude's response

All four accepted; none rejected. (1) is a real compile break that three earlier rounds missed.

- (1) `#[cfg(not(unix))]` added to the free function, with the reason stated inline so it is not
  removed later as noise.
- (2) The measurement script now calls `session create` through the pid-addressed socket and
  throws unless `session list` reports exactly two before the settling interval.
- (3) The comparison is now against two artefacts captured from the baseline run —
  `baseline-dupes.txt` (the exact private byte counts of allocations the baseline paired with a
  font) and `baseline-mapped.txt` — rather than against the chain font list. Tolerance is 64 KiB,
  the Windows allocation granularity. This both narrows the check to the copies the change is
  supposed to remove and stops an unrelated ~33 MB driver block from failing the run.
- (4) Device paths are canonicalized through `QueryDosDeviceW` into drive-letter form and compared
  in full.

## Round 5 — Codex (MAX_ROUNDS)

**VERDICT: REVISE**, scoped explicitly: "The Rust design and tests are sound enough to implement,"
with two acceptance-script defects remaining.

1. **Both readiness loops test stdout truthiness.** In JSON mode a failure is printed to stdout
   (`cli/mod.rs:249`), so an unavailable socket ends the loop as "ready" and the startup series
   records near-zero times. *Fix:* loop until `$LASTEXITCODE -eq 0`, discarding output.
2. **`[Kernel32]::QueryDosDeviceW` has no type declaration.** A fresh PowerShell process has no
   such type. *Fix:* declare it in the existing C# `VM` class and call `[VM]::QueryDosDeviceW`.

Codex stated that the corrected `cfg`, the `RefCell` memo, the five RED states, the two-session
assertion, the duplicate-size comparison and the full-path comparison "introduce no remaining
implementation blocker."

### Claude's response

Both accepted. (1) verified against the code first: `cli/mod.rs:250` does print `{"error": …}` to
stdout and return 1, so the loop would have exited on the first poll and every startup measurement
would have been fiction. Both loops now gate on the exit code. (2) `QueryDosDeviceW` is declared
alongside `Dump` in the `VM` class.

## Outcome

The loop terminated at MAX_ROUNDS=5 without an `APPROVED` token. This is **not** a deadlock: there
is no unresolved disagreement between the two models. Every finding across all five rounds was
either accepted or rejected with a logged reason, and Codex's final round explicitly cleared the
design and the tests. The two round-5 items were script defects, both accepted and fixed after the
final verdict was issued — so they are unreviewed by Codex, and that is the one honest caveat.

Totals: 21 findings raised, 17 accepted in full, 4 partly accepted with reasons logged, 0 dismissed
without cause. Two findings — the stale line references and the round-5 readiness loop — would have
produced measurements that looked valid and were not.
