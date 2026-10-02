# The `diagnostics` module: crash diagnostics

This module explains why the game crashed. It reads the logs the crash left behind (the crash report, `latest.log`, `launch.log`, `hs_err`) and checks them against 13 rules. It shows the result in a crash dialog with advice on what to do, and on a "Diagnostics" tab of the build's content. When Fabric or Quilt has printed a fatal error and is waiting for its own error window to be closed, the module stops the process.

## Status: postponed until after the release

The module is not part of:

- `cargo xtask dev|build --modules all` (`xtask::profile::POSTPONED_MODULES`);
- any build profile;
- the default features.

Regular runs (`cargo test --workspace`, `cargo xtask check`) compile only its types (`dto.rs`). The backend and the UI are built only with the `mod-diagnostics` feature.

## How to turn it on

- **The app:** `cargo xtask dev --modules diagnostics,modrinth,curseforge,backups,reports`.
- **The interface in a browser:** `trunk serve --config crates/launcher-ui/Trunk.toml --features mod-diagnostics`. Open the page with `?crash=1`, and every launch will crash with a sample diagnosis.
- **Tests:** `cargo test -p module-diagnostics --features backend,ui`.
- **Lints:** `cargo clippy -p module-diagnostics --features backend,ui --all-targets -- -D warnings`.

## How it is built

- **`src/dto.rs`**: the diagnosis types, the `app://module/diagnostics/diagnosis` event and the `build_diagnostics` command.
- **`src/backend/`**: the engine and its rules, log reading, the module's command and `CrashWatcher`. `CrashWatcher` plugs into the core as a `GameWatcher` and reports when the game is stuck or has crashed.
- **`src/ui/`**:
  - the crash dialog (the module's `ModuleOverlay` window);
  - the "Diagnostics" tab (`ModuleTab`);
  - opening a build and launching it through the app's `ModuleHost`;
  - styles in `styles/diagnostics.css`.

## What is left

- **The loader's error window.** Stop the loader only when its error window is really open and its log has stopped growing. The integration test needs a window in `fake-game` for this (`-Dfake.window=TITLE`).
- **Mod compatibility.** A button on the "Diagnostics" tab will start a mod compatibility check.
