<p align="right"><a href="../en/development.md">English</a> · <b>Українська</b></p>

# Розробка

## Вимоги

- **Rust stable.** `rust-toolchain.toml` додає `rustfmt`, `clippy` і ціль `wasm32-unknown-unknown`.
- **[trunk](https://trunkrs.dev):** `cargo install trunk --locked`.
- **Tauri CLI 2** для пакування й іконок: `cargo install tauri-cli --version "^2" --locked`.
- **Системні залежності Tauri:** https://tauri.app/start/prerequisites/.
  - Windows: WebView2 (вбудований у Windows 10 і 11) і MSVC Build Tools.
  - Linux (Ubuntu 22.04):
    ```bash
    sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev libasound2-dev patchelf
    ```
    Для пакування AppImage потрібен ще `libfuse2` (гравцям він не потрібен).
  - macOS: Xcode Command Line Tools.

Після клонування один раз увімкніть хуки репозиторію:

```bash
cargo xtask hooks
```

## Команди

Усе запускається через `cargo xtask`:

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

Для CurseForge потрібен API-ключ лаунчера. Покладіть свою копію в `.dev/curseforge-api-key` (git цю теку ігнорує): `cargo xtask dev` і `build` передають ключ у збирання застосунку, але ніколи не в інтерфейс. Ключ, заданий у `CURSEFORGE_API_KEY`, має пріоритет. Без ключа застосунок працює без CurseForge.

Під час розробки (`dev` та інші налагоджувальні збірки) застосунок тримає всі свої дані в `.dev/` у корені репозиторію, тож справжні дані вашого лаунчера не зачіпаються. Щоб налагоджувальна збірка використовувала системні теки, задайте `LAUNCHER_DEV=0`. Щоб будь-яка збірка тримала все в одній теці, вкажіть абсолютний шлях у `LAUNCHER_APP_BASE`.

У налагоджувальній збірці Shift+правий клік відкриває меню браузера, і доступні інструменти розробника. У релізній збірці меню браузера немає (у текстових полях воно лишається), а клавіші браузера (F5, F12, Ctrl+R, Ctrl+Shift+I…) нічого не роблять.

### Інтерфейс у браузері

Інтерфейс працює й без Tauri, з імітацією бекенда:

```bash
cd crates/launcher-ui && trunk serve
```

Потім відкрийте `http://127.0.0.1:1420`:

- `/dev/kit` — галерея компонентів;
- `?setup=1` — майстер першого запуску;
- `?media=http://127.0.0.1:8000` — картинки майстра з іншої теки (ще не запушені);
- `?lang=en_US`, `?recent=0`, `?cards=bar`, `?sidebar=full` — старт з іншими налаштуваннями;
- `?held=wait` — файли модпака, які треба завантажити вручну (Adrenaline), так і не з'являються, тож їхній діалог не зникає;
- `?builds=none` — без збірок;
- `?profiles=none` — без профілів: кнопка профілів підказує створити перший;
- `?installfail=1` — створення збірки не може завантажити файли (збій із «Повідомити»);
- `?gamecrash=1` — після «Грати» гра вилітає: її вікно відкриває краш-репорт і логи;
- `?reports=crash` — лаунчер минулого разу аварійно закрився: на старті з'являється запитання; `?reports=fail` — сервер звітів недоступний (обидва: `?reports=crash,fail`).

## Профілі

Профілі лежать у `build-profiles/*.toml`. Кожен задає:

- модулі;
- бренд;
- редакцію (`edition`);
- репозиторій оновлень;
- звідки завантажуються картинки майстра першого запуску (`media_url`);
- налаштування модулів.

| Профіль | Модулі | Редакція | Публікується |
|---|---|---|---|
| `standard` | modrinth, backups, reports | `standard` | так |
| `full` | tensa, modrinth, backups, reports | `tensa` | так |
| `core` | немає | `core` | ні |
| `mock-updates` | як у standard | `standard` | ні (лише для перевірки оновлень) |

Профіль публікується, коли в ньому є `update_repo` і немає `update_api`. Див. [releasing.md](releasing.md).

### Картинки майстра першого запуску

Майстер показує, як виглядає кожен варіант (Головна, кнопка «Грати», бічна панель). Картинки для цього він завантажує під час показу, у лаунчер вони не пакуються: `<media_url>/<lang>/<name>.jpg`, з `setup/` у гілці `media` (там лежать картинки репозиторію, щоб не тримати їх у `main`). Уже випущені лаунчери й далі їх завантажують, тому картинку замінюють під тією самою назвою і ніколи не перейменовують і не видаляють. Без мережі варіант показує свою іконку. Коли ці частини інтерфейсу змінюються, зніміть картинки заново (обидві мови, 1280 × 800 з подвоєною щільністю пікселів, обрізані до 16:10). Як це зробити і правила гілки — у [media.md](media.md).

## Правила коду

- **Бренд.** Назва й бренд є лише в профілях, `tauri.conf.json`, іконках, `launcher-shared/src/branding.rs` і документації. Тест в `xtask` (сторож бренду) перевіряє кожен файл репозиторію.
- **Завантаження.** Кожне завантаження йде через єдиний завантажувач ядра, `launcher_core::net::downloader`. Тест в `xtask` стежить, щоб жоден інший код не завантажував нічого сам.
- **Мова.** Тексти інтерфейсу лежать у `assets/langs/{uk_UA,en_US}.json` і в `locales/` модулів. Код, коментарі й повідомлення комітів пишуться англійською; документація — англійською, і англійські документи лежать у `README.md` і `docs/en/`, а кожен має українську копію (`README.uk.md`, `docs/uk/`), яку оновлюють разом з англійським.

## Коміти й хуки

Повідомлення комітів відповідають [Conventional Commits](https://www.conventionalcommits.org):

```
kind(scope): summary
```

- **`kind`:** `feat`, `fix`, `ui`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore` або `release`.
- **`scope`:** необов'язковий, у нижньому регістрі.
- **`!` перед двокрапкою** позначає несумісну зміну (breaking change).
- **`summary`:** англійською (ASCII), від 15 до 120 символів. Розмиті слова на кшталт `wip`, `fix` чи `update` не приймаються.
- **Власні заголовки git** (`Merge …`, `Revert …`, `fixup! …`, `squash! …`) приймаються як є.

Приклади:

```
fix(updater): each edition updates from its own files
ui(home): cards keep one height in every row
release: bump version to 0.2.0
```

Коміти `feat`, `fix`, `ui` і `perf` потрапляють у примітки до релізу. Інші типи внутрішні, і в примітки вони не потрапляють.

Хуки лежать у `.githooks/` і вмикаються командою `cargo xtask hooks`.

- **`pre-commit`** запускає `cargo xtask hook pre-commit`:
  - `cargo fmt --check`;
  - `git diff --check` для проіндексованих змін;
  - `cargo clippy --workspace --all-targets -D warnings`.

  `LAUNCHER_SKIP_PRECOMMIT=1` вимикає ці перевірки для одного коміту. У свіжому клоні хук спершу збирає фронтенд, бо без нього clippy для застосунку не працює.
- **`commit-msg`** запускає `cargo xtask commit-msg <file>`.

Якщо вам потрібні додаткові перевірки поза репозиторієм, покладіть їх у `.git/hooks/pre-commit.local` і `.git/hooks/commit-msg.local`. Хуки запускають їх після власних перевірок, і `LAUNCHER_SKIP_PRECOMMIT` їх не пропускає.

CI повторює обидві перевірки й запускає тести, які хуки пропускають:
- `cargo xtask lint` один раз, на Linux;
- `cargo xtask test` на Windows, Linux і macOS паралельно;
- збирання профілів і запуск застосунку один раз, на Linux;
- перевірку повідомлення кожного коміту в пул-реквесті.

У `main` кожен коміт зберігає свій прогін CI (новіший пуш скасовує лише старіший прогін пул-реквесту): реліз читає вердикт свого коміту.
