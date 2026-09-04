# Font scan off the UI thread design

**Goal:** no font coverage scanning runs on the UI thread on any startup path, a warm launch reaches its first frame with a correct fallback chain, and a cold launch reaches it with a partial one that corrects itself within a second.

**Issue:** [#27](https://github.com/AbysmalBiscuit/alacritree/issues/27), a sub-issue of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22).

**Branch:** `perf/font-scan-background`, cut from `perf/pool-accounting`. The scan is submitted to the `jobs.rs` pool, so it sits above `perf/nonblocking-ui` (which introduced the pool) and `perf/pool-accounting` (which reworked its ceilings). See the unresolved questions: neither has an open PR today.

**Initiative:** this is one component of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22), which moves work that does not need to be synchronous off the UI thread. That framing decides two things below. The scan goes through the shared pool rather than a thread of its own, because a bespoke thread per moved component is what the pool exists to prevent. And replacing the fallback subsystem outright is out of scope, however attractive: the job here is to move this work, not to delete it.

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

## Prior art

Checked against the versions `docm` resolves for this project: zed `main` (91c57e81470e), wezterm `main` (4fbd6b8e90e2), ghostty `main` (c81f0b26871c), kitty `master` (b14ae3bf21ee), alacritty `master` (d692748d3f61), Windows Terminal `main` (093e49e29a9f).

**Nobody lowers a thread's priority for background work.**

zed's `Priority` is `{RealtimeAudio, High, Medium, Low}` and its own docstring rejects strict ordering, saying the scheduler "may interleave tasks of different priorities to prevent starvation" (`crates/scheduler/src/scheduler.rs:30`). The mechanism is a weighted coin flip, 60/30/10, over the non-empty queues (`crates/gpui/src/queue.rs:255-282`). OS priority is touched in exactly three places, all raising, all for realtime audio: `SetThreadPriority(THREAD_PRIORITY_TIME_CRITICAL)` at `crates/gpui_windows/src/dispatcher.rs:157`, `thread_policy_set` at `crates/gpui_macos/src/dispatcher.rs:81-160`, `pthread_setschedparam(SCHED_FIFO, 65)` at `crates/gpui_linux/src/linux/dispatcher.rs:138-157`. Background priority on Windows is queue admission into the OS thread pool through `TP_CALLBACK_PRIORITY_LOW` (`gpui_windows/src/dispatcher.rs:56-70, 108-119`), and on Linux it is the weighted queue and nothing else (`gpui_linux/src/linux/dispatcher.rs:105-109`).

ghostty does lower, but only on macOS and only on long-lived dedicated threads. `setQosClass` wraps `pthread_set_qos_class_self_np` (`src/os/macos.zig:60`) behind `if (comptime !builtin.target.os.tag.isDarwin()) return;` (`src/renderer/Thread.zig:283`). The renderer swaps class as its window changes, `.utility` when occluded through `.user_interactive` when focused (`src/renderer/Thread.zig:286-303`), and the search thread is pinned at `.utility` for its whole life (`src/terminal/search/Thread.zig:151`). Never per job.

alacritty, kitty, wezterm and Windows Terminal contain no thread-priority calls at all.

alacritree's own `focus_priority` module, landing in PR 202, reached the same shape from the other direction: it raises the focused session, Windows only, and its module docs already state the Linux objection an earlier draft of this design worked out independently, that a nice value is inherited and lowering one back is privileged, which makes per-job switching "the wrong shape for that platform rather than merely unwritten".

**Nobody builds a coverage index over every installed face.**

wezterm resolves fallback lazily, on the miss. Shaping collects the codepoints it could not draw and schedules them (`wezterm-font/src/lib.rs:228-241`); the work runs on one dedicated thread spawned on first need and fed by a plain `channel` (`lib.rs:540-552`); on Windows it is answered by `dwrote::FontFallback::get_system_fallback().map_characters` (`wezterm-font/src/locator/gdi.rs:260-360`). No priority is set on that thread. `enumerate_all_fonts` exists (`gdi.rs:363`) but serves the font-listing command, not fallback.

Windows Terminal calls `IDWriteFontFallback::MapCharacters` per text run at paint time (`src/renderer/atlas/AtlasEngine.cpp:990, 1008`), having taken the system fallback once at startup (`AtlasEngine.cpp:47`). Their comment at `AtlasEngine.cpp:943` calls the API "awfully slow", and their answer is to coalesce consecutive runs that map to the same face, not to precompute an index.

zed builds a fallback object once, reading `GetUnicodeRanges` for the user's configured fallback families only, then appends `GetSystemFontFallback()` so the OS answers everything else (`crates/gpui_windows/src/direct_write.rs:388-444`). ghostty's Windows discovery is a lazy directory walk that returns the first family match and stops, and its `discoverFallback` ignores the codepoint entirely (`src/font/discovery.zig:993-1001`). kitty's `all_fonts_map` is an `lru_cache` computed on first use with no disk cache and no thread (`kitty/fonts/fontconfig.py:53-54`), with fallback through `fc_match` (`fontconfig.py:92-94`).

Two things follow. The scan needs no priority machinery beyond the queue it already sits in, which is what section 2 settles. And the coverage scan itself is the outlier among these projects, which is what "What this does not do" records and defers.

## 1. Startup builds the chain from the cache

`SystemFonts` stops scanning lazily and starts being handed its coverage.

Two constructors replace the `OnceCell` that today calls `scan_coverage`:

- `SystemFonts::from_cache(db: Arc<fontdb::Database>, cache_path: Option<&Path>)` reads `coverage-cache.v1.bin`, stats each face file, and keeps only entries whose recorded size and mtime still match. It builds `(Candidate, Coverage)` for those from `db.faces()` and the cached ranges. It parses no cmap. A face that is missing from the cache, or whose file has changed, is simply absent from the candidate pool.
- `SystemFonts::from_scan(db: Arc<fontdb::Database>, coverage: Vec<(Candidate, Coverage)>)` takes a full scan result.

`install_terminal_fonts` builds the database once, wraps it in an `Arc`, constructs `SystemFonts::from_cache`, and calls the existing `build_font_definitions` unchanged. Both the startup path and the background path go through that one function, differing only in the coverage they were handed, so there are not two chain-building implementations to keep in agreement.

On a cold launch the candidate pool is empty, `gather_fallback_faces` returns nothing, and the chain is the primary face plus the user's `[font] fallback` entries plus egui's bundled faces. Latin and a fair range of symbols still draw. Anything outside them draws as tofu until the swap lands.

`gather_fallback_faces` also asks for the seed face's own coverage, which falls through to `face_coverage`, a direct parse of one file. That parse stays: it is a single face, it is sub-millisecond, and without it a cold launch cannot trim its fallbacks at all.

Startup's blocking cost becomes the 26 ms enumeration, the 14 ms stat pass, and the cache read, against 2860 ms today.

## 2. The background scan

`AlacritreeApp::new` submits **one** job to the `jobs.rs` pool at `Priority::Background`, carrying the `Arc<Database>`, the font config, the cache path, and the chain startup installed. One job, not four: the fan-out happens inside it, so the scan occupies a single pool slot and cannot crowd out git status however many cores it uses.

The job does four things in order. It runs `scan_coverage` across four scoped threads pulling indices from a shared `AtomicUsize` over `db.faces()`. It writes the refreshed cache. It constructs `SystemFonts::from_scan` and calls `build_font_definitions`. It compares the resulting chain against the one it was handed.

The job returns `Option<(FontDefinitions, Vec<ChainFace>)>`, `None` when the chains are equal. `ChainFace` already derives `PartialEq`, so the comparison is direct. A warm launch on a machine whose fonts have not changed does no swap at all.

Building the `FontDefinitions` inside the job is what keeps the UI thread's share to a single `set_fonts` call. `trim_by_coverage` and `order_candidates` run over the whole candidate pool four times, once per variant, and `map_font_file` maps every chain face. None of that belongs on the UI thread. `FontDefinitions` is `Send`, since `FontData` holds a `Cow<'static, [u8]>`.

`scan_coverage` takes a `&Blocking`, so the token that already makes blocking helpers uncallable from `update` covers the scan too. That is what stops a later change from quietly putting it back on the UI thread.

**No new priority class, and no OS priority call.** A dedicated CPU class with its own ceiling and a `THREAD_PRIORITY_BELOW_NORMAL` nudge on Windows was considered and rejected. The ceiling would guard against four shards taking four pool slots, which submitting a single job makes impossible. The OS nudge has no support in the prior art: of the six projects surveyed, only ghostty lowers a thread at all, and it does so per long-lived thread on macOS rather than per job. Leaving it out also means no Unix parity gap to answer for, since there is nothing to be at parity about.

### What this takes from zed, and what it leaves

zed is the reference for the threading work in #22, and the part worth copying is its priority model, not its plumbing.

Worth copying: priority is queue ordering and nothing else, with no OS call anywhere except raising a realtime audio thread; and selection is a weighted draw rather than strict precedence, 60/30/10 across High/Medium/Low, which is how zed keeps low-priority work from starving without needing a per-class ceiling at all (`crates/gpui/src/queue.rs:255-282`). That is about fifteen lines of plain Rust and it would let `jobs.rs` drop its ceilings. It belongs in `perf/pool-accounting`, which owns pool admission, not here; this design only needs the pool to be fair, not to be fair by any particular means.

Left alone: the executor underneath it. zed runs `async_task::Runnable` over `futures`, with `Task<T>` handles, `dispatch_after` timers and a per-platform `PlatformDispatcher`, because it schedules thousands of small futures across a large app. alacritree submits a handful of long, self-contained closures per session and reads their results in `update`. `Job<T>` and a worker pool cover that. Adding an async runtime would buy nothing this app can spend.

## 3. The swap

`update` polls the job. On `Some((defs, chain))` the UI thread calls `ctx.set_fonts(defs)`, replaces `self.glyph_cache` with a fresh `GlyphCache`, replaces `self.color_glyphs` with `ColorGlyphCache::new(chain, budget)`, and requests a repaint.

The glyph cache is cleared explicitly rather than left to `AtlasState::outlived_by`. That heuristic notices a rebuilt atlas by its fill ratio dropping, which normally catches a `set_fonts`, but it is evaluated once per frame in `begin_frame`. A swap landing after `begin_frame` in the same frame would leave galleys addressing repacked atlas slots, painting the wrong characters for one frame.

The colour glyph cache is replaced rather than mutated because its chain is fixed at construction, and its per-character claim cache is keyed against that chain.

No request-ordering counter is needed. `install_terminal_fonts` has one call site and runs once per process, and changing the font config already requires a restart, so there is no newer request for a late result to race against. If that ever stops being true, the discard rule from #24 applies here too.

## 4. The scan indicator

An `egui::Area` anchored `Align2::LEFT_TOP` inside the terminal panel, at `Order::Tooltip` and `interactable(false)` so it never takes a click from the grid. A semi-transparent rounded background and one line of text, painted over the top-left cells.

An overlay rather than a sidebar row because either sidebar can be collapsed, and rather than a status strip because a new persistent region would have to be subtracted from the grid's cell fitting for a message that appears rarely.

It appears only if the job is still running 500 ms after submission, and fades over roughly 200 ms when the swap lands or the job fails. A scan that finishes inside the threshold shows nothing at all, which on the measurements above is the common case; the overlay exists for the machine where the scan takes seconds, so the tofu has an explanation.

## 5. Configuration

A `[ui]` boolean, `font_scan_notice`, default `true`, gates the overlay. It follows the naming of the other plain boolean options in `RawUi` (`notifications`, `pr_status`, `worktree_liveness`).

The background scan itself is not gated. It is the bugfix, not a feature, and gating it would ship the multi-second UI-thread block as a supported configuration.

Default `true` because the overlay only ever appears on a launch that would otherwise show unexplained tofu. Defaulting it off would ship the confusing case as the default and leave the explanation to a user who does not know the option exists.

The doc comment on the `RawUi` field is the hover text the published JSON Schema carries, so `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.

## Error handling

A cache that is absent, truncated, corrupt, or version-mismatched already makes `disk_cache::load` return `None`. Startup then has an empty candidate pool, which is the cold path, and the background scan repopulates it. No new failure mode.

A cache write that fails is already swallowed after a debug log. The next launch rescans.

A scan job that panics is reported by `Job::failed`. The overlay clears, the chain stays as startup installed it, and the pool logs the payload. A cold launch that hits this keeps a primary-only chain for the session.

A primary face that cannot be resolved at all is unchanged: `unresolvable_font_definitions` still runs on the UI thread at startup, and the job's result is discarded, because a chain built against a font that does not exist has nothing to install.

## Testing

Correctness of the cache-built chain:

- `SystemFonts::from_cache` against a pinned cache path produces the same candidate set as a full scan of the same faces. The `CacheLocation::Fixed` hook for pinning already exists.
- A face whose recorded size or mtime no longer matches its file is absent from the cache-built pool and present after the scan.
- A cache file that fails its magic or version check yields an empty pool rather than an error.

Correctness of the parallel scan:

- The four-thread scan produces the same `(Candidate, Coverage)` set as the serial one, compared order-independently, since the atomic queue does not preserve face order.

Correctness of the swap:

- A scan whose chain equals the cache-built chain returns `None` and triggers no `set_fonts`.
- The swap path clears the glyph cache, tested by observing that a galley cached before the swap is not returned after it.
- A failed job leaves the startup chain in place and clears the overlay.

The overlay's not-shown path is checked against the allocation-free unchanged-frame assertion in `steady_state.rs`.

## What this does not do

Replace the coverage scan with DirectWrite. Windows answers "which font covers this codepoint" itself through `IDWriteFontFallback::MapCharacters`, reachable from Rust via `dwrote`, and it is what wezterm, zed and Windows Terminal all use instead of an index. alacritree cannot ask on demand today because egui fixes `FontDefinitions` before layout with no per-glyph hook, though a paint-time miss could be collected and resolved the way wezterm does. That is a different piece of work: it replaces the fallback subsystem rather than moving it off the UI thread, so it is a separate issue and not part of #22. Worth filing; not worth blocking this on.

Mid-session font installation. The chain is built once per process. A font installed while alacritree is running is picked up on the next launch. The issue scopes this out and notes the answer may well be no.

Linux and macOS. The scan does not exist there.

Removing the last of the UI thread's font work. The 26 ms fontdb enumeration and the 14 ms stat pass stay, roughly 40 ms against 2860 ms today. Moving them would mean caching the resolved primary face path across launches, which trades a startup cost for a correctness risk on the one face that must be right at the first frame.

## Unresolved questions

1. Base branch. This depends on `jobs.rs`, so it must sit above `perf/nonblocking-ui` and `perf/pool-accounting`. Neither has an open PR, while the open stack (202 through 206) descends from `master` without them. Either those two open first, or this branch cuts from `perf/pool-accounting` and waits for them.
2. Whether `perf/pool-accounting` adopts zed's weighted draw in place of per-class ceilings. Not this branch's call, but it is the one part of zed's model that transfers directly, and deciding it there keeps the pool from growing a ceiling per component as #22 proceeds.
3. Benchmark noise. Every number above was taken while another session ran mutation tests. Re-run on an idle machine before the plan cites 567 ms as the expected scan span.
4. The option name `font_scan_notice`.
5. The 500 ms appearance threshold and the 200 ms fade are chosen by argument, not measurement. They can only be judged against a real slow launch.
6. Whether four scan threads is right on a four-core machine, where four shards plus the UI thread plus a shell spawn oversubscribe. A fixed four may want to become `min(4, available)`.
