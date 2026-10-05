//! The GPU switch (`auto`, `igpu`, `dgpu`) of Settings → Java and of a build's settings. On Windows
//! a GPU of one's own is kept in the registry: before the first such choice the user is told what
//! the launcher writes there and why, and nothing changes until they agree.

use leptos::prelude::*;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, DialogTone, SegOption, Segmented, Variant};

use crate::store::use_store;

/// Whether `mode` on system `os` has the launcher write to the registry (a GPU the user chose, on
/// Windows; `auto` leaves it to Windows).
pub fn writes_registry(os: &str, mode: &str) -> bool {
    os == "windows" && mode != "auto"
}

/// Whether going from `before` to `after` needs the user's word first: the first registry choice.
pub fn asks_first(os: &str, before: &str, after: &str) -> bool {
    writes_registry(os, after) && !writes_registry(os, before)
}

#[component]
pub fn GpuChoice(
    value: RwSignal<String>,
    /// Called with the mode once it is taken (the user agreed where that was asked).
    #[prop(optional, into)]
    on_change: Option<Callback<String>>,
) -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let t = move |key: &'static str| Signal::derive(move || i18n.t(key));
    let os = Memo::new(move |_| store.info.with(|i| i.as_ref().map(|i| i.os.clone()).unwrap_or_default()));
    // What the switch shows: the value, except while a choice waits for the user's word.
    let shown = RwSignal::new(value.get_untracked());
    Effect::new(move |_| shown.set(value.get()));
    let pending = RwSignal::new(None::<String>);
    let open = RwSignal::new(false);
    let take = move |mode: String| {
        value.set(mode.clone());
        if let Some(cb) = on_change {
            cb.run(mode);
        }
    };
    let picked = Callback::new(move |mode: String| {
        let before = value.get_untracked();
        if asks_first(&os.get_untracked(), &before, &mode) {
            shown.set(before);
            pending.set(Some(mode));
            open.set(true);
        } else {
            take(mode);
        }
    });
    let agree = move |_| {
        open.set(false);
        if let Some(mode) = pending.get_untracked() {
            pending.set(None);
            take(mode);
        }
    };
    let cancel = Callback::new(move |_| {
        open.set(false);
        pending.set(None);
    });
    let options = Signal::derive(move || {
        vec![
            SegOption::new("auto", i18n.t("gpu_mode_auto")),
            SegOption::new("igpu", i18n.t("gpu_mode_integrated")),
            SegOption::new("dgpu", i18n.t("gpu_mode_discrete")),
        ]
    });
    let points =
        ["gpu_registry_where", "gpu_registry_only_java", "gpu_registry_antivirus", "gpu_registry_undo"];
    view! {
        <Segmented options=options value=shown on_change=picked />
        <Dialog
            open=open
            title=t("gpu_registry_title")
            icon="developer_board"
            tone=DialogTone::Warning
            on_close=cancel
        >
            <DialogFooter slot>
                <Button variant=Variant::Ghost on_click=move |_| cancel.run(())>{move || i18n.t("cancel")}</Button>
                <Button variant=Variant::Primary icon="check" on_click=agree>
                    {move || i18n.t("gpu_registry_agree")}
                </Button>
            </DialogFooter>
            <p class="gpu-registry__lead">{move || i18n.t("gpu_registry_lead")}</p>
            <ul class="gpu-registry__points">
                {points.map(|key| view! { <li>{move || i18n.t(key)}</li> }).collect_view()}
            </ul>
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_gpu_of_ones_own_on_windows_touches_the_registry() {
        assert!(writes_registry("windows", "dgpu") && writes_registry("windows", "igpu"));
        assert!(!writes_registry("windows", "auto"));
        assert!(!writes_registry("linux", "dgpu") && !writes_registry("macos", "igpu"));
    }

    #[test]
    fn the_user_is_asked_only_before_the_first_registry_choice() {
        assert!(asks_first("windows", "auto", "dgpu"));
        assert!(!asks_first("windows", "igpu", "dgpu"), "already in the registry");
        assert!(!asks_first("windows", "dgpu", "auto"), "back to auto takes it out: nothing to ask");
        assert!(!asks_first("linux", "auto", "dgpu"));
    }
}
