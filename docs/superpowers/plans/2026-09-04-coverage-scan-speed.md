# Coverage scan speed implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** make the Windows font coverage scan roughly ten times faster while producing byte-identical coverage, by building ranges during the cmap walk instead of sorting every codepoint, and by scanning faces in parallel with rayon.

**Architecture:** Two independent changes inside `alacritree/src/fonts.rs`. `Coverage` gains a constructor that folds an ascending codepoint walk straight into ranges, and `cmap_coverage` uses it instead of collecting 23.6M codepoints and sorting them. Separately, `scan_coverage` splits into a serial stat pass, a `rayon` parallel phase over faces, and a serial accumulation pass that builds the disk cache exactly as the current loop does.

**Tech Stack:** Rust edition 2024, `fontdb` 0.23, `ttf-parser` 0.25, `rayon` 1 (new, Windows-only). Tests run with `cargo nextest run -p alacritree`.

**Spec:** `docs/superpowers/specs/2026-09-04-coverage-scan-speed-design.md`

## Global Constraints

- Every file touched is `alacritree/src/fonts.rs` and `alacritree/Cargo.toml`. Do not modify the `alacritty*` crates; they are vendored and read-only.
- All code in this plan is inside `#[cfg(not(unix))]` regions or the platform-neutral `mod coverage`. Never widen a `cfg`.
- The scan's output must stay identical. Every task's tests exist to prove that; a task that changes coverage content has failed.
- Comments explain *why*, never restate *what*. No issue or PR references in comments, no change-relative phrasing ("now we", "previously"), no TDD narration ("makes the test pass").
- Markdown and comments use straight quotes and no em dashes.
- Commit messages follow Conventional Commits, imperative mood, subject under 72 characters, and carry the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Run tests with `cargo nextest run -p alacritree`, not `cargo test`.
- `rustfmt` is enforced. Run `cargo fmt` before every commit.

---

## File structure

| File | Responsibility | Change |
| --- | --- | --- |
| `alacritree/Cargo.toml` | Dependency manifest | Add `rayon` under a `cfg(not(unix))` target block |
| `alacritree/src/fonts.rs`, `mod coverage` | Platform-neutral coverage sets | Add `Coverage::from_ascending_walk` |
| `alacritree/src/fonts.rs`, `cmap_coverage` | One face's cmap to a `Coverage` | Use the new constructor |
| `alacritree/src/fonts.rs`, `scan_coverage` | All faces to candidate list plus disk cache | Split into stat, parallel, accumulate |

Everything lands in one file because that is where this logic already lives, and `mod coverage` is already the seam between the platform-neutral set algebra and the Windows scan that drives it. No new files.

---

### Task 1: `Coverage::from_ascending_walk`

The constructor that folds an ascending codepoint walk into ranges without a sort. Lives inside `mod coverage`, which is platform-neutral and already unit-tested on every platform, so this task's tests run everywhere.

**Files:**
- Modify: `alacritree/src/fonts.rs` (inside `mod coverage`, after `from_codepoints` at :2459)
- Test: `alacritree/src/fonts.rs` (the `mod tests` inside `mod coverage`, alongside `merge_coalesces_overlapping_and_adjacent_ranges` at :2719)

**Interfaces:**
- Consumes: `Coverage::from_codepoints(Vec<u32>) -> Self`, `Coverage::ranges(&self) -> &[(u32, u32)]`, both existing.
- Produces: `Coverage::from_ascending_walk(walk: impl Fn(&mut dyn FnMut(u32))) -> Coverage`. Task 2 calls this.

- [ ] **Step 1: Write the failing tests**

Add to the test module inside `mod coverage`, next to the existing `merge_*` tests:

```rust
#[test]
fn ascending_walk_folds_runs_into_ranges() {
    let cov = Coverage::from_ascending_walk(|emit| {
        for cp in [1u32, 2, 3, 10, 11, 50] {
            emit(cp);
        }
    });
    assert_eq!(cov.ranges(), &[(1, 3), (10, 11), (50, 50)]);
}

#[test]
fn ascending_walk_matches_from_codepoints() {
    let cps: Vec<u32> = (0..500).chain(1000..1200).chain([9000, 9001, 65535]).collect();
    let folded = Coverage::from_ascending_walk(|emit| {
        for &cp in &cps {
            emit(cp);
        }
    });
    assert_eq!(folded, Coverage::from_codepoints(cps));
}

#[test]
fn ascending_walk_falls_back_when_a_codepoint_repeats() {
    // A repeat is not a regression, but folding it blindly would push
    // (5, 5) after a range already ending at 5 and produce an overlap.
    let cov = Coverage::from_ascending_walk(|emit| {
        for cp in [1u32, 2, 5, 5, 6] {
            emit(cp);
        }
    });
    assert_eq!(cov.ranges(), &[(1, 2), (5, 6)]);
}

#[test]
fn ascending_walk_falls_back_when_the_walk_goes_backwards() {
    let cov = Coverage::from_ascending_walk(|emit| {
        for cp in [10u32, 11, 3, 4] {
            emit(cp);
        }
    });
    assert_eq!(cov.ranges(), &[(3, 4), (10, 11)]);
}

#[test]
fn ascending_walk_handles_a_codepoint_at_the_top_of_the_range() {
    // `end + 1` would overflow here; the fold compares the other way around.
    let cov = Coverage::from_ascending_walk(|emit| {
        emit(u32::MAX - 1);
        emit(u32::MAX);
    });
    assert_eq!(cov.ranges(), &[(u32::MAX - 1, u32::MAX)]);
}

#[test]
fn ascending_walk_over_nothing_is_empty() {
    let cov = Coverage::from_ascending_walk(|_emit| {});
    assert_eq!(cov, Coverage::default());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree -E 'test(ascending_walk)'`

Expected: FAIL to compile, with `no function or associated item named `from_ascending_walk` found for struct `Coverage``.

- [ ] **Step 3: Write the implementation**

Add inside `impl Coverage`, directly after `from_codepoints`:

```rust
/// Build from a walk that emits codepoints in ascending order, folding
/// them into ranges as they arrive rather than sorting them afterwards.
///
/// cmap subtables are required to enumerate ascending, so this is the
/// normal path.  A walk that turns out not to be ascending is re-run
/// through `from_codepoints`, which is why `walk` is `Fn`: `codepoints`
/// has no early exit, so the first pass has to finish before the second
/// can start.
pub fn from_ascending_walk(walk: impl Fn(&mut dyn FnMut(u32))) -> Self {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut ascending = true;
    walk(&mut |cp| {
        match ranges.last_mut() {
            Some((_, end)) if cp.checked_sub(1) == Some(*end) => *end = cp,
            // Equality counts: a repeat would otherwise push a range that
            // overlaps the one before it.  Rejecting `cp <= end` is also
            // what keeps `end` the maximum seen so far, which is what
            // makes this check total.
            Some((_, end)) if cp <= *end => ascending = false,
            _ => ranges.push((cp, cp)),
        }
    });
    if ascending {
        return Self { ranges };
    }
    let mut codepoints = Vec::new();
    walk(&mut |cp| codepoints.push(cp));
    Self::from_codepoints(codepoints)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree -E 'test(ascending_walk)'`

Expected: PASS, 6 tests.

- [ ] **Step 5: Run the whole suite and format**

Run: `cargo fmt && cargo nextest run -p alacritree`

Expected: PASS. Nothing else calls the new constructor yet, so no existing test can change behaviour.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/fonts.rs
git commit -m "feat(fonts): fold ascending codepoint walks into ranges

Coverage::from_codepoints sorts every codepoint a face reports, which
across a full system font set is a sort over tens of millions of
elements.  cmap subtables are required to enumerate ascending, so the
ranges can be built during the walk instead.

The fold rejects a codepoint less than or equal to the last range's end,
not merely a smaller one, so a repeated codepoint cannot produce an
overlapping range.  A walk that is not ascending falls back to the sort.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `cmap_coverage` uses the new constructor

Swap the collect-and-sort body for the fold. This is the change that produces the speedup; the equivalence test is what proves it changed nothing else.

**Files:**
- Modify: `alacritree/src/fonts.rs:1138-1148` (`cmap_coverage`)
- Test: `alacritree/src/fonts.rs` (the `#[cfg(not(unix))]` test region, near `face_coverage_maps_the_file_instead_of_reading_it` at :2420)

**Interfaces:**
- Consumes: `Coverage::from_ascending_walk` from Task 1, `Coverage::merge(&mut self, &Coverage)` at :2491, existing.
- Produces: no signature change. `cmap_coverage(face: &ttf_parser::Face) -> Option<coverage::Coverage>` keeps its shape, so Task 3 sees the same function.

- [ ] **Step 1: Write the failing test**

The test needs an oracle, because the function it checks is the one being replaced. Add a private copy of today's body to the test module and compare against it over the real system font set.

Add to the `#[cfg(not(unix))]` test region:

```rust
/// Today's collect-and-sort body, kept so the fold has something to be
/// equivalent to.  If this and `cmap_coverage` ever disagree, the fold is
/// wrong, not this.
#[cfg(not(unix))]
fn cmap_coverage_by_sorting(face: &ttf_parser::Face) -> Option<coverage::Coverage> {
    let cmap = face.tables().cmap?;
    let mut codepoints = Vec::new();
    for subtable in cmap.subtables {
        if !subtable.is_unicode() {
            continue;
        }
        subtable.codepoints(|cp| codepoints.push(cp));
    }
    Some(coverage::Coverage::from_codepoints(codepoints))
}

#[cfg(not(unix))]
#[test]
fn cmap_coverage_matches_the_sorting_implementation_on_every_system_face() {
    let fonts = SystemFonts::with_cache_dir(None);
    let db = fonts.db();
    let mut compared = 0usize;
    for face in db.faces() {
        let both = db.with_face_data(face.id, |data, index| {
            let parsed = ttf_parser::Face::parse(data, index).ok()?;
            Some((cmap_coverage(&parsed), cmap_coverage_by_sorting(&parsed)))
        });
        let Some(Some((folded, sorted))) = both else {
            continue;
        };
        assert_eq!(folded, sorted, "coverage differs for {:?}", face.source);
        compared += 1;
    }
    assert!(compared > 0, "no system faces were parsed, so this proved nothing");
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo nextest run -p alacritree -E 'test(cmap_coverage_matches)'`

Expected: PASS. Both functions exist after Step 1 and both still hold the collect-and-sort body, so they agree trivially.

This is the one test in the plan that does not go red first, and that is deliberate. The behaviour it guards is "unchanged", so it cannot fail against a bug that has not been introduced yet. It is a characterization test: passing here establishes the baseline, and a failure after Step 3 means the fold changed coverage. Do not treat the green in Step 4 as evidence the rewrite happened; check the diff for that.

- [ ] **Step 3: Rewrite `cmap_coverage`**

Replace the body at `fonts.rs:1138-1148`:

```rust
#[cfg(not(unix))]
fn cmap_coverage(face: &ttf_parser::Face) -> Option<coverage::Coverage> {
    let cmap = face.tables().cmap?;
    // A font's BMP and full subtables overlap heavily, so the per-subtable
    // sets are unioned rather than concatenated.  `merge` coalesces
    // overlapping and adjacent ranges, which concatenation would not.
    let mut covered = coverage::Coverage::default();
    for subtable in cmap.subtables {
        if !subtable.is_unicode() {
            continue;
        }
        let one = coverage::Coverage::from_ascending_walk(|emit| subtable.codepoints(emit));
        covered.merge(&one);
    }
    Some(covered)
}
```

`Coverage::default()` is the empty set, so a face whose cmap holds no unicode subtable still returns `Some`, as before. Returning `None` there would log the face unparseable and drop it from the scan.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p alacritree -E 'test(cmap_coverage_matches)'`

Expected: PASS, and the `compared > 0` assertion confirms real faces were checked. On a machine with few fonts installed this proves less, but it never passes vacuously.

- [ ] **Step 5: Run the whole suite**

Run: `cargo fmt && cargo nextest run -p alacritree`

Expected: PASS. `coverage_cache_round_trips_across_scans` and `coverage_cache_corruption_falls_back_to_full_rescan` both exercise `scan_coverage` end to end and would catch a coverage change.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/fonts.rs
git commit -m "perf(fonts): build cmap coverage without sorting codepoints

Collecting every codepoint of every unicode subtable and sorting the
union dominated the startup font scan.  Folding each subtable's
ascending walk into ranges and unioning the per-subtable sets produces
the same coverage for about a quarter of the cost.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Add rayon, Windows only

An isolated dependency change so the next task's diff is logic only, and so a broken manifest is one revert rather than a bisect.

**Files:**
- Modify: `alacritree/Cargo.toml` (after the `[target.'cfg(windows)'.dependencies]` block at :83)

**Interfaces:**
- Produces: `rayon` available to `#[cfg(not(unix))]` code in `alacritree`. Task 4 uses `rayon::prelude::*` and `rayon::ThreadPoolBuilder`.

- [ ] **Step 1: Add the dependency**

Append a new target block to `alacritree/Cargo.toml`. Put it after the existing `[target.'cfg(windows)'.dependencies]` block and its entries:

```toml
# The coverage scan is the only parallel work in this crate and it is
# `cfg(not(unix))`, so the gate matches the code rather than the platform
# name: a target that is neither unix nor windows would otherwise compile
# the scan without its executor.
[target.'cfg(not(unix))'.dependencies]
rayon = "1"
```

- [ ] **Step 2: Verify it resolves and nothing else moved**

Run: `cargo check -p alacritree`

Expected: succeeds. `Cargo.lock` gains five packages — `rayon`, `rayon-core`, `either`, `crossbeam-deque`, `crossbeam-epoch`. `crossbeam-utils` is already in the lock at 0.8.21 and every new dependent accepts it, so it does not move.

- [ ] **Step 3: Confirm the Unix build does not pull it**

Run: `cargo tree -p alacritree --target x86_64-unknown-linux-gnu -i rayon`

Expected: `warning: nothing to print.` If rayon appears in a tree instead, the target gate is wrong.

- [ ] **Step 4: Commit**

```bash
git add alacritree/Cargo.toml Cargo.lock
git commit -m "build(fonts): add rayon for the windows coverage scan

Gated on cfg(not(unix)) to match the gate on the scan itself, so a
Linux or macOS build compiles none of it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Parallelise `scan_coverage`

Split the single loop into a serial stat pass, a rayon parallel phase, and a serial accumulation. The accumulation stays serial on purpose.

**Files:**
- Modify: `alacritree/src/fonts.rs:176-275` (`scan_coverage` and its doc comment)
- Test: `alacritree/src/fonts.rs` (the `#[cfg(not(unix))]` test region, near `coverage_cache_round_trips_across_scans` at :2141)

**Interfaces:**
- Consumes: `cmap_coverage` from Task 2, `rayon` from Task 3, `disk_cache::{load, write, stat_file, CachedFile}`, `coverage::{Candidate, Coverage}`, all existing.
- Produces:
  - `fn worker_count(reported: usize) -> usize`
  - `fn scan_coverage_with_workers(db: &fontdb::Database, cache_path: Option<&Path>, workers: usize) -> (Vec<(coverage::Candidate, coverage::Coverage)>, usize)` returning the scan and the cache-hit count.
  - `scan_coverage(db, cache_path) -> Vec<(coverage::Candidate, coverage::Coverage)>` keeps its current signature and delegates, so the four existing call sites at :2147, :2151, :2165 and :2170 are untouched.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(not(unix))]
#[test]
fn worker_count_clamps_to_the_measured_knee() {
    assert_eq!(worker_count(1), 1);
    assert_eq!(worker_count(2), 2);
    assert_eq!(worker_count(4), 4);
    assert_eq!(worker_count(36), 4);
    // `available_parallelism` cannot report zero, but a caller mapping an
    // error to zero would build a pool with no threads.
    assert_eq!(worker_count(0), 1);
}

#[cfg(not(unix))]
#[test]
fn a_parallel_scan_matches_a_serial_one_element_for_element() {
    // Both runs pass `None`, so neither writes a cache the other could
    // read back: a second scan against a populated cache takes the
    // stored-ranges branch and parses no cmap at all.
    let serial_fonts = SystemFonts::with_cache_dir(None);
    let (serial, _) = scan_coverage_with_workers(serial_fonts.db(), None, 1);

    let parallel_fonts = SystemFonts::with_cache_dir(None);
    let (parallel, _) = scan_coverage_with_workers(parallel_fonts.db(), None, 4);

    assert_eq!(serial, parallel);
    assert!(!serial.is_empty(), "no faces were scanned, so this proved nothing");
}

#[cfg(not(unix))]
#[test]
fn every_face_of_a_collection_file_is_a_cache_hit_on_the_second_scan() {
    // A .ttc holds several faces behind one path, and they share one
    // CachedFile.  Accumulating that per worker rather than serially would
    // drop all but one worker's faces, and the only symptom would be that
    // they reparse on every launch.
    let cache_path = scratch_cache_path("collection_hits");
    std::fs::remove_file(&cache_path).ok();

    let cold_fonts = SystemFonts::with_cache_dir(None);
    let (cold, cold_hits) = scan_coverage_with_workers(cold_fonts.db(), Some(&cache_path), 4);
    assert_eq!(cold_hits, 0, "a cold scan cannot hit the cache");

    let warm_fonts = SystemFonts::with_cache_dir(None);
    let (warm, warm_hits) = scan_coverage_with_workers(warm_fonts.db(), Some(&cache_path), 4);

    assert_eq!(cold, warm);

    // Two faces of one path is the hazard this test exists for, so fail
    // loudly on a machine that has no collection file rather than pass
    // without exercising it.
    let mut faces_per_path: HashMap<&PathBuf, usize> = HashMap::new();
    for (candidate, _) in &warm {
        *faces_per_path.entry(&candidate.path).or_default() += 1;
    }
    assert!(
        faces_per_path.values().any(|&n| n > 1),
        "no multi-face font file was scanned, so this proved nothing"
    );

    // Not every face can be cached: one whose file cannot be stat'd never
    // reaches `fresh_files`, and one whose cmap emits a codepoint above
    // U+10FFFF is rejected on the way back out of the cache.  Both are
    // properties of the font set, not of the accumulation.
    let cacheable = cold
        .iter()
        .filter(|(candidate, cov)| {
            candidate.bytes > 0
                && coverage::Coverage::from_stored_ranges(cov.ranges().to_vec()).is_some()
        })
        .count();
    assert_eq!(warm_hits, cacheable, "every cacheable face should come from the cache");

    std::fs::remove_file(&cache_path).ok();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree -E 'test(worker_count) or test(a_parallel_scan) or test(every_face_of_a_collection)'`

Expected: FAIL to compile, with `cannot find function `worker_count`` and `cannot find function `scan_coverage_with_workers``.

- [ ] **Step 3: Rewrite `scan_coverage`**

Replace `fonts.rs:176-275`, which is the function and the doc comment above it. The replacement below carries its own copy of that doc comment, so starting at 181 would leave the old five lines stranded above the new `worker_count` and rustdoc would attach them to it. Add `use rayon::prelude::*;` inside the function rather than at module scope, so the import is also `cfg`-gated by its enclosing item.

```rust
/// How many faces to scan at once.  Four is where the measured curve
/// flattens; the work is memory-bound, so more cores stop helping well
/// before they run out.  The floor keeps a restricted container from
/// asking for a pool with no threads.
#[cfg(not(unix))]
fn worker_count(reported: usize) -> usize {
    reported.clamp(1, 4)
}

/// Scan every system face's cmap, reusing ranges from `cache_path` for
/// files whose size and mtime still match a prior scan.  `cache_path` is a
/// parameter (rather than always `disk_cache::default_cache_path()`) so
/// tests can point it at a scratch directory instead of the real
/// `%LOCALAPPDATA%`.
#[cfg(not(unix))]
fn scan_coverage(
    db: &fontdb::Database,
    cache_path: Option<&Path>,
) -> Vec<(coverage::Candidate, coverage::Coverage)> {
    let workers = worker_count(std::thread::available_parallelism().map_or(1, |n| n.get()));
    scan_coverage_with_workers(db, cache_path, workers).0
}

/// The scan proper, with the worker count injected so tests can compare a
/// parallel run against a serial one.  Returns the candidate list and how
/// many faces came from the cache.
#[cfg(not(unix))]
fn scan_coverage_with_workers(
    db: &fontdb::Database,
    cache_path: Option<&Path>,
    workers: usize,
) -> (Vec<(coverage::Candidate, coverage::Coverage)>, usize) {
    use rayon::prelude::*;

    let started = std::time::Instant::now();
    let cache = cache_path.and_then(disk_cache::load).unwrap_or_default();

    // Faces addressable by path, in database order.  A `.ttc` contributes
    // several entries sharing one path.
    let faces: Vec<(PathBuf, u32, &fontdb::FaceInfo)> = db
        .faces()
        .filter_map(|face| match &face.source {
            // Embedded faces aren't path-addressable by our loader.
            fontdb::Source::Binary(_) => None,
            fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => {
                Some((p.clone(), face.index, face))
            },
        })
        .collect();

    // Stat once per distinct file, before the fan-out, so the parallel
    // phase reads a finished map instead of contending on one.
    let mut stat_memo: HashMap<PathBuf, Option<(u64, u64)>> = HashMap::new();
    for (path, _, _) in &faces {
        stat_memo
            .entry(path.clone())
            .or_insert_with(|| disk_cache::stat_file(path));
    }

    let scan_one = |(path, face_index, face): &(PathBuf, u32, &fontdb::FaceInfo)| {
        let path_key = path.to_string_lossy().into_owned();
        let stat = stat_memo[path];

        let cached_ranges = stat.and_then(|(size, mtime_millis)| {
            let cached_file = cache.get(&path_key)?;
            (cached_file.size == size && cached_file.mtime_millis == mtime_millis)
                .then(|| cached_file.faces.get(face_index).cloned())
                .flatten()
        });

        let (cov, from_cache) = match cached_ranges.and_then(coverage::Coverage::from_stored_ranges)
        {
            Some(cov) => (cov, true),
            None => {
                let parsed = db.with_face_data(face.id, |data, index| {
                    let parsed = ttf_parser::Face::parse(data, index).ok()?;
                    cmap_coverage(&parsed)
                });
                let Some(Some(cov)) = parsed else {
                    log::debug!("skipping unparseable font {}", path.display());
                    return None;
                };
                (cov, false)
            },
        };

        let family = face.families.first().map(|(name, _)| name.clone()).unwrap_or_default();
        let candidate = coverage::Candidate {
            path: path.clone(),
            face_index: *face_index,
            family,
            weight: face.weight.0,
            italic: face.style != fontdb::Style::Normal,
            monospaced: face.monospaced,
            bytes: stat.map_or(0, |(size, _)| size),
        };
        Some((path_key, *face_index, stat, candidate, cov, from_cache))
    };

    // `par_iter().collect()` preserves input order, so the scan stays
    // deterministic with nothing carried or sorted to make it so.  A local
    // pool rather than rayon's global one, which would keep its threads for
    // the life of the process for a scan that runs once.
    let scanned_faces: Vec<_> = match rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
    {
        Ok(pool) => pool.install(|| faces.par_iter().filter_map(scan_one).collect()),
        Err(err) => {
            log::debug!("scanning fonts serially, thread pool unavailable: {err}");
            faces.iter().filter_map(scan_one).collect()
        },
    };

    // Accumulation stays serial.  One `CachedFile` holds every face of a
    // collection, so folding per-worker fragments would have to merge those
    // per-file maps or silently drop faces.  This is one pass over a list
    // that is already built; parallelising it would buy nothing.
    let mut fresh_files: HashMap<String, disk_cache::CachedFile> = HashMap::new();
    let mut scanned = Vec::with_capacity(scanned_faces.len());
    let mut hits = 0usize;
    let mut any_fresh = false;

    for (path_key, face_index, stat, candidate, cov, from_cache) in scanned_faces {
        if from_cache {
            hits += 1;
        } else {
            any_fresh = true;
        }
        if let Some((size, mtime_millis)) = stat {
            fresh_files
                .entry(path_key)
                .or_insert_with(|| disk_cache::CachedFile {
                    size,
                    mtime_millis,
                    faces: HashMap::new(),
                })
                .faces
                .insert(face_index, cov.ranges().to_vec());
        }
        scanned.push((candidate, cov));
    }

    // A cache that was absent or invalid produced zero hits, so every face
    // above went through the fresh-parse branch and `any_fresh` is already
    // true; no separate "was the cache valid" bookkeeping is needed.
    if any_fresh {
        if let Some(cache_path) = cache_path {
            disk_cache::write(cache_path, &fresh_files);
        }
    }

    log::info!(
        "scanned {} font faces for fallback coverage in {} ms ({} from cache)",
        scanned.len(),
        started.elapsed().as_millis(),
        hits
    );
    (scanned, hits)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree -E 'test(worker_count) or test(a_parallel_scan) or test(every_face_of_a_collection)'`

Expected: PASS, 3 tests.

If `every_face_of_a_collection_file_is_a_cache_hit_on_the_second_scan` fails with `warm_hits` less than `cacheable`, the accumulation was parallelised or the `fresh_files` entry does not merge per file. That is the bug this test exists for.

- [ ] **Step 5: Run the whole suite**

Run: `cargo fmt && cargo nextest run -p alacritree`

Expected: PASS. `coverage_cache_round_trips_across_scans`, `coverage_cache_corruption_falls_back_to_full_rescan` and the five Windows fallback tests all drive `scan_coverage` and would catch a regression in ordering or content.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/fonts.rs
git commit -m "perf(fonts): scan face coverage in parallel

Each face's cmap is independent of every other, so the scan fans out
over a rayon pool sized to the measured knee of four workers.

The stat pass is hoisted ahead of the fan-out so the parallel phase
reads a finished map, and the cache accumulation stays serial: one
CachedFile holds every face of a collection file, and per-worker
fragments would have to merge those maps or silently drop faces.

Coverage is unchanged face for face.  The one behavioural difference
is that an unparseable face no longer marks the cache dirty, so a
launch where every other face hits skips a rewrite it used to do.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Record the resulting scan time on the issue

#55's last acceptance criterion, and the input #27 is waiting on. This is the deliverable, not a formality: #27's entire design is priced against a 2860 ms scan.

Be careful which number you compare against. 2860 ms comes from #27 and its conditions are not recorded. The spec's own before-number is 1393 ms, serial, warm filesystem cache, best of 5, on the 16-CPU machine with 928 faces. Post the comparison against 1393 ms with those conditions named, and mention 2860 ms only as the figure #27 was written against.

**Files:** none.

- [ ] **Step 1: Measure a cold scan**

Delete the real cache and run the app once, then read the line `scan_coverage` logs:

```bash
rm -f "$LOCALAPPDATA/alacritree/coverage-cache.v1.bin"
cargo run -p alacritree --release
```

Expected: a log line of the form `scanned N font faces for fallback coverage in M ms (0 from cache)`. Record `N` and `M`.

- [ ] **Step 2: Measure a warm scan**

Run it a second time without deleting the cache. Record the same line; hits should equal the face count.

- [ ] **Step 3: Post both numbers to the issue**

```bash
gh issue comment 55 -R AbysmalBiscuit/alacritree --body "Cold scan after the change: <M> ms for <N> faces on <machine, core count>, warm filesystem cache. The serial before-number under the same conditions was 1393 ms. Warm scan: <M2> ms, all from cache."
```

- [ ] **Step 4: Correct #55's second acceptance criterion**

It reads "produces the same candidate set as a serial run, compared order-independently". `par_iter().collect()` preserves input order by construction, and the test in Task 4 asserts element-for-element equality, which is strictly stronger. Edit the issue body so the criterion matches what is actually guaranteed:

```bash
gh issue view 55 -R AbysmalBiscuit/alacritree --json body -q .body > /tmp/55.md
sed -i 's/compared order-independently/compared element for element, since rayon preserves input order/' /tmp/55.md
gh issue edit 55 -R AbysmalBiscuit/alacritree --body-file /tmp/55.md
```

- [ ] **Step 5: Note the consequence for #27**

```bash
gh issue comment 27 -R AbysmalBiscuit/alacritree --body "#55 has landed and the cold scan is now <M> ms. The design in this issue is priced against 2860 ms and should be re-scoped before implementation."
```

---

## Self-review

**Spec coverage.** Section 1 of the spec is Tasks 1 and 2: the constructor, the three-way fold with `<=`, `checked_sub` for the overflow, the re-walk fallback, `Coverage::default()` for a cmap with no unicode subtable, and reuse of the existing `merge`. Section 2 is Tasks 3 and 4: the target-gated dependency, the hoisted stat pass, `par_iter` with order preserved by construction, the local pool, `worker_count`, and the serial accumulation with its per-file merge. The spec's testing section maps onto the tests in Tasks 1, 2 and 4. The spec's "why not DirectWrite" and "why not a single subtable" sections are rejected alternatives with no task. The last acceptance criterion on #55 is Task 5.

**Not covered by a task, deliberately.** The spec's unresolved questions 1 and 2 are measurements Task 5 partly answers for the scan as a whole; the stat pass is not separately instrumented, because doing so means adding a timer to production code for a one-off question. Question 3 is Task 5 Step 4, which edits #55's second criterion to say element-for-element.

**Type consistency.** `Coverage::from_ascending_walk` takes `impl Fn(&mut dyn FnMut(u32))` in Task 1 and is called that way in Task 2. `worker_count` and `scan_coverage_with_workers` are declared in Task 4's interface block and used with those exact signatures in its tests. `scan_coverage` keeps `-> Vec<(coverage::Candidate, coverage::Coverage)>` so the four existing call sites compile untouched, while `scan_coverage_with_workers` returns the tuple with `hits` that Task 4's third test needs.

**One risk the plan cannot remove.** Task 2's equivalence test is a characterization test, not a red-then-green one: the behaviour it guards is "unchanged", so it passes before and after. Step 2 says to run it before the rewrite for exactly that reason. Reviewers should not read its passing as evidence the rewrite happened.
