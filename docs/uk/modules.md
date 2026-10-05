<p align="right"><a href="../en/modules.md">English</a> · <b>Українська</b></p>

# Модулі

Модуль — це опційна частина лаунчера: збірка містить його, лише коли цього вимагає її профіль. Ядро не знає про жоден конкретний модуль і з усіма працює через два контракти. Бекенд реалізує `launcher_core::modules::Module`, інтерфейс — `ui_kit::module::UiModule`.

| Модуль | Що додає | У профілях |
|---|---|---|
| `modrinth` | моди, ресурспаки, шейдери й модпаки з Modrinth (провайдер вмісту) | standard, full |
| `backups` | бекапи світів | standard, full |
| `reports` | звіти про проблеми самого лаунчера (збій операції, аварія лаунчера, те, що описав користувач), які надсилаються на `endpoint` профілю | standard, full |
| `tensa` | збірки серверів TensaCraft на Головній | full |
| `curseforge` | моди, ресурспаки, шейдери й модпаки з CurseForge (провайдер вмісту; потрібен API-ключ CurseForge, див. нижче) | standard, full |
| `diagnostics` | відкладений модуль діагностики | немає (поза `--modules all`) |

## Структура

```
modules/<id>/
  Cargo.toml        # features backend (launcher-core) and ui (ui-kit, leptos)
  locales/          # uk_UA.json, en_US.json: the module's texts over the common ones
  src/lib.rs        # pub const ID; mod backend (feature backend); mod ui (feature ui)
  src/backend/      # impl Module
  src/ui/           # impl UiModule
  tests/            # backend tests
```

- **Реєстрація.** У `launcher-app` і `launcher-ui` для кожного модуля є фіча `mod-<id>`. Списки модулів збірки лежать у `crates/launcher-app/src/modules.rs` і `crates/launcher-ui/src/modules.rs`.
- **Типові фічі.** Модулі поза типовими фічами (`tensa`, `diagnostics`) `cargo xtask check` і `test` перевіряють окремо: це список `EXTRA_MODULES` у `xtask/src/cmd.rs`.
- **Ключ CurseForge.** `curseforge` працює лише з API-ключем лаунчера для CurseForge. Релізний воркфлоу передає його із секрету репозиторію `CURSE_FORGE_KEY` у крок Package як `CURSEFORGE_API_KEY`. Читає ключ лише бекенд модуля (`option_env!`), а збирання інтерфейсу його ніколи не отримує. Збірка без ключа (CI, форки) компілюється, проходить тести й просто не показує CurseForge. Ключ ніколи не потрапляє в репозиторій: сторож в `xtask` падає на будь-якому рядку, схожому на ключ.

## Бекенд: `Module`

Усі методи, крім `id` і `version`, необов'язкові:

| Метод | Для чого |
|---|---|
| `config_defaults` | ключі конфігу модуля зі значеннями за замовчуванням |
| `init` | старт разом з ядром (`ModuleContext`: теки, конфіг, сповіщення) |
| `provider_info` / `provider` | модуль є провайдером вмісту (див. нижче) |
| `launch_hook` | крок перед кожним запуском гри (бекап світу в `backups`, синхронізація серверної збірки в `tensa`) |
| `game_watcher` | стеження за запущеною грою |
| `call` | власні команди модуля, які інтерфейс викликає через `module_invoke` |
| `changes_builds` | команда змінює збірки: після неї застосунок оновлює список збірок |

## Інтерфейс: `UiModule`

Модуль може додати в інтерфейс:

- сторінки бічної панелі (`pages`);
- вкладки вмісту збірки (`content_tabs`);
- розділи налаштувань (`settings_sections`);
- опції в діалозі «Видалити збірку» (`delete_options`);
- вікна поверх усього застосунку (`overlays`);
- картки на Головній (`home_cards`). Компонент картки через `report_home_cards` повідомляє Головній, скільки карток він показує, тож Головна пише «збірок ще немає», лише коли на ній нічого немає;
- пункти меню збірки (`build_actions`);
- кнопки попередження, про яке можна повідомити (`alert_actions`);
- попередження над вкладкою провайдера (`content_notices`);
- рядки вікна «Підтримка» (`support_actions`);
- переклади (`locale_json`).

Сповіщення про збій операції пропонує «Повідомити» через `ui_kit::problem`: застосунок дає `ProblemReporter`, а модуль, який уміє надсилати звіти, вмикає його `available` і відповідає на `request`.

Компоненти беруться з `ui-kit`, тому модулі виглядають так само, як ядро.

## Провайдери вмісту

Провайдер — це модуль, який знаходить і встановлює вміст. У `ProviderInfo` він оголошує:

- що вміє шукати й встановлювати (`content`);
- що вміє оновлювати (`updates`);
- чи працює з модпаками (`modpacks`, `modpack_updates`).

Контракт `launcher_core::providers::ContentProvider` має `search`, `plan`, `install`, `overview`, `modpacks`, `modpack_versions` та інші методи. Усі вони необов'язкові: за замовчуванням відповідають «не підтримується». Ядро питає провайдера лише про те, що той оголосив. Інтерфейс вмісту спільний для всіх провайдерів, тож новому провайдеру власні сторінки не потрібні.

## Налаштування з профілю

Розділ `[modules.<id>]` профілю перетворюється на змінні часу компіляції `LAUNCHER_MOD_<ID>_<KEY>`, які модуль читає через `option_env!`. Наприклад, `[modules.reports] endpoint` стає `LAUNCHER_MOD_REPORTS_ENDPOINT`.

## Новий модуль

```bash
cargo xtask new-module <id>
```

Команда створює заготовку й підказує, де її зареєструвати:

1. `Cargo.toml`: учасник воркспейсу й залежність `module-<id>`.
2. `crates/launcher-app/Cargo.toml` і `crates/launcher-ui/Cargo.toml`: фіча `mod-<id>`.
3. `crates/launcher-app/src/modules.rs` і `crates/launcher-ui/src/modules.rs`: модуль під `#[cfg(feature = "mod-<id>")]`.
4. `xtask/src/profile.rs`: `KNOWN_MODULES`. Також додайте модуль у профілі, яким він потрібен, і в `EXTRA_MODULES`, якщо його немає серед типових фіч.
