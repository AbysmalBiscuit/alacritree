# Move tool-specific `[ui]` keys into `[integrations]` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `ui.pr_status`, `ui.pr_status_concurrency` and `ui.icons.herdr` to `integrations.gh.pr_status`, `integrations.gh.pr_status_concurrency` and `integrations.herdr.icon`, in the config file and in the resolved `Config` alike, keeping each old file key as a deprecated fallback.

**Architecture:** The raw TOML structs gain the new keys, and the old raw fields become `Option`s marked deprecated. `into_config` takes the old values out of `RawUi` first and hands them to `RawIntegrations::resolve` as one `MovedUiKeys` bundle, beside the `delta_path` it already carries. One helper, `moved_key`, applies the rule for every moved key: the old value holds while the new key sits at its default, and loading the old key logs a warning. The resolved config mirrors the file: `IntegrationsConfig::gh` becomes a `GhConfig` carrying the two PR keys, `HerdrConfig` gains `icon`, and `UiTheme` and `Icons` lose theirs. Readers in `app.rs`, `app/sidebar.rs` and `app/palette.rs` follow. The startup log's settings dump then names the same path the file uses.

**Tech Stack:** Rust 2024, serde, schemars, `log`, egui, nextest through `devkit run task`.

**Spec:** https://github.com/AbysmalBiscuit/alacritree/issues/96, plus decisions settled in session:
- The fallbacks warn like `[ui] delta_path`. `[ui.wsl] automount_root` does not warn and is not the model.
- `integrations.herdr.icon` keeps the `RawIconStyle` shape, a bare glyph string or a styling table.
- `RawGh` is written out by hand the way `RawHerdr` is.
- The runtime fields move in this PR too. It lands after #83 and #91, and the conflicts get resolved at rebase.
- A new key written at its default counts as unset, the same as `delta_config` treats it.
- Doc comments and docs name no default values. Defaults live in the `Raw*` `Default` impls, where the schema publishes them.

## Global Constraints

- Worktree: `C:/Users/Lev/Git/github/alacritree-worktrees/refactor-config-move-tool-specific-ui-keys`, written `<worktree>` below. Branch base: `upstream/feat-ux-configurable-diff-viewer` (PR #233). Edit only `alacritree/`, `docs/` and `schema/` inside it.
- Claim each file before editing, from inside the worktree: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "issue 96"`.
- Run checks through devkit: `devkit run task check|test|clippy|fmt --dir <worktree>`. The task passes no extra arguments to cargo, so a single test runs as the whole `test` task. Read the nextest output for the named test.
- New config keys publish their default in the schema. A key with no fixed default goes in `alacritree/tests/schema-defaults-allowlist.txt`.
- Doc comments and docs written or moved by this PR state no default values. They follow unslop: no em dashes, no parentheses as asides, no colon connectors, plain words. Comments explain why and never narrate the change.
- Conventional Commits, subject at most 50 characters, body wrapped at 72, trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- This issue's parent is #92, not #70. Open no PR and integrate nothing into `all-features` unless Lev asks.

---

### Task 1: `[integrations.gh]` owns `pr_status` and `pr_status_concurrency`

**Files:**
- Modify: `alacritree/src/config.rs`:
  - `IntegrationsConfig` and its `Default`, near 519 and 554
  - `ToolConfig`, near 579
  - `UiTheme::pr_status` and `pr_status_concurrency` and their `Default`, near 1320, 1336, 1420 and 1423
  - `raw_tool_table!(RawGh, "gh")`, near 2834
  - `RawIntegrations::resolve`, near 2947
  - `delta_config`, near 2970
  - `RawUi` fields and `Default`, near 3173, 3188, 3255 and 3258
  - `into_config`, near 3429, 3526, 3529 and 3700
  - tests near 5107 and 5118
- Modify: `alacritree/src/app.rs:485` and `alacritree/src/app.rs:648`
- Modify: `alacritree/src/app/sidebar.rs:455`
- Modify: `alacritree/src/bindings.rs:565-574`
- Modify: `alacritree/src/cli/schema.rs`, the `STARTER` template near 107 and tests near 207, 225, 304 and 329
- Modify: `docs/alacritree.md`, the `[ui]` block near 512, the `worktree_name` comment near 531, the `[ui.icons]` comment near 588, and a new `[integrations.gh]` block after `[integrations.git]` near 640
- Modify: `docs/keyboard-shortcuts.md:319-320`
- Regenerate: `schema/alacritree-config.json`, `alacritree/tests/schema-defaults-allowlist.txt`, `alacritree/tests/stock-config.json`

**Interfaces:**
- Produces: `fn moved_key<T: PartialEq>(new: T, default: T, old: Option<T>, from: &str, to: &str) -> T`.
- Produces: `#[derive(Default)] struct MovedUiKeys { delta_path: Option<String>, pr_status: Option<bool>, pr_status_concurrency: Option<usize> }`. Task 2 adds `herdr_icon: Option<RawIconStyle>`.
- Produces: `fn RawIntegrations::resolve(self, moved: MovedUiKeys) -> IntegrationsConfig`, replacing `resolve(self, deprecated_delta_path: Option<String>)`.
- Produces: `pub struct GhConfig { pub path: String, pub wsl_path: Option<String>, pub pr_status: bool, pub pr_status_concurrency: Option<usize> }`, and `IntegrationsConfig::gh: GhConfig`. `IntegrationsConfig::paths` keeps reading `self.gh.path` and `self.gh.wsl_path` and needs no edit.
- Removes: `UiTheme::pr_status` and `UiTheme::pr_status_concurrency`.

- [ ] **Step 1: Write the failing tests**

In the `config.rs` tests, replace `pr_status_defaults_off_and_parses_on` and `pr_status_concurrency_is_unset_by_default` with these three:

```rust
    #[test]
    fn pr_status_reads_from_integrations_gh() {
        let stock = config_from("");
        assert!(!stock.integrations.gh.pr_status);
        assert_eq!(stock.integrations.gh.pr_status_concurrency, None);

        let set = config_from("[integrations.gh]\npr_status = true\npr_status_concurrency = 4\n");
        assert!(set.integrations.gh.pr_status);
        assert_eq!(set.integrations.gh.pr_status_concurrency, Some(4));
    }

    /// The old `[ui]` keys keep working, and each new key wins once it moves
    /// off its default.
    #[test]
    fn the_deprecated_ui_pr_status_keys_apply_until_integrations_gh_sets_them() {
        let old = config_from("[ui]\npr_status = true\npr_status_concurrency = 3\n");
        assert!(old.integrations.gh.pr_status);
        assert_eq!(old.integrations.gh.pr_status_concurrency, Some(3));

        let both = config_from(
            "[ui]\npr_status = false\npr_status_concurrency = 3\n[integrations.gh]\npr_status = \
             true\npr_status_concurrency = 5\n",
        );
        assert!(both.integrations.gh.pr_status);
        assert_eq!(both.integrations.gh.pr_status_concurrency, Some(5));
    }

    /// The startup log reports a setting under the table the file now uses,
    /// even when the file still spells it the old way.
    #[test]
    fn a_moved_key_is_dumped_under_its_new_table() {
        let json = changed("[ui]\npr_status = true\n");

        assert_eq!(json["integrations"]["gh"]["pr_status"], serde_json::json!(true));
        assert!(json.get("ui").is_none(), "{json}");
    }
```

In the `cli/schema.rs` tests, point both schema assertions at `RawGh`:

```rust
    #[test]
    fn doc_comments_reach_the_schema_as_hover_text() {
        let schema = parsed();
        let gh = &schema["$defs"]["RawGh"]["properties"];
        assert!(
            gh["pr_status"]["description"].as_str().unwrap().contains("pull request"),
            "descriptions are the only documentation an editor shows"
        );
    }
```

```rust
        let pr_status = &schema["$defs"]["RawGh"]["properties"]["pr_status"];
        assert_eq!(pr_status["type"], "boolean");
        assert_eq!(pr_status["default"], serde_json::json!(false));
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devkit run task test --dir <worktree>`

Expected: the build fails with `error[E0609]: no field `pr_status` on type `ToolConfig``, pointing at the new `config.rs` tests. That is the missing runtime field this task adds. Any other compile error, such as a typo in a test, gets fixed before going on.

- [ ] **Step 3: Implement the raw keys and their resolution**

Replace `raw_tool_table!(RawGh, "gh");` with a table written out by hand, plus its `resolve`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(default)]
struct RawGh {
    /// The program to run on Windows or natively. Its own name is looked up
    /// on PATH; any other value runs as written.
    path: String,
    /// The program to run inside every WSL distro, as written. Empty finds it
    /// by name through the distro's login shell.
    wsl_path: String,
    /// Poll `gh` for each branch's open pull request, which drives the PR row
    /// icons, the PR-state filters, and `$pr` in row templates.
    pr_status: bool,
    /// Max `gh` lookups in flight at once. Unset lets the pool decide, which
    /// is one below its own background ceiling so a lookup can never take
    /// the last slot local work needs. A value lowers that; nothing raises
    /// it, because the pool's ceiling binds underneath either way.
    pr_status_concurrency: Option<usize>,
}

impl Default for RawGh {
    fn default() -> Self {
        Self {
            path: "gh".to_string(),
            wsl_path: String::new(),
            pr_status: false,
            pr_status_concurrency: None,
        }
    }
}

impl RawGh {
    fn resolve(self, moved: &MovedUiKeys) -> GhConfig {
        let default = Self::default();
        let tool = tool_config(self.path, self.wsl_path, Tool::Gh);
        GhConfig {
            path: tool.path,
            wsl_path: tool.wsl_path,
            pr_status: moved_key(
                self.pr_status,
                default.pr_status,
                moved.pr_status,
                "[ui] pr_status",
                "[integrations.gh] pr_status",
            ),
            pr_status_concurrency: moved_key(
                self.pr_status_concurrency,
                default.pr_status_concurrency,
                moved.pr_status_concurrency.map(Some),
                "[ui] pr_status_concurrency",
                "[integrations.gh] pr_status_concurrency",
            ),
        }
    }
}
```

Add the bundle and the helper next to `delta_config`:

```rust
/// Settings that used to live under `[ui]`, taken out of the raw `[ui]`
/// table so the `[integrations]` table that owns them now can resolve them.
#[derive(Default)]
struct MovedUiKeys {
    delta_path: Option<String>,
    pr_status: Option<bool>,
    pr_status_concurrency: Option<usize>,
}

/// A key that moved keeps working from where it used to live. The old value
/// applies while the new key sits at its default, because raw config structs
/// accept unknown keys and dropping the old field would lose the override
/// without a word.
fn moved_key<T: PartialEq>(new: T, default: T, old: Option<T>, from: &str, to: &str) -> T {
    let Some(old) = old else {
        return new;
    };
    log::warn!("{from} is deprecated; set {to}");
    if new == default { old } else { new }
}
```

`RawIntegrations::resolve` takes the bundle:

```rust
impl RawIntegrations {
    fn resolve(self, moved: MovedUiKeys) -> IntegrationsConfig {
        IntegrationsConfig {
            git: tool_config(self.git.path, self.git.wsl_path, Tool::Git),
            gh: self.gh.resolve(&moved),
            doppler: tool_config(self.doppler.path, self.doppler.wsl_path, Tool::Doppler),
            herdr: self.herdr.resolve(),
            delta: delta_config(self.delta.path, self.delta.wsl_path, moved.delta_path),
            tuicr: tool_config(self.tuicr.path, self.tuicr.wsl_path, Tool::Tuicr),
            diff_viewer: self.diff_viewer.resolve(),
        }
    }
}
```

`impl Default for IntegrationsConfig` calls `RawIntegrations::default().resolve(MovedUiKeys::default())`.

In `RawUi`, both fields become deprecated `Option`s, and `impl Default for RawUi` sets both to `None`:

```rust
    /// Deprecated location: `[integrations.gh] pr_status` supersedes this and
    /// wins once set.
    pr_status: Option<bool>,
```

```rust
    /// Deprecated location: `[integrations.gh] pr_status_concurrency`
    /// supersedes this and wins once set.
    pr_status_concurrency: Option<usize>,
```

In `into_config`, change the signature to `fn into_config(mut self) -> Config`. Add the bundle as its first line:

```rust
        let moved = MovedUiKeys {
            delta_path: self.ui.delta_path.take(),
            pr_status: self.ui.pr_status,
            pr_status_concurrency: self.ui.pr_status_concurrency,
        };
```

Delete the `pr_status: self.ui.pr_status,` and `pr_status_concurrency: self.ui.pr_status_concurrency,` lines from the `UiTheme` literal. Change `integrations: self.integrations.resolve(self.ui.delta_path),` to `integrations: self.integrations.resolve(moved),`.

- [ ] **Step 4: Move the runtime fields and their readers**

In `config.rs`, add `GhConfig` after `ToolConfig`. Its field docs come from `ToolConfig` and the two `UiTheme` fields, with the default-value wording dropped:

```rust
/// `[integrations.gh]`: where the GitHub CLI lives and whether the sidebar
/// asks it about pull requests.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GhConfig {
    /// The tool's own name, or a native path that runs as written.
    pub path: String,
    /// A path that runs as written inside every WSL distro, or `None` to
    /// find the tool by name there.
    pub wsl_path: Option<String>,
    /// Paint PR-status badges on worktree rows and poll `gh` for expanded
    /// projects' worktrees. Best-effort like the diff-base lookup: no `gh`,
    /// no auth, or no PR paints nothing.
    pub pr_status: bool,
    /// Max `gh` lookups in flight at once. Unset lets the pool decide, which
    /// is one below its own background ceiling so a lookup can never take the
    /// last slot local work needs. A value lowers that; nothing raises it,
    /// because the pool's ceiling binds underneath either way.
    pub pr_status_concurrency: Option<usize>,
}
```

Change `IntegrationsConfig::gh` to `pub gh: GhConfig,`. Remove `pr_status` and `pr_status_concurrency`, with their doc comments, from `UiTheme` and from `impl Default for UiTheme`.

Readers:
- `app.rs:485`: `config.ui.pr_status,` becomes `config.integrations.gh.pr_status,`
- `app.rs:648`: `let pr_status_concurrency = config.ui.pr_status_concurrency;` becomes `let pr_status_concurrency = config.integrations.gh.pr_status_concurrency;`
- `app/sidebar.rs:455`: `let pr_enabled = self.config.ui.pr_status;` becomes `let pr_enabled = self.config.integrations.gh.pr_status;`
- `bindings.rs:565-574`: each `(requires [ui] pr_status)` becomes `(requires [integrations.gh] pr_status)`

In `cli/schema.rs`, the starter moves the line under its own table:

```
# [ui]
# sidebar_accent = "#6a9fb5"
# upstream_status = true    # badge each worktree with its branch's upstream state

# [integrations.gh]
# pr_status = true          # poll `gh` for each branch's open pull request
```

`init_leaves_a_config_that_already_names_a_schema_alone` and `init_keeps_the_settings_a_config_already_had` only need some setting as sample content. Swap `[ui]\npr_status = true\n` for `[ui]\nupstream_status = true\n`, and the `contains("pr_status = true")` assertion for `contains("upstream_status = true")`, so no test writes a deprecated key.

Run `rg -n "ui\.pr_status|\[ui\] pr_status" <worktree>/alacritree <worktree>/docs`. Expected: matches only in the deprecation doc comments and warning strings written above.

- [ ] **Step 5: Update the docs**

In `docs/alacritree.md`:
- Delete the `pr_status` and `pr_status_concurrency` lines, with their comment continuation lines, from the `[ui]` block.
- In the `worktree_name` comment, `needs` / `pr_status)` becomes `needs` / `[integrations.gh] pr_status)`. Rewrap the comment column if it overflows.
- In `[ui.icons]`, `the four PR glyphs need pr_status = true` becomes `the four PR glyphs need [integrations.gh] pr_status = true`, rewrapped the same way.
- After the `[integrations.git]` block, insert the block below. The sample values show what a user might write, and the comments name no defaults:

```toml
[integrations.gh]
path     = "gh"             # set like [integrations.git] above
wsl_path = ""
pr_status = true            # poll `gh` for each branch's open PR, which drives
                            # the PR row icons, the PR-state filters, and $pr
                            # in row templates. Supersedes the deprecated
                            # [ui] pr_status
pr_status_concurrency = 4   # cap concurrent `gh` PR lookups, minimum 1. Unset
                            # stays one below the job pool's background
                            # ceiling, which also caps any value set here.
                            # Supersedes the deprecated [ui]
                            # pr_status_concurrency
```

In `docs/keyboard-shortcuts.md:319-320`, `` `[ui] pr_status =`` / ``true` (default `false`); `` becomes `` `[integrations.gh]`` / ``pr_status = true`; ``. Keep the file's hard wrap.

- [ ] **Step 6: Regenerate the committed fixtures**

Run: `devkit run task test --dir <worktree> --env ALACRITREE_UPDATE_SCHEMA=1 --env ALACRITREE_UPDATE_ALLOWLIST=1 --env ALACRITREE_UPDATE_STOCK=1`

If `devkit run task test` rejects `--env`, read `devkit run task --help` and use the flag it names.

Then: `git -C <worktree> diff -- schema/alacritree-config.json alacritree/tests/schema-defaults-allowlist.txt alacritree/tests/stock-config.json`

Expected:
- **Allowlist.** `+RawGh.pr_status_concurrency` and `+RawUi.pr_status`, nothing else. `RawUi.pr_status_concurrency` stays.
- **Schema.** `RawGh` gains `pr_status` with `"default": false`, plus `pr_status_concurrency`. `RawUi.pr_status` loses its default and takes the deprecation description.
- **Stock config.** `"pr_status": false` and `"pr_status_concurrency": null` leave `ui` and appear under `integrations.gh` with the same values.
- **Anything else.** Any value change, or any other allowlist line, is a mistake to find before going on.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `devkit run task test --dir <worktree>`

Expected: everything passes, including the three tests from Step 1, the two edited `cli/schema.rs` tests, `config_schema`, `schema_defaults`, `the_stock_config_is_unchanged` and `the_deprecated_ui_delta_path_fills_each_side_left_at_its_default`.

- [ ] **Step 8: Commit**

```bash
git -C <worktree> add alacritree/src/config.rs alacritree/src/app.rs alacritree/src/app/sidebar.rs alacritree/src/bindings.rs alacritree/src/cli/schema.rs docs/alacritree.md docs/keyboard-shortcuts.md schema/alacritree-config.json alacritree/tests/schema-defaults-allowlist.txt alacritree/tests/stock-config.json
git -C <worktree> diff --staged --stat
git -C <worktree> commit -m "refactor(config): move pr_status into [integrations.gh]" -m "The pr_status keys only control how gh is polled, so they move into
gh's own table, in the file and in the resolved config. [ui] pr_status
and pr_status_concurrency still apply while the new keys sit at their
defaults, and loading them logs a deprecation warning.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `[integrations.herdr]` owns the herdr icon

**Files:**
- Modify: `alacritree/src/config.rs`:
  - `HerdrConfig` doc and fields, near 618
  - `impl Default for HerdrConfig`, near 645
  - `Icons` field, near 1019
  - `Icons::map_colors`, near 1048
  - `RawIcons.herdr` and its `Default`, near 2459 and 2512
  - `build_icons`, near 2542
  - `RawHerdr` and its `Default` and `resolve`, near 2994, 3044 and 3058
  - `MovedUiKeys` and `RawIntegrations::resolve`, from Task 1
  - `into_config`'s `moved` binding, from Task 1
  - tests near 4013 and 5630
- Modify: `alacritree/src/app.rs`:
  - the `icons` field doc and a new `herdr_icon` field, near 385
  - construction, near 505
  - `session_row` test calls at 7313, 7416, 7432, 7442 and 7634
  - `app.config.ui.icons.herdr` in tests at 4686, 4747, 4810, 4875, 6275 and 6296
- Modify: `alacritree/src/app/sidebar.rs`:
  - `SidebarPaint`, near 665
  - its construction, near 222
  - `session_row` call, near 923
  - `herdr_row` call, near 950
  - `session_row`, near 1749
  - `paint_managed_mark`, near 1910
  - `herdr_row`, near 1930
- Modify: `alacritree/src/app/palette.rs:155`
- Modify: `docs/alacritree.md`, the `[integrations.herdr]` block near 674 and "Icon styling" near 800
- Regenerate: `schema/alacritree-config.json`, `alacritree/tests/schema-defaults-allowlist.txt`, `alacritree/tests/stock-config.json`

**Interfaces:**
- Consumes: `moved_key`, `MovedUiKeys` and `RawIntegrations::resolve(self, moved: MovedUiKeys)` from Task 1.
- Produces: `HerdrConfig::icon: IconStyle`, and `fn RawHerdr::resolve(self, old_icon: Option<RawIconStyle>) -> HerdrConfig`.
- Produces: `MovedUiKeys::herdr_icon: Option<RawIconStyle>`, and `RawIcons.herdr: Option<RawIconStyle>`.
- Produces: `fn paint_managed_mark(ui: &mut egui::Ui, icon: &IconStyle<Color32>, theme: &Theme, color: Color32) -> egui::Rect`.
- Produces: `session_row(ui, row, is_cursor, scroll_into_view, draggable, icons, herdr_icon: &IconStyle<Color32>, theme)`. `herdr_row` swaps its `icons: &Icons<Color32>` for `herdr_icon: &IconStyle<Color32>`.
- Produces: `AlacritreeApp::herdr_icon: IconStyle<Color32>`, and `SidebarPaint::herdr_icon: &'a IconStyle<Color32>`.
- Removes: `Icons::herdr`, and `fn build_icons` no longer reads `raw.herdr`.

- [ ] **Step 1: Write the failing tests**

In the `config.rs` tests:

```rust
    #[test]
    fn the_herdr_icon_reads_from_integrations_herdr_in_either_form() {
        let stock = config_from("");
        assert_eq!(stock.integrations.herdr.icon.or_glyph(""), DEFAULT_HERDR_ICON.as_str());

        let bare = config_from("[integrations.herdr]\nicon = \"✦\"\n");
        assert_eq!(bare.integrations.herdr.icon.or_glyph(""), "✦");

        let styled = config_from(
            "[integrations.herdr]\nicon = { glyph = \"✦\", bold = true, size = 8 }\n",
        );
        assert_eq!(
            styled.integrations.herdr.icon,
            IconStyle { glyph: Some("✦".into()), bold: true, size: Some(8.0), ..Default::default() }
        );
    }

    /// A table under the old key keeps its styling, and the new key wins once
    /// it moves off the built-in glyph.
    #[test]
    fn the_deprecated_ui_icons_herdr_applies_until_integrations_herdr_sets_icon() {
        let old = config_from("[ui.icons]\nherdr = { glyph = \"✦\", bold = true }\n");
        assert_eq!(
            old.integrations.herdr.icon,
            IconStyle { glyph: Some("✦".into()), bold: true, ..Default::default() }
        );

        let both =
            config_from("[ui.icons]\nherdr = \"✦\"\n[integrations.herdr]\nicon = \"◆\"\n");
        assert_eq!(both.integrations.herdr.icon.or_glyph(""), "◆");
    }
```

In `app.rs` tests, the six `app.config.ui.icons.herdr.glyph = ...` lines at 4686, 4747, 4810, 4875, 6275 and 6296 become `app.config.integrations.herdr.icon.glyph = ...`. `configured_workspace_labels_and_herdr_icons_reach_palette_items` near 6268 then proves the palette reads the new field.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devkit run task test --dir <worktree>`

Expected: the build fails with `error[E0609]: no field `icon` on type `HerdrConfig``, at the new `config.rs` tests and the six `app.rs` lines. Any other compile error gets fixed first.

- [ ] **Step 3: Implement the raw key and its resolution**

In `RawIcons`, replace the `herdr` field:

```rust
    /// Deprecated location: `[integrations.herdr] icon` supersedes this and
    /// wins once set.
    #[serde(skip_serializing_if = "Option::is_none")]
    herdr: Option<RawIconStyle>,
```

In `impl Default for RawIcons`, `herdr: raw_glyph(DEFAULT_HERDR_ICON),` becomes `herdr: None,`. In `build_icons`, delete `herdr: raw.herdr.into(),`. In `Icons<C>`, delete `pub herdr: IconStyle<C>,`. In `Icons::map_colors`, delete `herdr: self.herdr.map_color(f),`.

In `RawHerdr`, add the field after `wsl_path`, and `icon: raw_glyph(DEFAULT_HERDR_ICON),` to its `Default`:

```rust
    /// The glyph on a herdr pane's sidebar row and palette entry. A bare
    /// string sets the glyph; a table also styles its color, weight, slant
    /// and size, the way `[ui.icons]` keys do.
    icon: RawIconStyle,
```

`RawHerdr::resolve` takes the old key:

```rust
    fn resolve(self, old_icon: Option<RawIconStyle>) -> HerdrConfig {
        let default_icon = IconStyle::from(Self::default().icon);
        let tool = tool_config(self.path, self.wsl_path, Tool::Herdr);
        HerdrConfig {
            path: tool.path,
            wsl_path: tool.wsl_path,
            icon: moved_key(
                self.icon.into(),
                default_icon,
                old_icon.map(IconStyle::from),
                "[ui.icons] herdr",
                "[integrations.herdr] icon",
            ),
            enabled: self.enabled,
            poll_interval: Duration::from_millis(self.poll_interval_ms),
            show_unmatched: self.show_unmatched,
            show_panes: self.show_panes,
            attach: parse_closed_set("integrations.herdr.attach", &self.attach),
            follow_focus: parse_closed_set("integrations.herdr.follow_focus", &self.follow_focus),
        }
    }
```

Add `herdr_icon: Option<RawIconStyle>,` to `MovedUiKeys`. In `RawIntegrations::resolve`, `herdr: self.herdr.resolve(),` becomes `herdr: self.herdr.resolve(moved.herdr_icon),`. It must come after `gh: self.gh.resolve(&moved),` and before `moved.delta_path` is moved, which the field order in Task 1 already gives. In `into_config`'s `moved` binding, add `herdr_icon: self.ui.icons.herdr.take(),`.

`impl Default for HerdrConfig` calls `RawHerdr::default().resolve(None)`. The test near 4013 becomes `assert_eq!(HerdrConfig::default(), RawHerdr::default().resolve(None));`.

- [ ] **Step 4: Move the runtime field and its readers**

In `HerdrConfig`, add after `wsl_path`:

```rust
    /// The glyph on a herdr pane's sidebar row and palette entry.
    pub icon: IconStyle,
```

Its struct doc drops the default wording:

```rust
/// `[integrations.herdr]`: whether alacritree lists agents running under a
/// herdr server in the sidebar, what opening one attaches to, and the glyph
/// that marks them. A probe with no herdr binary or server present costs
/// nothing.
```

The `IntegrationsConfig` doc comment says the herdr glyph is not a setting there, which is no longer true. Its last sentence goes:

```rust
/// `[integrations]`: how alacritree invokes external tools and the controls
/// specific to those integrations, such as the diff viewer and its section
/// buttons. General sidebar and terminal appearance stays under `[ui]`.
```

In `app/palette.rs:155`, `self.config.ui.icons.herdr.or_glyph(...)` becomes `self.config.integrations.herdr.icon.or_glyph(...)`.

In `app.rs`, add a field after `icons` near 386:

```rust
    /// `config.integrations.herdr.icon` with its color converted for painting.
    herdr_icon: IconStyle<Color32>,
```

Construct it after `icons` near 505, while `config` is still borrowable:

```rust
            herdr_icon: config.integrations.herdr.icon.map_color(rgb_to_color32),
```

In `app/sidebar.rs`:

```rust
/// A view paired with the app's icon set.  The icons stay a borrow of their
/// own field, so the panel closure can still borrow `projects` mutably.
#[derive(Clone, Copy)]
struct SidebarPaint<'a> {
    view: &'a SidebarView,
    icons: &'a Icons<Color32>,
    herdr_icon: &'a IconStyle<Color32>,
}
```

- Construction near 222: `let paint = SidebarPaint { view: &view, icons: &self.icons, herdr_icon: &self.herdr_icon };`
- `paint_managed_mark`: its second parameter becomes `icon: &IconStyle<Color32>`, and its call becomes `resolve_icon(icon, DEFAULT_HERDR_ICON, color, 10.0, 10.0, theme);`
- `herdr_row`: the parameter `icons: &Icons<Color32>` becomes `herdr_icon: &IconStyle<Color32>`, and `paint_managed_mark(ui, icons, theme, theme.text_dim)` becomes `paint_managed_mark(ui, herdr_icon, theme, theme.text_dim)`. The call near 950 passes `paint.herdr_icon` in place of `paint.icons`.
- `session_row`: add `herdr_icon: &IconStyle<Color32>,` after `icons`. The function now takes eight arguments, so add `#[allow(clippy::too_many_arguments)]` above it, as `git_panel.rs:887` and `terminal_view.rs` do. `paint_managed_mark(ui, icons, theme, theme.text_muted)` becomes `paint_managed_mark(ui, herdr_icon, theme, theme.text_muted)`. The call near 923 passes `paint.herdr_icon,` after `paint.icons,`.

In the `app.rs` tests, the five `session_row(ui, &x, false, false, false, &icons, &theme)` calls at 7313, 7416, 7432, 7442 and 7634 become `session_row(ui, &x, false, false, false, &icons, &herdr_icon, &theme)`. Each enclosing test gets, beside its `let icons = ...` line:

```rust
        let herdr_icon = HerdrConfig::default().icon.map_color(rgb_to_color32);
```

Add `HerdrConfig` to that test module's `use crate::config::{...}` if it is not imported there.

In the test near 5630, `every_icon_default_round_trips`, `build_icons(back)` and `build_icons(raw)` keep their one argument. `RawIcons::default()` now serializes without `herdr`, and deserializing gives `None` back, so the assertion holds unchanged.

Run `rg -n "icons\.herdr|ui\.icons\.herdr" <worktree>/alacritree <worktree>/docs`. Expected: matches only in the deprecation doc comment and warning string.

- [ ] **Step 5: Update the docs**

In `docs/alacritree.md`'s `[integrations.herdr]` block, after `wsl_path`:

```toml
icon             = "✦"      # the glyph on herdr rows and palette entries. A
                            # bare string or a table, like [ui.icons] keys
                            # (see Icon styling). Supersedes the deprecated
                            # [ui.icons] herdr
```

In "Icon styling", the opening sentence becomes "Every `[ui.icons]` key, and `[integrations.herdr] icon`, takes either a bare glyph string, as shown above, or a table that styles it further:". Keep the file's hard wrap.

- [ ] **Step 6: Regenerate the committed fixtures**

Run: `devkit run task test --dir <worktree> --env ALACRITREE_UPDATE_SCHEMA=1 --env ALACRITREE_UPDATE_ALLOWLIST=1 --env ALACRITREE_UPDATE_STOCK=1`

Then: `git -C <worktree> diff -- schema/alacritree-config.json alacritree/tests/schema-defaults-allowlist.txt alacritree/tests/stock-config.json`

Expected:
- **Allowlist.** `+RawIcons.herdr`, nothing else.
- **Schema.** `RawHerdr` gains `icon` with `"default": "◫"`. `RawIcons.herdr` loses its default and takes the deprecation description.
- **Stock config.** The `herdr` entry leaves `ui.icons` and appears as `icon` under `integrations.herdr` with the same glyph.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `devkit run task test --dir <worktree>`

Expected: everything passes, including both Step 1 tests, the six edited `app.rs` herdr tests, `icon_tooltips_reach_the_session_and_home_row_buttons`, `every_icon_default_round_trips`, `config_schema`, `schema_defaults` and `the_stock_config_is_unchanged`.

- [ ] **Step 8: Commit**

```bash
git -C <worktree> add alacritree/src/config.rs alacritree/src/app.rs alacritree/src/app/sidebar.rs alacritree/src/app/palette.rs docs/alacritree.md schema/alacritree-config.json alacritree/tests/schema-defaults-allowlist.txt alacritree/tests/stock-config.json
git -C <worktree> diff --staged --stat
git -C <worktree> commit -m "refactor(config): move the herdr icon into herdr" -m "The herdr glyph exists only because of herdr, so it moves next to
herdr's other settings as [integrations.herdr] icon, in the file and in
the resolved config, keeping the bare string or styled table shape
every icon takes. [ui.icons] herdr still applies while the new key sits
at the built-in glyph, and loading it logs a deprecation warning.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Verify the branch

**Files:** none, unless a check fails.

- [ ] **Step 1: Format**

Run: `cargo +nightly fmt -p alacritree --manifest-path <worktree>/Cargo.toml --check`

Expected: no output. `devkit run task fmt` has no check mode. On a diff, run `devkit run task fmt --dir <worktree>` and commit the result as `fixup!` of the commit that touched the file.

- [ ] **Step 2: Lint**

Run: `devkit run task clippy --dir <worktree>`, then the same on a checkout of `upstream/feat-ux-configurable-diff-viewer` if a count is in doubt.

Expected: no warning in `config.rs`, `app.rs`, `app/sidebar.rs`, `app/palette.rs`, `cli/schema.rs` or `bindings.rs` that the base lacks. The base already has unrelated warnings, so compare per file, not the total.

- [ ] **Step 3: Tests and whitespace**

Run: `devkit run task test --dir <worktree>`, then `git -C <worktree> diff --check upstream/feat-ux-configurable-diff-viewer`

Expected: every test passes and `diff --check` prints nothing.

- [ ] **Step 4: No stale path left**

Run: `rg -n "ui\.pr_status|ui\.icons\.herdr|\[ui\] pr_status|icons\.herdr\b" <worktree>/alacritree/src <worktree>/docs`

Expected: only the three deprecation doc comments and three warning strings.

---

## Out of scope

Default-value wording on items this plan does not write or move stays for #100.
