# Working on RustEmuDroid

Read [RULES.md](RULES.md) for runtime and data constraints and [AI-MEMORY.md](AI-MEMORY.md) for the current architecture and verification record.

| Area | Start here |
|---|---|
| CLI and VM lifecycle | `src/main.rs` and `src/android.rs` |
| Desktop launcher, settings and translations | `src/ui.rs` and `src/ui/` |
| Guest display, keyboard, gamepads and Android actions | `src/display.rs`, `src/display/`, `src/controls.rs` and `src/toolbar.rs` |
| Android Home and its embedded APK | `android/home/` |
| BlueStacks integration | `src/bluestacks.rs` |
| Build matrix and releases | `.github/workflows/build.yml` and `docs/releases.md` |

Use `cargo fmt --all -- --check`, `cargo test --locked --features ui` and `cargo build --locked --release --features ui`. The CLI can also be built without the `ui` feature. Windows-specific integrations need a Linux build check. Real VM tests require an installed runtime and an idle profile.

For the manuals, keep `README.md` (English), `README.pt-BR.md` and `README.es-ES.md` together. Capture the actual client and Android Home in the language of each manual. The Windows integration test accepts `RUSTEMUDROID_TEST_STATE`, `RUSTEMUDROID_TEST_LOCALE` and `RUSTEMUDROID_TEST_CAPTURES`. Run the three locales sequentially against a test profile.
