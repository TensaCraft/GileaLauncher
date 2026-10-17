//! One-time hints: which may show now. One shows at a time — the first profile's before the rest —
//! and each of the others until the player follows it or closes it (`SettingUpdate::HintLearned`).
//! Pages mark the control a hint points at (`NudgeMark`); the shell's `NudgeLayer` draws it.

use launcher_shared::{BuildDto, ContentKind, SettingUpdate, mods_supported};
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{NudgeMark, NudgeSide};

use crate::shell::sidebar::{calls_for_profile, is_active};
use crate::store::{AppStore, SettingsWriter, use_settings_writer, use_store};

/// No profile yet: the profile button calls for one.
pub const PROFILE: &str = "profile";
/// Where a build's content is (a Builds row's «Вміст збірки», a Home card).
pub const BUILD_CONTENT: &str = "build_content";
/// Where new content is found (the content page's sources).
pub const CONTENT_SOURCES: &str = "content_sources";

/// Whether hint `id` may show: once the launcher is ready, the profile hint while a profile is
/// called for; any other while hints are on, it is not learned and the profile hint is not showing.
pub fn may_show(id: &str, ready: bool, enabled: bool, done: &[String], profile_wanted: bool) -> bool {
    if !ready {
        return false;
    }
    if id == PROFILE {
        return profile_wanted;
    }
    enabled && !profile_wanted && !done.iter().any(|d| d == id)
}

/// The app's hints. Made by `provide_hints` in the shell (inside the router).
#[derive(Clone, Copy)]
pub struct Hints {
    store: AppStore,
    writer: SettingsWriter,
    i18n: I18nCtx,
    profile_wanted: Memo<bool>,
}

impl Hints {
    /// Hint `id` may show now (tracked).
    pub fn may_show(&self, id: &'static str) -> bool {
        let (ready, wanted) = (self.store.ready.get(), self.profile_wanted.get());
        self.store.settings.with(|s| may_show(id, ready, s.hints, &s.hints_done, wanted))
    }

    /// Some hint may show now (tracked): the hint layer looks for a marked control meanwhile.
    pub fn any(&self) -> bool {
        [PROFILE, BUILD_CONTENT, CONTENT_SOURCES].into_iter().any(|id| self.may_show(id))
    }

    /// The player followed or closed hint `id`: it shows no more (nothing is written twice).
    pub fn learn(&self, id: &'static str) {
        if id == PROFILE || self.store.settings.with_untracked(|s| s.hints_done.iter().any(|d| d == id)) {
            return;
        }
        self.writer.apply(SettingUpdate::HintLearned(id.to_string()));
    }

    /// The mark a page puts on the control hint `id` points at; every hint but the profile one has
    /// a ✕.
    pub fn mark(&self, id: &'static str, text: String, side: NudgeSide) -> NudgeMark {
        let close = (id != PROFILE).then(|| self.i18n.t("hint_close"));
        NudgeMark { id: id.to_string(), text, side, close }
    }
}

/// The build the content hint points at: the one just made while it is listed, else the first.
pub fn build_hint_target(builds: &[BuildDto], fresh: Option<&str>) -> Option<String> {
    fresh
        .filter(|key| builds.iter().any(|b| b.key == *key))
        .map(str::to_string)
        .or_else(|| builds.first().map(|b| b.key.clone()))
}

/// What the content hint says on a build — what to click, on a Builds row's «Вміст збірки» or a Home
/// card — and before it «Збірку готово!» on a build just made; a vanilla build hears nothing of mods.
pub fn build_hint_keys(card: bool, fresh: bool, vanilla: bool) -> (Option<&'static str>, &'static str) {
    let what = match (card, vanilla) {
        (false, false) => "hint_build_content",
        (false, true) => "hint_build_content_vanilla",
        (true, false) => "hint_build_card",
        (true, true) => "hint_build_card_vanilla",
    };
    (fresh.then_some("hint_build_ready"), what)
}

/// The content hint's mark on build `build` — a Home card's (`card`) or a Builds row's «Вміст
/// збірки» — while it may show there.
pub fn build_mark(hints: Hints, build: &BuildDto, card: bool, side: NudgeSide) -> Option<NudgeMark> {
    if !hints.may_show(BUILD_CONTENT) {
        return None;
    }
    let (target, fresh) = hints.store.builds.with(|list| {
        let fresh = hints.store.fresh_build.get();
        (build_hint_target(list, fresh.as_deref()), fresh)
    });
    (target.as_deref() == Some(build.key.as_str())).then(|| {
        let fresh = fresh.as_deref() == Some(build.key.as_str());
        let (ready, what) = build_hint_keys(card, fresh, !mods_supported(build.client.as_deref()));
        let text = match ready {
            // On a line of its own: the bubble keeps the texts' line breaks.
            Some(ready) => format!("{}\n{}", hints.i18n.t(ready), hints.i18n.t(what)),
            None => hints.i18n.t(what),
        };
        hints.mark(BUILD_CONTENT, text, side)
    })
}

/// Row `key` is brought into sight: it is the build just made and carries the content hint (a new
/// build sorts last, often below the fold, where the hint would not show).
pub fn reveals_fresh(fresh: Option<&str>, key: &str, hinted: bool) -> bool {
    hinted && fresh == Some(key)
}

/// What the sources hint says on a tab of `kind`.
pub fn sources_hint_key(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mods => "hint_sources_mods",
        ContentKind::ResourcePacks => "hint_sources_resourcepacks",
        ContentKind::ShaderPacks => "hint_sources_shaders",
    }
}

pub fn provide_hints() -> Hints {
    let store = use_store();
    let location = use_location();
    let profile_wanted = Memo::new(move |_| {
        let path = location.pathname.get();
        path != "/setup"
            && calls_for_profile(
                store.profiles_loaded.get(),
                store.profiles.with(Vec::len),
                is_active(&path, "/profiles"),
            )
    });
    let hints = Hints { store, writer: use_settings_writer(), i18n: use_i18n(), profile_wanted };
    provide_context(hints);
    hints
}

pub fn use_hints() -> Hints {
    expect_context::<Hints>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_hint_shuts_out_the_rest() {
        let none: Vec<String> = Vec::new();
        assert!(may_show(PROFILE, true, true, &none, true));
        assert!(!may_show(BUILD_CONTENT, true, true, &none, true));
        assert!(may_show(BUILD_CONTENT, true, true, &none, false));
        assert!(!may_show(PROFILE, true, true, &none, false), "no call for a profile, no profile hint");
    }

    #[test]
    fn learned_hints_and_switched_off_hints_stay_away_but_the_profile_one_does_not() {
        let done = vec![BUILD_CONTENT.to_string()];
        assert!(!may_show(BUILD_CONTENT, true, true, &done, false));
        assert!(may_show(CONTENT_SOURCES, true, true, &done, false));
        assert!(!may_show(CONTENT_SOURCES, true, false, &[], false), "hints off");
        assert!(may_show(PROFILE, true, false, &[], true), "the profile hint ignores the switch");
    }

    fn build(key: &str, client: &str) -> BuildDto {
        BuildDto {
            key: key.into(),
            version_id: key.into(),
            name: key.into(),
            version: None,
            loader: None,
            client: Some(client.into()),
            loader_version: None,
            game_dir: String::new(),
            image: None,
            description: String::new(),
            running: false,
            profile: None,
        }
    }

    #[test]
    fn h1_points_at_the_fresh_build_while_it_is_listed_else_the_first() {
        let list = vec![build("aero", "Fabric"), build("zeta", "Minecraft")];
        assert_eq!(build_hint_target(&list, Some("zeta")).as_deref(), Some("zeta"));
        assert_eq!(build_hint_target(&list, Some("gone")).as_deref(), Some("aero"));
        assert_eq!(build_hint_target(&list, None).as_deref(), Some("aero"));
        assert_eq!(build_hint_target(&[], None), None);
    }

    #[test]
    fn h1_tells_what_to_click_and_says_nothing_about_mods_to_a_vanilla_build() {
        assert_eq!(build_hint_keys(false, false, false), (None, "hint_build_content"));
        assert_eq!(build_hint_keys(false, false, true), (None, "hint_build_content_vanilla"));
        assert_eq!(build_hint_keys(true, false, false), (None, "hint_build_card"));
        assert_eq!(build_hint_keys(true, false, true), (None, "hint_build_card_vanilla"));
        assert_eq!(build_hint_keys(false, true, false), (Some("hint_build_ready"), "hint_build_content"));
        assert_eq!(build_hint_keys(true, true, true), (Some("hint_build_ready"), "hint_build_card_vanilla"));
    }

    #[test]
    fn the_list_brings_a_fresh_build_into_sight_only_while_it_carries_the_hint() {
        assert!(reveals_fresh(Some("new"), "new", true));
        assert!(!reveals_fresh(Some("new"), "new", false), "the hint learned or switched off");
        assert!(!reveals_fresh(Some("new"), "old", true), "another row");
        assert!(!reveals_fresh(None, "old", true), "no build made in this run");
    }

    #[test]
    fn h2_names_the_open_tab_s_kind() {
        assert_eq!(sources_hint_key(ContentKind::Mods), "hint_sources_mods");
        assert_eq!(sources_hint_key(ContentKind::ResourcePacks), "hint_sources_resourcepacks");
        assert_eq!(sources_hint_key(ContentKind::ShaderPacks), "hint_sources_shaders");
    }

    #[test]
    fn nothing_shows_before_the_launcher_is_ready() {
        assert!(!may_show(PROFILE, false, true, &[], true));
        assert!(!may_show(BUILD_CONTENT, false, true, &[], false));
    }
}
