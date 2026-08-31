# Clipboard image paste

**Status:** design complete, all open questions decided. Revised once after an
adversarial review, then trimmed against overbuilding.
**Date:** 2026-07-29

## Problem

`Win+Shift+S` puts a bitmap on the Windows clipboard. `clipboard::read`
(`clipboard.rs:42`) only asks arboard for text, so with an image-only clipboard
it returns `None` and `NamedAction::Paste` (`app.rs:2186`) does nothing at all.
The image cannot reach a program running in the terminal.

Claude Code, the motivating consumer, already accepts a *path*: its
bracketed-paste handler splits the pasted text on newlines and on spaces that
precede a new path, strips one matching pair of surrounding quotes, tests each
item against `/\.(png|jpe?g|gif|webp)$/i`, converts `C:\…` to a distro path with
`wslpath` when it detects WSL, reads the bytes and attaches them as an image —
which is what renders as `[Image #N]`. So a path is a complete answer, and the
existing drop payload is already in the accepted shape.

The gap is only that no file exists. Close it by writing one.

## Scope

In scope: an image-only system clipboard becomes a PNG on disk whose path is
pasted, plus copied filesystem paths (`CF_HDROP`) — files or folders — pasting
as they are.

Out of scope: the X11 PRIMARY selection (`PasteSelection` stays text-only); any
change to what dropping a file does.

## Prior art

Worth stating what is being mirrored, since CLAUDE.md's rule is to follow
alacritty and alacritty does nothing here — its clipboard is text-only
(`alacritty/src/clipboard.rs:70` returns a `String`), and it pastes a path only
on *drop*: `self.ctx.paste(&(path + " "), true)` at `alacritty/src/event.rs:2009`,
which is where this fork's trailing space comes from.

- **Windows Terminal** pastes the path of a file copied in Explorer, for any
  file type. It is long-standing enough that breaking it in 1.19 was filed as a
  regression (microsoft/terminal#16627, fixed for 1.21).
- **wezterm** and **ghostty** ship neither half. wezterm users hand-roll the
  bitmap case in Lua `action_callback` — save the clipboard image to a file,
  paste the path — which is this design. ghostty has an open request proposing
  `paste-image-as-file = ~/Pictures/clips`, essentially `[ui.paste] image_dir`,
  with the maintainer unconvinced it belongs in a terminal at all.
- **kitty** can pull an image off the clipboard into a file, but as an explicit
  command (`kitten clipboard -g picture.png`) rather than as paste behavior.

So step 2 follows Windows Terminal exactly, including its lack of a file-type
filter. Step 3 has no shipping precedent in any of them; the closest is iTerm2's
"Save image data to file", and ghostty's proposed config shape agrees with ours.

## Behavior

`Paste` (`Ctrl+Shift+V`, `Cmd+V` on macOS) first resolves the target session —
before touching the clipboard, so no clipboard or filesystem work happens with
nothing to paste into. It then resolves the clipboard in order:

1. **Text** — unchanged, exactly today's behavior. Text always wins, so a
   clipboard carrying both text and an image (a browser "copy image" often does)
   pastes the text.
2. **File list** — `CF_HDROP`. Paste those paths, whatever they are, as Windows
   Terminal does. Nothing is written to disk. Explorer puts *directories* in the
   same list, and they are pasted too — a copied folder's path is as useful on a
   command line as a file's. An empty list counts as the format being absent and
   falls through to step 3.
3. **Bitmap** — encode to PNG, store it under `image_dir`, paste that one path.

A step falls through to the next **only** on a confirmed "this format is not on
the clipboard". Any other clipboard failure aborts the whole paste and pastes
nothing — see Clipboard resolution.

A step switched off in config is skipped, falling through to the next. With both
off, or on any failure, `Paste` behaves exactly as it does today: with no text,
it does nothing.

Only image paths become `[Image #N]` in Claude Code, since that is its
extension test — a copied `.txt` pastes a path that is useful to a shell and
inert to Claude Code. That is the consumer's business, not the terminal's.

Explorer's **Cut** advertises a move effect alongside the same `CF_HDROP` list.
Reading the paths neither performs nor completes that move, so the effect is
ignored and Cut pastes exactly what Copy does.

## Clipboard resolution

Steps 1-3 are three separate reads, and arboard opens and closes the Win32
clipboard once per read — `Get::new` calls `Clipboard::open` for every format
(`arboard-3.6.1/src/platform/windows.rs:577`). Holding one `arboard::Clipboard`
across the three probes therefore does *not* give a coherent snapshot: the
clipboard can change underneath them, and a paste could mix generations.

Checking `GetClipboardSequenceNumber` before and after would close that on
Windows, and is deliberately **not** in scope: it is Windows-only code guarding
a race that requires re-copying between two probes microseconds apart, and every
terminal today has the same race. Noted as a follow-up rather than built now.

What *is* in scope, because it is nearly free and prevents pasting the wrong
thing rather than a stale thing: `clipboard::read` currently collapses every failure to `None`, which was fine
when `None` meant "paste nothing" and is not fine now that `None` means "try the
next format". The new internal reads return a typed result distinguishing
*format absent* from *failure*:

- `ContentNotAvailable` → fall through to the next step.
- Anything else — `ClipboardOccupied`, conversion failure, backend error →
  abort the entire paste.

Contention needs no retry logic here: arboard already retries `OpenClipboard`
five times at 5 ms and then returns `ClipboardOccupied`
(`windows.rs:533-560`), which is a distinct error from `ContentNotAvailable`.

## Configuration

New `[ui.paste]` table in `alacritree.toml`, single-level to match `[ui.drop]`:

```toml
[ui.paste]
files = true       # step 2: paste the paths of copied files and folders
image = true       # step 3: clipboard bitmap becomes a PNG whose path is pasted
image_dir = "..."  # step 3 only; where written PNGs go, default below
image_keep = 20    # step 3 only; files retained, and only in the default dir
```

Two switches, because the steps are independent: step 2 writes nothing and only
reads a path Windows already put on the clipboard, step 3 creates a file. Either
can be off without affecting the other, and both off leaves `Paste` exactly as
it is today.

`image_dir` defaults to `std::env::temp_dir()/alacritree/clipboard` —
`%TEMP%\alacritree\clipboard\` on Windows. Disposable by nature, and reachable
from a WSL session once `shell_payload` translates it, for any `%TEMP%` on a
local drive.

**Pruning only ever runs in the default directory.** A directory alacritree
created and owns is the only one where deleting by filename pattern is
defensible; in a directory the user chose, a file named `clipboard-<hex>.png` is
not proof alacritree wrote it, and `Pictures\Screenshots` is exactly where being
wrong costs the most. Setting `image_dir` therefore means keeping every image
and cleaning up yourself. `image_keep` bounds the default directory only, and
has a floor of 1: the paste hands the shell a path and the shell opens it after
the sweep has run, so the file just written can never be a candidate.

`image = true` by default. This is an intentional behavior change, not a
no-op-to-no-op: a paste that does nothing today will start writing a file and
injecting terminal input. It is defensible because nothing is taken away and no
keystroke moves, and `[ui.drop]` sets the precedent of shipping enabled.

## WSL paths

A path pasted into a WSL session must be the path *that distro* resolves. Both
steps get there the same way: `shell_payload`, exactly as a dropped file does.
Windows drives are automounted under `/mnt` by default, so a `%TEMP%` path is
readable from inside the distro — this is not a new mechanism, it is what drops
already rely on.

Writing the PNG *inside* the distro instead — via `\\wsl.localhost\<distro>\…`
into the runtime dir the helper already creates — was considered and rejected. It
would remove translation from step 3, but step 2 must translate arbitrary copied
paths regardless, so the automount work below is needed either way; once it is
done, step 3 is correct without relocating anything. A second storage location, a
second prune target and a dependency on the helper being up is a poor price for
covering a case the fix already covers.

### A known limitation, deliberately not fixed here

`AUTOMOUNT_ROOT` (`wsl.rs:52`) is a single process-wide value set from
`[wsl] automount_root`, so two distros with different `automount.root` in their
`/etc/wsl.conf` cannot both be translated correctly, and neither is discovered.
Paste inherits this exactly as drops have it today.

The fix — carry each distro's automount root in the helper hello beside
`runtime_dir`, cache it per distro, give `windows_to_linux` a per-distro variant
— is **out of scope for this change**. It touches `wsl.rs`, bumps the helper
protocol, and alters drop behavior, so it belongs in its own change where it can
be reviewed as a WSL fix rather than as a rider on a paste feature. Until then
`[wsl] automount_root` is the single knob, and a multi-distro setup with
differing roots must pick one.

### What still has no correct answer

Some paths have no distro-side spelling at all, and no amount of translation
invents one:

- a non-WSL UNC share (`\\server\share\…`) the distro has not mounted;
- another distro's `\\wsl.localhost\Other\…`, which is not reachable from inside
  this one;
- a drive letter that is a `subst` or a mapped network drive, which WSL does not
  automount;
- any drive when the distro sets `automount.enabled = false` — which, since
  step 3 stores under `%TEMP%`, means step 3 cannot work in that configuration
  either. Documented, logged, and not worked around.

For all of these, the raw Windows path is pasted and the reason logged — exactly
what dropping the same file does today (`file_drop.rs:126`). A wrong-looking path
the user can see and fix beats silently pasting nothing.

## Dependencies

`alacritree/Cargo.toml:50` pins arboard with `default-features = false`, which
switches off `image-data` — arboard's own default feature. Turning it back on
adds `image 0.25` with `default-features = false` and only `png` + `bmp` on
Windows, `png` on Linux, `tiff` on macOS (verified in the resolved
`arboard-3.6.1/Cargo.toml`). `file_list` is on `Get` unguarded and needs no
feature (`arboard-3.6.1/src/lib.rs:204`). The `png` crate used for encoding is
already a direct dependency (`main.rs` decodes the window icon with it).

Two alternatives were considered and rejected. Reading `CF_HDROP` and the
registered `PNG` clipboard format through `windows-sys` directly avoids the
dependency and skips a decode/re-encode round trip, but the `CF_DIBV5` fallback
means hand-writing DIB→RGBA — `BI_BITFIELDS` masks, bottom-up rows,
premultiplied alpha — and leaves Linux and macOS unimplemented. Shelling out to
PowerShell the way Claude Code does needs no dependency but spawns a process on
the UI thread on every keypress. This fork's standing rule is to reuse the
solved abstraction rather than diverge, and arboard is already that abstraction
here.

## Components

Following `file_drop.rs`: decisions are pure functions, `app.rs` owns the sinks.

**`digest.rs`** (new, ~15 lines). `stable_digest` moves here verbatim from
`scratchpad.rs`, which imports it. Two callers now need the same FNV-1a.

**`config.rs`** grows `PathSpelling { quote, wsl_translate }`, and `DropConfig`
holds one instead of the two loose fields. `shell_payload` takes a
`&PathSpelling` rather than a whole `&DropConfig`. The `[ui.drop]` keys and
their parsing are unchanged; this only stops paste code from receiving drop
enablement flags it must not consult, and stops a future `shell_payload` that
starts honoring `DropConfig::enabled` from silently changing paste.

**`clipboard.rs`** gains the typed reads described in Clipboard resolution,
alongside the existing `read`, which keeps its current signature and callers.

**`clipboard_image.rs`** (new). Pure, testable without a clipboard or a window:

- `encode_png(&ImageData) -> Result<Vec<u8>, EncodeError>` — RGBA8 out of
  arboard, `png::Encoder` at `Compression::Fast`. Rejects an image over
  `MAX_PIXELS` (64 MP) or whose `bytes.len()` disagrees with `width * height * 4`,
  so a hostile or malformed `ImageData` cannot make the UI thread encode for
  seconds. The guard bounds the *encode* only, not the pipeline: by the time
  `encode_png` runs, arboard has already decoded the clipboard bitmap and
  allocated it. `Get::image()` on Windows goes through `DynamicImage::into_rgba8`,
  which allocates from the *declared* dimensions before reading a pixel, and the
  BMP decoder caps a dimension at `0xFFFF` — so a malformed `CF_DIBV5` header can
  drive a 65535×65535 (~17 GB) allocation attempt, which aborts the process rather
  than returning `Err`. That risk lives inside a vendored dependency and cannot be
  closed through arboard's API; it is a known limitation, not something
  `MAX_PIXELS` mitigates.

  Measured `png::Encoder` cost at `Compression::Fast`, RGBA8, on the development
  machine — worst-case noise versus flat content: 2 MP 25/1 ms, 8 MP 100/4 ms,
  14 MP 179/7 ms, 16 MP 213/9 ms. A real 4K screenshot lands between the columns,
  so a paste stalls the UI thread for roughly 50-150 ms plus arboard's decode.
  That vindicates encoding synchronously. It also means 64 MP is a backstop rather
  than a latency budget — at the cap the encode alone would be ~800 ms.
- `file_name(png: &[u8]) -> String` — `clipboard-<16 hex of stable_digest>.png`,
  the full 64-bit digest rather than the scratchpad's truncated 48 bits, since
  here a collision pastes the *wrong image* instead of merely colliding a
  human-readable label.
- `store(dir, png, keep) -> io::Result<PathBuf>` — see below, cap included.

**`wsl.rs` and `wsl_helper.rs` are untouched.** Paste reaches WSL through the
existing `shell_payload` translation, with the single-global-root limitation
documented under WSL paths.

**`app.rs`**. The two paste arms collapse into one helper that resolves the
target, then the clipboard, to `Vec<PathBuf>` or text, and reuses the existing
sinks. No new sink, no new session plumbing.

## Storing a PNG

Content-addressing means an existing file with the right name is *probably* the
right bytes, and "probably" is not enough to hand to a consumer. `store`:

1. Ensure `dir` exists.
2. If the destination exists, accept it only if it is a **regular file** whose
   length equals `png.len()`; anything else — a directory, a link, a truncated
   or foreign file — is replaced. `fs::symlink_metadata` is used so a link is
   seen as a link rather than followed. `rename` replaces a file but not a
   directory, so a directory on the name is removed first — and only if it is
   empty. A populated one is something this module did not create, and losing
   its contents to a name collision is worse than failing the paste.
3. On reuse, refresh its mtime. Without this, a reused file keeps an old
   timestamp and the very next cap sweep can delete the path just handed out.
   If the refresh fails, do not reuse — fall to step 4 and rewrite the file,
   which sets a fresh mtime as a side effect.
4. Otherwise write `clipboard-<hex>.png.<pid>.<counter>.tmp` beside it — unique
   per process and per call — then rename onto the destination. If the rename
   fails because the destination now exists, re-run step 2 against it: another
   instance writing the same content is a success, not an error. Either way the
   temp file does not survive.
5. Apply the count cap (see below).

`store` returns the path in every success case.

## Keeping the directory bounded

A count cap, and nothing else. After a write, if the directory is the default
app-owned one, delete files matching `clipboard-<16 hex>.png` past the `keep`
newest by mtime. Synchronous — it is a handful of directory entries — and its
failures only `log::debug!`.

Three conditions come along because each is a single line and each prevents a
real defect, not a hypothetical one:

- Only the default directory is touched. A user-set `image_dir` is never
  cleaned, which is what makes pointing it at `Pictures\Screenshots` safe
  without having to reason about ownership at all.
- The path just returned is never deleted, whatever its mtime. This holds within
  one `store` call. A second alacritree instance sharing the directory can still
  delete a path the first just handed out; retention across instances is
  best-effort, and cross-process locking is not worth its cost here.
- Reuse refreshes mtime (`store` step 3), so a re-pasted old screenshot does not
  immediately become the oldest file and delete itself.

No age-based grace period, no background thread. With `keep = 20`, a pasted path
survives nineteen further pastes, which is longer than any path sits unsent in a
line editor.

## Data flow

```
Ctrl+Shift+V
  └─ resolve target session ── none ──▶ stop, touch nothing
       └─ text? ── Some ──▶ existing paste path
            │ absent   error ──▶ abort, paste nothing
            ├─ file list? ─ paths ──────┐
            │ absent                    │
            └─ bitmap? ─ encode ─ store ┤
                 │ absent → nothing     ▼
                     terminal: shell_payload ─▶ paste::paste(bracketed)
                   scratchpad: insert at cursor
```

The whole path is synchronous on the UI thread, which is what keeps a paste
ordered against the keystrokes around it. The size cap in `encode_png` bounds the
encode, `Compression::Fast` trades a larger file nobody keeps for latency on a
keypress, and the count cap walks a directory holding `keep` entries.

## Scratchpad insertion

A paste is not a drop here. `document_payload` deliberately adds boundary
newlines so a dropped block does not weld onto surrounding text
(`file_drop.rs:142`); applying that to a paste would turn typing a sentence and
pasting a screenshot into a structural document edit the user did not ask for.

Image paste therefore inserts the path **literally at the cursor**, replacing
the selection, exactly as pasting text does. Several copied paths are joined
with newlines between them, with no newline added before the first or after the
last. `document_payload` stays as it is and keeps serving drops.

No new editor API is needed: `Editor::insert_at_cursor` (`scratchpad.rs:45`)
already deletes the selected range and inserts at its start. `cursor_boundary`
is the drop-specific half and must not be called on this path.

## Error handling

Mirrors the drop system: log and do nothing. Never panic, never surface a modal.

- Format absent → try the next step. Any other clipboard error → abort the
  whole paste (see Clipboard resolution).
- Image over the size cap, or `bytes.len()` inconsistent with its dimensions →
  `log::warn!`, no paste.
- PNG encode fails → `log::warn!`, no paste.
- `image_dir` cannot be created, or the write fails → `log::warn!` naming the
  directory, no paste. A repeatedly failing write is visible in the log rather
  than silently dropping every screenshot.
- Prune fails on a file → `log::debug!` and continue.

Generated *basenames* are hex and inert, but `is_terminal_safe` tests the whole
path, and the directory prefix comes from `%TEMP%` or from `image_dir` — either
can carry control characters or shell metacharacters. Image paste therefore
reuses `shell_payload` whole rather than a copy of it, filter included, and a
`image_dir` that would produce an unsafe path pastes nothing and logs.

**Inherited limitation, not introduced here:** `Quoting::Auto` resolves from the
host OS and whether the session is WSL (`config.rs:276`); it does not identify
cmd.exe versus PowerShell versus Nushell, and `ShellQuoting::Windows` does not
escape embedded quotes. A *copied file* whose name contains shell metacharacters
inherits that exposure. The *quoting* is identical to a drop's, but the
*reachability* is not: dropping a file is a deliberate, aimed act on one named
file, while `Ctrl+Shift+V` is a reflex that now turns whatever the file manager
last put on the clipboard into a shell word. A file named `a" & calc & "b.txt`
pasted into a cmd.exe session lands as a quote-broken command line. What bounds
the damage is that `is_terminal_safe` still strips control characters, so nothing
self-submits: the text is visible and the user must press Enter. `quote = "posix"`
is the only mode that makes an arbitrary filename inert, and `docs/alacritree.md`
points a `[ui.paste]` reader at it. Generated paths cannot carry any of this: they
are hex under a fixed directory. Fixing per-dialect quoting is a separate change
to the drop system, not a prerequisite for this one.

## Testing

`clipboard_image.rs` is unit-testable with `tempfile`, as `file_drop.rs` is.
Happy-path tests are the smaller half; the ones that matter assert the safety
properties above:

- Encode round-trip — RGBA in, decode the PNG back with the `png` crate already
  in the tree, assert dimensions and pixels survive.
- An `ImageData` over the pixel cap, and one whose `bytes.len()` disagrees with
  its dimensions, are both rejected before any allocation.
- Identical images produce identical names; `store` twice leaves one file.
- Reuse refreshes mtime, and the cap applied straight after does not delete the
  returned path — the regression the review's first finding describes.
- The cap leaves a file named anything else untouched, and does nothing at all
  when `dir` is not the owned default.
- `keep` smaller than the number of live files still keeps the returned path.
- A destination that exists as a directory or as a regular file of the wrong
  length is replaced rather than trusted.
- No temp file survives a completed `store`.
- A destination whose mtime cannot be refreshed is rewritten rather than reused,
  so the returned path always carries a current timestamp.
- An `image_dir` containing a control character produces no paste, because the
  whole path — not just the basename — goes through `is_terminal_safe`.
- The seam, in the spirit of `file_drop.rs`'s last test: a stored path fed
  through `shell_payload` comes back unquoted and unmangled, and under a WSL
  distro comes back as `/mnt/c/…`.
- A path with no distro-side spelling — a plain UNC share, another distro's UNC
  path — survives step 2 as the raw Windows path rather than being mangled.
- Scratchpad insertion mid-line, at line start, and over a selection inserts the
  path with no boundary newlines added.

The arboard wrappers stay untested, like `clipboard::read` — they need a live
clipboard. End-to-end verification is manual, in the GUI lab: take a
`Win+Shift+S` capture, `Ctrl+Shift+V` into a Claude Code session, confirm
`[Image #N]`, and confirm the same in a WSL session.

## Review findings not adopted

- **Cryptographic content digest (SHA-256/BLAKE3, ≥128 bits).** The threat model
  is a user pasting their own screenshots, not an adversary steering a hash.
  Widening to the full 64-bit digest plus the length check in `store` step 2
  makes an undetected wrong-image paste require a 64-bit collision *and* equal
  file lengths, at no dependency cost.
- **Hardening against hostile symlinks and reparse points in `image_dir`.** An
  attacker who can plant files in your temp directory has better options than
  swapping your screenshots. The mundane half — a destination that is a link, a
  directory, or the wrong length — is handled in `store` step 2 regardless.
- **Moving the whole pipeline off the UI thread with a pending-paste operation
  keyed on clipboard generation, session id and cursor version.** Correct, and
  disproportionate: it buys latency at the cost of the ordering guarantee that
  makes a paste predictable. The size cap bounds the synchronous cost instead.
- **Age-based retention with a grace period, and pruning on a background
  thread.** Cut as overbuilt. A count cap over a directory holding `keep`
  entries is not slow enough to need a thread, and with `keep = 20` a pasted
  path already survives nineteen further pastes.
- **A `GetClipboardSequenceNumber` check around the format probes.** Windows-only
  code for a race that needs a re-copy between two probes microseconds apart.
  Recorded as a follow-up.
- **Per-shell-dialect quoting.** Pre-existing in the drop system, unchanged in
  exposure, and a separate change. Documented above.
- **Feedback for partially consumed mixed selections.** Moot now that step 2
  takes every copied file; nothing is silently dropped.
- **Restricting step 2 to attachable image formats.** Reversed after surveying
  prior art: no terminal filters copied files by type, and the one that ships
  this at all pastes any of them. See Prior art.

## Decisions

All open questions are resolved; recorded here so the reasoning is not lost.

- **Trigger** — a fallback inside the existing `Paste` action, not a new binding.
- **Step 2 takes any copied path**, file or folder, not only images, following
  Windows Terminal,
  the only terminal that ships this. See Prior art.
- **Scratchpad insertion is literal**: the path goes in at the cursor and
  replaces the selection, exactly as pasting text does. `document_payload`'s
  boundary newlines stay a drop behavior.
- **Per-distro automount roots are out of scope**, with the limitation documented
  under WSL paths and the fix left to its own change.
- **`image_keep = 20`**, count only.
- **`image = true` and `files = true` by default**, an intentional behavior
  change in a case that is a no-op today.

## Follow-ups

Deliberately deferred, each with its reasoning above:

1. Per-distro automount roots, via the helper hello — a WSL fix, not a paste fix.
2. A `GetClipboardSequenceNumber` guard around the format probes.
3. Per-shell-dialect quoting, which the drop system needs equally.
