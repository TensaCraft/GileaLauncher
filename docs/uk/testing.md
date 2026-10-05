<p align="right"><a href="../en/testing.md">English</a> · <b>Українська</b></p>

# Тести

```bash
cargo xtask test     # the workspace and every module outside the default features (with its features)
cargo xtask lint     # fmt + clippy (native, wasm, modules), no tests
cargo xtask check    # lint + test: run it before pushing; CI runs the same, split into jobs
```

Окремий крейт чи тест запускається звичайною командою `cargo test -p <crate> <filter>`. Для тестів інтерфейсу `test` і `check` спершу збирають фронтенд (`trunk build`).

## Правила

- **Жодних справжніх тек, реєстру чи мережі.** Тести працюють у тимчасових теках (`tempfile`) і з локальними фейковими серверами на `127.0.0.1`. Бекенд тестується через `PathEnv` з тимчасовими шляхами, ніколи через `PathEnv::from_system()`.
- **Спершу тест, що падає.** Кожна зміна поведінки й кожне виправлення починаються з тесту, який відтворює проблему.
- **Сторожі в `xtask`.** Ці тести перевіряють увесь репозиторій:
  - `brand`: бренд не потрапляє в код;
  - `downloads`: кожне завантаження йде через завантажувач ядра;
  - `release_builds_have_no_devtools`: у релізних збірках немає інструментів розробника;
  - `workflows_call_existing_commands`: воркфлоу GitHub викликають лише наявні команди `xtask`.
- **Smoke-тест.** `launcher-app --smoke-test` перевіряє, що готова програма знаходить і створює свої теки. CI запускає його на налагоджувальній збірці, а релізний воркфлоу — на кожному пакеті.

## Linux у WSL

На Windows роботу під Linux можна перевірити у WSL (Ubuntu 22.04). Встановіть залежності з [development.md](development.md), а потім у клоні всередині WSL запустіть:

```bash
cargo test -p launcher-core
cargo xtask check
cargo xtask package --profile standard   # the AppImage and the executable in dist/release
LAUNCHER_APP_BASE="$(mktemp -d)" ./dist/release/*-standard-x86_64 --smoke-test
```

Тримайте клон у файловій системі WSL, а не в `/mnt/<drive>`: там збирання значно швидше.

## Перевірка оновлень лаунчера

Самооновлення тестується на локальній імітації GitHub Releases API (`crates/mock-github`), без справжнього репозиторію:

```bash
cargo xtask mock-releases demo                         # 0.1.0 → 0.2.0 end to end (Windows, Linux)
cargo xtask mock-releases serve --scenario slow        # a server on 127.0.0.1:1430
cargo xtask mock-releases publish 0.3.0 --beta --notes "Test"
cargo xtask mock-releases reset
```

Сценарії сервера:

- `normal`, `slow`, `drop`;
- `bad-hash`;
- `rate-limit`, `server-error`;
- `paged-assets`, `channels`.

Серверу на цьому комп'ютері довіряють лише збірки профілю `mock-updates` (фіча `mock-updates`). Їхня сторінка «Про програму» показує, що оновлення приходять з тестового сервера. Цей профіль не можна зібрати з `--release` чи через `package`, тож у реліз він ніколи не потрапляє.
