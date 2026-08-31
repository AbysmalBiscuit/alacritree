# Stop copying font files into memory

**Status:** revised after Codex adversarial review round 1 (2026-08-09). Uncommitted by design —
`docs/superpowers/` is in `.git/info/exclude`, and `AGENTS.local.md` states plans and specs are
never committed. PR descriptions carry the context instead.

**Baseline:** `upstream/master` at `3617fe53`. References lead with symbol names; line numbers are
hints only, re-derived against that commit.

## Problem

A freshly started alacritree holds 1,113 MB of private committed memory with two terminal sessions
open and no scrollback. Walking the process's committed regions with `VirtualQueryEx`, grouping by
`AllocationBase`, and resolving names through `GetMappedFileNameW` returns pairs of identically
sized allocations — a private region and a mapped view of the same font file:

| Private | Mapped | File | On disk |
|---|---|---|---|
| 792 MB | 792 MB | `%LOCALAPPDATA%\Microsoft\Windows\Fonts\Sarasa-SuperTTC.ttc` | 792.8 MB |
| 33 MB × 5 | 33 MB × 5 | `C:\Windows\Fonts\NotoSansMonoCJK{jp,tc,sc,kr,hk}-VF.ttf` | 33.7 MB each |

That is 960 MB, 86% of private, present before the first keystroke. Scrollback is not a factor: a
session with a full 10,000-line history at 215 columns measures 52 MB, an empty one 5 MB, and
`alacritty_terminal`'s `Storage::with_capacity` allocates history lazily.

Three defects produce it.

### (a) Retained whole-file copies — 960 MB

`color_glyph::load` (`color_glyph.rs:251`) does `std::fs::read` into an `Arc<Vec<u8>>` and stores
it in `ColorGlyphCache::files: HashMap<PathBuf, Option<Arc<Vec<u8>>>>` (`:51`). Nothing evicts that
map: the cell-size reset clears only `entries`, `used` and `bytes` (`:96`), and `evict_to_budget`
touches only `entries` and `used` (`:224`). The `color_glyph_cache_mb` budget (default 10,
`config.rs:1050`) accounts for rasterized glyph textures via `CachedColorGlyph::bytes` (`:41`) and
never for the font bytes behind them.

The copies are not emoji-triggered. `terminal_view.rs:1098` calls `ColorGlyphCache::get` for every
painted character that is not a space and not a built-in box glyph, gated on
`config.font.color_glyphs`, which defaults to `true` (`config.rs:1049`). A cache miss calls
`claiming_index` (`color_glyph.rs:141`), which walks the fallback chain from index 0 calling `load`
on each face until one claims the character. The first ordinary ASCII glyph therefore copies the
792 MB primary. A character the earlier faces do not claim drags in the rest.

The same bytes are already available for free: `fonts::map_font_file` (`fonts.rs:1146`)
memory-maps each font and `Box::leak`s the mapping (`:1161`), which is what produces the mapped
column above.

### (b) Uncached seed-coverage reads — up to 4 × 792 MB per launch

`gather_fallback_faces` (`fonts.rs:1097`, the `#[cfg(not(unix))]` variant) needs the coverage of
its seed face and gets it from `face_coverage` (`:1087`), another whole-file `std::fs::read`. It
runs once per variant seed and there are four — normal, bold, italic, bold-italic (`:958`, looped
at `:969-972`).

The coverage disk cache (persisted to `%LOCALAPPDATA%\alacritree\coverage-cache.v1.bin`, keyed by
file size and mtime) covers the scan of *candidate* fallback faces via `scanned_coverage`
(`:140`). The seed lookup at `:1105` bypasses it entirely.

The memory is transient — each read is freed when the function returns — so this does not appear in
the 960 MB. It appears as time before the window opens.

### (c) A memo that never answers

`let index = self.claiming_index(c)?;` (`color_glyph.rs:121`) propagates `None` through `?` and
returns *before* `self.source.insert(c, None)` at `:127`, the line that records "egui owns this
character." A character no face claims is therefore never memoized and re-walks the entire chain on
every frame it is on screen.

Separately, `source: HashMap<char, Option<usize>>` (`:54`) is read in exactly one place, `:114`, as
`== Some(&None)`. The `Some(index)` written at `:131` is never consulted. It is not dead weight but
an unfinished optimization: when `evict_to_budget` drops a glyph from `entries`, the index survives
in `source`, yet the next lookup falls through to `claiming_index` and rediscovers it by re-walking
and re-parsing the chain.

## Decisions

Settled at design review and after Codex round 1.

| Decision | Choice | Rationale |
|---|---|---|
| Scope | (a), (b) and (c) in one PR | One coherent story: the font path stops reading files it has already mapped. |
| Cache shape in `color_glyph` | Keep `files`, change its value type | User's explicit call. See the noted alternative under Part 1. |
| Approach for (b) | Per-install seed memo, then scan lookup, then mmap fallback | Removes the parse entirely on a warm cache and bounds it to one parse per distinct face otherwise. |
| The stored chain index | Use it on the miss path rather than delete it | Turns an unfinished optimization into a real one. `HashSet<char>` would save ~100 KB in a heavy session and forfeit that. |
| Tests | One test per defect, driven through the layer the defect surfaces at | Codex round 1 showed leaf-level assertions for (b) and (c) could pass against the broken baseline. |
| Config gate | None | `AGENTS.local.md` requires new UX behind a config option. Rendering and behaviour are unchanged; only memory and startup time move. |

## Design

### Part 1 — borrow the mapping instead of copying

Fixes (a).

`fonts::map_font_file` becomes `pub(crate)`. `ColorGlyphCache::files` becomes
`HashMap<PathBuf, Option<&'static [u8]>>` and `load` maps rather than reads:

```rust
fn load(
    files: &mut HashMap<PathBuf, Option<&'static [u8]>>,
    path: &Path,
) -> Option<&'static [u8]> {
    *files.entry(path.to_path_buf()).or_insert_with(|| match crate::fonts::map_font_file(path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            // Unreachable in production; see the invariant below.
            log::warn!("colour font {} is in the chain but will not map: {e}", path.display());
            None
        },
    })
}
```

`warn`, not `debug`: the cached `None` already limits it to once per path per cache, and reaching
it means the chain and `FONT_MAPS` have diverged.

Call sites at `color_glyph.rs:144` and `:173` are unchanged. The returned reference is `'static`,
so the mutable borrow of `files` ends at the call and `chain` and `scale` remain usable —
confirmed by review. `swash 0.2.9`'s `FontRef::from_index` takes `&'a [u8]` and the scaler borrows
it only for its own scope.

The doc comment on `load` ("Read a font file once and keep it…") states the old intent and must be
rewritten to say why the mapping is borrowed.

**Invariant: the `None` arm is unreachable in production.** Every face that reaches the chain was
mapped successfully during install — the primary at `fonts.rs:890`, user fallbacks at `:548`,
automatic fallbacks at `:1020` — and `FONT_MAPS` (`:1144`) never evicts. A face whose mapping
failed never reaches the chain at all. The arm is kept as a defensive path and must log at
`warn` once if ever taken, because reaching it means the chain and `FONT_MAPS` have diverged.

**Noted alternative, not adopted.** Because that invariant holds, `files` could be replaced with a
`Vec<Option<&'static [u8]>>` preloaded in `new()` and indexed by chain position, eliminating the
per-lookup hash and `PathBuf` clone. Rejected for this PR: the user chose to keep `files`, and
this is a memory fix rather than a hot-path optimization. Raised at sign-off.

**This adds no new mappings.** `map_font_file` is called at `fonts.rs:548` and `:1020` before the
`is_color_only` check at `:555` and `:1027`, because that check needs the bytes. Every path `load`
can be handed is therefore already in `FONT_MAPS`. See Risks for what does change.

### Part 2 — stop re-reading the seed

Fixes (b). Windows only: `gather_fallback_faces` exists as a `#[cfg(unix)]` variant (`:1060`) and a
`#[cfg(not(unix))]` variant (`:1097`), and only the latter consults `face_coverage`. Unix resolves
coverage through fontconfig.

Seed coverage resolves in three steps, cheapest first.

**Step 1 — a per-install memo on `SystemFonts`.** `install_terminal_fonts` runs four variant seeds
(`:969-972`) which commonly resolve to the same one or two files, and then `install_ui_font`
(`:974`) reaches `gather_fallback_faces` again for the UI family. A memo scoped to the four
terminal seeds would miss that call, so it belongs on `SystemFonts` — the per-install object
already threaded through every resolution as `&SystemFonts`:

```rust
#[cfg(not(unix))]
seed_coverage: RefCell<HashMap<(PathBuf, u32), Option<coverage::Coverage>>>,
```

`RefCell` rather than `OnceCell` because the map is keyed and grows; `&self` access matches the
existing `db` and `coverage` fields. One method owns the lookup and **caches misses as well as
hits**, so an unresolvable seed is not retried once per variant:

```rust
#[cfg(not(unix))]
fn seed_coverage(&self, face: &ResolvedFace) -> Option<coverage::Coverage> {
    let key = (face.path.clone(), face.face_index);
    if let Some(hit) = self.seed_coverage.borrow().get(&key) {
        return hit.clone();
    }
    let computed = scanned_seed_coverage(self, face)
        .or_else(|| face_coverage(&face.path, face.face_index));
    self.seed_coverage.borrow_mut().insert(key, computed.clone());
    computed
}
```

The `#[cfg(not(unix))]` is required: the field it reads and the `face_coverage` it falls back to
are both gated that way. `scanned_seed_coverage` stays a free function taking `&SystemFonts`, so
it is called as one. The free function needs the same gate — it reads `scanned_coverage`, which is
itself `#[cfg(not(unix))]`, so leaving it ungated breaks the Unix build.

The `borrow()` is released before `face_coverage` runs, so the `borrow_mut()` cannot panic on a
re-entrant call. This is the only step that helps fonts configured by explicit path, which the scan
never contains.

**Step 2 — the scan.** Look the seed up in `scanned_coverage` (`:140`), which already carries every
system face and is disk-cached:

```rust
#[cfg(not(unix))]
fn scanned_seed_coverage(fonts: &SystemFonts, face: &ResolvedFace) -> Option<coverage::Coverage> {
    fonts
        .scanned_coverage()
        .iter()
        .find(|(candidate, _)| {
            candidate.path == face.path && candidate.face_index == face.face_index
        })
        .map(|(_, coverage)| coverage.clone())
}
```

`coverage::Candidate` carries `path` and `face_index` (`fonts.rs:2208`), so the match is exact.
`gather_fallback_faces` calls `scanned_coverage()` a few lines later regardless, so initializing
the `OnceCell` earlier costs nothing.

**Step 3 — the fallback.** `face_coverage` remains, switched from `fs::read` to `map_font_file`,
serving faces the scan does not contain: fonts resolved by explicit path, and faces whose cmap
parsing failed during the scan.

The seed resolution in `gather_fallback_faces` becomes one call:

```rust
let seed_coverage = resolve_face(family, style, variant, fonts)
    .and_then(|face| fonts.seed_coverage(&face))
    .unwrap_or_default();
```

**Qualified claim.** A warm launch does zero reads and zero cmap parses *for seeds present in the
scan with a valid cache entry*. A seed resolved by explicit path, or one absent from the scan,
still parses — but at most once per distinct face rather than once per variant.

**A deliberate consistency choice.** Step 2 makes seed coverage trust the same size-plus-mtime
identity that candidate coverage already trusts. A font replaced by a same-size file within the
same millisecond would yield stale seed coverage where today it yields fresh. This is accepted
rather than mitigated: that same stale cache already determines every candidate's coverage and
therefore the whole chain, so the seed being freshly parsed only produces an inconsistent chain,
not a correct one. Strengthening the cache key is a separate change that would invalidate every
user's cache.

### Part 3 — make the memo answer

Fixes (c). Record the negative result, and consult the memo before walking:

```rust
let index = match self.source.get(&c) {
    // Known monochrome: egui's own glyph pipeline draws it.
    Some(None) => return None,
    // Re-render after a budget eviction.  The chain is fixed at
    // construction, so the recorded index still names the same face.
    Some(Some(i)) => *i,
    None => match self.claiming_index(c) {
        Some(i) => i,
        None => {
            self.source.insert(c, None);
            return None;
        },
    },
};
```

This subsumes the existing early return at `:114`, which is removed. `chain` is assigned once in
`new()` and read only at `:122`, `:142-143` and `:160`, so a recorded index stays valid for the
cache's lifetime. Review confirmed the restructure preserves `entries`, `used`, byte budgeting,
eviction and cell-size-reset behaviour — `source` already survives the cell-size reset today.

`source` keeps its type. Its doc comment at `:52` should note that the index is what a
post-eviction re-render uses.

## Test plan

All tests live in-module under `#[cfg(test)]`, per `AGENTS.md`. CI runs
`cargo test -p alacritree --locked` on `ubuntu-latest` and `windows-2022`
(`.github/workflows/ci.yml:33`, `:78`), so Windows-gated tests do run. Tests must pass at the
default thread count, not only under `--test-threads=1`.

`map_font_file` must be made `pub(crate)` first, as a separate behaviour-free step, so the RED runs
compile. `fonts.rs:1560` already calls it from a test, so the pattern is established.

**Fixture handling.** `FONT_MAPS` is global, never cleared, and shared across parallel tests. Any
test that asserts something *about* mapping must therefore work on a path no other test touches:
copy `alacritree/assets/alacritree-symbols.ttf` (3,912 bytes, reached through
`concat!(env!("CARGO_MANIFEST_DIR"), "/assets/alacritree-symbols.ttf")`) into a unique temporary
file per test, and assert it is unmapped before the exercise.

1. **`load` returns the mapping, not a copy.** Copy the fixture to a unique temp path, then assert
   `std::ptr::eq(fonts::map_font_file(&path)?.as_ptr(), load(&mut files, &path)?.as_ptr())`.
   RED at `3617fe53`: `Arc<Vec<u8>>` is a fresh heap buffer at a different address. Compiles
   against both types because `.as_ptr()` is available on each.

2. **The seed is not re-read.** Two subcases, both driven through `gather_fallback_faces` rather
   than through `scanned_seed_coverage`, because a leaf test of the lookup passes even if the
   caller ignores it. Windows-gated.

   The counter on `face_coverage` must be **thread-local and resettable**, not a global — the
   existing Windows fallback tests call `gather_fallback_faces` concurrently and would contaminate
   a process-wide count. `steady_state.rs` already establishes this pattern with its thread-local
   `MEASURING` flag, and for the same reason.

   - **Scan hit.** Assert first that the seed is present in `scanned_coverage`, so a miss cannot
     pass the test by silently falling through. Reset the counter, call `gather_fallback_faces`,
     assert the count is 0. RED at `3617fe53`: the baseline calls `face_coverage` once, so the
     count is 1.
   - **Explicit path.** An explicit path is *not* automatically outside the scan —
     `resolve_via_path` (`fonts.rs:1308`) accepts a system font's own path, which
     `scanned_coverage` also contains. So construct the case deliberately: build the `SystemFonts`
     with `with_cache_dir(None)` to keep the disk cache out of it, copy the fixture to a unique
     temporary path, and **assert the resolved face is absent from `scanned_coverage`** before
     resetting the counter. Then call `gather_fallback_faces` twice against that same
     `SystemFonts` and assert the count is exactly 1. This is what proves the memo exists; the
     scan-hit subcase alone passes with the memo omitted. RED at `3617fe53`: the count is 2.

3. **`face_coverage` maps rather than reads.** Copy the fixture to a unique temp path, assert
   `!fonts::is_mapped(&path)`, call `face_coverage`, assert `fonts::is_mapped(&path)`. Requires a
   new `#[cfg(test)] pub(crate) fn is_mapped(path: &Path) -> bool` in `fonts.rs`; test-only so
   release builds carry no dead code. Windows-gated. RED at `3617fe53`: the baseline never touches
   `FONT_MAPS`, so the second assertion fails — but only if the path is unique, since another test
   mapping the shared asset would otherwise satisfy it.

4. **An unclaimed character is memoized.** Build a cache whose chain holds only the fixture, call
   `get()` for a character the fixture's cmap does not cover, and assert
   `source.get(&c) == Some(&None)`. No system font needed. RED at `3617fe53`: the `?` at `:121`
   returns before the insert.

5. **A post-eviction re-render skips the chain walk.** Requires a font with real colour artwork, so
   it reuses `chain_with_color_fonts` (`color_glyph.rs:329`) and skips when that returns `None` —
   the same environment gate five existing tests already use (`:364`, `:390`, `:416`, `:447`,
   `:465`). The test is therefore environment-dependent by construction; that is stated rather
   than disguised.

   `chain_with_color_fonts` proves renderability for U+1F600 only, so a second glyph cannot be
   assumed. Before the measured sequence, probe a small candidate set (for example U+1F600,
   U+1F601, U+2764, U+1F44D) against a throwaway cache and keep the characters that actually
   return `Some`. Skip the test explicitly if fewer than two survive; on a machine with a colour
   font but only one renderable candidate the test reports "skipped", never a false pass.

   Then, against a **fresh** cache built with a budget of 0, with every step asserted so a silent
   `None` cannot masquerade as success:
   1. Render A; assert `get` returned `Some`.
   2. Render B; assert `get` returned `Some`, and assert A is no longer in `entries` —
      `evict_to_budget` keeps only the newest entry.
   3. Record `chain_walks`, re-request A, assert `get` returned `Some`, and assert `chain_walks` is
      unchanged.

   The cache must be fresh so the probe's own lookups do not pre-populate `source` or `entries`.
   RED at `3617fe53`: step 3's re-request falls through to `claiming_index`, so the counter
   increases.

Test 5 needs observability that does not exist today. Add a test-only counter to
`ColorGlyphCache`:

```rust
#[cfg(test)]
chain_walks: usize,
```

incremented at the top of `claiming_index`. It is `usize` rather than an atomic because
`claiming_index` takes `&mut self`. Together with the `face_coverage` counter in test 2 and
`is_mapped` in test 3, these are the only places the tests reach into production code, and all
three are `#[cfg(test)]`.

## Verification

Acceptance is a before/after measurement recorded in the PR description. Conditions are fixed so
the numbers are reproducible:

- Same machine, same `release` profile, both binaries built from the same toolchain.
- Same config: `~/.config/alacritty/alacritty.toml` and `alacritree.toml` unchanged between runs,
  including the 20-entry fallback chain and `Sarasa Fixed K` as primary.
- Exactly two sessions open, no scrollback, window not resized.
- 60 seconds of idle settling after the window appears, so first-paint transients drain.
- Three samples per binary; report the median and the spread.
- Startup is measured warm and cold for **both** binaries, four series in all. The cold runs
  delete `%LOCALAPPDATA%\alacritree\coverage-cache.v1.bin` immediately before *each* launch, so
  neither binary inherits a cache the other populated.

**Startup timing method.** Wall clock from process start until the instance answers IPC, which
needs no new instrumentation and is scriptable:

```powershell
$exe = "C:\path\to\the\binary\under\test\alacritree.exe"   # explicit, per series
$sw  = [Diagnostics.Stopwatch]::StartNew()
$p   = Start-Process $exe -PassThru
$sock = "\\.\pipe\alacritree-$($p.Id).sock"
do {
    Start-Sleep -Milliseconds 50
    & $exe --socket $sock --json session list 2>$null | Out-Null
} until ($LASTEXITCODE -eq 0)
$sw.Stop(); $sw.ElapsedMilliseconds
Stop-Process -Id $p.Id
```

The loop gates on the **exit code**, never on stdout being non-empty. In JSON mode a failure is
printed to stdout as `{"error": …}` with exit 1 (`cli/mod.rs:250`), so a truthiness test treats an
unavailable socket as readiness and reports a startup time near zero.

The executable is named explicitly and the socket is addressed by the launched pid, so neither the
timer nor the client can attach to a different instance. `ipc_socket` must be left enabled for the
runs, since the poll is the stop signal. Exactly one instance exists per sample; the process is
terminated before the next.

This bounds font installation from above rather than isolating it; that is the point, since the
claim is about time the user waits.

The measurement is this region walk, run against each process id:

```powershell
$src = @'
using System; using System.Runtime.InteropServices; using System.Collections.Generic;
public class VM {
  [StructLayout(LayoutKind.Sequential)]
  public struct MBI { public IntPtr BaseAddress; public IntPtr AllocationBase; public uint AllocationProtect;
    public IntPtr RegionSize; public uint State; public uint Protect; public uint Type; }
  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint a, bool i, int pid);
  [DllImport("kernel32.dll")] public static extern IntPtr VirtualQueryEx(IntPtr h, IntPtr a, out MBI m, IntPtr l);
  [DllImport("psapi.dll", CharSet=CharSet.Unicode)] public static extern uint GetMappedFileNameW(IntPtr h, IntPtr a, System.Text.StringBuilder n, uint sz);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern uint QueryDosDeviceW(string dev, System.Text.StringBuilder target, uint max);
  public static string Dump(int pid) {
    IntPtr h = OpenProcess(0x0400|0x0010, false, pid);
    if (h == IntPtr.Zero) return "open failed";
    var sb = new System.Text.StringBuilder(); var byAlloc = new Dictionary<long, long[]>();
    long priv=0, mapped=0; IntPtr addr = IntPtr.Zero; MBI m; int sz = Marshal.SizeOf(typeof(MBI));
    while (VirtualQueryEx(h, addr, out m, (IntPtr)sz) != IntPtr.Zero) {
      long size = (long)m.RegionSize;
      if (m.State == 0x1000) {
        long ab = (long)m.AllocationBase;
        if (!byAlloc.ContainsKey(ab)) byAlloc[ab] = new long[2];
        if (m.Type == 0x20000) { byAlloc[ab][0] += size; priv += size; }
        else if (m.Type == 0x40000) { byAlloc[ab][1] += size; mapped += size; }
      }
      long next = (long)m.BaseAddress + size;
      if (next <= (long)addr) break;
      addr = (IntPtr)next; if (next > 0x7FFFFFFF0000L) break;
    }
    sb.AppendLine(String.Format("TOTALS,{0},{1}", priv, mapped));
    foreach (var kv in byAlloc) {
      string name = "";
      if (kv.Value[1] > 0) { var nb = new System.Text.StringBuilder(1024);
        if (GetMappedFileNameW(h,(IntPtr)kv.Key,nb,1024) > 0) name = nb.ToString(); }
      sb.AppendLine(String.Format("ALLOC,0x{0:X},{1},{2},{3}", kv.Key, kv.Value[0], kv.Value[1], name));
    }
    return sb.ToString();
  }
}
'@
Add-Type -TypeDefinition $src -Language CSharp
```

Every allocation is emitted, with **exact byte counts** and no size floor, because the criteria are
stated in bytes and one lost 33 MB mapping must be visible. `Dump` is called with a specific
process id, never `(Get-Process alacritree).Id` — that returns an array when more than one instance
is running and silently measures the wrong one:

```powershell
$exe = "C:\path\to\the\binary\under\test\alacritree.exe"   # explicit, per series
$p   = Start-Process $exe -PassThru
$sock = "\\.\pipe\alacritree-$($p.Id).sock"
do {                                                        # exit code, not stdout — see below
    Start-Sleep -Milliseconds 50
    & $exe --socket $sock --json session list 2>$null | Out-Null
} until ($LASTEXITCODE -eq 0)

# The window opens with one session (app.rs:899).  The stated condition is two,
# so create the second explicitly and prove the count before measuring.
& $exe --socket $sock session create | Out-Null
$n = (& $exe --socket $sock --json session list | ConvertFrom-Json).sessions.Count
if ($n -ne 2) { throw "expected 2 sessions, found $n" }

Start-Sleep -Seconds 60                                     # settle
$rows = [VM]::Dump($p.Id) -split "`r?`n" | Where-Object { $_ }
Stop-Process -Id $p.Id                                      # one instance per sample
```

Addressing the socket by the launched pid, rather than letting the client discover an instance,
stops a second alacritree from answering for the one being measured.

**Checking the criteria** compares the revised run against the *baseline run's own findings*, not
against the chain font list directly. The distinction matters: "any private allocation within
±1 MiB of any chain font's size" would flag an unrelated heap or GPU-driver block that happens to
land near 33 MB, and would not identify which copies were actually eliminated.

Two artefacts come out of the baseline run and drive the comparison:

- `baseline-dupes.txt` — the exact private byte counts of the allocations the baseline paired with
  a font file. On this machine that is one entry near 831,328,256 and five near 35,336,192.
- `baseline-mapped.txt` — the full canonical path of every font mapping present in the baseline.

Device paths must be canonicalized before comparison. `GetMappedFileNameW` returns
`\Device\HarddiskVolume4\Windows\Fonts\…`, and comparing basenames alone lets a different
same-named font under another directory produce a false pass:

```powershell
$dosByDevice = @{}
foreach ($d in (Get-CimInstance Win32_Volume | Where-Object DriveLetter)) {
    $target = New-Object Text.StringBuilder 260
    [void][VM]::QueryDosDeviceW($d.DriveLetter, $target, 260)   # declared alongside Dump, above
    $dosByDevice[$target.ToString()] = $d.DriveLetter
}
function Canonical($devicePath) {
    foreach ($k in $dosByDevice.Keys) {
        if ($devicePath.StartsWith($k, 'OrdinalIgnoreCase')) {
            return ($dosByDevice[$k] + $devicePath.Substring($k.Length)).ToLowerInvariant()
        }
    }
    return $devicePath.ToLowerInvariant()
}

$allocs = $rows | Where-Object { $_ -like "ALLOC,*" } | ForEach-Object {
    $f = $_ -split ',', 5
    [pscustomobject]@{ Priv = [long]$f[2]; Mapped = [long]$f[3]; Path = Canonical $f[4] }
}

# criterion 1: none of the baseline's duplicate sizes survives as a private allocation.
# 64 KiB is the Windows allocation granularity, so an exact byte match is not required.
$dupes = Get-Content baseline-dupes.txt | ForEach-Object { [long]$_ }
$dupes | ForEach-Object { $size = $_
    $allocs | Where-Object { $_.Priv -gt 0 -and [math]::Abs($_.Priv - $size) -le 64KB }
}

# criterion 3: every font path mapped in the baseline is still mapped, compared in full
$mappedNow = $allocs | Where-Object { $_.Mapped -gt 0 } | Select-Object -ExpandProperty Path
Get-Content baseline-mapped.txt | Where-Object { $mappedNow -notcontains $_.ToLowerInvariant() }
```

Both queries must return nothing.

**Pass criteria**, all falsifiable on the medians of three samples:

| Criterion | Threshold | Fails if |
|---|---|---|
| The baseline's duplicate allocations are gone | no private allocation within 64 KiB (the Windows allocation granularity) of any size recorded in `baseline-dupes.txt` | any survives |
| Private committed | ≤ 250 MB (baseline median ~1,113 MB; arithmetic predicts ~153 MB, and the 97 MB of headroom absorbs allocator and GPU-driver variance) | > 250 MB |
| Mapped retention, per path | every font path in `baseline-mapped.txt` is still mapped, compared as a full canonicalized path | any is absent |
| Mapped committed, aggregate | within ±5% of the baseline median | outside that band |
| Startup, warm cache | revised median ≤ baseline median + 100 ms | above that |
| Startup, cold cache | revised median ≤ baseline median + 100 ms | above that |

The mapped column staying flat is as much the point as the private column falling: the bytes must
stay reachable and stay evictable by the OS. The aggregate ±5% band alone is too coarse to catch a
single lost 33 MB Noto mapping against a ~1.1 GB total, which is why the per-path check sits above
it and is the binding one.

The startup criteria are one-sided by design. Part 2 should improve them, but the change is not
justified by startup time, so a flat result passes and only a regression beyond the stated bound
fails.

Also required: `cargo fmt`, `cargo clippy`, and `cargo test -p alacritree` at the default thread
count. Local release builds on this machine need `-j 1`; parallel rustc dies with
`STATUS_STACK_BUFFER_OVERRUN` on cold builds of this workspace.

## Risks

**The mmap access window widens, for colour-only faces only.** A font file truncated or replaced in
place while mapped faults the process — the bet documented at `map_font_file` (`fonts.rs:1146`).
Part 1 creates no new mapping, but it does change *when* mappings are read. Every chain face except
colour-only ones is already handed to epaint as borrowed `FontData`, so the process already
dereferences those mappings on every frame; nothing changes for them. Colour-only faces are the
exception: they are skipped before `insert_face`, so today their mapping is touched only during
startup classification, while rendering goes through an owned `Arc<Vec<u8>>`. After this change,
paint-time rasterization dereferences their mapping too.

This is accepted rather than mitigated. Retaining owned snapshots for colour-only faces would not
close the window, because `claiming_index` already dereferences every chain face's cmap at first
sight of a new character — hours into a session, not at startup.

**Widened visibility.** `map_font_file` goes from private to `pub(crate)`; `is_mapped` and two call
counters are added as `#[cfg(test)]`. Crate-internal only.

**Behaviour.** None intended. Identical glyphs, identical fallback order. Part 3 changes *when* the
chain is walked, never which face wins, because the recorded index is the one `claiming_index`
would have returned.

## Non-goals

- Trimming the user's 20-entry fallback chain, or the choice of the Sarasa SuperTTC bundle over a
  single-family build. Both are configuration.
- A config option to disable colour glyphs as a memory workaround. `font.color_glyphs = false`
  already exists (`config.rs:1362`) and short-circuits at `terminal_view.rs:1097`; this PR makes it
  unnecessary rather than promoting it.
- Strengthening the coverage cache's size-plus-mtime identity. Real but pre-existing, and a change
  that would invalidate every user's cache for unrelated reasons.
- Reporting font memory from `alacritree doctor`. Plausible follow-up, out of scope.
- Any change to scrollback sizing. `[scrolling] history` is already configurable.

## Branch and PR

No PRs are open on `mathix420/alacritree`; the highest merged marker is `[10]` (#171), so this is
`[11]` and branches off `upstream/master`.

```sh
git worktree add ../alacritree-worktrees/fix-font-bytes -b fix-font-bytes upstream/master
```

PR title: `fix(fonts): map font files instead of copying them [11]`, opened against `master` on
`mathix420/alacritree`, pushed to `AbysmalBiscuit/alacritree`. Commits carry
`Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

After merge: merge into `all-features`, then run `install.local.ps1`.

## Open questions

One, for the sign-off gate: whether to adopt the `Vec<Option<&'static [u8]>>` preload from Part 1's
noted alternative instead of keeping the `files` map. The current spec keeps `files` per the design
review decision.
