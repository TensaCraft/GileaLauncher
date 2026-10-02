# Architecture

## Crates

| Path | Purpose |
|---|---|
| `crates/launcher-shared` | DTOs, events, error codes, the brand (`branding`), release file names, shared functions. Builds both natively and for wasm. |
| `crates/launcher-core` | all the launcher's logic without Tauri (see below) |
| `crates/launcher-app` | the Tauri app: commands, the event bridge, command-line arguments, single instance, the window |
| `crates/ui-kit` | the design system: components, i18n, typed IPC, sounds, layers, the modules' UI hooks |
| `crates/launcher-ui` | the shell and pages (Leptos CSR), with a mock backend for working in a browser |
| `crates/mock-github` | a local imitation of GitHub Releases for testing updates |
| `modules/*` | optional modules (see [modules.md](modules.md)) |
| `xtask` | builds, checks, hooks, packaging, the update imitation |
| `build-profiles/*.toml` | profiles: modules, brand, edition, updates |
| `assets/` | translations, fonts, background, sounds, logo, icon |
| `tools/` | helper scripts (click sound synthesis) |

`launcher-core` is split into parts:

| Part | What it does |
|---|---|
| `paths`, `storage`, `settings`, `setup` | folders, atomic writes, config, the first-run wizard |
| `auth` | Microsoft and offline profiles |
| `minecraft`, `loaders`, `java` | versions, libraries, assets, Forge/NeoForge/Fabric/Quilt, Mojang's Java |
| `builds`, `content` | builds and their content: mods, resource packs, shaders, screenshots |
| `launch` | starting the game, the registry of running games, the `running-games.json` ledger (games survive a launcher restart) |
| `net` | one downloader for everything, metadata requests, disk space checks |
| `updater` | the launcher's self-update |
| `modules`, `providers` | the module registry and the content provider contract |
| `feedback`, `logging` | operations, notifications, the activity log, logs with secrets redacted |

## How it fits together

```
launcher-ui (wasm) ──ui-kit::ipc──▶ Tauri commands (launcher-app) ──▶ launcher-core services
      ▲                                                                    │
      └──────────── events (launcher-app bridge) ◀── FeedbackService ◀─────┘
```

- **From the interface to the core.** The interface calls Tauri commands through `ui-kit`'s typed IPC. `launcher-app` passes the call on to `CoreApp`'s services.
- **Answers and state.** The core reports operations, notifications and state changes through `FeedbackService`. `launcher-app` turns them into Tauri events, and the interface updates its store when they arrive.
- **Module commands.** Modules have commands of their own. The interface calls them through `module_invoke`.

## Brand and editions

- **Brand.** The code does not know the launcher's name. At compile time `launcher-shared::branding` reads the `LAUNCHER_*` variables that `xtask` takes from the profile:
  - name and identifier;
  - edition;
  - update repository;
  - support address;
  - module settings.

  Without a profile, defaults apply. `package` passes the name and identifier to Tauri the same way.
- **The edition** decides which modules a build has and which release files it updates from (`{name}-{edition}…`). One function, `launcher_shared::release_file_names`, sets the file names, and both the updater and packaging use it.

## Data folders

`launcher-core::paths` picks the folders:

- **A debug build** keeps everything in the repository's `.dev/`. `LAUNCHER_DEV=0` turns that off.
- **`LAUNCHER_APP_BASE`** is a portable mode: everything in one folder.
- **Otherwise** the system folders:

| System | State and settings | Games | Cache |
|---|---|---|---|
| Windows | `%LOCALAPPDATA%\<name>` | `%APPDATA%\<name>` | `%LOCALAPPDATA%\<name>\cache` |
| Linux | `$XDG_CONFIG_HOME/<name>` | `$XDG_DATA_HOME/<name>` | `$XDG_CACHE_HOME/<name>` |
| macOS | `~/Library/Application Support/<name>` | `…/<name>/minecraft` | `~/Library/Caches/<name>` |

The first-run wizard can move the state and the games to another folder. The pointer to it (`storage.json`) stays in the default state folder.

## Network

- **Downloads.** Everything the launcher downloads (game files, Java, mods, updates) goes through `launcher_core::net::downloader`. It:
  - downloads in parallel, with a limit;
  - retries and resumes partly downloaded files;
  - checks size and hash;
  - checks free space before the first byte (`net::preflight`).
- **Metadata.** Small requests (version lists, manifests) go through `net::meta`. Answers are cached for an hour.
- **Addresses.** Only `https` is allowed, checked on every redirect. Only debug builds and builds with the `mock-updates` feature may reach `127.0.0.1`/`localhost`.

## Self-update

1. **Finding a release.** `updater::github` reads the releases of the profile's `update_repo`. `updater::select` takes the newest release newer than the running version. Betas count only with "Beta updates" turned on.
2. **Choosing the file.** It takes its own edition's file for its own system. When there is none, no update is offered.
3. **Download.** The file comes through the shared downloader and is checked against the SHA-256 that GitHub gives. Then `updater::stage` prepares it for installation.
4. **Replacement.** The launcher exits, and a copy of it in `--apply-update` mode (`updater::apply`) replaces the program:
   - Windows and Linux: the executable;
   - AppImage: the AppImage itself;
   - macOS: the `.app` bundle from the `.dmg` image.

   If the replacement fails, the previous version comes back. Then the updated launcher starts.
