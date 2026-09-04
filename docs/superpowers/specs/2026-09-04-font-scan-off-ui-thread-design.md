# Font scan off the UI thread design

**Goal:** no font coverage scanning runs on the UI thread on any startup path, a warm launch reaches its first frame with a correct fallback chain, and a cold launch reaches it with a partial one that corrects itself within a second.

**Issue:** [#27](https://github.com/AbysmalBiscuit/alacritree/issues/27), a sub-issue of [#22](https://github.com/AbysmalBiscuit/alacritree/issues/22).

**Branch:** `perf/font-scan-background`, cut from `fix-selecting-text-near-the-left-side-of-the` (PR 206), marker `[5]`. The scan runs on its own thread rather than through `jobs.rs`, so this branch needs neither `perf/nonblocking-ui` nor `perf/pool-accounting` underneath it and stacks on the live stack tip like every other branch.

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

The issue says the background side "needs its own `fontdb::Database` rather than a borrow of the one startup used". `fontdb::Database` is `Send + Sync`, verified by compile check, because its sources are `Arc<dyn AsRef<[u8]> + Sync + Send>`. What is not `Send` is `SystemFonts`, which holds `OnceCell` and `RefCell`. That type is constructed inside the scan thread and never crosses a thread, so an `Arc<Database>` is shared instead of rebuilt, saving a second enumeration and a duplicate copy.

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

Two things follow. The fan-out gets a dedicated thread rather than a pool priority class, which is what wezterm does and costs nothing to copy. And the coverage scan itself is the outlier, which is the subject of "Alternative considered" below.

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

`AlacritreeApp::new` spawns one named thread, `font-scan`, carrying the `Arc<Database>`, the font config, the cache path, and the chain startup installed. The result returns on an `mpsc::Sender` paired with `ctx.request_repaint()`, the pattern `EventProxy` already uses for PTY events, so the swap wakes the egui loop instead of waiting for the next input event.

The thread does four things in order. It runs `scan_coverage` across four scoped threads pulling indices from a shared `AtomicUsize` over `db.faces()`. It writes the refreshed cache. It constructs `SystemFonts::from_scan` and calls `build_font_definitions`. It compares the resulting chain against the one it was handed.

It sends `Option<(FontDefinitions, Vec<ChainFace>)>`, `None` when the chains are equal. `ChainFace` already derives `PartialEq`, so the comparison is direct. A warm launch on a machine whose fonts have not changed does no swap at all.

Building the `FontDefinitions` on the scan thread is what keeps the UI thread's share to a single `set_fonts` call. `trim_by_coverage` and `order_candidates` run over the whole candidate pool four times, once per variant, and `map_font_file` maps every chain face. None of that belongs on the UI thread. `FontDefinitions` is `Send`, since `FontData` holds a `Cow<'static, [u8]>`.

**No priority is set, on any platform.** An earlier draft added a `Priority::Cpu` class to `jobs.rs` with a `THREAD_PRIORITY_BELOW_NORMAL` nudge on Windows. The prior art does not support it: of the six projects surveyed, only ghostty lowers a thread at all, and it does so per long-lived thread on macOS, not per job. The scan is one bounded burst that ends on its own, competing mostly with an idle UI thread waiting for PTY output. Dropping the class also removes the Unix parity question the earlier draft could not answer, since there is now nothing to be at parity about.

**Not the `jobs.rs` pool.** The pool is documented as sized for IO-bound work that spends its time waiting rather than saturating a core, and four CPU-bound shards would take every background slot on a four-worker pool, stalling git status on exactly the low-core machine where the scan is slowest. Sizing around that meant a new priority class, a new ceiling, and a dependency on two branches with no open PR. A dedicated thread for a one-shot, once-per-process job is what wezterm does for the same work, and it costs one `spawn`.

## 3. The swap

`update` drains the channel. On `Some((defs, chain))` the UI thread calls `ctx.set_fonts(defs)`, replaces `self.glyph_cache` with a fresh `GlyphCache`, replaces `self.color_glyphs` with `ColorGlyphCache::new(chain, budget)`, and requests a repaint.

The glyph cache is cleared explicitly rather than left to `AtlasState::outlived_by`. That heuristic notices a rebuilt atlas by its fill ratio dropping, which normally catches a `set_fonts`, but it is evaluated once per frame in `begin_frame`. A swap landing after `begin_frame` in the same frame would leave galleys addressing repacked atlas slots, painting the wrong characters for one frame.

The colour glyph cache is replaced rather than mutated because its chain is fixed at construction, and its per-character claim cache is keyed against that chain.

No request-ordering counter is needed. `install_terminal_fonts` has one call site and runs once per process, and changing the font config already requires a restart, so there is no newer request for a late result to race against. If that ever stops being true, the discard rule from #24 applies here too.

## 4. The scan indicator

An `egui::Area` anchored `Align2::LEFT_TOP` inside the terminal panel, at `Order::Tooltip` and `interactable(false)` so it never takes a click from the grid. A semi-transparent rounded background and one line of text, painted over the top-left cells.

An overlay rather than a sidebar row because either sidebar can be collapsed, and rather than a status strip because a new persistent region would have to be subtracted from the grid's cell fitting for a message that appears rarely.

It appears only if the scan is still running 500 ms after the thread starts, and fades over roughly 200 ms when the swap lands or the thread dies. A scan that finishes inside the threshold shows nothing at all, which on the measurements above is the common case; the overlay exists for the machine where the scan takes seconds, so the tofu has an explanation.

## 5. Configuration

A `[ui]` boolean, `font_scan_notice`, default `true`, gates the overlay. It follows the naming of the other plain boolean options in `RawUi` (`notifications`, `pr_status`, `worktree_liveness`).

The background scan itself is not gated. It is the bugfix, not a feature, and gating it would ship the multi-second UI-thread block as a supported configuration.

Default `true` because the overlay only ever appears on a launch that would otherwise show unexplained tofu. Defaulting it off would ship the confusing case as the default and leave the explanation to a user who does not know the option exists.

The doc comment on the `RawUi` field is the hover text the published JSON Schema carries, so `schema/alacritree-config.json` is regenerated with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.

## Error handling

A cache that is absent, truncated, corrupt, or version-mismatched already makes `disk_cache::load` return `None`. Startup then has an empty candidate pool, which is the cold path, and the background scan repopulates it. No new failure mode.

A cache write that fails is already swallowed after a debug log. The next launch rescans.

A scan thread that panics closes its sender. The UI thread sees the channel disconnect, clears the overlay, and keeps the chain startup installed. A cold launch that hits this keeps a primary-only chain for the session. The panic reaches `crash_log` through the existing hook.

A primary face that cannot be resolved at all is unchanged: `unresolvable_font_definitions` still runs on the UI thread at startup, and the scan result is discarded, because a chain built against a font that does not exist has nothing to install.

## Testing

Correctness of the cache-built chain:

- `SystemFonts::from_cache` against a pinned cache path produces the same candidate set as a full scan of the same faces. The `CacheLocation::Fixed` hook for pinning already exists.
- A face whose recorded size or mtime no longer matches its file is absent from the cache-built pool and present after the scan.
- A cache file that fails its magic or version check yields an empty pool rather than an error.

Correctness of the parallel scan:

- The four-thread scan produces the same `(Candidate, Coverage)` set as the serial one, compared order-independently, since the atomic queue does not preserve face order.

Correctness of the swap:

- A scan whose chain equals the cache-built chain sends `None` and triggers no `set_fonts`.
- The swap path clears the glyph cache, tested by observing that a galley cached before the swap is not returned after it.
- A disconnected channel leaves the startup chain in place and clears the overlay.

The overlay's not-shown path is checked against the allocation-free unchanged-frame assertion in `steady_state.rs`.

## Alternative considered: ask DirectWrite instead of scanning

Windows answers "which font covers this codepoint" itself, through `IDWriteFontFallback::MapCharacters`. wezterm reaches it from Rust with the `dwrote` crate in about a hundred lines (`wezterm-font/src/locator/gdi.rs:260-360`). Every terminal surveyed above uses that API or its platform equivalent, and none of them pays a scan.

alacritree does not, because of egui. `FontDefinitions` fixes the family list before layout and offers no per-glyph hook to consult on a miss, so the chain has to be complete up front, and completeness is what costs 2860 ms. The other terminals own their shaper and can ask at the moment of the miss.

That is a smaller obstacle than it looks. `ctx.set_fonts` can be called at any point, and the grid painter already knows when it draws a cell with no glyph. A wezterm-shaped loop is available: collect missed codepoints during paint, resolve them off-thread with `MapCharacters`, append the faces, call `set_fonts`. That deletes the scan, the 928-face parse, the coverage cache, its binary format and version, and this design's indicator, and replaces them with a query the OS answers while shaping a single run.

Against it: it deletes issue #27 rather than implementing it, replaces the fallback subsystem instead of moving it off the UI thread, adds a dependency, and leaves Unix on the fontconfig path so the two platforms stop sharing a shape. `MapCharacters` is unmeasured here, and Windows Terminal's own comment warns that it is slow per call.

Recommendation: file it as its own issue, measure `MapCharacters` against the 2860 ms scan before committing to it, and ship this design meanwhile. This design is a strict improvement either way, and its cache-first startup path survives into that one unchanged.

## What this does not do

Mid-session font installation. The chain is built once per process. A font installed while alacritree is running is picked up on the next launch. The issue scopes this out and notes the answer may well be no.

Linux and macOS. The scan does not exist there.

Removing the last of the UI thread's font work. The 26 ms fontdb enumeration and the 14 ms stat pass stay, roughly 40 ms against 2860 ms today. Moving them would mean caching the resolved primary face path across launches, which trades a startup cost for a correctness risk on the one face that must be right at the first frame.

## Unresolved questions

1. The DirectWrite alternative. Confirm that it is filed as its own issue and this design ships meanwhile, or stop here and measure `MapCharacters` first.
2. Benchmark noise. Every number above was taken while another session ran mutation tests. Re-run on an idle machine before the plan cites 567 ms as the expected scan span.
3. The option name `font_scan_notice`.
4. The 500 ms appearance threshold and the 200 ms fade are chosen by argument, not measurement. They can only be judged against a real slow launch.
5. Whether four scan threads is right on a four-core machine, where four shards plus the UI thread plus a shell spawn oversubscribe. A fixed four may want to become `min(4, available)`.
