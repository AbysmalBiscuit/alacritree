# SDD ledger — plan: C:/Users/Lev/Git/github/alacritree/docs/superpowers/plans/2026-08-31-decoration-metrics.md

Setup: worktree feat/decoration-metrics at BASE aafeb4ed (= perf/instanced-grid tip), clean.
Briefs written for tasks 1-3.
Pre-flight scan: one item. Task 3 Step 9 is a visual pass on a GUI demo sheet and
edits the user's live alacritty.toml/alacritree.toml. A subagent cannot look at a
screen, and the live config is off-limits without the user present. Step 9 is
excluded from the Task 3 dispatch and handed to the user after the branch is done
(tracked as task #105). Steps 1-8 and 10 run as written.

Task 1: complete (commits aafeb4e..0ef68ef, review clean)
Task 1: minor (deferred): dead_code warning on AlacritreeApp::face_metrics until Task 3 consumes it; left unsuppressed by agreement of implementer and reviewer.
Task 1: minor (deferred): pre-existing cloned_ref_to_slice_refs clippy warning in build_font_definitions (fonts.rs), untouched and unrelated.
Task 1: note: the crate is a binary, so tests run as `cargo test -p alacritree --bin alacritree <filter>`, not `--lib`.

Task 2: complete (commits 0ef68ef..8ae9cc4, review clean)
Task 2: note: the plan called the struct `Ui`; it is really `UiTheme`, with a
  hand-written impl Default that also needed `decorations: Decorations::default()`.
  `config.ui.decorations` is the correct path for Task 3 (verified: Config.ui: UiTheme).
Task 2: note: schemars put "pattern" directly on all four properties; the RgbStr
  newtype contingency was not needed.

Task 3: implemented at bd5f9031. Spec compliance clean. Two Important findings enter the fix loop.
Task 3: controller check: clippy -D warnings fails on ~41 lints that are all present
  at base aafeb4ed under the same toolchain. The plan's gate was written against a
  wrong assumption; the branch introduced none. Not a finding.
Task 3: minor (deferred): DOUBLE/curl misbehave only when t > descent/4 (a 300% thickness knob); no crash, ink clipped.
Task 3: minor (deferred): the_curl_stays_inside_the_descent_area passes with ~0.1px margin; any amplitude change re-rolls it.
Task 3: minor (deferred): the_strikeout_keeps_its_own_weight asserts bar >= 6, one-sided; assert_eq!(bar, 6) would pin both directions.
Task 3: minor (deferred): no test drives the two strikeout knobs, only the underline ones.
Task 3: minor (deferred): a face reporting a positive descender flows through unguarded; .max(0.0) on descent is cheap insurance.
Task 3: minor (deferred): Adjust::Scale on a *position* scales distance from the cell top, so "150%" scales by the ascent. Task 2 owns the semantics.
Task 3: minor (deferred): the ascent fallback in show substitutes a row height (~25% large) when a laid-out "M" yields no glyph.
Task 3: note: reviewer traced epaint 0.31.1 by hand and confirmed Glyph::font_ascent
  is the galley-top-to-baseline distance, and that the GPU path's glyph quads agree
  with it. The untested ascent path is a real but acceptable gap.

Task 3: fix round 1/5 (2 addressed, 0 open — vacuous thickness-ordering test; unenforced DOUBLE stem gap; commits bd5f903..b9959c3)
Task 3: complete (commits 8ae9cc4..b9959c3, review clean)
Task 3: minor (deferred, new): the DOUBLE arm's upper stem has no top-of-cell clamp; only the lower one is pulled up. Pre-existing, not a regression.

Final whole-branch review at b9959c37: no Critical, two Important, merge-with-fixes.
  Important 1: underline_position knob does not reach DOUBLE or curly (both anchor on
    the descent); the four doc comments and the published schema claim it does, and
    claim kitty parity that does not hold for those two styles.
  Important 2: a face reporting a non-negative descender survives resolve_fallbacks,
    so descent goes negative and both descent-anchored styles invert.
  Taking Minor 3 (negative ascender, same function) and Minor 6 (no test drives the
    two strikeout knobs) into the same wave.
  Triage of the deferred minors: all others stand. The T1 dead_code line resolved
    itself; the T3 ascent-fallback line is fine because it matches epaint's own
    fallback (Font::ascent returns row_height on an empty family).

Fix wave: 4 commits b9959c3..95db567, all four findings ADDRESSED by scoped re-review, no new breakage.
Controller verification at 95db5673: tree clean, cargo fmt --check clean, cargo test -p alacritree 1099+7+2 passed / 0 failed.
Still owed: the visual pass (task #105). Nobody has looked at how any of this renders.
