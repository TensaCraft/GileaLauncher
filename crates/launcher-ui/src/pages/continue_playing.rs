//! Home's «Продовжити гру»: the builds played last, each with its last server (its MOTD,
//! players and ping, as the game's list shows them) or world, «Грати» straight into it and a cube
//! for the build alone.

use launcher_shared::BuildDto;
use launcher_shared::SettingUpdate;
use launcher_shared::recent::RECENT_MOST;
use launcher_shared::recent::{Activity, MotdSpan, RecentBuild};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{
    Button, Dialog, DialogFooter, Icon, IconAction, Select, SelectOption, SettingRow, Size, Tag, TagTone,
    Variant,
};

use crate::builds::dialogs::use_build_menu;
use crate::builds::launch::use_launch_flow;
use crate::builds::{build_subtitle, loader_icon};
use crate::fold::{FoldHead, fold_state};
use crate::recent::{
    Ago, Ping, address_label, ago, difficulty_key, mode_key, motd_lines, ping_bars, server_key, use_recent,
};
use crate::shots::viewer::date_time;
use crate::store::{use_settings_writer, use_store};

/// Local "dd.mm.yyyy" of Unix milliseconds.
fn date_of(ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!("{:02}.{:02}.{}", d.get_date(), d.get_month() + 1, d.get_full_year())
}

fn played(i18n: I18nCtx, played_ms: u64) -> String {
    let n = |count: u64| [("n", count.to_string())];
    match ago(js_sys::Date::now() as u64, played_ms) {
        Ago::JustNow => i18n.t("played_just_now"),
        Ago::Minutes(m) => i18n.tp("played_minutes_ago", &n(m)),
        Ago::Hours(h) => i18n.tp("played_hours_ago", &n(h)),
        Ago::Yesterday => i18n.t("played_yesterday"),
        Ago::Days(d) => i18n.tp("played_days_ago", &n(d)),
        Ago::Long => date_of(played_ms),
    }
}

/// A MOTD in its colours; the colours are the parser's `#rrggbb`.
fn span_view(span: &MotdSpan) -> impl IntoView + use<> {
    let mut style = span.color.as_ref().map(|c| format!("color:{c};")).unwrap_or_default();
    if span.bold {
        style.push_str("font-weight:700;");
    }
    if span.italic {
        style.push_str("font-style:italic;");
    }
    let lines: Vec<&str> = [(span.underlined, "underline"), (span.strikethrough, "line-through")]
        .into_iter()
        .filter_map(|(on, line)| on.then_some(line))
        .collect();
    if !lines.is_empty() {
        style.push_str(&format!("text-decoration:{};", lines.join(" ")));
    }
    view! { <span style=style>{span.text.clone()}</span> }
}

/// A MOTD as the server list shows it: two lines at most, a line the server padded to the middle
/// centred over the widest one.
fn motd_view(spans: &[MotdSpan]) -> impl IntoView + use<> {
    motd_lines(spans)
        .into_iter()
        .map(|line| {
            view! {
                <div class="motd-line" class:is-centered=line.centered>
                    {line.spans.iter().map(span_view).collect_view()}
                </div>
            }
        })
        .collect_view()
}

fn ping_view(ms: u32) -> impl IntoView {
    let bars = ping_bars(ms);
    let tone = match bars {
        4.. => "is-good",
        2..=3 => "is-fair",
        _ => "is-poor",
    };
    view! {
        <span class=format!("ping-bars {tone}") aria-hidden="true">
            {(1..=5u8).map(|bar| view! { <i class:on=move || { bar <= bars }></i> }).collect_view()}
        </span>
    }
}

/// The picture of a server or a world: its own, else `fallback`.
fn tile_icon(image: Option<String>, fallback: &'static str) -> impl IntoView {
    match image {
        Some(src) => view! { <img src=src alt="" draggable="false" /> }.into_any(),
        None => view! { <Icon name=fallback /> }.into_any(),
    }
}

#[component]
fn ServerTile(host: String, port: u16) -> impl IntoView {
    let i18n = use_i18n();
    let recent = use_recent();
    let key = server_key(&host, port);
    let ping = Signal::derive(move || recent.pings.with(|p| p.get(&key).cloned()));
    let favicon = move || match ping.get() {
        Some(Ping::Answered(status)) => status.favicon,
        _ => None,
    };
    let text = move || match ping.get() {
        Some(Ping::Answered(status)) if status.motd.iter().any(|s| !s.text.trim().is_empty()) => {
            view! { <div class="recent-tile__motd">{motd_view(&status.motd)}</div> }.into_any()
        }
        Some(Ping::Answered(status)) => {
            view! { <div class="recent-tile__sub">{status.version}</div> }.into_any()
        }
        Some(Ping::Silent) => {
            view! { <div class="recent-tile__sub is-silent">{i18n.t("recent_server_silent")}</div> }
                .into_any()
        }
        _ => view! { <div class="recent-tile__sub">{i18n.t("recent_server_asking")}</div> }.into_any(),
    };
    let side = move || {
        match ping.get() {
        Some(Ping::Answered(status)) => view! {
            <div class="recent-tile__side">
                {ping_view(status.ping_ms)}
                <div>
                    {format!("{}/{}", status.online, status.max)}
                    " · "
                    {i18n.tp("recent_ping_ms", &[("ms", status.ping_ms.to_string())])}
                </div>
            </div>
        }
        .into_any(),
        Some(Ping::Silent) => {
            view! { <div class="recent-tile__side is-silent"><Icon name="wifi_off" /></div> }.into_any()
        }
        _ => view! { <div class="recent-tile__side"><span class="ping-bars is-asking"><i></i><i></i><i></i><i></i><i></i></span></div> }
            .into_any(),
    }
    };
    view! {
        <div class="recent-tile">
            <div class="recent-tile__icon is-server">{move || tile_icon(favicon(), "dns")}</div>
            <div class="recent-tile__text">
                <div class="recent-tile__title">
                    <span class="recent-tile__name">{address_label(&host, port)}</span>
                    <span class="recent-tile__kind is-server">{move || i18n.t("recent_kind_server")}</span>
                </div>
                {text}
            </div>
            {side}
        </div>
    }
}

#[component]
fn WorldTile(
    name: String,
    mode: String,
    difficulty: String,
    hardcore: bool,
    icon: Option<String>,
    quick_play: bool,
) -> impl IntoView {
    let i18n = use_i18n();
    let details = move || {
        let mut parts = vec![i18n.t(mode_key(&mode))];
        parts.extend(difficulty_key(&difficulty).map(|key| i18n.t(key)));
        if hardcore {
            parts.push(i18n.t("world_hardcore"));
        }
        parts.join(" · ")
    };
    view! {
        <div class="recent-tile">
            <div class="recent-tile__icon is-world">{tile_icon(icon, "public")}</div>
            <div class="recent-tile__text">
                <div class="recent-tile__title">
                    <span class="recent-tile__name">{name}</span>
                    <span class="recent-tile__kind is-world">{move || i18n.t("recent_kind_world")}</span>
                </div>
                <div class="recent-tile__sub">{details}</div>
            </div>
            {(!quick_play).then(|| view! {
                <div class="recent-tile__side" data-tip=move || i18n.t("recent_world_old_tip") data-tip-side="top">
                    <Icon name="info" />
                </div>
            })}
        </div>
    }
}

#[component]
fn RecentRow(dto: BuildDto, recent: RecentBuild) -> impl IntoView {
    let build = dto;
    let i18n = use_i18n();
    let store = use_store();
    let flow = use_launch_flow();
    let build_menu = use_build_menu();
    let key = build.key.clone();
    let starting = Signal::derive(move || store.launching.with(|s| s.contains(&key)));
    // A server that does not answer still takes «Грати», which steps back to a plain button.
    let silent = {
        let state = use_recent();
        let server = match &recent.activity {
            Some(Activity::Server { host, port }) => Some(server_key(host, *port)),
            _ => None,
        };
        Signal::derive(move || {
            server.as_ref().is_some_and(|key| state.pings.with(|p| p.get(key) == Some(&Ping::Silent)))
        })
    };
    let activity = recent.activity.clone();
    let play_tip = {
        let activity = activity.clone();
        Signal::derive(move || match &activity {
            Some(Activity::Server { host, port }) => {
                i18n.tp("recent_play_server_tip", &[("server", address_label(host, *port))])
            }
            Some(Activity::World { name, quick_play: true, .. }) => {
                i18n.tp("recent_play_world_tip", &[("world", name.clone())])
            }
            Some(Activity::World { .. }) => i18n.t("recent_world_old_tip"),
            None => i18n.t("play"),
        })
    };
    let (for_play, for_build, for_menu) = (build.clone(), build.clone(), build.clone());
    let join = activity.as_ref().map(Activity::join);
    let play = Callback::new(move |()| flow.start_into(for_play.clone(), join.clone()));
    let build_only = Callback::new(move |()| flow.start(for_build.clone()));
    let on_menu = move |ev: MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        build_menu.open(for_menu.clone(), ev.client_x(), ev.client_y());
    };
    let picture = match build.image.clone().filter(|s| !s.trim().is_empty()) {
        Some(src) => view! { <img src=src alt="" draggable="false" /> }.into_any(),
        None => view! { <Icon name=loader_icon(build.client.as_deref().or(build.loader.as_deref())) /> }
            .into_any(),
    };
    let tile = match activity.clone() {
        Some(Activity::Server { host, port }) => view! { <ServerTile host=host port=port /> }.into_any(),
        Some(Activity::World { name, mode, difficulty, hardcore, icon, quick_play, .. }) => view! {
            <WorldTile name=name mode=mode difficulty=difficulty hardcore=hardcore icon=icon quick_play=quick_play />
        }
        .into_any(),
        None => view! {
            <div class="recent-tile is-empty">
                <div class="recent-tile__sub">{move || i18n.t("recent_no_activity")}</div>
            </div>
        }
        .into_any(),
    };
    let has_activity = activity.is_some();
    let running = build.running;
    let played_ms = recent.played_ms;
    view! {
        <div class="list-row recent-row" class:is-running=running on:contextmenu=on_menu>
            <div class="build-row__icon">{picture}</div>
            <div class="recent-row__build">
                <div class="build-row__name" data-tip=build.name.clone() data-tip-side="top">{build.name.clone()}</div>
                <div class="build-row__sub">{build_subtitle(&build)}</div>
                <div class="recent-row__when">
                    {move || played(i18n, played_ms)}
                    {running.then(|| view! { <Tag tone=TagTone::Vanilla>{move || i18n.t("version_running_badge")}</Tag> })}
                </div>
            </div>
            {tile}
            <div class="recent-row__actions">
                {move || {
                    let variant = if silent.get() { Variant::Secondary } else { Variant::Primary };
                    view! {
                        <Button variant=variant icon="play_arrow" loading=starting title=play_tip on_click=play>
                            {move || i18n.t("play")}
                        </Button>
                    }
                }}
                {has_activity.then(|| view! {
                    <Button
                        size=Size::Md
                        icon="view_in_ar"
                        title=Signal::derive(move || i18n.t("recent_build_only"))
                        disabled=starting
                        on_click=build_only
                    />
                })}
            </div>
        </div>
    }
}

/// What «Продовжити гру» shows: how many builds, and its history cleared or given back.
#[component]
fn RecentSettings(open: RwSignal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let store = use_store();
    let writer = use_settings_writer();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let count = RwSignal::new(store.settings.get_untracked().home_recent_builds.to_string());
    Effect::new(move |_| count.set(store.settings.get().home_recent_builds.to_string()));
    let counts = Signal::derive(move || {
        (0..=RECENT_MOST)
            .map(|n| {
                let label = if n == 0 { i18n.t("home_recent_builds_none") } else { n.to_string() };
                SelectOption::new(n.to_string(), label)
            })
            .collect::<Vec<_>>()
    });
    let cleared = Memo::new(move |_| store.settings.with(|s| s.home_recent_cleared_ms));
    let history_desc = Signal::derive(move || match cleared.get() {
        Some(at) => i18n.tp("recent_history_cleared", &[("date", date_time(at))]),
        None => i18n.t("recent_history_desc"),
    });
    view! {
        <Dialog open=open icon="history" title=t("continue_playing") subtitle=t("recent_settings_desc")>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            <SettingRow title=t("home_recent_builds") desc=t("home_recent_builds_desc")>
                <div style="width:200px">
                    <Select
                        options=counts
                        value=count
                        icon="history"
                        on_change=Callback::new(move |v: String| {
                            if let Ok(n) = v.parse() {
                                writer.apply(SettingUpdate::HomeRecentBuilds(n));
                            }
                        })
                    />
                </div>
            </SettingRow>
            <SettingRow title=t("recent_history") desc=history_desc>
                {move || match cleared.get() {
                    Some(_) => view! {
                        <Button icon="restore" on_click=move |_| writer.apply(SettingUpdate::HomeRecentClear(false))>
                            {move || i18n.t("recent_restore")}
                        </Button>
                    }
                    .into_any(),
                    None => view! {
                        <Button variant=Variant::Danger icon="delete_sweep" on_click=move |_| writer.apply(SettingUpdate::HomeRecentClear(true))>
                            {move || i18n.t("recent_clear")}
                        </Button>
                    }
                    .into_any(),
                }}
            </SettingRow>
        </Dialog>
    }
}

/// «Продовжити гру»: its bar (fold, settings) and the builds played last. Shown while it may show
/// some (a count above 0) and has some, or was cleared (it says so, and how to get them back).
#[component]
pub fn ContinuePlaying(
    /// Whether the section is there (Home's «Усі збірки» bar comes with it).
    shown: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let folded = fold_state("home.fold.recent");
    let store = use_store();
    let recent = use_recent();
    let settings_open = RwSignal::new(false);
    // Asked on each visit; again when a game starts or ends, the count changes or the history is
    // cleared (servers already asked this visit are not asked again).
    let count = Memo::new(move |_| store.settings.with(|s| s.home_recent_builds));
    let cleared = Memo::new(move |_| store.settings.with(|s| s.home_recent_cleared_ms));
    let running = Memo::new(move |_| {
        store.builds.with(|b| b.iter().filter(|b| b.running).map(|b| b.key.clone()).collect::<Vec<_>>())
    });
    Effect::new(move |visit: Option<()>| {
        count.track();
        cleared.track();
        running.track();
        if count.get_untracked() > 0 {
            recent.refresh(visit.is_none());
        }
    });
    let rows = move || {
        let found = recent.builds.get().unwrap_or_default();
        store.builds.with(|builds| {
            found
                .into_iter()
                .take(usize::from(count.get()))
                .filter_map(|r| builds.iter().find(|b| b.key == r.key).cloned().map(|b| (b, r)))
                .collect::<Vec<_>>()
        })
    };
    Effect::new(move |_| shown.set(count.get() > 0 && (!rows().is_empty() || cleared.get().is_some())));
    view! {
        <Show when=move || shown.get()>
            <section class="recent">
                <FoldHead icon="history" title_key="continue_playing" folded=folded>
                    <IconAction
                        icon="tune"
                        title=Signal::derive(move || i18n.t("recent_settings"))
                        on_click=Callback::new(move |()| settings_open.set(true))
                    />
                </FoldHead>
                <div class="recent__rows" class:is-hidden=folded>
                    {move || {
                        let rows = rows();
                        if rows.is_empty() {
                            view! { <div class="recent__cleared">{i18n.t("recent_cleared_empty")}</div> }.into_any()
                        } else {
                            rows.into_iter().map(|(build, recent)| view! { <RecentRow dto=build recent=recent /> }).collect_view().into_any()
                        }
                    }}
                </div>
            </section>
        </Show>
        <RecentSettings open=settings_open />
    }
}
