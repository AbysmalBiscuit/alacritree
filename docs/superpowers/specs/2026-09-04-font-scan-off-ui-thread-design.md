# Font scan off the UI thread design

**Goal:** no font coverage scanning runs on the UI thread on any startup path, a warm launch reaches its first frame with a correct fallback chain, and a cold launch reaches it with a partial one that corrects itself within a second.

**Issue:** [#27](https://github.com/AbysmalBiscuit/alacritree/issues/27), a sub-issue of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22).

**Branch:** `perf/font-scan-background`, cut from `perf/pool-accounting`. The scan is submitted to the `jobs.rs` pool and adds a helper to it, so it sits above `perf/nonblocking-ui` (which introduced the pool) and `perf/pool-accounting` (which reworked its ceilings). See the unresolved questions: neither has an open PR today.

**Initiative:** this is one component of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22), which moves work that does not need to be synchronous off the UI thread. That framing decides three things below. The scan goes through the shared pool rather than a thread of its own, because a bespoke thread per moved component is what the pool exists to prevent. Its fan-out becomes a pool primitive rather than a local idiom, because it is the pool's first CPU-parallel job and will not be the last. And replacing the fallback subsystem outright is out of scope, however attractive: the job here is to move this work, not to delete it.

**Platform:** Windows only. Every part of the coverage scan is `#[cfg(not(unix))]`. Linux and macOS get their fallback chain from fontconfig's `FcFontSort`, backed by the system's own `fc-cache`, so there is nothing for alacritree to scan or persist there.

## Context

`AlacritreeApp::new` calls `fonts::install_terminal_fonts` before the first frame. That call reaches `SystemFonts::scanned_coverage`, which parses every system face's cmap to learn which codepoints it covers. The window exists by then, since `new` is handed a live `CreationContext`, but it has not painted and will not until the call returns. The issue records a cold scan of 2860 ms for 928 faces.

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

The stat pass is cheap. At 14 ms it can stay on the UI thread too, which answers the issue's second open question: startup spends it and catches a font file that changed since the last launch, rather than trusting the cache outright. The 14 ms figure is from a warm filesystem cache and is unmeasured after a reboot, so this is the one deliberate exception to the initiative's rule, and unresolved question 4 asks whether it survives measurement.

Rayon is not worth a dependency. It ties the dependency-free atomic queue on time and beats it only on variance (a 64 ms spread against 212 ms), which is its work-stealing smoothing over scheduler noise. Both beat static chunking, confirming that per-face costs are uneven enough that static partitioning leaves the slowest chunk setting the wall clock. The fan-out uses an atomic index queue.

### Two corrections to the issue

The issue says the background side "needs its own `fontdb::Database` rather than a borrow of the one startup used". `fontdb::Database` is `Send + Sync`, verified by compile check, because its sources are `Arc<dyn AsRef<[u8]> + Sync + Send>`, and `with_face_data` takes `&self`, so the fan-out threads can share one. What is not `Send` is `SystemFonts`, which holds `OnceCell` and `RefCell`. That type is constructed inside the job and never crosses a thread, so an `Arc<Database>` is shared instead of rebuilt, saving a second enumeration and a duplicate copy.

The issue says "the cache answers startup". It cannot answer it alone. The cache stores only `path -> {size, mtime, face_index -> ranges}`, while a `coverage::Candidate` also needs family, weight, italic, monospaced and file size, all of which come from `db.faces()`. The cache supplies the coverage; fontdb supplies the identity.

## Prior art

Checked against the versions `docm` resolves for this project: zed `main` (91c57e81470e), wezterm `main` (4fbd6b8e90e2), ghostty `main` (c81f0b26871c), kitty `master` (b14ae3bf21ee), alacritty `master` (d692748d3f61), Windows Terminal `main` (093e49e29a9f).

**Of the terminals and editors surveyed, only ghostty lowers a thread's priority, and never per job.**

zed's `Priority` is `{RealtimeAudio, High, Medium, Low}` and its own docstring rejects strict ordering, saying the scheduler "may interleave tasks of different priorities to prevent starvation" (`crates/scheduler/src/scheduler.rs:23-27`). The mechanism is a weighted coin flip, 60/30/10, over the non-empty queues (`crates/gpui/src/queue.rs:255-282`). OS priority is touched in exactly three places, all raising, all for realtime audio: `SetThreadPriority(THREAD_PRIORITY_TIME_CRITICAL)` at `crates/gpui_windows/src/dispatcher.rs:157`, `thread_policy_set` at `crates/gpui_macos/src/dispatcher.rs:96, 114, 151`, `pthread_setschedparam(SCHED_FIFO, 65)` at `crates/gpui_linux/src/linux/dispatcher.rs:138-157`. Background priority on Windows is queue admission into the OS thread pool through `TP_CALLBACK_PRIORITY_LOW` (`gpui_windows/src/dispatcher.rs:107-118`), and on Linux it is the weighted queue and nothing else (`gpui_linux/src/linux/dispatcher.rs:107-111`).

ghostty does lower, but only on macOS and only on long-lived dedicated threads. `setQosClass` wraps `pthread_set_qos_class_self_np` (`src/os/macos.zig:60`) behind `if (comptime !builtin.target.os.tag.isDarwin()) return;` (`src/renderer/Thread.zig:267`). The renderer swaps class as its window changes, `.utility` when occluded through `.user_interactive` when focused (`src/renderer/Thread.zig:286-303`), and the search thread is pinned at `.utility` for its whole life (`src/terminal/search/Thread.zig:151`). Never per job.

alacritty, kitty, wezterm and Windows Terminal contain no thread-priority calls at all.

alacritree is the outlier here, deliberately and in both directions. `focus_priority`, landing in PR 202, raises the focused session's job object, Windows only, and its module docs already state the Linux objection: a nice value is inherited and lowering one back is privileged, which makes per-job switching "the wrong shape for that platform rather than merely unwritten". And `jobs.rs` lowers every background worker to `THREAD_PRIORITY_BELOW_NORMAL` for the duration of its job (`jobs.rs:373, 399` on `perf/pool-accounting`). Section 2 keeps both. The survey is why this design adds no *new* priority class, not a case for unwinding what the pool already does.

**Nobody builds a coverage index over every installed face.**

wezterm resolves fallback lazily, on the miss. Shaping collects the codepoints it could not draw and schedules them (`wezterm-font/src/lib.rs:228-241`); the work runs on one dedicated thread spawned on first need and fed by a plain `channel` (`lib.rs:540-552`); on Windows it is answered by `dwrote::FontFallback::get_system_fallback().map_characters` (`wezterm-font/src/locator/gdi.rs:260-360`). No priority is set on that thread. `enumerate_all_fonts` exists (`gdi.rs:363`) but serves the font-listing command, not fallback.

Windows Terminal calls `IDWriteFontFallback::MapCharacters` per text run at paint time (`src/renderer/atlas/AtlasEngine.cpp:990, 1008`), having taken the system fallback once at startup (`AtlasEngine.cpp:47`). Their comment at `AtlasEngine.cpp:943` calls the API "awfully slow", and their answer is to coalesce consecutive runs that map to the same face, not to precompute an index.

zed builds a fallback object once, reading `GetUnicodeRanges` for the user's configured fallback families only, then appends `GetSystemFontFallback()` so the OS answers everything else (`crates/gpui_windows/src/direct_write.rs:398-441`). ghostty's Windows discovery is a lazy directory walk that returns the first family match and stops, and its `discoverFallback` ignores the codepoint entirely (`src/font/discovery.zig:993-1001`). kitty's `all_fonts_map` is an `lru_cache` computed on first use with no disk cache and no thread (`kitty/fonts/fontconfig.py:53-54`), with fallback through `fc_match` (`fontconfig.py:92-94`).

The coverage scan is therefore the outlier among these projects, which is what "What this does not do" records and defers.

## 1. Startup builds the chain from the cache

`SystemFonts` stops scanning lazily and starts being handed its coverage.

Two constructors replace the `OnceCell` that today calls `scan_coverage`:

- `SystemFonts::from_cache(cache_path: Option<&Path>)` reads `coverage-cache.v1.bin`, stats each face file, and keeps only entries whose recorded size and mtime still match. It builds `(Candidate, Coverage)` for those from `db.faces()` and the cached ranges. It parses no cmap. A face that is missing from the cache, or whose file has changed, is simply absent from the candidate pool.
- `SystemFonts::from_scan(coverage: Vec<(Candidate, Coverage)>)` takes a full scan result.

Both are `#[cfg(not(unix))]`, and so is the `Arc<fontdb::Database>` they populate. On Unix the `db` field stays the `OnceCell` it is today, because fontconfig answers fallback there and only the `[ui.font]` gate and the no-fontconfig fallback ever touch the database. Making the `Arc` unconditional would make every Linux and macOS launch enumerate system fonts for nothing.

`install_terminal_fonts` builds the database once on Windows, wraps it in an `Arc`, constructs `SystemFonts::from_cache`, and calls the existing `build_font_definitions` unchanged. Both the startup path and the background path go through that one function, differing only in the coverage they were handed, so there are not two chain-building implementations to keep in agreement.

On a cold launch the candidate pool is empty, `gather_fallback_faces` returns nothing, and the chain is the primary face plus the user's `[font] fallback` entries plus egui's bundled faces. Latin and a fair range of symbols still draw. Anything outside them draws as tofu until the swap lands.

`gather_fallback_faces` also asks for the seed face's own coverage, which falls through to `face_coverage`, a direct parse of one file. That parse stays: it is a single face, and without it a cold launch cannot trim its fallbacks at all.

Startup's blocking cost becomes the 26 ms enumeration, the 14 ms stat pass, the cache read, and `build_font_definitions` itself, against 2860 ms today. `build_font_definitions` is not free: it runs `order_candidates` and `trim_by_coverage` once per terminal variant and once per UI family, four to eight passes over the candidate pool, and per chain face it calls `map_font_file`, `epaint_can_parse` and `is_color_only`, the last with up to 64 outline probes. On a cold launch the pool is empty and those passes are trivial; on a warm one they are not, and unresolved question 3 asks for the measurement.

## 2. The background scan

`AlacritreeApp::new` submits **one** job to the `jobs.rs` pool, carrying the `Arc<Database>`, the font config, the cache path, and the candidate pool startup built. One job, not four: the fan-out happens inside it, so the scan occupies a single pool slot and cannot crowd out git status however many cores it uses.

Priority follows what the user can see. A cold launch, where the cache-built pool came back empty, submits at `Priority::Interactive`, because the section 4 overlay is a pending state and the grid is showing tofu until the job lands, which is what the pool's own comment reserves `Interactive` for. A warm launch submits at `Priority::Background`: nothing is visibly wrong, and the refresh is housekeeping.

The job does four things in order. It runs `scan_coverage` through `Blocking::parallel`. It writes the refreshed cache. It constructs `SystemFonts::from_scan` and calls `build_font_definitions`. It compares its candidate pool against the one it was handed.

The job returns `Option<(FontDefinitions, Vec<ChainFace>)>`, `None` when the pools are equal.

**The comparison is over candidate pools, not chains.** `FallbackBook::extend_chain` records a face only for `Variant::Normal` and returns early for every other variant (`fonts.rs:444-447`), so bold, italic, bold-italic and the two `[ui.font]` chains never enter `book.chain`. Comparing `Vec<ChainFace>` would report equality whenever a newly scanned face changes only a bold or UI chain, and a warm launch that follows installing a bold-only font would keep its stale chain for the whole session. `Candidate` and `Coverage` both derive `PartialEq`, so the job sorts both pools and compares them directly.

Building the `FontDefinitions` inside the job is what keeps the UI thread's share to a single `set_fonts` call. `FontDefinitions` is `Send`, since `FontData` holds a `Cow<'static, [u8]>`.

`scan_coverage` takes a `&Blocking`, so the token that already makes blocking helpers uncallable from `update` covers the scan too. That is what stops a later change from quietly putting it back on the UI thread.

**No new priority class.** A dedicated CPU class with its own ceiling was considered and rejected: the ceiling would guard against four shards taking four pool slots, which submitting a single job makes impossible. The pool's existing `Interactive`/`Background` split carries this job.

### Blocking::parallel

The pool gains one method, and this branch is what adds it:

```rust
impl Blocking {
    /// Run `f` over `0..len` across helper threads, and stop early when the
    /// owning job is cancelled.
    fn parallel(&self, len: usize, want: usize, f: impl Fn(usize) + Sync);
}
```

It spawns `min(want, len, available_parallelism())` scoped helper threads over a shared `AtomicUsize`, and checks `self.cancelled()` between items so a quit during a cold launch stops the scan instead of finishing it and writing a cache after the window closed. `Job::drop` already sets that flag; nothing reads it inside a running scan today.

Each helper calls `lower_this_thread` for itself with the owning job's slot. This is the part that has to be in `jobs.rs` rather than in `fonts.rs`: Windows starts a new thread at `THREAD_PRIORITY_NORMAL` regardless of its creator, so helper threads spawned inside a `Background` job escape the `THREAD_PRIORITY_BELOW_NORMAL` the pool applied at `jobs.rs:373`. Hand-rolled scoped threads would leave the pool's only lowered thread the one blocked in `thread::scope` while every CPU-bound helper ran at normal priority. `Blocking` therefore carries its slot.

The helper owns the thread count, which is why it takes `want` as a request rather than a mandate. Four is right on this machine and oversubscribes a four-core one, where four helpers compete with the UI thread and a shell spawn; clamping in one place means the next CPU-parallel job in #22 inherits the policy instead of re-deciding it.

### What this takes from zed, and what it leaves

zed is the reference for the threading work in #22, and the part worth copying is its priority model, not its plumbing.

Worth copying, beyond what this branch needs: selection as a weighted draw rather than strict precedence, 60/30/10 across the classes, which is how zed keeps low-priority work from starving without a per-class ceiling at all (`crates/gpui/src/queue.rs:255-282`); and `RunnableMeta { location, spawned }` (`crates/scheduler/src/scheduler.rs:63-75`), which stamps every task with its spawn site and time, and would give `Job` both the "which job is hogging a worker" log line and the submit timestamp section 4 needs. Both belong in `perf/pool-accounting`, which owns pool admission, not here.

Left alone: the executor underneath it. zed runs `async_task::Runnable` over `futures`, with `Task<T>` handles, `dispatch_after` timers and a per-platform `PlatformDispatcher`, because it schedules thousands of small futures across a large app. alacritree submits a handful of long, self-contained closures per session and reads their results in `update`. `Job<T>` and a worker pool cover that. Adding an async runtime would buy nothing this app can spend.

## 3. The swap

`update` polls the job before `self.glyph_cache.begin_frame(ctx)`, at the top of the frame. On `Some((defs, chain))` it calls `ctx.set_fonts(defs)`, calls `glyph_cache.fonts_changed()`, and stores `chain` as pending.

The one-frame delay is egui's, not a choice. `ctx.set_fonts` only stores the definitions; the next `begin_pass` applies them and builds a fresh `FontsImpl` whose atlas starts at height 32. So the frame that receives the result still paints with the old fonts, and the frame after it paints with the new ones. The pending chain replaces `self.color_glyphs` with `ColorGlyphCache::new(chain, budget)` at the start of that next frame, so the colour glyphs and the outlines never disagree about which chain is live.

`fonts_changed` sets a flag that `begin_frame` clears the cache on, unconditionally, on its next call. Replacing the `GlyphCache` outright is wrong: a fresh cache has `atlas: None`, the receiving frame fills it with galleys addressed against the old atlas, and the following `begin_frame` finds no atlas to compare and so does not clear, painting those galleys against the repacked one. `AtlasState::outlived_by` would in fact catch this case on its own, since the rebuilt atlas differs in both `image_size` and `fill_ratio`, but that is a heuristic reading fill ratios, and a font swap is a fact the code already knows.

The colour glyph cache is replaced rather than mutated because its chain is fixed at construction, and its per-character claim cache is keyed against that chain.

No request-ordering counter is needed. `install_terminal_fonts` has one call site and runs once per process, and changing the font config already requires a restart, so there is no newer request for a late result to race against. If that ever stops being true, the discard rule from #24 applies here too.

## 4. The scan indicator

An `egui::Area` anchored `Align2::LEFT_TOP` inside the terminal panel, at `Order::Tooltip` and `interactable(false)` so it never takes a click from the grid. A semi-transparent rounded background and one line of text, painted over the top-left cells.

An overlay rather than a sidebar row because either sidebar can be collapsed, and rather than a status strip because a new persistent region would have to be subtracted from the grid's cell fitting for a message that appears rarely.

It appears only if the job is still running 500 ms after submission, and fades over roughly 200 ms when the swap lands or the job fails. A scan that finishes inside the threshold shows nothing at all, which on the measurements above is the common case; the overlay exists for the machine where the scan takes seconds, so the tofu has an explanation.

Both timings need the frame clock, which an idle egui app does not run. The pool's waker fires at job end and nothing else, so the submitting frame calls `request_repaint_after(500 ms)` to wake the frame that decides whether to show the overlay, and each frame of the fade requests the next. Without those the overlay would appear only if something else happened to repaint.

The overlay's decision is a pure function of the submit instant, the current instant and the job's state, kept out of the egui code so it can be tested directly.

## 5. Configuration

A `[ui]` boolean, `font_scan_notice`, default `true`, gates the overlay. It follows the naming of the other plain boolean options in `RawUi` (`notifications`, `pr_status`, `worktree_liveness`).

The background scan itself is not gated. It is the bugfix, not a feature, and gating it would ship the multi-second UI-thread block as a supported configuration.

Default `true` because the overlay only ever appears on a launch that would otherwise show unexplained tofu. Defaulting it off would ship the confusing case as the default and leave the explanation to a user who does not know the option exists.

The doc comment on the `RawUi` field is the hover text the published JSON Schema carries, so `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.

## Error handling

A cache that is absent, truncated, corrupt, or version-mismatched already makes `disk_cache::load` return `None`. Startup then has an empty candidate pool, which is the cold path, and the background scan repopulates it. No new failure mode.

A cache write that fails is already swallowed after a debug log. The next launch rescans.

A scan job that panics is reported by `Job::failed`. A panic inside a `Blocking::parallel` helper unwinds through the scope into the pool's existing `catch_unwind`, so it latches the same way. The overlay clears, the chain stays as startup installed it, and the pool logs the payload. A cold launch that hits this keeps a primary-only chain for the session.

A job cancelled by quit stops between faces and writes no cache.

A primary face that cannot be resolved at all is unchanged: `unresolvable_font_definitions` still runs on the UI thread at startup, and the job's result is discarded, because a chain built against a font that does not exist has nothing to install.

## Testing

Correctness of the cache-built chain:

- `SystemFonts::from_cache` against a pinned cache path produces the same candidate set as a full scan of the same faces. The `CacheLocation::Fixed` hook for pinning already exists.
- A face whose recorded size or mtime no longer matches its file is absent from the cache-built pool and present after the scan.
- A cache file that fails its magic or version check yields an empty pool rather than an error.
- An empty candidate pool still yields the primary face, the user's fallbacks and the bundled faces, without `order_candidates` or `trim_by_coverage` panicking on nothing.
- A cold startup parses at most the seed faces, asserted through the existing `FACE_COVERAGE_PARSES` counter (`fonts.rs:1155`), which is what proves acceptance criterion 1 rather than a timing.

Correctness of the parallel scan:

- `Blocking::parallel` produces the same `(Candidate, Coverage)` set as a serial scan, compared order-independently, since the atomic queue does not preserve face order.
- A cancelled job stops before visiting every index.
- A panic in one helper surfaces as `Job::failed`.

Correctness of the swap:

- A scan whose candidate pool differs from the cache-built one **only in a face that no normal-variant chain selects** still swaps. This is the regression test for comparing pools rather than chains; comparing `Vec<ChainFace>` fails it.
- A scan whose pool equals the cache-built pool returns `None` and triggers no `set_fonts`.
- The glyph cache is cleared across the swap, tested over two frames the way `a_repacked_atlas_discards_the_cached_galleys` already does (`glyph_cache.rs:283-305`): swap in one frame, run the next, and assert the served galley's atlas UV matches a fresh layout rather than the pre-swap one. A single-frame assertion passes trivially against a fresh cache and would not catch the stale-galley case.
- The colour glyph cache is replaced in the same frame the new fonts take effect, not the frame the result arrives.
- A failed job leaves the startup chain in place and clears the overlay.

Correctness of the overlay model:

- Below the threshold it never appears; above it, it appears and then fades. Tested against the pure model, since `steady_state.rs` asserts on the `sidebar_focus` reconcile rather than on `update` and covers nothing here.

Existing tests to migrate: five Windows tests build `SystemFonts::with_cache_dir(None)` and rely on the pool populating itself lazily (`fonts.rs:1878, 1894, 1935, 2366, 2395`). Under `from_cache`/`from_scan` they need an explicit scan, which is the largest mechanical part of the change.

## What this does not do

Replace the coverage scan with DirectWrite. Windows answers "which font covers this codepoint" itself through `IDWriteFontFallback::MapCharacters`, reachable from Rust via `dwrote`, and it is what wezterm, zed and Windows Terminal all use instead of an index. alacritree cannot ask on demand today because egui fixes `FontDefinitions` before layout with no per-glyph hook, though a paint-time miss could be collected and resolved the way wezterm does. That is a different piece of work: it replaces the fallback subsystem rather than moving it off the UI thread, so it is a separate issue and not part of #22. Worth filing; not worth blocking this on.

Mid-session font installation. The chain is built once per process. A font installed while alacritree is running is picked up on the next launch. The issue scopes this out and notes the answer may well be no.

Linux and macOS. The scan does not exist there.

Removing the last of the UI thread's font work. The enumeration, the stat pass and `build_font_definitions` stay. Moving the enumeration would mean caching the resolved primary face path across launches, which trades a startup cost for a correctness risk on the one face that must be right at the first frame.

## Unresolved questions

1. Base branch. This depends on `jobs.rs` and extends it, so it must sit above `perf/nonblocking-ui` and `perf/pool-accounting`. Neither has an open PR, while the open stack (202 through 206) descends from `master` without them. Either those two open first, or this branch cuts from `perf/pool-accounting` and waits for them.
2. Whether `perf/pool-accounting` adopts zed's weighted draw in place of per-class ceilings, and `RunnableMeta`'s spawn site and timestamp on `Job`. Not this branch's call, but section 4 wants the timestamp, and deciding the draw there keeps the pool from growing a ceiling per component as #22 proceeds.
3. Startup cost after the change is projected, not measured. Measure the enumeration, the stat pass, the cache read and `build_font_definitions` together on a warm launch before the plan claims a number.
4. Whether the UI-thread stat pass earns its exception. Without it a changed face carries stale coverage for under a second and the swap corrects it, `epaint_can_parse` still keeps an unreadable file out of egui, and `scan_coverage` stats everything again anyway. The 14 ms is warm-cache and unmeasured cold, in the one place `tools/ui-thread-audit.py` cannot see.
5. Whether `ctx.set_fonts` is as cheap as section 3 assumes. egui compares the new definitions against the current ones before queueing, and `FontData` derives `PartialEq`, so a partial-to-full swap may memcmp mapped font files until the first difference. Measure before relying on it.
6. Benchmark noise. Every number above was taken while another session ran mutation tests. Re-run on an idle machine before the plan cites 567 ms as the expected scan span.
7. The option name `font_scan_notice`.
8. The 500 ms appearance threshold and the 200 ms fade are chosen by argument, not measurement. They can only be judged against a real slow launch.
