# Codex kitty allowlist change for alacritree

A drafted upstream change to `openai/codex` that lets Codex draw its pets with the kitty graphics protocol in alacritree, keyed on `TERM_PROGRAM=alacritree`. It is ready to open once alacritree/alacritree#382 has landed. It has not been opened.

- Patch: `2026-09-29-codex-kitty-allowlist.patch`, beside this file, made with `git format-patch` against `openai/codex` at `aec653daa9873bf44517a623fd033722737817a8`.
- Apply it with `git am` on a Codex checkout. The base can be refreshed with `docm sync codex`, and `docm info codex` prints the commit.
- Test it with `cargo nextest run -p codex-tui --lib -E 'test(/pets::image_protocol::/)'` from `codex-rs/`.

Every `path:line` below is relative to the Codex checkout at that commit.

## Why Codex needs a change

Codex picks a pet image protocol only from the environment. `ProtocolSelection::Auto.resolve()` is its only runtime path (`codex-rs/tui/src/pets/ambient.rs:355`). The `kitty` and `sixel` selections exist (`codex-rs/tui/src/pets/image_protocol.rs:89-110`), but nothing outside that file parses one, so a user has no override.

`detect_pet_image_support` refuses under tmux and zellij, accepts `KITTY_WINDOW_ID` and WezTerm's variables, and otherwise asks `pet_image_support_for_terminal` (`image_protocol.rs:112-133`). That function accepts kitty graphics only through `supports_kitty_graphics`, which matches the terminal names Ghostty, kitty and WezTerm, or `kitty`, `ghostty` or `wezterm` inside `TERM` or `TERM_PROGRAM` (`image_protocol.rs:178-188`).

When `TERM_PROGRAM` is set, `detect_terminal_info_from_env` returns early through `TerminalInfo::from_term_program`, which records `term: None` (`codex-rs/terminal-detection/src/lib.rs:305-317`, `lib.rs:108-120`). In an alacritree pane Codex therefore sees `TerminalName::Unknown`, `term_program = "alacritree"` and no `TERM`, and falls through to `PetImageUnsupportedReason::Terminal`.

The same early return means `TERM_PROGRAM=alacritree` masks an inherited `WT_SESSION` (`lib.rs:295`, `lib.rs:376`). Before alacritree set `TERM_PROGRAM`, an alacritree started from Windows Terminal passed `WT_SESSION` down, and Codex chose sixel (`image_protocol.rs:190-195`), which alacritree does not draw.

## The change

`supports_kitty_graphics` also accepts a `TERM_PROGRAM` containing `alacritree`. The test that lists kitty-capable terminals gains the case alacritree produces: `TerminalName::Unknown`, `term_program = "alacritree"`, `term = None`. The patch touches `codex-rs/tui/src/pets/image_protocol.rs` only.

## What Codex sends, and where #382 covers it

Codex's kitty path writes two kinds of APC (`codex-rs/tui/src/pets/mod.rs:150-200`):

- `kitty_delete_image` sends `ESC _ G a=d,d=I,i=<id>,q=2; ESC \` (`image_protocol.rs:219-221`) before each frame and when the pet goes away.
- `kitty_transmit_png_with_id` sends `ESC _ G a=T,t=d,f=100,c=<cols>,r=<rows>,q=2,i=<id>,m=<more>;<base64> ESC \`, the base64 PNG split into 4096-byte chunks, each later chunk as `ESC _ G m=<more>;<chunk> ESC \` (`image_protocol.rs:223-252`).
- The cursor is placed with `SavePosition` and `MoveTo` before the transmit (`mod.rs:209-224`), so the placement lands at the live cursor.

alacritree/alacritree#382 lists each of these in its scope:

| Codex sends | #382 scope |
|---|---|
| `a=T` | actions `t`, `T`, `p`, `d`, `q` |
| `t=d` | media `d`, `f` and `t` |
| `f=100` | formats 24, 32 and 100 (PNG) |
| `c`, `r` | placement keys `x`, `y`, `w`, `h`, `X`, `Y`, `c`, `r`, `C`, `z` |
| `i` | keys `i`, `I`, `p`, `q` |
| `q=2` | replies honour `q` |
| `m` chunks | chunk reassembly, one accumulator per session |
| `a=d,d=I` | every `a=d` specifier kitty implements, lowercase and uppercase |
| placement at the cursor | the APC runs in stream order and sees the live cursor |

This is coverage against #382's written scope. It becomes evidence of behaviour once #382 lands and Codex's pets are seen drawing in an alacritree pane. That check is the last step before opening the upstream PR.

## Draft PR text

Title: `tui: draw pets with kitty graphics in alacritree`

alacritree is a terminal that implements the kitty graphics protocol and identifies itself with `TERM_PROGRAM=alacritree` and `TERM_PROGRAM_VERSION`, the way Ghostty and WezTerm do. Codex's pet detection only accepts kitty graphics from known names, so in alacritree it reports that the terminal has no image support.

This adds `alacritree` to the `TERM_PROGRAM` matches in `supports_kitty_graphics`, and a test case for the environment alacritree produces. The pets' kitty path sends `a=T,t=d,f=100` transmissions with `c`, `r`, `i`, `q=2` and `m` chunking, and `a=d,d=I` deletes, all of which alacritree handles.
