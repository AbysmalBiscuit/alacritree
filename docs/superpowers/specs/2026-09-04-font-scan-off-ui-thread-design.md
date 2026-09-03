# Font scan off the UI thread design

**Goal:** no font coverage scanning runs on the UI thread on any startup path, a warm launch reaches its first frame with a correct fallback chain, and a cold launch reaches it with a partial one that corrects itself within a second.

**Issue:** [#27](https://github.com/AbysmalBiscuit/alacritree/issues/27), a sub-issue of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22).

**Branch:** `perf/font-scan-background`, cut from `perf/pool-accounting`. This design adds a priority class to `jobs.rs`, so it needs both `perf/nonblocking-ui` (which introduced the pool) and `perf/pool-accounting` (which reworked its ceilings) underneath it. See the unresolved questions: neither has an open PR today.

**Platform:** Windows only. Every part of the coverage scan is `#[cfg(not(unix))]`. Linux and macOS get their fallback chain from fontconfig's `FcFontSort`, backed by the system's own `fc-cache`, so there is nothing for alacritree to scan or persist there.

## Context

`AlacritreeApp::new` calls `fonts::install_terminal_fonts` before the first frame. That call reaches `SystemFonts::scanned_coverage`, which parses every system face's cmap to learn which codepoints it covers, and the window does not exist until it returns. The issue records a cold scan of 2860 ms for 928 faces.

`scan_coverage` already persists per-face codepoint ranges to `coverage-cache.v1.bin` under `%LOCALAPPDATA%\alacritree`, keyed by each font file's size and mtime, so a warm launch reuses them and skips the parses. It still runs on the UI thread, still stats every face file, and still writes the cache back before the first frame.

### Measurements

Taken on a 16-core Windows 11 machine with 928 system faces, using a standalone probe against `fontdb` 0.23 and `ttf-parser` 0.25. A mutation-test session was loading the machine throughout, so the absolute numbers are pessimistic. The ordering held across every round.

| Step | Cost |
| --- | --- |
| `fontdb::Database::load_system_fonts` | 23 to 26 ms |
| stat pass over all 928 face files | 14 ms |
| serial cmap coverage over all 928 faces | 1559 to 1870 ms |
| the same scan, 16 threads, atomic index queue | 209 to 245 ms |
| the same scan, 4 threads, atomic index queue | median 567 ms, spread 528 to 740 |
| the same scan, 4 threads, rayon | median 593 ms, spread 579 to 643 |

Three things follow.

The fontdb enumeration is cheap. At 26 ms it can stay on the UI thread, which matters because the primary family resolves through `resolve_via_fontdb` and so needs the database before the first frame regardless.

The stat pass is cheap. At 14 ms it can stay on the UI thread too, which answers the issue's second open question: startup spends it and catches a font file that changed since the last launch, rather than trusting the cache outright.

Rayon is not worth a dependency. It ties the dependency-free atomic queue on time and beats it only on variance (a 64 ms spread against 212 ms), which is its work-stealing smoothing over scheduler noise. Both beat static chunking, confirming that per-face costs are uneven enough that static partitioning leaves the slowest chunk setting the wall clock. The design uses an atomic index queue over scoped threads.

### Two corrections to the issue

The issue says the background side "needs its own `fontdb::Database` rather than a borrow of the one startup used". `fontdb::Database` is `Send + Sync`, verified by compile check, because its sources are `Arc<dyn AsRef<[u8]> + Sync + Send>`. What is not `Send` is `SystemFonts`, which holds `OnceCell` and `RefCell`. That type is constructed inside the job and never crosses a thread, so an `Arc<Database>` is shared instead of rebuilt, saving a second enumeration and a duplicate copy.

The issue says "the cache answers startup". It cannot answer it alone. The cache stores only `path -> {size, mtime, face_index -> ranges}`, while a `coverage::Candidate` also needs family, weight, italic, monospaced and file size, all of which come from `db.faces()`. The cache supplies the coverage; fontdb supplies the identity.

## 1. Startup builds the chain from the cache

`SystemFonts` stops scanning lazily and starts being handed its coverage.

Two constructors replace the `OnceCell` that today calls `scan_coverage`:

- `SystemFonts::from_cache(db: Arc<fontdb::Database>, cache_path: Option<&Path>)` reads `coverage-cache.v1.bin`, stats each face file, and keeps only entries whose recorded size and mtime still match. It builds `(Candidate, Coverage)` for those from `db.faces()` and the cached ranges. It parses no cmap. A face that is missing from the cache, or whose file has changed, is simply absent from the candidate pool.
- `SystemFonts::from_scan(db: Arc<fontdb::Database>, coverage: Vec<(Candidate, Coverage)>)` takes a full scan result.

`install_terminal_fonts` builds the database once, wraps it in an `Arc`, constructs `SystemFonts::from_cache`, and calls the existing `build_font_definitions` unchanged. Both the startup path and the background path go through that one function, differing only in the coverage they were handed, so there are not two chain-building implementations to keep in agreement.

On a cold launch the candidate pool is empty, `gather_fallback_faces` returns nothing, and the chain is the primary face plus the user's `[font] fallback` entries plus egui's bundled faces. Latin and a fair range of symbols still draw. Anything outside them draws as tofu until the swap lands.

`gather_fallback_faces` also asks for the seed face's own coverage, which falls through to `face_coverage`, a direct parse of one file. That parse stays: it is a single face, it is sub-millisecond, and without it a cold launch cannot trim its fallbacks at all.

Startup's blocking cost becomes the 26 ms enumeration, the 14 ms stat pass, and the cache read, against 2860 ms today.

## 2. The background job

`AlacritreeApp::new` submits one job at `Priority::Cpu`, carrying the `Arc<Database>`, the font config, the cache path, and the chain startup installed.

The job does four things in order. It runs `scan_coverage` across four scoped threads pulling indices from a shared `AtomicUsize` over `db.faces()`, each thread calling `jobs::lower_this_thread` for itself, because a new Windows thread starts at normal priority rather than inheriting its creator's. It writes the refreshed cache. It constructs `SystemFonts::from_scan` and calls `build_font_definitions`. It compares the resulting chain against the one it was handed.

The job returns `Option<(FontDefinitions, Vec<ChainFace>)>`, `None` when the chains are equal. `ChainFace` already derives `PartialEq`, so the comparison is direct. A warm launch on a machine whose fonts have not changed does no swap at all.

Building the `FontDefinitions` inside the job is what keeps the UI thread's share to a single `set_fonts` call. `trim_by_coverage` and `order_candidates` run over the whole candidate pool four times, once per variant, and `map_font_file` maps every chain face. None of that belongs on the UI thread. `FontDefinitions` is `Send`, since `FontData` holds a `Cow<'static, [u8]>`.

## 3. The swap

`update` polls the job. On `Some((defs, chain))` the UI thread calls `ctx.set_fonts(defs)`, replaces `self.glyph_cache` with a fresh `GlyphCache`, replaces `self.color_glyphs` with `ColorGlyphCache::new(chain, budget)`, and requests a repaint.

The glyph cache is cleared explicitly rather than left to `AtlasState::outlived_by`. That heuristic notices a rebuilt atlas by its fill ratio dropping, which normally catches a `set_fonts`, but it is evaluated once per frame in `begin_frame`. A swap landing after `begin_frame` in the same frame would leave galleys addressing repacked atlas slots, painting the wrong characters for one frame.

The colour glyph cache is replaced rather than mutated because its chain is fixed at construction, and its per-character claim cache is keyed against that chain.

No request-ordering counter is needed. `install_terminal_fonts` has one call site and runs once per process, and changing the font config already requires a restart, so there is no newer request for a late result to race against. If that ever stops being true, the discard rule from #24 applies here too.

## 4. Priority::Cpu

`jobs::Priority` gains a `Cpu` variant ranked between `Interactive` and `Background`.

`State` gains a `cpu` queue and a `cpu_running` count. `take()` drains `Interactive` first, then `Cpu`, then `Background`, and gates each class by its own ceiling, so the classes cannot starve each other. Font scanning wins a contested worker over a git walk, because a wrong glyph in the grid is more noticeable than a git panel that fills in a beat later, while git status keeps a reserved slot rather than waiting out the whole scan.

The class exists because the pool is documented as "sized for IO-bound work, subprocesses and git walks that spend their time waiting, not saturating a core". The coverage scan is the opposite shape. Without a separate class, four CPU-saturating shards would take every background slot on a four-worker pool and stall every other background job, on exactly the low-core machine where the scan is slowest.

`lower_this_thread` treats `Cpu` like `Background` on Windows: `THREAD_PRIORITY_BELOW_NORMAL`, so the UI thread outranks both. It becomes `pub(crate)` so the scan's own fan-out threads can call it, which is the smallest change that works; a fan-out helper owned by `jobs` would be a nicer boundary but has exactly one caller.

`scan_coverage` takes a `&Blocking`, so the token that already makes blocking helpers uncallable from `update` covers the scan too. That is what stops a later change from quietly putting it back on the UI thread.

The variant is added here rather than in `perf/pool-accounting` because this is its first and only consumer. The design call and the evidence for it belong in one diff.

### What Cpu means on Linux and macOS

The queue ordering in `take()` is plain Rust and behaves identically everywhere. The scheduling nudge does not.

`lower_this_thread` is `#[cfg(windows)]` and an empty function on every other target, so `Priority::Background` is already ordering-only on Unix. This branch does not change that, and the scan being `cfg(not(unix))` means `Priority::Cpu` has no caller on Linux or macOS at all.

Parity is uneven work. macOS has a near-exact analogue in `pthread_set_qos_class_self_np` with `QOS_CLASS_UTILITY`: per-thread, self-applied, unprivileged, reversible, and covering I/O and timer coalescing as Windows' background mode does. Linux does not fit the model. `nice` is per-thread there, a deliberate deviation from POSIX, but lowering yourself is free while raising back needs `CAP_SYS_NICE` or an `RLIMIT_NICE` allowance. A worker that drops to background for one job and cannot climb back is stuck there for every job after it, which breaks the per-job class the pool comment describes. `SCHED_IDLE` may be reversible unprivileged on modern kernels; that is unverified. Linux most likely needs a dedicated low-priority worker set rather than a per-job switch.

So Unix parity is a pool redesign belonging with the other pool issues, not behind them. This branch leaves the gap where it already is and files a separate issue covering `Background` and `Cpu` together. This is an assumption, recorded in the unresolved questions.

## 5. The scan indicator

An `egui::Area` anchored `Align2::LEFT_TOP` inside the terminal panel, at `Order::Tooltip` and `interactable(false)` so it never takes a click from the grid. A semi-transparent rounded background and one line of text, painted over the top-left cells.

An overlay rather than a sidebar row because either sidebar can be collapsed, and rather than a status strip because a new persistent region would have to be subtracted from the grid's cell fitting for a message that appears rarely.

It appears only if the job is still running 500 ms after submission, and fades over roughly 200 ms when the swap lands or the job fails. A scan that finishes inside the threshold shows nothing at all, which on the measurements above is the common case; the overlay exists for the machine where the scan takes seconds, so the tofu has an explanation.

## 6. Configuration

A `[ui]` boolean, `font_scan_notice`, default `true`, gates the overlay. It follows the naming of the other plain boolean options in `RawUi` (`notifications`, `pr_status`, `worktree_liveness`).

The background scan itself is not gated. It is the bugfix, not a feature, and gating it would ship the multi-second UI-thread block as a supported configuration.

Default `true` because the overlay only ever appears on a launch that would otherwise show unexplained tofu. Defaulting it off would ship the confusing case as the default and leave the explanation to a user who does not know the option exists.

The doc comment on the `RawUi` field is the hover text the published JSON Schema carries, so `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.

## Error handling

A cache that is absent, truncated, corrupt, or version-mismatched already makes `disk_cache::load` return `None`. Startup then has an empty candidate pool, which is the cold path, and the background scan repopulates it. No new failure mode.

A cache write that fails is already swallowed after a debug log. The next launch rescans.

A scan job that panics is reported by `Job::failed`. The overlay clears, the chain stays as startup installed it, and the pool logs the payload. A cold launch that hits this keeps a primary-only chain for the session.

A primary face that cannot be resolved at all is unchanged: `unresolvable_font_definitions` still runs on the UI thread at startup, and the background job's result is discarded, because a chain built against a font that does not exist has nothing to install.

## Testing

Correctness of the cache-built chain:

- `SystemFonts::from_cache` against a pinned cache path produces the same candidate set as a full scan of the same faces. The `CacheLocation::Fixed` hook for pinning already exists.
- A face whose recorded size or mtime no longer matches its file is absent from the cache-built pool and present after the scan.
- A cache file that fails its magic or version check yields an empty pool rather than an error.

Correctness of the parallel scan:

- The four-thread scan produces the same `(Candidate, Coverage)` set as the serial one, compared order-independently, since the atomic queue does not preserve face order.

Correctness of the pool change:

- With `Interactive`, `Cpu` and `Background` tasks all queued, `take()` returns them in that order.
- Enough queued `Cpu` tasks cannot exhaust the `Background` ceiling, and the reverse.

Correctness of the swap:

- A scan whose chain equals the cache-built chain returns `None` and triggers no `set_fonts`.
- The swap path clears the glyph cache, tested by observing that a galley cached before the swap is not returned after it.

The overlay's not-shown path is checked against the allocation-free unchanged-frame assertion in `steady_state.rs`.

## What this does not do

Mid-session font installation. The chain is built once per process. A font installed while alacritree is running is picked up on the next launch. The issue scopes this out and notes the answer may well be no.

Linux and macOS. The scan does not exist there.

Removing the last of the UI thread's font work. The 26 ms fontdb enumeration and the 14 ms stat pass stay, roughly 40 ms against 2860 ms today. Moving them would mean caching the resolved primary face path across launches, which trades a startup cost for a correctness risk on the one face that must be right at the first frame.

## Unresolved questions

1. Unix thread priority scope. The design assumes the gap stays and gets its own issue. Confirm, or pull macOS parity into this branch.
2. Base branch. This depends on `Priority::Cpu` in `jobs.rs`, so it must sit above `perf/nonblocking-ui` and `perf/pool-accounting`. Neither has an open PR, while the open stack (202 through 206) descends from `master` without them. Either those two open first, or this branch cuts from `perf/pool-accounting` and waits for them.
3. Benchmark noise. Every number above was taken while another session ran mutation tests. Re-run on an idle machine before the plan cites 567 ms as the expected scan span.
4. The option name `font_scan_notice`.
5. The 500 ms appearance threshold and the 200 ms fade are chosen by argument, not measurement. They can only be judged against a real slow launch.
6. Whether four scan threads is right on a four-core machine, where four shards plus the UI thread plus a shell spawn oversubscribe. A fixed four may want to become `min(4, available)`.
