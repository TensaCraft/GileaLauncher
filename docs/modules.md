# Modules

A module is an optional part of the launcher: a build includes it only when its profile asks for it. The core does not know about any particular module; it talks to all of them through two contracts. The backend implements `launcher_core::modules::Module`, the interface `ui_kit::module::UiModule`.

| Module | What it adds | In profiles |
|---|---|---|
| `modrinth` | mods, resource packs, shaders and modpacks from Modrinth (a content provider) | standard, full |
| `backups` | world backups | standard, full |
| `reports` | error reports sent to the profile's `endpoint` | standard, full |
| `tensa` | TensaCraft server builds on Home | full |
| `curseforge` | mods, resource packs, shaders and modpacks from CurseForge (a content provider; needs the CurseForge API key, see below) | standard, full |
| `diagnostics` | the postponed diagnostics module | none (outside `--modules all`) |

## Layout

```
modules/<id>/
  Cargo.toml        # features backend (launcher-core) and ui (ui-kit, leptos)
  locales/          # uk_UA.json, en_US.json: the module's texts over the common ones
  src/lib.rs        # pub const ID; mod backend (feature backend); mod ui (feature ui)
  src/backend/      # impl Module
  src/ui/           # impl UiModule
  tests/            # backend tests
```

- **Registration.** `launcher-app` and `launcher-ui` have a `mod-<id>` feature per module. The lists of a build's modules are in `crates/launcher-app/src/modules.rs` and `crates/launcher-ui/src/modules.rs`.
- **Default features.** Modules outside the default features (`tensa`, `diagnostics`) are checked separately by `cargo xtask check` and `test`: the `EXTRA_MODULES` list in `xtask/src/cmd.rs`.
- **The CurseForge key.** `curseforge` works only with the launcher's CurseForge API key. The release workflow passes it from the repository secret `CURSE_FORGE_KEY` to the Package step as `CURSEFORGE_API_KEY`; only the module's backend reads it (`option_env!`), and the interface build never gets it. A build without the key (CI, forks) compiles and passes its tests and simply shows no CurseForge. The key never goes into the repository: a guard in `xtask` fails on any key-shaped string.

## Backend: `Module`

Every method but `id` and `version` is optional:

| Method | For |
|---|---|
| `config_defaults` | the module's config keys with default values |
| `init` | starting with the core (`ModuleContext`: folders, config, notifications) |
| `provider_info` / `provider` | the module is a content provider (see below) |
| `launch_hook` | a step before every game start (world backup in `backups`, server build sync in `tensa`) |
| `game_watcher` | watching a running game |
| `call` | the module's own commands, which the interface calls through `module_invoke` |
| `changes_builds` | a command changes builds: the app refreshes the build list after it |

## Interface: `UiModule`

A module can add to the interface:

- sidebar pages (`pages`);
- tabs of a build's content (`content_tabs`);
- settings sections (`settings_sections`);
- options in the "delete build" dialog (`delete_options`);
- windows shown over the whole app (`overlays`);
- cards on Home (`home_cards`). The card component tells Home how many cards it shows with `report_home_cards`, so Home only says "no builds yet" when there is nothing on it;
- build menu entries (`build_actions`);
- translations (`locale_json`).

Components come from `ui-kit`, so modules look like the core.

## Content providers

A provider is a module that finds and installs content. It declares in `ProviderInfo`:

- what it can search and install (`content`);
- what it can update (`updates`);
- whether it works with modpacks (`modpacks`, `modpack_updates`).

The `launcher_core::providers::ContentProvider` contract has `search`, `plan`, `install`, `overview`, `modpacks`, `modpack_versions` and more. All are optional: by default they answer "not supported". The core asks a provider only about what it declared. The content interface is shared by all providers: a new provider needs no pages of its own.

## Settings from the profile

A profile's `[modules.<id>]` section becomes compile-time variables `LAUNCHER_MOD_<ID>_<KEY>`, which the module reads with `option_env!`. For example, `[modules.reports] endpoint` becomes `LAUNCHER_MOD_REPORTS_ENDPOINT`.

## A new module

```bash
cargo xtask new-module <id>
```

The command creates a skeleton and prints where to register it:

1. `Cargo.toml`: a workspace member and the `module-<id>` dependency.
2. `crates/launcher-app/Cargo.toml` and `crates/launcher-ui/Cargo.toml`: the `mod-<id>` feature.
3. `crates/launcher-app/src/modules.rs` and `crates/launcher-ui/src/modules.rs`: the module under `#[cfg(feature = "mod-<id>")]`.
4. `xtask/src/profile.rs`: `KNOWN_MODULES`. Also add the module to the profiles that need it, and to `EXTRA_MODULES` when it is not in the default features.
