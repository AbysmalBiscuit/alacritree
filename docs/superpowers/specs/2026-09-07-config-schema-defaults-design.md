# Config schema defaults design

**Goal:** every config key whose absence resolves to a fixed value carries that value as a JSON Schema `default`, so taplo shows it on hover and offers it in completion. The value comes from the same code the parser resolves through, so a schema default cannot disagree with the running one.

**Issues:** [#81](https://github.com/AbysmalBiscuit/alacritree/issues/81), sub-issue of [#43](https://github.com/AbysmalBiscuit/alacritree/issues/43).

**Branch:** `feat/config-schema-defaults`, marker `[14]`. Cut from the open PR carrying the highest `[n]`, which was PR 215 (`feat/herdr-integration`, marker `[13]`) when this was written. Read the tip fresh at setup time.

**Platform:** all. Nothing here touches WSL, conpty or any per-platform path.

**Config:** no new keys and no changed defaults. Every value this publishes is one the parser already resolves to today. Three keys stop advertising defaults for behaviour that does not exist, and one default string changes spelling without changing behaviour.

## Context

### The mechanism already runs

schemars emits a `"default"` for every field automatically. Given `#[serde(default)]` on the container, `field_default_expr` (schemars_derive 1.2.2, `schema_exprs.rs:772`) resolves the field out of `Self::default()`, and the caller at `schema_exprs.rs:712` serializes it into the schema:

```rust
#default.and_then(|d| schemars::_schemars_maybe_to_value!(d))
    .map(|d| #SCHEMA.insert("default".into(), d));
```

Nothing about this is opt-in. `RawConfig` and every nested `Raw*` already carry `#[serde(default)]`, so it fires for all 197 fields on every generation.

It produces something useful for exactly seven of them:

```
$defs.RawColors.properties.draw_bold_text_with_bright_colors.default = false
$defs.RawIconStyle.anyOf.1.properties.bold.default                   = false
$defs.RawIconStyle.anyOf.1.properties.italic.default                 = false
$defs.BindingCommand.anyOf.1.properties.args.default                 = []
$defs.RawProfile.properties.args.default                             = []
$defs.RawShell.anyOf.1.properties.args.default                       = []
properties.env.default                                               = {}
```

Those seven are precisely the Raw fields that are not `Option` and whose type implements `Serialize`. There are no exceptions in either direction, which gives an exact rule:

> A Raw field gets a correct schema default automatically iff it is non-`Option` and its type implements `Serialize`.

The other 190 produce nothing usable, for one of two reasons. Most are `Option<T>` where `T: Serialize`, so the emitted value is `null` — `Option::default()` is `None`. The rest have a type implementing no `Serialize` at all, and take schemars' `NoSerialize` blanket impl (`schemars-1.2.2/src/_private/mod.rs:185`), which returns `None` and inserts no key whatsoever; for those, dropping the `Option` alone would change nothing. Section 0 is about the type that dominates the second group.

`drop_nulls` (`cli/schema.rs:56`) sweeps the first group, and its own comment is the bug report:

```
/// ... a `"default": null` [describes] something no config file can hold — and
/// [is] worse than noise, since an editor shows it as the key's default where
/// the real one is whatever `Config::default` returns.
```

The sweep is correct given `Option`. It is the `Option` that is wrong.

### The defaults are already written twice

Take `[integrations.herdr] show_unmatched`. The literal `true` appears in `impl Default for HerdrConfig` (`config.rs:602`) and again in the resolution at `config.rs:3103`:

```rust
show_unmatched: self.integrations.herdr.show_unmatched.unwrap_or(true),
```

That shape repeats across the file. Adding a hand-written `#[schemars(extend("default" = true))]` would make three copies of one fact. Inverting makes one.

### The codebase already leans this way

`stock_config()` (`config.rs:114`) is `RawConfig::default().into_config()`, under a doc comment saying `Config::default` is the wrong baseline. `changed_from_defaults` (`config.rs:101`) diffs the live config against it, so the settings dump keeps working through the inversion with no change.

## 0. `RgbStr` needs a `Serialize`

`struct RgbStr(Rgb)` derives `Debug, Clone, Copy` and nothing else (`config.rs:2716`). It is the type behind every colour key in the file, so without this section every colour default is silently dropped by `NoSerialize` while the build stays green. Sections 1, 2 and 3 all depend on it:

- `colors.primary.foreground` / `.background` sit inside section 1's field count.
- Every ANSI slot in section 2 is an `RgbStr`.
- Section 3 does not compile at all without it: `RawIconStyle::Table` carries `color: Option<RgbStr>` (`config.rs:2323`), so `#[derive(Serialize)]` on the enum fails on that field.

A derive on `RgbStr` is the wrong fix. The inner `vte::ansi::Rgb` has a derived struct serializer, so the default would publish as `{"r":24,"g":24,"b":24}` — invalid against the `Color` def's own schema, which is `"type": "string"` with a `^(0[xX]|#)?[0-9a-fA-F]{6}$` pattern (`config.rs:2725`), and invalid against `RgbStr`'s hand-written `Deserialize` (`config.rs:2731`), which parses a string.

So `Serialize` is hand-written to emit `#rrggbb`, mirroring the hand-written `Deserialize` and the hand-written `JsonSchema` already sitting beside it — three impls, one accepted spelling, for the same reason the comment above `JsonSchema` gives. `PartialEq` comes along for section 5's round-trip test.

## 1. Invert the Raw layer

For every field whose resolution is a plain `unwrap_or(literal)` or an `if let Some(v)` over a `Config::default()` base, drop the `Option` and move the literal into the Raw struct's `Default`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(default)]
struct RawHerdr {
    enabled: bool,
    poll_interval_ms: u64,
    show_unmatched: bool,
    attach: String,
}

impl Default for RawHerdr {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 2000,
            show_unmatched: true,
            attach: "agent".to_string(),
        }
    }
}

impl Default for HerdrConfig {
    fn default() -> Self { RawHerdr::default().resolve() }
}
```

The resolved type's `Default` becomes a projection of the Raw one, never the other way round. That is the whole inversion: the layer the schema reflects is the layer that owns the value.

Roughly 75 fields take this shape, across `[integrations.herdr]`, `[debug]`, `[general]`, `[scrolling]`, `[window]`, `[selection]`, `[ui.drop]`, `[ui.paste]`, every `[ui]` boolean, and every string-enum. It is 75 and not 79 because `cursor.unfocused_hollow`, `cursor.blink_interval` and `cursor.blink_timeout` have this shape but are excluded by section 6 — they resolve to a value nothing reads.

Clamps survive because they apply to the literal identically: `font.size.max(1.0)` (`config.rs:2907`), `thickness.max(0.5)` (`:2864`), `image_keep.max(1)` (`:2897`), `window.opacity.clamp` (`:2978`). No invertible field's fallback depends on another field being present; the two that do are the alias chains section 4 keeps as `Option`.

The string-enums invert without a behaviour change. Each `parse_*` helper is uniformly `None => X::default()` and `Some(<the default spelling>) => that same value`, so changing the signature from `Option<&str>` to `&str` and defaulting the field to the spelling is a rewrite of the same function. Verified on `confirm_session_close`, `quoting`, `attach_mode` and `adjust`. Taplo then renders `"default": "never"` beside the `enum` list it already shows.

Each converted struct loses `Default` from its derive list and gains a hand-written `impl`. Roughly 30 such impls.

## 2. Split `RawSet`

`RawSet` (`config.rs:2168`) serves three fields with three different base palettes:

```rust
normal: RawSet,
bright: RawSet,
dim: Option<RawSet>,
```

One `Default` can carry only one palette, and `apply_set` (`config.rs:3134`) writes per slot, so an absent `[colors.bright]` under a shared default would overwrite bright with normal's hexes.

The split has to be into two structs that each declare all eight fields. A newtype wrapping a shared defaults-free type does not work: schemars emits a field's default from the *declaring* struct's `Default` (`schema_exprs.rs:772`, `#STRUCT_DEFAULT.#member`), so under either newtype shape the leaf default stays `RawSet::default().black == None`. A `#[serde(transparent)]` newtype forwards to the inner `JsonSchema` and gets no `$defs` entry of its own; a plain one becomes a `$ref` to `RawSet`. Neither reaches the leaves.

So a `macro_rules!` declares the eight fields once and expands to `RawNormalSet` and `RawBrightSet`, each with its own `Default` reading the stock palette consts (`config.rs:1543`). Doc comments and the `JsonSchema` derive expand fine from inside it. The macro also emits `fn into_optional(self) -> RawSet`, so `apply_set` keeps its signature: feeding it `Some(stock)` for every slot writes the stock value over the stock value, which is what an absent section does today.

`dim` keeps `RawSet` exactly as it is. Absent there genuinely means "derive them from normal", which is a computed default and not a fixed one, so an all-`Option` type with no defaults is the correct schema for it. That also leaves `an_optional_table_refers_straight_to_its_own_schema` (`cli/schema.rs:236`), which pins `dim` to `#/$defs/RawSet`, passing unchanged.

Keeping `dim` off the defaulted types is the one place in this change where a wrong value is reachable rather than a missing one: sharing a def would hover `[colors.dim] black` as defaulting to normal's `#181818`.

That publishes 16 leaf defaults — the values someone most plausibly opens the palette section to look up.

## 3. Unblock `[ui.icons]`

`RawIconStyle` (`config.rs:2309`) derives `Debug, Deserialize, JsonSchema` and no `Serialize`, so schemars' `NoSerialize` blanket impl yields nothing for it whatever the `Option` says. Its first variant under `#[serde(untagged)]` is `Glyph(String)`.

Every one of the 24 entries in `Icons::default()` (`config.rs:1086`) is `glyph(DEFAULT_X_ICON)` — glyph only, no colour, no size, no weight. So:

- Add `Serialize` to `RawIconStyle`, which section 0 has made possible. Untagged, so `Glyph(s)` serializes as a bare JSON string, which is exactly the shape `Deserialize` accepts back.
- Make `RawIcons`' fields non-`Option`, with a hand-written `RawIcons::default()` of 24 `RawIconStyle::Glyph(DEFAULT_X_ICON.into())` lines reusing the consts `Icons::default()` already uses. One literal per icon, not two.
- `style_or` (`config.rs:2271`) goes away, and with it `build_icons`' `let d = Icons::default()` (`config.rs:2276`). Every `RawIcons` field is now present, so `build_icons` is one `IconStyle::from` per field and calls nothing.
- `Icons::default()` becomes `build_icons(RawIcons::default())`, matching the direction of section 1.

Deleting `style_or` is what makes that last line safe. `build_icons` calls `Icons::default()` today; leaving that in while `Icons::default()` calls `build_icons` overflows the stack on the first config load.

Behaviour is unchanged in both directions. An absent key yielded `d.clone()`, whose `glyph` is `Some(DEFAULT)`; it now yields `RawIconStyle::Glyph(DEFAULT)`, resolving to the same `IconStyle`. A user writing `worktree = { bold = true }` still keeps the built-in glyph, because `Table { glyph: None, .. }` resolves to `IconStyle { glyph: None }` and `or_glyph` (`config.rs:1131`) supplies the default at paint — the same as today, since a present key has always overridden the default wholesale.

No `From<IconStyle> for RawIconStyle` is needed, and no colour round-trip is exercised, because no default carries a colour.

The hazard is that `Serialize` could emit a shape `Deserialize` rejects, putting an invalid default in a published document. Section 5 closes it.

## 4. What stays `Option`

Three groups keep their `Option` and get no `default`, which is correct rather than a gap.

**Genuinely optional (34 fields).** The resolved `Config` field is `Option` too. Absent means "no value", not "this value".

**Computed at runtime.** Config dirs, font family fallbacks, `colors.dim`, `cursor.style`. Absent means "derive it", and there is no literal to publish. Their derivation goes in the doc comment where it is not already.

**Deprecated-alias chains (2 fields).** `wsl.automount_root` resolves as `wsl.automount_root.or(ui.wsl.automount_root)`. Give the first a non-`None` default and the deprecated `[ui.wsl]` location is never consulted again. Permanent; the default goes in prose.

`colors.cursor.text` / `.foreground` and `colors.selection.text` / `.foreground` share that alias shape but resolve to `Option` anyway, so they fall in the first group.

A fourth group is defaultless by definition rather than by choice: the required fields of array entries and enum variants — `RawBinding.key`, `RawIndexed.index` / `.color`, `RawWorktreeOverride`'s fields, `program` on `RawShell` and `BindingCommand`. Their containers carry no `#[serde(default)]` because an absent one is a parse error, not a defaulted value.

## 5. The coverage guard

A test walks the published schema, collects every leaf property with no `default`, and compares the set against an allowlist. Adding an `Option<T>` field by reflex later fails the build with the key named.

The walk is the substance of the test, not an afterthought. It must dereference `$ref`, descend every `anyOf` branch (four untagged enums), descend `items` for the array entries, and treat `additionalProperties` — which is how `env` is shaped — as a leaf. It must also decide whether a nullable table property (`window.padding`, `colors.dim`, `cursor.style`, `terminal.shell`) counts as a leaf; the spec's answer is no, since the default that matters is on the fields inside it.

The allowlist is keyed by `$def.field`, not by config path. `Color` is reached by 69 paths, `RawIconStyle` by 24 and `RawFontFace` by 4, so a path-keyed list would be mostly duplicates of each other and would churn whenever an unrelated key gained a colour.

That allowlist runs to about 61 entries once sections 1 through 3 land: 34 genuinely optional, 10 optional-within-a-required-container (5 on `RawBinding`, 5 across the `RawIconStyle::Table` and `RawCursorStyle::Detailed` variants), 7 required fields, 4 arrays, 3 dead cursor keys, 2 alias chains, and `cursor.style`. The document holds 122 unique leaf properties in total, of which 7 carry a default today.

Regenerate the counts rather than trusting these:

```sh
ALACRITREE_UPDATE_SCHEMA=1 devkit run task test -- config_schema
```

A second test round-trips all 24 icon defaults: serialize `RawIcons::default()`, deserialize the result, assert equality. That is what makes section 3's `Serialize` safe to publish, and it is why section 0 adds `PartialEq` to `RgbStr`. It needs `Serialize` on `RawIcons` too, which has a visible side effect: schemars then publishes a 24-key object at `$defs.RawUi.properties.icons.default`. Harmless, but it shows up in the schema diff and is expected there.

Both live beside the existing schema tests in `alacritree/tests/config_schema.rs`, which already fails the build while `schema/alacritree-config.json` is stale.

## 6. Incidentals

**Three dead cursor keys.** `cursor.blink_interval` and `cursor.blink_timeout` (`config.rs:2019`, `:2022`) are declared, documented in the published schema with "Default `750`" and "Default `5`", and read nowhere in the crate. `cursor.unfocused_hollow` is stored at `config.rs:2957` and likewise never read; `CursorShape::HollowBlock` renders, but nothing selects it on focus loss.

All three stay accepted — they are legitimate alacritty keys, and the schema's own description says unknown keys are allowed because `alacritty.toml` carries keys only real alacritty acts on. What changes is the wording: each doc comment says the key is accepted for alacritty compatibility and that alacritree does not act on it, and none of the three advertises a default. Making them true is [#80](https://github.com/AbysmalBiscuit/alacritree/issues/80), out of scope here.

**`ui.decorations.*` defaults to `"0px"`, not `"0"`.** `Adjust::parse("0")` yields `Points(0.0)` (`config.rs:1188`) where `Adjust::NONE` is `Pixels(0.0)` (`config.rs:1176`). Identical through `apply`, but unequal under the `PartialEq` that `changed_from_defaults` relies on, so `"0"` would make an unmodified config report as modified. `"0px"` parses to exactly `Adjust::NONE`. The `RawDecorations` struct doc (`config.rs:2377`) and `parse_adjust`'s warning (`config.rs:1240`) both say `"0"` and change with it.

**Prose defaults come out.** 81 doc-comment lines in the Raw section spell a default in words — "Default `true`", "(default)". Once the schema carries the value, those are the third copy this design exists to remove, and they are the copy most likely to drift, since nothing checks them. They go wherever the schema now says the same thing; they stay wherever it does not, which is section 4's four groups.

## Testing

- The two new tests in section 5.
- `schema/alacritree-config.json` regenerated; its existing staleness test is the diff review.
- Two existing tests in `cli/schema.rs` change with the inversion, and both are load-bearing rather than incidental. `an_optional_key_is_not_described_as_nullable` (`:226`) asserts `RawUi.pr_status` has no `default`; section 1 gives it `false`, so the assertion inverts to naming the value. `an_optional_table_refers_straight_to_its_own_schema` (`:236`) pins `dim` to `#/$defs/RawSet` and passes unchanged, which is one of the reasons section 2 leaves `dim` on that type.
- The full suite. The inversion touches how every key resolves, so the existing config tests are the real regression net — particularly the merge tests at `config.rs:3949` and `:4060`, which prove array-concatenate and table-merge semantics survive. They should, since `merge` / `merge_tables` (`config.rs:1768`) run on `toml::Value` and `try_into::<RawConfig>` happens afterwards (`:1664`), so `Option` against `Default` is invisible to them.
- One targeted check that `stock_config()` is unchanged: serialize it before and after the inversion and diff. A field whose default moved by accident shows up there and nowhere else.

## Open questions

1. **Does taplo honour a `default` on a property whose schema is `{"anyOf": [{"$ref": ...}]}`?** That is the shape every `[ui.icons]` key takes, and section 3 is 24 leaves of work that buys nothing if taplo ignores it. Worth ten minutes hovering a key in an editor against a hand-edited schema before the branch is cut. The same question does not hang over sections 1 and 2, whose leaves are plain typed properties.
2. Section 2 costs a macro and two generated types. The alternative is to splice the 16 palette defaults into `document()` from the stock palette by slot name — the approach this design rejects everywhere else, but on a fixed 16-key surface with a stable string shape, reading the same consts the parser reads. It avoids the type split; it does not avoid section 0, which sections 1 and 3 need anyway. If review would rather not restructure the colour types, that is the trade; dropping section 2 outright costs the 16 defaults and nothing else.
