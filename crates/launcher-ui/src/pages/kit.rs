use launcher_shared::Level;
use leptos::prelude::*;
use ui_kit::menu::use_context_menu;
use ui_kit::*;

use crate::profiles::launch::{LaunchProfileSelector, ProfileRequiredDialog};
use crate::shell::PageHeader;

#[component]
pub fn KitPage() -> impl IntoView {
    if !cfg!(debug_assertions) {
        return view! { <PageHeader title_key="home_title" /> }.into_any();
    }
    let toasts = use_toasts();
    let menu = use_context_menu();
    let need_profile = RwSignal::new(false);
    let pick_profile = RwSignal::new(false);
    let name = RwSignal::new("Aeronautics".to_string());
    let port = RwSignal::new("70000".to_string());
    let path = RwSignal::new("C:\\Games\\Launcher".to_string());
    let loader = RwSignal::new("21.1.250".to_string());
    let on = RwSignal::new(true);
    let off = RwSignal::new(false);
    let seg = RwSignal::new("installed".to_string());
    let ram = RwSignal::new(8.0);
    let jvm = RwSignal::new("-XX:+UseG1GC\n-XX:+UseStringDeduplication\n-XX:MaxGCPauseMillis=80".to_string());
    let tab = RwSignal::new("mods");
    let dialog = RwSignal::new(false);
    let confirm = RwSignal::new(false);
    let loaders = Signal::derive(|| {
        vec![
            SelectOption::new("21.1.250", "21.1.250").in_group("Стабільні").with_meta("остання"),
            SelectOption::new("21.1.228", "21.1.228").in_group("Стабільні"),
            SelectOption::new("21.1.251-beta", "21.1.251-beta").in_group("Бета"),
        ]
    });
    let s = |v: &str| {
        Signal::derive({
            let v = v.to_string();
            move || v.clone()
        })
    };

    view! {
        <PageHeader title_key="home_title" />
        <div class="kit">
            <div class="kit__card"><h4>"Кнопки"</h4>
                <div class="stack">
                    <div class="wrap">
                        <Button variant=Variant::Primary icon="play_arrow">"Грати"</Button>
                        <Button icon="add">"Додати збірку"</Button>
                        <Button variant=Variant::Ghost>"Скасувати"</Button>
                        <Button variant=Variant::Danger icon="delete" outlined=true>"Видалити"</Button>
                    </div>
                    <div class="wrap">
                        <Button variant=Variant::Primary size=Size::Sm>"Малий"</Button>
                        <Button variant=Variant::Primary>"Середній"</Button>
                        <Button variant=Variant::Primary size=Size::Lg>"Великий"</Button>
                        <Button variant=Variant::Primary loading=true>"Встановлення"</Button>
                        <Button disabled=true>"Недоступно"</Button>
                    </div>
                    <div class="wrap">
                        <Button size=Size::Sm icon="person_add">"Офлайн"</Button>
                        <Button variant=Variant::Primary size=Size::Sm icon="add">"Microsoft"</Button>
                        <TextInput value=name size=Size::Sm />
                    </div>
                    <div class="wrap">
                        <Button icon="add">"Середня"</Button>
                        <Button variant=Variant::Primary icon="play_arrow">"Грати"</Button>
                        <TextInput value=name />
                        <IconAction icon="folder" title=s("Тека") />
                    </div>
                    <div class="wrap">
                        <Button icon="refresh" />
                        <IconAction icon="play_arrow" title=s("Грати") />
                        <IconAction icon="power_settings_new" tone=ActionTone::Ok title=s("Вимкнути") />
                        <IconAction icon="delete" tone=ActionTone::Danger title=s("Видалити") />
                        <Tooltip text=s("Підказка праворуч")><IconAction icon="folder" title=s("Тека") /></Tooltip>
                    </div>
                </div>
            </div>
            <div class="kit__card"><h4>"Поля"</h4>
                <div class="grid2">
                    <Field label=s("Назва збірки")><TextInput value=name /></Field>
                    <Field label=s("Порт сервера") error="Порт має бути від 1 до 65535"><TextInput value=port invalid=true /></Field>
                    <Field label=s("Каталог Minecraft") optional_note="необов'язково" hint="Вільно 412 ГБ">
                        <PathField value=path browse_label=s("Огляд") on_browse=Callback::new(|_| {}) />
                    </Field>
                    <Field label=s("Версія NeoForge")><Select options=loaders value=loader /></Field>
                </div>
            </div>
            <div class="kit__card"><h4>"Перемикачі"</h4>
                <div class="stack">
                    <div class="wrap"><Switch checked=on /><Switch checked=off /><Checkbox checked=on label=s("Показувати снапшоти") /></div>
                    <Segmented
                        value=seg
                        options=Signal::derive(|| vec![
                            SegOption::new("installed", "Встановлені").with_icon("inventory_2"),
                            SegOption::new("modrinth", "Modrinth").with_icon("brand:modrinth"),
                        ])
                    />
                    <Slider value=ram min=1.0 max=29.0 recommended=8.0 ticks=vec!["1".into(), "8".into(), "16".into(), "29 ГБ".into()] />
                    <span class="value">{move || format!("{} ГБ", ram.get())}</span>
                </div>
            </div>
            <div class="kit__card"><h4>"JVM, мітки, прогрес"</h4>
                <div class="stack">
                    <CodeEditor value=jvm />
                    <div class="wrap">
                        <Tag tone=TagTone::Vanilla>"Minecraft"</Tag>
                        <Tag tone=TagTone::Fabric>"Fabric"</Tag>
                        <Tag tone=TagTone::Forge>"Forge"</Tag>
                        <Tag tone=TagTone::NeoForge>"NeoForge"</Tag>
                        <Tag tone=TagTone::Quilt>"Quilt"</Tag>
                        <Tag tone=TagTone::Snapshot>"Snapshot"</Tag>
                    </div>
                    <ProgressBar value=Signal::derive(|| Some(56.0)) />
                    <ProgressBar value=Signal::derive(|| None) />
                    <Skeleton width=60 /><Skeleton width=85 />
                </div>
            </div>
            <div class="kit__card"><h4>"Вкладки та порожній стан"</h4>
                <Tabs
                    active=tab
                    tabs=vec![
                        TabDef { id: "mods", icon: "extension", label: s("Моди") },
                        TabDef { id: "packs", icon: "palette", label: s("Ресурспаки") },
                        TabDef { id: "shaders", icon: "wb_sunny", label: s("Шейдери") },
                    ]
                />
                <EmptyState icon="layers" title=s("Ще немає збірок") desc="Створіть першу збірку." />
            </div>
            <div class="kit__card"><h4>"Оверлеї"</h4>
                <div class="wrap">
                    <Button on_click=move |_| dialog.set(true)>"Діалог"</Button>
                    <Button variant=Variant::Danger on_click=move |_| confirm.set(true)>"Підтвердження"</Button>
                    <Button on_click=move |_| need_profile.set(true)>"Потрібен профіль"</Button>
                    <Button on_click=move |_| pick_profile.set(true)>"Вибір профілю"</Button>
                    <Button on_click=move |_| toasts.show(Level::Success, "Збірку створено".into(), Some("Готова до гри.".into()))>"Тост ✓"</Button>
                    <Button on_click=move |_| toasts.show(Level::Warning, "Закрийте Minecraft".into(), None)>"Тост !"</Button>
                    <Button on_click=move |_| toasts.push(ToastItem { id: 7, level: Level::Info, title: "Оновлення 1.1.0".into(), message: None, action: Some(("Встановити".into(), "update".into())), duration_ms: 9000 })>"Тост з дією"</Button>
                    <div
                        class="list-row"
                        on:contextmenu=move |ev| {
                            ev.prevent_default();
                            menu.open_at(ev.client_x(), ev.client_y(), vec![
                                MenuEntry::item("play", "Грати").icon("play_arrow").shortcut("Enter"),
                                MenuEntry::item("copy", "Копіювати").icon("content_copy"),
                                MenuEntry::Separator,
                                MenuEntry::item("delete", "Видалити збірку").icon("delete").danger(),
                            ]);
                        }
                    >"ПКМ тут — контекстне меню"</div>
                </div>
                <Dialog open=dialog title=s("Створити збірку") subtitle="Minecraft 1.21.1 · NeoForge" icon="add" footer_hint="~ 420 МБ до завантаження">
                    <DialogFooter slot>
                        <Button variant=Variant::Ghost on_click=move |_| dialog.set(false)>"Скасувати"</Button>
                        <Button variant=Variant::Primary icon="download" on_click=move |_| dialog.set(false)>"Встановити"</Button>
                    </DialogFooter>
                    <Field label=s("Назва") error="Збірка з такою назвою вже існує"><TextInput value=name invalid=true /></Field>
                </Dialog>
                <ConfirmDialog
                    open=confirm
                    danger=true
                    title=s("Видалити збірку?")
                    message=s("Теку збірки буде видалено разом зі світами.")
                    confirm_label=s("Видалити")
                    cancel_label=s("Скасувати")
                    on_confirm=Callback::new(|_| {})
                />
                <ProfileRequiredDialog open=need_profile />
                <LaunchProfileSelector
                    open=pick_profile
                    version=Signal::derive(|| "Aeronautics".to_string())
                    on_pick=Callback::new(move |key: String| toasts.show(Level::Info, format!("Запуск від {key}"), None))
                />
            </div>
        </div>
    }
    .into_any()
}
