Plan: docs/superpowers/plans/2026-07-15-searchable-sidebars.md
Branch: feat/searchable-sidebars (base 554c694e, off integration/all-features)
Task 1: complete (commits 554c694e..2661a10e, review clean)
Task 2: complete (commit 8fc59997, 284/284 tests passing incl. new panel_filter suite, 1 pre-existing ignore)
Task 2: complete (commits 2661a10e..8fc59997, review approved)
  Minor (for final review triage):
  - panel_filter.rs tests: no multi-toggle active_toggles() ordering test (assert allowed-order with 's','a' both active)
  - panel_filter.rs new(): struct-literal field order differs from declaration order (cosmetic)
  - dead-code warnings in non-test build until Task 5 wires PanelFilter in (expected, do not #[allow])
Task 3: complete (commits 8fc59997..47bdf3ed incl. coverage fix 47bdf3ed, re-review approved)
  Minor (for final review triage):
  - sidebar_nav.rs: no explicit empty-projects test for filtered_rows (optional)
Task 4: complete (commits 47bdf3ed..69d2dbaf, review approved)
  Minor (for final review triage):
  - git_nav.rs: no test for Conflicted row that fails query_pass (bypass is kind-only; inspection-verified)
  - git_nav.rs step(): missing-cursor falls back to index 0; add doc note that callers run ensure_cursor first
Task 5: complete (commits 69d2dbaf..090d31b6, review approved; interactive GUI smoke NOT RUN - human to verify)
  Minor (for final review triage):
  - app.rs focus_sidebar(): hopping left from git panel leaves an auto-shown right panel visible (mirrors identical pre-existing left-panel gap; self-heals via focus_terminal)
  - app.rs git cursor repair: fallback-to-first-row doesn't set git_cursor_moved, outline may paint off-screen until next key
Task 6: complete (commits 090d31b6..53038aa3 incl. search-jump fix 53038aa3, re-review approved)
  Minor (for final review triage):
  - app.rs apply_sidebar_nav: dead defensive Key::Escape arm (filter consumes Escape in both modes)
  - filtered view: collapsed project's disclosure arrow can disagree with force-shown worktrees (display-only)
Task 7: complete (commits 53038aa3..90a8aee3 incl. doc-comment fix 90a8aee3, fix verified by controller)
  Minor (for final review triage):
  - app.rs: move_git_cursor / apply_git_sidebar_nav duplicate stale-cursor-to-first-row logic (matches existing left-panel idiom)
All 7 tasks complete. Next: final whole-branch review, then merge into integration/all-features.
Final whole-branch review: With fixes -> fixed in 776328c8 (stale-cursor fallback + refocus seed repair), all ledger Minors triaged ship-as-is.
Branch complete at 776328c8. Reminder: interactive GUI smoke (Ctrl+Shift+G round-trip, cursor outline, Enter-opens-diff, / search, s/a/m/d/u toggles) NOT RUN - human pass recommended.
