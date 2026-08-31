# Sidebar path display — design

Four changes to how alacritree renders paths, plus the config to control them.
Two are bug fixes that make Windows behave like Linux; two are new opt-in
display features.

Out of scope: the git-panel base-branch bug, parked in `docs/bugs_to_fix.md`.

---

## 1. Diff pane title is clobbered on Windows

### Problem

`open_diff` names the pane `diff: <path>` (`app.rs:3426`), which is what Linux
shows. On Windows the pane instead reads `C:\Program Files\Git\cmd\git …`.

ConPTY publishes the child's console title as an OSC-0 sequence at startup, and
Windows defaults that title to the child's command line. `apply_term_event`
(`session.rs:1114`) accepts every title event and overwrites `Session.title`.
On Linux nothing emits a title, so the spawn-time value survives. The same
ConPTY quirk is already documented at `session.rs:1011`.

`TermEvent::Title` is the only post-construction write to `Session.title`.

### Design

A diff pane's title is set by alacritree and is never meant to change. Pin it.

No new field. `Session` already stores `kind` (`session.rs:88`), so
`drain_events` derives the flag before it mutably borrows `self.title`:

```rust
let title_pinned = matches!(self.kind, SessionKind::Diff { .. });
```

A separate `title_pinned: bool` set in `spawn_command` would be redundant state
that can drift, and its wiring through `spawn_command` / `spawn_with`
(`session.rs:812`, `:837`) is not covered by the unit test below. Deriving from
`kind` makes the wrong state unrepresentable.

`apply_term_event` takes `pinned: bool`; when set, the `TermEvent::Title` arm is
skipped entirely. Every other event (`PtyWrite`, `ChildExit`, `Bell`,
`ClipboardStore`) is unaffected, so shell sessions keep accepting titles.

**Knock-on effects on diff sessions**, all downstream of the title never
changing — acceptable for a pane with no agent running in it, but they are
consequences, not no-ops:

- the spinner-transition attention trigger (`session.rs:1115-1121`);
- `agent_glyph`, which inspects the title (`session.rs:985`);
- `is_busy`, which reads the title (`session.rs:999`);
- BEL attention debounce, which polls the retained title (`app.rs:5027`).

### Scope

The title is not just the pane header. It propagates to the tab tooltip
(`app.rs:2283`), the sidebar session row (`app.rs:4932`), and command-palette
session entries (~`app.rs:5498`). Fixing it fixes all four.

---

## 2. Git panel header shows a UNC path for WSL workspaces

### Problem

The git panel prints its workspace path raw at `app.rs:3180`, producing
`\\wsl.localhost\kali-linux\home\lev\Git\adaptyv\monorepo` instead of
`/home/lev/Git/adaptyv/monorepo`.

This is not a discovery defect. `Project::discover_wsl` deliberately re-emits
every git-reported path through `wsl::linux_to_windows` (`projects.rs:103`) so
path equality survives refreshes, and `state.toml` persists the UNC form. The
header simply never converts back.

### Design

Add to `wsl.rs`, next to the existing converters:

```rust
/// How a workspace path should read to the user: WSL workspaces in the
/// distro's own spelling, native paths untouched.  Not `windows_to_linux`,
/// which also rewrites `C:\…` into `/mnt/c/…` — correct for handing a path
/// to git inside a distro, wrong for showing a Windows user their own path.
pub fn display_path(path: &Path) -> String {
    match classify(path) {
        Location::Wsl { linux_path, .. } => linux_path,
        Location::Windows(_) => path.display().to_string(),
    }
}
```

`classify` (`wsl.rs:62-87`) already handles both `\\wsl$\` and
`\\wsl.localhost\`, their verbatim UNC forms, and a path at the distro root
(which becomes `/`). Its tests at `wsl.rs:385-413` cover the plain prefixes and
the distro root but **not** the verbatim `\\?\UNC\…` spelling — add that case
alongside the `display_path` tests rather than assuming it works.

Known limitation, inherited not introduced: `classify` discards every
non-`Component::Normal` component (`wsl.rs:77`), so a UNC path containing `..`
renders as though the parent component were absent. This already affects
normalization; `display_path` makes it user-visible. Not fixed here.

### Call sites

Every user-visible rendering of a workspace or project path, so the same
worktree cannot read as `/home/lev/…` in one panel and `\\wsl.localhost\…` in
another:

| Site | Location |
| --- | --- |
| git panel header | `app.rs:3180` |
| `$path` in `worktree_name` template | `row_label.rs:58` |
| `$path` in `project_name` template | `row_label.rs:77` |
| command-palette workspace entry | `app.rs:5509` |
| command-palette project detail | `app.rs:5537` |
| command-palette session detail | `app.rs:5494-5498` |
| command-palette workspace primary label | `app.rs:5517-5530` |
| base-branch picker title | `app.rs:5759-5763` |
| project name fallback when `file_name()` is absent | `projects.rs:273-276` |

Desktop notification text (`app.rs:6495-6501`) was listed here and has been removed:
`notify_attention` takes `file_name()` and falls back to `session.title`, never to a
path, so no UNC spelling can reach it and there is nothing to convert.

The last few usually render only a leaf name, so an ordinary worktree never
shows the UNC spelling there. Root-only paths do — a distro root has no
`file_name()`, so `projects.rs:273` falls back to the full path. That is
exactly the case the bug report is about, so they are in scope.

This changes default output with no config, which is the point — it is the
reported bug.

---

## 3. `path_style` — abbreviated path rendering

### Config

Per-site table only. There is deliberately **no scalar shorthand**: TOML cannot
have `path_style = "zed"` and `[ui.path_style.filename]` in one document,
because that is the same key used two ways, so a shorthand would be unusable
with the emphasis in §4.

```toml
[ui.path_style]              # any omitted key is "full"
diff_title = "zed"           # the `diff: <path>` pane title
git_rows   = "fish"          # Staged / Unstaged / Changes-vs file rows
git_header = "full"          # the workspace path atop the git panel

[ui.path_style.filename]     # zed style only — see §4
color  = "#e6e6e6"
bold   = true

[ui.path_style.parent]
color  = "#6b6b6b"
```

An unrecognized style string warns and falls back to `full`, mirroring
`parse_scrollbar` (`config.rs:260`). Each key is parsed once in
`RawConfig::into_config`; three misspelled keys warn three times, which is
correct — they are three separate mistakes.

Emphasis colors reuse `RgbStr`, which **rejects a blank string**
(`config.rs:1120`), and any raw-schema error discards the entire merged config
(`config.rs:657`). So `color = ""` is a hard config error, not "same as
absent" — omit the key instead. Documented rather than worked around; a
tolerant parser here would diverge from every other color field.

Resolved form on `UiTheme`:

```rust
pub enum PathStyle { Full, Fish, Zed }   // Default::default() == Full

pub struct TextEmphasis {
    pub color: Option<Color32>,          // None inherits the site's normal color
    pub bold: bool,
    pub italic: bool,
}

pub struct PathStyleConfig {
    pub diff_title: PathStyle,
    pub git_rows: PathStyle,
    pub git_header: PathStyle,
    pub filename: TextEmphasis,
    pub parent: TextEmphasis,
}
```

Deliberately no `size` field: rows are laid out at `interact_size.y`, and a
larger span in one row would make section heights jitter.

### The module

New `alacritree/src/path_style.rs`, registered as `mod path_style;` in
`main.rs` alongside the existing modules. Pure and free of egui so it
unit-tests without a `Ui`:

```rust
pub struct Parts {
    pub root: String,     // "", "/", "C:\", "\\server\share\" — never abbreviated
    pub parent: String,   // keeps its trailing separator; empty for a bare name
    pub name: String,
}

pub fn split(path: &str, style: PathStyle, home: Option<&str>) -> Parts;
pub fn render(path: &str, style: PathStyle, home: Option<&str>) -> String;
```

#### Root token, then separator

The root is recognized **first**, and it decides the separator. Recognizing the
root by prefix rather than by scanning for separator characters is what makes
this safe: backslash and `:` are both legal inside a Unix filename, so neither
`dir/name\part.txt` nor `dir/name:\part` may be treated as Windows-spelled.

Roots, matched in this order:

| Prefix | Root token | Separator |
| --- | --- | --- |
| `\\?\UNC\server\share\` | the whole prefix | `\` |
| `\\?\C:\` | the whole prefix | `\` |
| `\\server\share\` | `\\server\share\` | `\` |
| `C:\` | `C:\` | `\` |
| `C:` (no separator) | `C:` — drive-relative, see below | `\` |
| `/` | `/` | `/` |
| anything else | `""` | `/` |

`\\wsl.localhost\distro\` and `\\wsl$\distro\` are ordinary UNC: the distro is
the *share*, so it belongs to the root and is never abbreviated. Anything not
matching a Windows root is POSIX, so a Unix filename containing `\` or `:` is
split only on `/`.

Splitting the root off before abbreviation is what keeps
`C:\Program Files\Git` from fish-abbreviating into `C\P\G`.

Windows paths spelled with forward slashes (`C:/Users/Lev`) match the `C:`
root and then split on **either** separator, re-joining with `\`.

#### Drive-relative paths

`C:foo` is relative to the drive's current directory, so no separator may be
inserted after the root. `root` therefore holds a possibly-separatorless
prefix, and rendering is plain concatenation `root + parent + name` — which is
why the root is stored as a string rather than a flag.

#### Edge cases

| Input | `Parts` (root, parent, name) | `render` (Fish) |
| --- | --- | --- |
| `""` | `""`, `""`, `""` | `""` |
| `/` | `/`, `""`, `""` | `/` |
| `C:\` | `C:\`, `""`, `""` | `C:\` |
| `C:` | `C:`, `""`, `""` | `C:` |
| `C:foo` | `C:`, `""`, `foo` | `C:foo` |
| `C:foo\bar` | `C:`, `foo\`, `bar` | `C:f\bar` |
| `f.txt` | `""`, `""`, `f.txt` | `f.txt` |
| `/f.txt` | `/`, `""`, `f.txt` | `/f.txt` |
| `a/b/` | `""`, `a/`, `b` | `a/b` |

A trailing separator is stripped and the last real segment becomes `name`; a
path that is only a root has an empty `name`.

**`Zed` re-emits the root too.** The earlier rule that a path with no parent
renders as the bare name applies to the *name-plus-parent* portion only —
`/f.txt` renders `/f.txt`, not `f.txt`. The root is never dropped and never
reordered; only parent and name swap.

#### Styles

- **`Full` is the identity.** `render` returns its input byte-for-byte, with no
  home collapsing. This is what keeps an untouched config unchanged.
- **Home collapse (`Fish` and `Zed` only).** When `home` is `Some(prefix)` and
  the path equals `prefix` or starts with `prefix` + separator, that span
  becomes `~`. Comparison is case-insensitive with separators normalized on
  Windows, so `c:\users\lev` collapses against `C:\Users\Lev`.
- **`Fish`.** Every parent segment collapses to its first character, keeping a
  leading dot so `.config` becomes `.c`. A `~` segment stays `~`. Root and the
  final segment are never abbreviated.
- **`Zed`.** Parent segments are kept whole; only the render order changes.

| Style | `path/to/file.txt` | `/home/lev/Git/x/y.rs`, home `/home/lev` |
| --- | --- | --- |
| `Full` | `path/to/file.txt` | `/home/lev/Git/x/y.rs` |
| `Fish` | `p/t/file.txt` | `~/G/x/y.rs` |
| `Zed` | `file.txt path/to/` | `y.rs ~/Git/x/` |

For `Zed`, a path with no parent renders as the bare name, no trailing space.

#### Where `home` comes from

The caller supplies it, so the module stays pure.

- **Native paths:** `home::home_dir()`.
- **WSL paths:** the distro's own `$HOME`, carried on `Project`. It is *not*
  inferred from the path — taking the first two segments of `/home/…` is a
  guess that is wrong for `/home/shared/repo` and misses `/root`.
- **Unknown:** `None`. No `~`, rather than a wrong `~`.

`$HOME` arrives as one more section on the existing `DISCOVER_SCRIPT`
(`projects.rs:282`), which already runs a `sep()`-delimited batch per project
inside the distro:

```sh
sep
printf '%s' "$HOME"
```

That costs no extra round-trip, needs no new cache, and stays on the background
discovery thread. Querying `wsl_helper::try_run` (`wsl_helper.rs:534`) at paint
time would block the UI thread — every other WSL query in this crate is already
computed off it, `StatusCache` being the model.

### Call sites

| Site | Source | Style key |
| --- | --- | --- |
| `open_diff` (`app.rs:3426`) | `req.file` | `diff_title` |
| `file_row` (`app.rs:3959`) | `change.path` | `git_rows` |
| `branch_diff_row` (`app.rs:4027`) | `stat.path` | `git_rows` |
| git panel header (`app.rs:3180`) | `wsl::display_path(&path)` | `git_header` |

Ordering at the header: `display_path` first (§2), then `path_style` — so the
style operates on `/home/lev/…`, not on the UNC form.

`worktree_name` / `project_name` templates get §2's conversion but not
`path_style`; they already have their own substitution language.

---

## 4. Zed-style filename emphasis

`filename` and `parent` emphasis apply **only to `Zed`**, and only at the two
egui sites (`git_rows`, `git_header`).

### One label, not two

The row renders as a single `Label` built from a `LayoutJob` with two
differently-formatted sections in `filename parent/` order. Two labels would:

- introduce `item_spacing.x` between them, so the gap is not the single space
  the style specifies;
- let an untruncated filename overflow `row_with_trailing`, which deliberately
  manages remaining width (`app.rs:4187`);
- split one response into two, complicating click fall-through and tooltips.

A single job keeps one widget, one response, exact spacing, existing monospace
sizing, click fall-through, and accessibility metadata.

The filename is *prioritized*, not guaranteed: epaint lays out one linear glyph
stream and truncates the suffix (`epaint-0.31.1/src/text/text_layout.rs:197-338`),
so putting the filename first means the parent is eaten before it — but a row
narrower than the filename plus the overflow marker still truncates the
filename. Two labels would be strictly worse; neither arrangement makes it
untouchable.

`Full` and `Fish` stay exactly as they are today: one plain truncating label,
no emphasis, no `LayoutJob`.

### The diff title cannot carry styling

`Session.title` is a `String` painted as one label, so `diff_title = "zed"`
yields plain `diff: file.txt path/to/`.

---

## 5. Hover tooltips revealing the untruncated text

### What already exists

egui 0.31 already does this. `Label::ui` adds the full text as a tooltip
whenever the galley elided (`egui-0.31.1/src/widgets/label.rs:256-259`):

```rust
if galley.elided {
    // Show the full (non-elided) text on hover:
    response = response.on_hover_text(galley.text());
}
```

So every `.truncate()` label in the sidebar is *supposed* to reveal its full
text on hover, gated on actual elision. Rows also carry their own tooltips
already — prunable worktrees at `app.rs:4842`, PR badges and controls at
`app.rs:4812-4835`.

### Problem

The behavior is not consistent across platforms: on Windows these tooltips do
not reliably appear. The requirement is identical behavior on Windows, Linux,
and macOS.

The mechanism is **not yet confirmed**. What the code establishes: the path
label is `.selectable(false)` (`app.rs:3964`), which egui senses as
`Sense::hover()` (`label.rs:128-135`), and the row then calls
`.interact(Sense::click())` on the enclosing rect *after* the label is
registered (`app.rs:3970`). Whether the label's tooltip fires therefore depends
on egui resolving hover between a hover-sense label and a later-registered
click-sense row covering the same pixels. `app.rs:4848` already documents this
ordering causing trouble for clicks. Credible, unproven.

### Why re-deriving elision is not the fix

The tempting design — lay the galley out ourselves, read `galley.elided`, hand
it to `Label::new(galley)` — does nothing. `Label::ui` computes that same flag
from that same field and installs the tooltip itself (`label.rs:250-259`), and
a supplied galley takes the fast path at `label.rs:154-162`, leaving the hover
response, which is the suspect, exactly as it was. "Owning the flag" is not
owning the behavior.

Nor can the built-in tooltip be turned off: egui exposes no such option, and an
elided galley triggers it unconditionally. So "attach our own tooltip to the
row" does not replace the label's — it stacks a second one wherever the label's
hover *does* work.

That rules out the custom-layout approach entirely. It would ship a
reimplementation of `WidgetText::into_galley`'s wrap/valign/halign preparation
(`widget_text.rs:673-705`, skipped by the fast path) and still leave the
reported Windows bug untouched.

### This is a diagnosis task

The fix cannot be designed before the cause is known, so §5 does not propose
one. Under `superpowers:systematic-debugging`:

1. In the GUI verification lab, on Windows and on the WSL/kali build, hover an
   elided git row and an elided worktree row.
2. Record which response reports `hovered()` — the label's or the row's — and
   whether `galley.elided` is true at that width.
3. Compare. The row's retroactive `.interact()` shadowing the label
   (`app.rs:3970`, the hazard already documented at `app.rs:4848`) is the
   leading hypothesis; confirm or kill it before writing code.

Expected shapes of the answer:

- **The row shadows the label.** Fix the shadowing — ordering or sense — so the
  label's own tooltip fires. No custom layout, no second tooltip, no config
  flag: the behavior already exists and merely becomes reliable. This is the
  outcome the evidence currently points at.
- **The label is hovered on both, but nothing paints on Windows.** Then the
  cause is below the widget layer and the fix is scoped once identified.

### Config

If the fix is "make the existing tooltip reliable", none — that is a bug fix
and Linux behavior is the target. A config flag is needed only if the outcome
deliberately moves the hover target from the label to the whole row, which is
new UX and would default to off.

---

## Invariants

- **`path_style` and the §4 emphasis are inert by default.** `PathStyle::Full`
  everywhere, one plain truncating label per path, no home collapsing. A guard
  test asserts `render(p, Full, home) == p` for absolute, relative, Windows,
  UNC, and WSL inputs.
- **§1, §2, and §5 deliberately change default output.** They are the reported
  bugs; gating them behind config would leave the bugs in place. The
  `render(Full)` test does not and cannot cover them — §1 is covered by the
  `apply_term_event` test, §2 by `wsl::display_path` tests, §5 by the lab
  measurement.
- **Formatting is display-only.** `diff_key`, the git filter, `git_nav`
  cursors, and `paint_git_row_cursor` all keep operating on the raw path.
  Typing `src/git` in the filter still matches a row displayed as
  `s/g/status.rs`.
- **`wsl::display_path` never touches native paths.**
- Shell sessions keep accepting title events; only `SessionKind::Diff` pins.

## Testing

`path_style.rs` is pure, so most of this is table-driven: each style against
relative, absolute, home-prefixed, dotted, single-segment, empty, root-only,
trailing-separator, drive-letter, UNC, and backslash-in-a-Unix-filename inputs.

Config tests mirror the existing `ui_from_toml` helpers: per-site table,
partial table defaulting to `full`, unknown style warning to `full`, absent
emphasis fields, and `color = ""` rejected.

For the ConPTY pin, `apply_term_event` is already unit-tested without a PTY
(`session.rs:1274-1279`). The regression test must go one level up: enqueue
`TermEvent::Title("C:\\Program Files\\Git\\cmd\\git")` and call
`Session::drain_events` on a session whose `kind` is `SessionKind::Diff`,
asserting the title stays `diff: src/app.rs` — plus the inverse for
`SessionKind::Shell`, and that `Bell` / `ChildExit` still work while pinned.

Calling `apply_term_event` with a hand-passed `pinned` would only exercise the
event arm; the thing that can break is the `self.kind → title_pinned` step
inside `drain_events`, and only `drain_events` covers it. RED is proven by
reverting the pin before the fix lands.

The crate has no PTY harness, so a real ConPTY spawn cannot be asserted in
CI — that path needs a manual check in the GUI verification lab, as does the
§5 measurement.
