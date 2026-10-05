<p align="right"><a href="../en/releasing.md">English</a> · <b>Українська</b></p>

# Випуск релізів

## Версія й тег

Версія лаунчера — це `[workspace.package] version` у кореневому `Cargo.toml`. Тег релізу має вигляд `v<version>`, наприклад `v0.2.0` чи `v0.3.0-beta.1`. Версія із суфіксом (`-beta.1`) і реліз, позначений як pre-release, належать до бета-каналу. Воркфлоу сам позначає реліз версії із суфіксом як pre-release, тож бета ніколи не стає «Latest».

```bash
cargo xtask release-meta                         # {"prerelease":false,"tag":"v0.1.0","title":"<name> 0.1.0","version":"0.1.0"}
cargo xtask release-meta --field tag             # v0.1.0
cargo xtask release-meta --editions              # [{"edition":"tensa","profile":"full"},{"edition":"standard","profile":"standard"}]
cargo xtask release-meta --field prerelease      # true for a suffixed version
cargo xtask release-meta --check-tag v0.1.0      # fails unless the tag is the version's
cargo xtask release-meta --check-new-tag v0.1.0  # fails when the tag already names another commit
```

## Редакції

Редакцію профілю задає його поле `edition`. Публікуються профілі з `update_repo` і без `update_api`. Зараз це:

| Профіль | Редакція | Модулі |
|---|---|---|
| `standard` | `standard` | modrinth, curseforge, backups, reports |
| `full` | `tensa` | tensa, modrinth, curseforge, backups, reports |

Одну редакцію може публікувати лише один профіль. Кожна збірка оновлюється лише з файлів своєї редакції. Якщо в релізі немає файлу її редакції для її системи, оновлення не пропонується.

Щоб додати редакцію, створіть профіль з новим `edition` (малі латинські літери й цифри) і тим самим `update_repo`. Воркфлоу підхопить його сам.

## Файли релізу

`cargo xtask package --profile <profile>` збирає профіль для релізу (`trunk --release`, `cargo tauri build`) і кладе файли в `dist/release/`. Для назви `GileaLauncher` і редакції `standard`:

| Система | Файли | Для чого |
|---|---|---|
| Windows | `GileaLauncher-standard.exe` | портативна версія й файл оновлення |
| | `GileaLauncher-standard-Setup.exe` | інсталятор NSIS для поточного користувача |
| Linux | `GileaLauncher-standard-x86_64.AppImage` | AppImage та його оновлення |
| | `GileaLauncher-standard-x86_64` | виконуваний файл та його оновлення |
| macOS | `GileaLauncher-standard-universal.dmg` | Apple Silicon та Intel |

Назви задає `launcher_shared::release_file_names`. Нею користуються і апдейтер, і `package`, тож назви не можуть розійтися; це перевіряє тест `packaged_names_are_the_updaters_names`.

Пакет можна зібрати лише на його власній системі. Для macOS потрібні цілі `aarch64-apple-darwin` і `x86_64-apple-darwin`.

## Примітки до релізу

```bash
cargo xtask release-notes --tag v0.2.0 [--previous v0.1.0] [--output RELEASE_NOTES.md]
```

Примітки складаються із заголовків комітів від попереднього тегу. Для стабільного тегу попереднім вважається попередній стабільний тег, тож примітки охоплюють і бети між ними.

| Тип коміту | Розділ приміток |
|---|---|
| `feat` | New Features |
| `fix` | Fixes |
| `ui` | Interface |
| `perf` | Performance |
| коміти без типу | Other Changes |

Коміти `build`, `chore`, `ci`, `docs`, `refactor`, `release` і `test` у примітки не потрапляють.

## Як випустити реліз

1. Підніміть версію в `Cargo.toml` і закомітьте її: `release: bump version to 0.2.0`.
2. Запуште коміт у `main`.
3. Вручну запустіть воркфлоу **Release** (`.github/workflows/release.yml`) на `main` (**Run workflow** або `gh workflow run release.yml --ref main`), за потреби з перемикачами `prerelease` і `draft`. Ніколи не створюйте тег чи реліз самі. Воркфлоу:
   1. перевіряє, що запуск іде на `main` і що тег `v<version>` не вказує на інший коміт: опублікований реліз ніколи не перезбирається з іншого коду;
   2. перевіряє, що ключ CurseForge (секрет `CURSE_FORGE_KEY`) є для кожної редакції з CurseForge;
   3. бере вердикт CI для коміту замість того, щоб перевіряти код ще раз: прогону CI, що ще триває, він дочікується, а провалений прогін зупиняє реліз. Лише коли в CI немає вердикту (коміт лише з документацією, скасований прогін), перевірки запускаються в самому релізі;
   4. збирає кожну редакцію на Windows, Linux і macOS і проганяє smoke-тест кожного пакета (паралельно з очікуванням CI);
   5. ставить тег на коміт, пише примітки й вивантажує файли в чернетку релізу;
   6. публікує реліз, щойно всі файли на місці (якщо не ввімкнено `draft`);
   7. видаляє тимчасові артефакти. Якщо якийсь крок падає, вони лишаються, і **Re-run failed jobs** може завершити реліз.

Одночасно може йти лише один реліз.

Перемикач `dry_run` лише збирає всі пакети й проганяє їхні smoke-тести, без тегу й релізу. Файли лишаються в артефактах прогону на добу. Так можна перевірити збирання на всіх системах перед першим релізом.

Поки файли релізу вивантажуються, апдейтер цей реліз не пропонує: у ньому ще немає файлу потрібної редакції.

## Підпис

Файли поки не підписані й не нотаризовані. Що робити гравцям на Windows (SmartScreen) і macOS (Gatekeeper), описано в [README](../../README.uk.md#встановлення).
