# Domain docs

This file tells the engineering skills how to read the domain documentation of this repo when they explore the codebase. The repo is single-context.

## Before exploring, read these

- `GLOSSARY.local.md` at the repo root holds the domain terms.
- `docs/adr/` holds one ADR per decision. Read the ADRs that touch the area you work in.

If one of these does not exist, proceed silently. Do not flag its absence and do not suggest creating it. The `/domain-modeling` skill creates them when a term or a decision is resolved.

## File structure

```
/
├── GLOSSARY.local.md
├── docs/adr/
│   ├── 2026-11-07_short-title.md
│   └── 2026-11-09_other-title.md
├── alacritree/
└── crates/
```

An ADR file name is `yyyy-mm-dd_short-title.md`, with the date of the decision.

Both locations are untracked. `GLOSSARY.local.md` matches the global `*.local.*` ignore rule, and `.git/info/exclude` lists `docs/adr/`. The `docs/specs-and-plans` branch carries both, through `sync.local.py`. Write a new ADR or a glossary change in the main checkout, as with specs and plans.

## Use the glossary's vocabulary

When your output names a domain concept, use the term that `GLOSSARY.local.md` defines. This applies to issue titles, refactor proposals, hypotheses and test names. Do not use a synonym that the glossary lists under `_Avoid_`.

A concept that the glossary does not hold is a signal. Either you invent language the project does not use, so reconsider, or there is a real gap, so note it for `/domain-modeling`.

## Flag ADR conflicts

If your output contradicts an existing ADR, say so explicitly. Do not override it silently:

> _Contradicts the 2026-11-07 ADR on short titles, but worth reopening because..._
