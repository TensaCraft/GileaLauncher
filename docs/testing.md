# Tests

```bash
cargo xtask test     # the workspace and every module outside the default features (with its features)
cargo xtask lint     # fmt + clippy (native, wasm, modules), no tests
cargo xtask check    # lint + test: run it before pushing; CI runs the same, split into jobs
```

A single crate or test runs with plain `cargo test -p <crate> <filter>`. For the interface's tests, `test` and `check` build the frontend first (`trunk build`).

## Rules

- **No real folders, registry or network.** Tests work in temporary folders (`tempfile`) and with local fake servers on `127.0.0.1`. The backend is tested through a `PathEnv` with temporary paths, never `PathEnv::from_system()`.
- **A failing test first.** Every behaviour change and every fix starts with a test that reproduces the problem.
- **Guards in `xtask`.** These tests check the whole repository:
  - `brand`: the brand stays out of code;
  - `downloads`: every download goes through the core's downloader;
  - `release_builds_have_no_devtools`: release builds have no developer tools;
  - `workflows_call_existing_commands`: GitHub workflows call only existing `xtask` commands.
- **Smoke test.** `launcher-app --smoke-test` checks that the finished program finds and creates its folders. CI runs it on the debug build, and the release workflow on every package.

## Linux in WSL

On Windows, Linux can be checked in WSL (Ubuntu 22.04). Install the dependencies from [development.md](development.md), then run in a clone inside WSL:

```bash
cargo test -p launcher-core
cargo xtask check
cargo xtask package --profile standard   # the AppImage and the executable in dist/release
LAUNCHER_APP_BASE="$(mktemp -d)" ./dist/release/*-standard-x86_64 --smoke-test
```

Keep the clone in the WSL file system rather than under `/mnt/<drive>`: builds are much faster there.

## Checking launcher updates

Self-update is tested against a local imitation of the GitHub Releases API (`crates/mock-github`), without a real repository:

```bash
cargo xtask mock-releases demo                         # 0.1.0 → 0.2.0 end to end (Windows, Linux)
cargo xtask mock-releases serve --scenario slow        # a server on 127.0.0.1:1430
cargo xtask mock-releases publish 0.3.0 --beta --notes "Test"
cargo xtask mock-releases reset
```

Server scenarios:

- `normal`, `slow`, `drop`;
- `bad-hash`;
- `rate-limit`, `server-error`;
- `paged-assets`, `channels`.

Only builds of the `mock-updates` profile (feature `mock-updates`) trust a server on this machine. Their About page shows that updates come from a test server. The profile cannot be built with `--release` or by `package`, so it never reaches a release.
