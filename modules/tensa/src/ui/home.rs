//! Home's cards of the server builds not installed yet: a card asks to confirm
//! with the build's description, installs it and starts it as its Play button would.

use std::cell::Cell;

use launcher_shared::{ErrorCode, Level, names};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use ui_kit::i18n::use_i18n;
use ui_kit::module::{report_home_cards, use_module_host};
use ui_kit::{BuildCard, Button, Dialog, DialogFooter, LatestRequest, Variant, ipc, use_toasts};

/// A server build Home offers (the module's `home_packs`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HomePack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// "Fabric 26.3".
    #[serde(default)]
    pub runs: String,
}

/// What `install` answers: the new build.
#[derive(Debug, Clone, Deserialize)]
struct Installed {
    key: String,
}

thread_local! {
    /// Bumped whenever the builds change: an install or a removal changes what Home offers.
    static REVISION: ArcRwSignal<u64> = ArcRwSignal::new(0);
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

fn revision() -> ArcRwSignal<u64> {
    REVISION.with(Clone::clone)
}

/// Hears of the builds from now on: once, however often Home is mounted.
fn listen() {
    if LISTENING.with(|l| l.replace(true)) {
        return;
    }
    let revision = revision();
    ipc::listen::<Value>(names::BUILDS, move |_| revision.update(|n| *n += 1));
}

/// How many cards the server builds put on Home: none until the server answers (Home waits
/// with its empty state), then one per pack, and one while the server does not answer.
pub fn cards_shown(answered: bool, packs: usize, failed: bool) -> usize {
    if answered { packs + usize::from(failed) } else { 1 }
}

/// Takes pack `id` off Home's cards: it is installed.
pub fn drop_pack(packs: &mut Vec<HomePack>, id: &str) {
    packs.retain(|p| p.id != id);
}

/// The description a card's dialog shows: the server's, else a note that there is none.
pub fn description_of(pack: &HomePack) -> Option<String> {
    pack.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(str::to_string)
}

#[component]
pub fn ServerBuildCards() -> impl IntoView {
    listen();
    let revision = revision();
    let packs = RwSignal::new(Vec::<HomePack>::new());
    // The server did not answer: Home says so, with a retry.
    let failed = RwSignal::new(false);
    // Home says it has no builds only when these cards are not there either.
    let answered = RwSignal::new(false);
    report_home_cards(
        crate::ID,
        Signal::derive(move || cards_shown(answered.get(), packs.with(Vec::len), failed.get())),
    );
    // The card being installed: busy until the new build is there.
    let pending = RwSignal::new(None::<String>);
    // Only the latest answer counts: the catalog may answer out of order.
    let latest = StoredValue::new_local(LatestRequest::new());
    Effect::new(move |_| {
        revision.track();
        let Some(request) = latest.try_with_value(|l| l.begin()) else { return };
        spawn_local(async move {
            let answer = ipc::module_invoke::<_, Vec<HomePack>>(crate::ID, "home_packs", &Value::Null).await;
            if !latest.try_with_value(|l| l.is_current(request)).unwrap_or(false) {
                return;
            }
            let _ = answered.try_set(true);
            let _ = failed.try_set(answer.is_err());
            let _ = packs.try_set(answer.unwrap_or_default());
        });
    });
    let asked = RwSignal::new(None::<HomePack>);
    let open = RwSignal::new(false);
    let ask = move |pack: HomePack| {
        asked.set(Some(pack));
        open.set(true);
    };
    let retry = Callback::new(move |_| self::revision().update(|n| *n += 1));
    view! {
        <For each=move || packs.get() key=|p| p.id.clone() let:pack>
            <PackCard pack=pack pending=pending on_play=Callback::new(ask) />
        </For>
        <Show when=move || failed.get()>
            <UnavailableCard on_retry=retry />
        </Show>
        <InstallDialog open=open asked=asked packs=packs pending=pending />
    }
}

/// The server builds' place on Home while their server does not answer.
#[component]
fn UnavailableCard(on_retry: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <BuildCard
            title=i18n.t("module_tensa_name")
            subtitle=i18n.t("server_builds_unavailable")
            image={None::<String>}
            play_label=Signal::derive(move || i18n.t("refresh"))
            icon="refresh"
            on_play=on_retry
        />
    }
}

#[component]
fn PackCard(pack: HomePack, pending: RwSignal<Option<String>>, on_play: Callback<HomePack>) -> impl IntoView {
    let i18n = use_i18n();
    let id = pack.id.clone();
    let busy = Signal::derive(move || pending.with(|p| p.as_deref() == Some(id.as_str())));
    let (title, subtitle, image) = (pack.name.clone(), pack.runs.clone(), pack.image.clone());
    view! {
        <BuildCard
            title=title
            subtitle=subtitle
            image=image
            play_label=Signal::derive(move || i18n.t("minecraft_components_install_action"))
            busy=busy
            icon="download"
            on_play=Callback::new(move |_| on_play.run(pack.clone()))
        />
    }
}

#[component]
fn InstallDialog(
    open: RwSignal<bool>,
    asked: RwSignal<Option<HomePack>>,
    packs: RwSignal<Vec<HomePack>>,
    pending: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    let host = use_module_host();
    let title = Signal::derive(move || {
        let name = asked.with(|p| p.as_ref().map(|p| p.name.clone()).unwrap_or_default());
        i18n.tp("tensacraft_install_confirm_title", &[("version", name)])
    });
    let install = move || {
        let Some(pack) = asked.get_untracked() else { return };
        open.set(false);
        if pending.get_untracked().is_some() {
            return;
        }
        pending.set(Some(pack.id.clone()));
        spawn_local(async move {
            let args = json!({"pack_id": pack.id});
            match ipc::module_invoke::<_, Installed>(crate::ID, "install", &args).await {
                Ok(installed) => {
                    // Its card goes now, not when the catalog answers again.
                    let _ = packs.try_update(|p| drop_pack(p, &pack.id));
                    if let Some(host) = host {
                        host.launch.run(installed.key);
                    }
                }
                // Other failures are told by the backend's alert, which can be reported.
                Err(e) if e.code == ErrorCode::Busy => {
                    toasts.show(Level::Info, i18n.t("installation_already_running"), None)
                }
                Err(_) => {}
            }
            let _ = pending.try_set(None);
        });
    };
    view! {
        <Dialog open=open title=title icon="download" wide=true>
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| open.set(false)>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary icon="download" on_click=move |_| install()>
                    {move || i18n.t("minecraft_components_install_action")}
                </Button>
            </DialogFooter>
            <p style="margin:0 0 12px">{move || i18n.t("tensacraft_install_confirm_message")}</p>
            <div style="padding:12px 14px;border:1px solid var(--line);border-radius:var(--r-sm);background:var(--s-row)">
                <div style="font-weight:600;margin-bottom:6px">{move || i18n.t("tensacraft_description_title")}</div>
                <p class="hint" style="white-space:pre-line;margin:0;max-height:240px;overflow:auto;user-select:text">
                    {move || {
                        asked
                            .with(|p| p.as_ref().and_then(description_of))
                            .unwrap_or_else(|| i18n.t("tensacraft_description_unavailable"))
                    }}
                </p>
            </div>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_counts_the_server_builds_cards() {
        assert_eq!(cards_shown(false, 0, false), 1, "Home is not called empty before the server answers");
        assert_eq!(cards_shown(true, 3, false), 3);
        assert_eq!(cards_shown(true, 0, false), 0, "nothing to offer");
        assert_eq!(cards_shown(true, 0, true), 1, "the card saying the server does not answer");
    }

    #[test]
    fn an_installed_pack_leaves_the_cards_at_once() {
        let pack = |id: &str| HomePack {
            id: id.into(),
            name: id.into(),
            description: None,
            image: None,
            runs: String::new(),
        };
        let mut packs = vec![pack("aero"), pack("tensa-lite")];
        drop_pack(&mut packs, "aero");
        assert_eq!(packs, [pack("tensa-lite")]);
    }

    #[test]
    fn a_pack_reads_the_module_s_answer() {
        let pack: HomePack = serde_json::from_value(json!({
            "id": "aero", "name": "Aero", "description": null, "image": "a.png", "runs": "Fabric 26.3"
        }))
        .unwrap();
        assert_eq!(pack.runs, "Fabric 26.3");
        assert_eq!(description_of(&pack), None);
        let described = HomePack { description: Some("  Planes\n".into()), ..pack };
        assert_eq!(description_of(&described).as_deref(), Some("Planes"));
    }
}
