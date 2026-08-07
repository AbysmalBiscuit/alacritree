# Changelog

## [0.9.0](https://github.com/mathix420/alacritree/compare/v0.8.1...v0.9.0) (2026-08-07)


### Features

* **cli:** add the crashes subcommand ([8eab42f](https://github.com/mathix420/alacritree/commit/8eab42f5dcfe9796eaefcb6c92722be20ce8ff37))
* **config:** add icon keys for the chrome action buttons ([6126dc1](https://github.com/mathix420/alacritree/commit/6126dc167537b38426d216bdf94a571073f5d112))
* **config:** add IconStyle parsing from a string or table ([9682285](https://github.com/mathix420/alacritree/commit/9682285f0b54b0363ba333b929189dcfe4fb86b0))
* **config:** add the [debug] section ([2cff9f7](https://github.com/mathix420/alacritree/commit/2cff9f7e033460b4720420a6d2768acb2bfc3114))
* **config:** add ui.font variant family overrides ([25ed550](https://github.com/mathix420/alacritree/commit/25ed5502ea08eb626a32fbf475827694441c14dc))
* **config:** add ui.font.builtin_symbols ([cdd62ec](https://github.com/mathix420/alacritree/commit/cdd62ecfe39a3bb0ef6f611426b135f42021f62d))
* **config:** let ui.sidebar_attention override the attention color ([c35674e](https://github.com/mathix420/alacritree/commit/c35674e34c9b2f2221a721a4ab8f6f94a20ef086))
* **doctor:** report recorded crashes ([d3f2d6c](https://github.com/mathix420/alacritree/commit/d3f2d6cd2c47b0a02f00be5a1e2b127d9057fd77))
* **fonts:** embed a symbol subset for built-in glyphs ([685d3b4](https://github.com/mathix420/alacritree/commit/685d3b4c7234c3d3b7310b77dc6b3876eb21dfcf))
* **fonts:** register bold and italic families for the ui chain ([dab9177](https://github.com/mathix420/alacritree/commit/dab91773c275a36379996be66f59b3d510e06253))
* **fonts:** register the symbol face as a last resort ([b8aacd9](https://github.com/mathix420/alacritree/commit/b8aacd90419135af33d2ff7ea637cf145c0dc48f))
* **logging:** add log directory and process identity ([941b2d2](https://github.com/mathix420/alacritree/commit/941b2d219e1102c4d0b2ab76bcbfa6af177adc23))
* **logging:** arm crash recording on the GUI path ([789fef9](https://github.com/mathix420/alacritree/commit/789fef9af36c0310caa05d97e2d2d99ef2b59c52))
* **logging:** bound panic records per process ([94994d8](https://github.com/mathix420/alacritree/commit/94994d8d0a7c0d2f6989d9cebce7cda320974c9b))
* **logging:** prune artifacts by age and liveness ([70bbf9c](https://github.com/mathix420/alacritree/commit/70bbf9c8355fd111ce35a010121cc84be14a409d))
* **logging:** record panics to a per-process artifact ([d7d72e5](https://github.com/mathix420/alacritree/commit/d7d72e5504878b3ea9453a434277eaa37a1e2718))
* **logging:** tee the log stream to a per-process file ([672ae76](https://github.com/mathix420/alacritree/commit/672ae768e6f81925d1d014eb66ea483440e70f7d))
* **paste:** paste a path when the clipboard has no text [3] ([#163](https://github.com/mathix420/alacritree/issues/163)) ([6d315f9](https://github.com/mathix420/alacritree/commit/6d315f90ac9c93bf18773db9838adc1214098342))
* **session:** land a close on the neighbour ([#160](https://github.com/mathix420/alacritree/issues/160)) ([afff20c](https://github.com/mathix420/alacritree/commit/afff20c90a795412510f6a2a71c5f70ef8f99988))
* **sidebar:** add filter actions, PR filters, and search scope [5] ([#164](https://github.com/mathix420/alacritree/issues/164)) ([30a1335](https://github.com/mathix420/alacritree/commit/30a13357630217e8fee7839ab83aafd6e3aebe98))
* **sidebar:** gate icon hints behind icon_tooltips ([a1b5351](https://github.com/mathix420/alacritree/commit/a1b5351cae0f3e203b612f8afad862a0faa98046))
* **sidebar:** name what a status icon reports on hover ([64b4204](https://github.com/mathix420/alacritree/commit/64b4204a0fcdd527306c1b5bd80b61bffc8a8443))
* **sidebar:** paint a branch upstream badge on worktree rows ([29b9ee4](https://github.com/mathix420/alacritree/commit/29b9ee45fd935a551b8c111125398f75c4f971ca))
* **sidebar:** paint action buttons from config ([a1811ce](https://github.com/mathix420/alacritree/commit/a1811ce47a1dddda73670092fb03d9a8654aad78))
* **sidebar:** spell full row names out on hover [6] ([#165](https://github.com/mathix420/alacritree/issues/165)) ([ac42518](https://github.com/mathix420/alacritree/commit/ac4251890ac3c021f9a659ea9e6281144e854aaf))
* **sidebar:** style icons with color, weight, slant, and size ([44b9784](https://github.com/mathix420/alacritree/commit/44b978447ad126f259a436ae7a4041fffd080d74))
* **upstream:** add the upstream state type and worktree fields ([902e5a9](https://github.com/mathix420/alacritree/commit/902e5a9a7296669f86529493a8a1e487f57e8085))
* **upstream:** gate upstream discovery behind ui.upstream_status ([ec9273d](https://github.com/mathix420/alacritree/commit/ec9273d22147dab7d31159362a6a88b13a51686c))
* **upstream:** parse for-each-ref upstream tracking output ([b8f2d26](https://github.com/mathix420/alacritree/commit/b8f2d266ba1bc1f7de6c9c80cfd47311d41c667d))
* **upstream:** populate upstream state during project discovery ([f829d39](https://github.com/mathix420/alacritree/commit/f829d39fb11c3c6281cbd58621325f6f8de92c2d))
* **upstream:** read upstream state from a git2 branch walk ([74ea387](https://github.com/mathix420/alacritree/commit/74ea3875facb8a83cc7b57fdcc6dc36d02126c78))


### Bug Fixes

* **crash:** secure diagnostic artifacts ([14f3a37](https://github.com/mathix420/alacritree/commit/14f3a3733d86464f526974ad2aef50d11b1773f7))
* **doctor:** pin crash counts and drop dead after_exit flag ([64c5759](https://github.com/mathix420/alacritree/commit/64c5759fd274a0bbe8e4f6dd95969075b5b74d03))
* **fonts:** bind chrome families when font resolution fails ([dc50fa3](https://github.com/mathix420/alacritree/commit/dc50fa35f9418cebe296eb91c87646969aa8a249))
* **icons:** gate test-only glyph aggregates and drop task pointer ([2edf131](https://github.com/mathix420/alacritree/commit/2edf131581060b962e19a2a42a5b21407b3f6e2b))
* **logging:** prune session logs even when persistent_logging is off ([09b0b4b](https://github.com/mathix420/alacritree/commit/09b0b4b8f648281bee68b2805efadc5fe1d703fe))
* **logging:** recover the crash artifact from real failure modes ([d6d142d](https://github.com/mathix420/alacritree/commit/d6d142da7d4e83b3c99f91bab512e101b663c9fb))
* **logging:** restore terminal colour under Target::Pipe ([7192673](https://github.com/mathix420/alacritree/commit/7192673d7ec25fb576763f2276ec0046db395cf3))
* **palette:** fit and wrap on a narrow window ([552d912](https://github.com/mathix420/alacritree/commit/552d912630c899dabe3f49b691cf21243daa7d75))
* **sidebar:** surface the hints icons inside a row owe ([bd0dcbd](https://github.com/mathix420/alacritree/commit/bd0dcbd306522dd2588421327c10b668ea48228d))
* **upstream:** drop badges for unreadable git state ([862c3d3](https://github.com/mathix420/alacritree/commit/862c3d356725a6b1cdd2ebde5838cdca657cd131))
* **upstream:** key wsl branches by plain ref name ([304c7fa](https://github.com/mathix420/alacritree/commit/304c7fa5ebdae8f8fe78683cf03903468b7117d9))

## [0.8.1](https://github.com/mathix420/alacritree/compare/v0.8.0...v0.8.1) (2026-08-03)


### Bug Fixes

* **mcp:** find socket without XDG_RUNTIME_DIR ([#169](https://github.com/mathix420/alacritree/issues/169)) ([1abb0f6](https://github.com/mathix420/alacritree/commit/1abb0f65cb5c6604cead163e2c8fd1c7e1063259))

## [0.8.0](https://github.com/mathix420/alacritree/compare/v0.7.1...v0.8.0) (2026-07-28)


### Features

* **palette:** add nav keys, sections, and columns ([#144](https://github.com/mathix420/alacritree/issues/144)) ([613393a](https://github.com/mathix420/alacritree/commit/613393a76194faf4c866987afd9938f2fc0e9d09))
* **sidebar:** add configurable path abbreviation styles [5] ([#152](https://github.com/mathix420/alacritree/issues/152)) ([497911b](https://github.com/mathix420/alacritree/commit/497911b7cce544234710412785cb265c9248db9d))
* **sidebar:** add search confirm/cancel actions [3] ([#138](https://github.com/mathix420/alacritree/issues/138)) ([d67c202](https://github.com/mathix420/alacritree/commit/d67c20255f32adb8f2b43e5b04a4540f20ab3d38))
* **sidebar:** keep the cursor when a row disappears [6] ([#153](https://github.com/mathix420/alacritree/issues/153)) ([ac6d3a5](https://github.com/mathix420/alacritree/commit/ac6d3a559c69a52feda9e7b9f21987b6f19976c2))
* **wsl:** advertise the binary path and hold the probe cache [1] ([#151](https://github.com/mathix420/alacritree/issues/151)) ([f637257](https://github.com/mathix420/alacritree/commit/f6372573426a45b524b572284261562ee74f8278))


### Bug Fixes

* **projects:** keep worktrees when unreachable ([#145](https://github.com/mathix420/alacritree/issues/145)) ([361ee88](https://github.com/mathix420/alacritree/commit/361ee8803108ca60a916e6225626e8b5f958b6f2))


### Performance Improvements

* **terminal:** cut input latency and output stalls [7] ([#154](https://github.com/mathix420/alacritree/issues/154)) ([2ea4408](https://github.com/mathix420/alacritree/commit/2ea44082fe0894ca9847117fef3673538cc071d1))

## [0.7.1](https://github.com/mathix420/alacritree/compare/v0.7.0...v0.7.1) (2026-07-23)


### Bug Fixes

* **macos:** use public process-group API ([#146](https://github.com/mathix420/alacritree/issues/146)) ([ddbd07e](https://github.com/mathix420/alacritree/commit/ddbd07e0d49c934fb1b343dfe009f3e5c04df017))

## [0.7.0](https://github.com/mathix420/alacritree/compare/v0.6.0...v0.7.0) (2026-07-23)


### Features

* **scratchpad:** add per-workspace notes ([#140](https://github.com/mathix420/alacritree/issues/140)) ([402b6c5](https://github.com/mathix420/alacritree/commit/402b6c5c8f2378d2fe7944e9b638aeacc8ac5ad6))


### Bug Fixes

* make missing worktree error dismissible ([#142](https://github.com/mathix420/alacritree/issues/142)) ([c1ad81c](https://github.com/mathix420/alacritree/commit/c1ad81c588969a5b4afd2cb02c1869d084bede76))
* **probe:** detect a nav-TUI anywhere in the foreground group ([#139](https://github.com/mathix420/alacritree/issues/139)) ([049b230](https://github.com/mathix420/alacritree/commit/049b23063935f901a1804c2659326ae0cd4b0dfa))

## [0.6.0](https://github.com/mathix420/alacritree/compare/v0.5.1...v0.6.0) (2026-07-21)


### Features

* add cross-workspace session cycling actions ([#124](https://github.com/mathix420/alacritree/issues/124)) ([403a1fe](https://github.com/mathix420/alacritree/commit/403a1fed48e8c82c1d580753e0d65cac30f042e2))
* add projects-sidebar keyboard actions ([#131](https://github.com/mathix420/alacritree/issues/131)) ([8e87b31](https://github.com/mathix420/alacritree/commit/8e87b319730af775ec6e2bd6c58b45aae29d7a47))
* **ipc:** share session id across the WSL boundary ([#132](https://github.com/mathix420/alacritree/issues/132)) ([0ab6d7e](https://github.com/mathix420/alacritree/commit/0ab6d7ecbb092fd38726032300d3604001341ebf))
* replace shortcuts window with a Ctrl+K command palette ([#133](https://github.com/mathix420/alacritree/issues/133)) ([e7ec0d2](https://github.com/mathix420/alacritree/commit/e7ec0d2e5d4b461783196b238c177c76840f52c9))
* **tabs:** hide session tab strip when only one session is open ([#130](https://github.com/mathix420/alacritree/issues/130)) ([32d0c07](https://github.com/mathix420/alacritree/commit/32d0c07deac27a7025ded032d88b14995732ea9b))


### Bug Fixes

* don't grab the parent console when stdout is already wired up ([#134](https://github.com/mathix420/alacritree/issues/134)) ([ec934d9](https://github.com/mathix420/alacritree/commit/ec934d93ea3a6f8c54b48beef3df6847f16d7421))

## [0.5.1](https://github.com/mathix420/alacritree/compare/v0.5.0...v0.5.1) (2026-07-20)


### Bug Fixes

* **macos:** dlopen libfontconfig to unblock the release build ([#125](https://github.com/mathix420/alacritree/issues/125)) ([1ff0fa3](https://github.com/mathix420/alacritree/commit/1ff0fa31b628d2e0a23a218422007b5444051c73))

## [0.5.0](https://github.com/mathix420/alacritree/compare/v0.4.1...v0.5.0) (2026-07-20)


### Features

* **config:** honor general.working_directory ([#113](https://github.com/mathix420/alacritree/issues/113)) ([93488be](https://github.com/mathix420/alacritree/commit/93488be3f6a10954b1231f6e3110f68ea60f4ec0))
* **ui:** debounce session attention pings ([#116](https://github.com/mathix420/alacritree/issues/116)) ([b7c2ffc](https://github.com/mathix420/alacritree/commit/b7c2ffcd6df638356c5c3842d5105cac5753dcbc))
* **ui:** focus sidebars on click ([#122](https://github.com/mathix420/alacritree/issues/122)) ([8412aa3](https://github.com/mathix420/alacritree/commit/8412aa3e3bf8f26683c5546da692f5375df73511))
* **wsl:** resident per-distro helper for probes and batched git [7] ([#110](https://github.com/mathix420/alacritree/issues/110)) ([652dc23](https://github.com/mathix420/alacritree/commit/652dc2340856014814c19e0bf7a4e4e01cb7e9d2))


### Bug Fixes

* **terminal:** drop pointer events under overlays ([#123](https://github.com/mathix420/alacritree/issues/123)) ([73d44d7](https://github.com/mathix420/alacritree/commit/73d44d77f930115caf85c8f22257b2516bf57438))
