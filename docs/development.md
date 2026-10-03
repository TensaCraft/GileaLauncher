# Development

## Requirements

- **Rust stable.** `rust-toolchain.toml` adds `rustfmt`, `clippy` and the `wasm32-unknown-unknown` target.
- **[trunk](https://trunkrs.dev):** `cargo install trunk --locked`.
- **Tauri CLI 2** for packaging and icons: `cargo install tauri-cli --version "^2" --locked`.
- **Tauri's system dependencies:** https://tauri.app/start/prerequisites/.
  - Windows: WebView2 (built into Windows 10 and 11) and the MSVC Build Tools.
  - Linux (Ubuntu 22.04):
    ```bash
    sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev libasound2-dev patchelf
    ```
    Packaging an AppImage also needs `libfuse2` (players do not need it).
  - macOS: Xcode Command Line Tools.

After cloning, turn on the repository's hooks once:

```bash
cargo xtask hooks
```

## Commands

Everything runs through `cargo xtask`:

```bash
cargo xtask dev                      # run with a hot-reloaded UI (profile standard)
cargo xtask dev --profile full       # with the tensa module
cargo xtask dev --modules all        # every module but the postponed diagnostics
cargo xtask build --modules backups  # any set of modules
cargo xtask build --release          # a release build without packaging
cargo xtask test                     # tests of the workspace and of modules outside the default features
cargo xtask lint                     # fmt + clippy (native and wasm), no tests
cargo xtask check                    # lint + tests, as CI runs them: before every push
cargo xtask profiles                 # list the profiles
cargo xtask new-module <id>          # skeleton of a new module
cargo xtask icons assets/icon/icon.png --macos assets/icon/icon-macos.png
cargo xtask package --profile full   # release packages in dist/release (see releasing.md)
```

CurseForge needs the launcher's API key. Put your copy in `.dev/curseforge-api-key` (the folder is ignored by git): `cargo xtask dev` and `build` pass it to the app build, never to the interface. A key set in `CURSEFORGE_API_KEY` wins. Without one the app runs without CurseForge.

While you develop (`dev` and other debug builds), the app keeps all its data in `.dev/` at the repository root, so your real launcher data is never touched. To make a debug build use the system folders, set `LAUNCHER_DEV=0`. To make any build keep everything in one folder, set an absolute path in `LAUNCHER_APP_BASE`.

In a debug build, Shift+right click opens the browser menu and the developer tools are available. A release build has no browser menu (text fields keep theirs), and browser keys (F5, F12, Ctrl+R, Ctrl+Shift+I…) do nothing.

### The interface in a browser

The interface also runs without Tauri, with a mock backend:

```bash
cd crates/launcher-ui && trunk serve
```

Then open `http://127.0.0.1:1420`:

- `/dev/kit` — the component gallery;
- `?setup=1` — the first-run wizard;
- `?builds=none` — no builds.

## Profiles

Profiles live in `build-profiles/*.toml`. Each sets:

- the modules;
- the brand;
- the edition (`edition`);
- the update repository;
- the modules' settings.

| Profile | Modules | Edition | Published |
|---|---|---|---|
| `standard` | modrinth, backups, reports | `standard` | yes |
| `full` | tensa, modrinth, backups, reports | `tensa` | yes |
| `core` | none | `core` | no |
| `mock-updates` | as in standard | `standard` | no (only for testing updates) |

A profile is published when it has `update_repo` and no `update_api`. See [releasing.md](releasing.md).

## Code rules

- **Brand.** The name and brand live only in the profiles, `tauri.conf.json`, the icons, `launcher-shared/src/branding.rs` and the docs. An `xtask` test (the brand guard) checks every file of the repository.
- **Downloads.** Every download goes through the core's one downloader, `launcher_core::net::downloader`. An `xtask` test makes sure no other code downloads by itself.
- **Language.** Interface texts live in `assets/langs/{uk_UA,en_US}.json` and in the modules' `locales/`. Code, comments, docs and commit messages are in English.

## Commits and hooks

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org):

```
kind(scope): summary
```

- **`kind`:** `feat`, `fix`, `ui`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore` or `release`.
- **`scope`:** optional, lower case.
- **`!` before the colon** marks a breaking change.
- **`summary`:** English (ASCII), 15 to 120 characters. Vague words such as `wip`, `fix` or `update` are refused.
- **Git's own subjects** (`Merge …`, `Revert …`, `fixup! …`, `squash! …`) are accepted as they are.

Examples:

```
fix(updater): each edition updates from its own files
ui(home): cards keep one height in every row
release: bump version to 0.2.0
```

`feat`, `fix`, `ui` and `perf` commits end up in the release notes. The other kinds are internal and stay out of them.

The hooks live in `.githooks/` and are turned on with `cargo xtask hooks`.

- **`pre-commit`** runs `cargo xtask hook pre-commit`:
  - `cargo fmt --check`;
  - `git diff --check` on the staged changes;
  - `cargo clippy --workspace --all-targets -D warnings`.

  `LAUNCHER_SKIP_PRECOMMIT=1` turns these checks off for one commit. In a fresh clone the hook builds the frontend first, because the app's clippy needs it.
- **`commit-msg`** runs `cargo xtask commit-msg <file>`.

If you want extra checks that are not part of the repository, put them in `.git/hooks/pre-commit.local` and `.git/hooks/commit-msg.local`. The hooks run them after their own checks, and `LAUNCHER_SKIP_PRECOMMIT` does not skip them.

CI repeats both checks, and runs the tests the hooks leave out:
- `cargo xtask lint` once, on Linux;
- `cargo xtask test` on Windows, Linux and macOS, side by side;
- the profiles' builds and the app's start once, on Linux;
- the message check for every commit of a pull request.

On `main` every commit keeps its CI run (a newer push cancels only a pull request's older run): a release reads the verdict of its commit.
