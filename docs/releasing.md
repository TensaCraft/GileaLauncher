# Releasing

## Version and tag

The launcher's version is `[workspace.package] version` in the root `Cargo.toml`. A release's tag is `v<version>`, for example `v0.2.0` or `v0.3.0-beta.1`. A version with a suffix (`-beta.1`) and a release marked as a pre-release belong to the beta channel. The workflow marks a suffixed version's release as a pre-release itself, so a beta never becomes "Latest".

```bash
cargo xtask release-meta                         # {"prerelease":false,"tag":"v0.1.0","title":"<name> 0.1.0","version":"0.1.0"}
cargo xtask release-meta --field tag             # v0.1.0
cargo xtask release-meta --editions              # [{"edition":"tensa","profile":"full"},{"edition":"standard","profile":"standard"}]
cargo xtask release-meta --field prerelease      # true for a suffixed version
cargo xtask release-meta --check-tag v0.1.0      # fails unless the tag is the version's
cargo xtask release-meta --check-new-tag v0.1.0  # fails when the tag already names another commit
```

## Editions

A profile's `edition` field sets its edition. Profiles with `update_repo` and without `update_api` are published. Today these are:

| Profile | Edition | Modules |
|---|---|---|
| `standard` | `standard` | modrinth, curseforge, backups, reports |
| `full` | `tensa` | tensa, modrinth, curseforge, backups, reports |

Only one profile may publish an edition. Every build updates only from its own edition's files. When a release has no file of its edition for its system, no update is offered.

To add an edition, create a profile with a new `edition` (lower-case letters and digits) and the same `update_repo`. The workflow picks it up by itself.

## Release files

`cargo xtask package --profile <profile>` builds the profile for release (`trunk --release`, `cargo tauri build`) and puts the files in `dist/release/`. For the name `GileaLauncher` and the edition `standard`:

| System | Files | For |
|---|---|---|
| Windows | `GileaLauncher-standard.exe` | the portable version and the update file |
| | `GileaLauncher-standard-Setup.exe` | the NSIS installer for the current user |
| Linux | `GileaLauncher-standard-x86_64.AppImage` | the AppImage and its updates |
| | `GileaLauncher-standard-x86_64` | the executable and its updates |
| macOS | `GileaLauncher-standard-universal.dmg` | Apple Silicon and Intel |

`launcher_shared::release_file_names` sets the names. Both the updater and `package` use it, so the names cannot drift apart; the `packaged_names_are_the_updaters_names` test checks it.

A package can only be built on its own system. macOS needs the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets.

## Release notes

```bash
cargo xtask release-notes --tag v0.2.0 [--previous v0.1.0] [--output RELEASE_NOTES.md]
```

The notes are made from the commit subjects since the previous tag. For a stable tag the previous one is the previous stable tag, so the notes also cover the betas in between.

| Commit kind | Notes section |
|---|---|
| `feat` | New Features |
| `fix` | Fixes |
| `ui` | Interface |
| `perf` | Performance |
| commits without a kind | Other Changes |

`build`, `chore`, `ci`, `docs`, `refactor`, `release` and `test` commits stay out of the notes.

## How to release

1. Raise the version in `Cargo.toml` and commit it: `release: bump version to 0.2.0`.
2. Push the commit to `main`.
3. Start the **Release** workflow (`.github/workflows/release.yml`) by hand on `main` (**Run workflow**, or `gh workflow run release.yml --ref main`), with the `prerelease` and `draft` switches if needed. Never create the tag or the release yourself. The workflow:
   1. checks that the run is on `main` and that the tag `v<version>` does not name another commit: a published release is never rebuilt from other code;
   2. checks that the CurseForge key (the `CURSE_FORGE_KEY` secret) is there for every edition with CurseForge;
   3. builds every edition on Windows, Linux and macOS and smoke-tests every package;
   4. tags the commit, writes the notes and uploads the files to a draft release;
   5. publishes the release once every file is in (unless `draft` is on);
   6. deletes the temporary artifacts. When a step fails they stay, so **Re-run failed jobs** can finish the release.

Only one release runs at a time.

The `dry_run` switch only builds every package and runs their smoke tests, with no tag and no release. The files stay in the run's artifacts for a day. Use it to check the builds on every system before the first release.

While a release's files are uploading, the updater does not offer that release: it has no file of the right edition yet.

## Signing

The files are not signed or notarized yet. What players do on Windows (SmartScreen) and macOS (Gatekeeper) is in the [README](../README.md#installation).
