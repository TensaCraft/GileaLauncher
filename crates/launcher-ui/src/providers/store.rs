//! What each provider last said of a build tab's installed files (`overview`), shared by the
//! Installed list's toolbar, its rows and the provider's own tab; and the update a row asked for,
//! which that provider's toolbar confirms and runs. Arc signals in a thread local, so no panel's
//! disposal takes them along.

use std::collections::{HashMap, HashSet};

use launcher_shared::ContentKind;
use launcher_shared::provider::{FileNote, Heard, Overview, settled_owner};
use leptos::prelude::*;

/// `(provider, build key, kind)`.
type Tab = (String, String, ContentKind);

/// An update a row asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAsk {
    pub provider: String,
    pub key: String,
    pub kind: ContentKind,
    pub note: FileNote,
    /// The row's name, for the confirmation.
    pub name: String,
}

#[derive(Clone)]
pub struct Store {
    overviews: ArcRwSignal<HashMap<Tab, Overview>>,
    checking: ArcRwSignal<HashSet<Tab>>,
    /// Tabs asked for again while their look ran: one more look follows it.
    again: ArcRwSignal<HashSet<Tab>>,
    asked: ArcRwSignal<Option<UpdateAsk>>,
}

pub fn store() -> Store {
    thread_local! {
        static STORE: Store = Store {
            overviews: ArcRwSignal::new(HashMap::new()),
            checking: ArcRwSignal::new(HashSet::new()),
            again: ArcRwSignal::new(HashSet::new()),
            asked: ArcRwSignal::new(None),
        };
    }
    STORE.with(Store::clone)
}

fn tab(provider: &str, key: &str, kind: ContentKind) -> Tab {
    (provider.to_string(), key.to_string(), kind)
}

impl Store {
    pub fn set_overview(&self, provider: &str, key: &str, kind: ContentKind, overview: Overview) {
        self.overviews.update(|m| {
            m.insert(tab(provider, key, kind), overview);
        });
    }

    pub fn overview(&self, provider: &str, key: &str, kind: ContentKind) -> Option<Overview> {
        self.overviews.with(|m| m.get(&tab(provider, key, kind)).cloned())
    }

    /// The provider's note of the row whose file is `file`.
    pub fn note(&self, provider: &str, key: &str, kind: ContentKind, file: &str) -> Option<FileNote> {
        self.overviews.with(|m| {
            m.get(&tab(provider, key, kind)).and_then(|o| o.notes.iter().find(|n| n.file == file).cloned())
        })
    }

    /// The provider a row of `file` is shown with (`order`: the providers in the app's order): the
    /// one that installed it, else the first that knows it — once no provider still to answer can
    /// change that (`settled_owner`); a provider with no overview of the tab yet has not answered.
    pub fn owner(&self, order: &[String], key: &str, kind: ContentKind, file: &str) -> Option<String> {
        self.overviews.with(|m| {
            let heard: Vec<(&str, Heard<'_>)> = order
                .iter()
                .map(|p| {
                    let said = match m.get(&tab(p, key, kind)) {
                        None => Heard::Waiting,
                        Some(o) => {
                            o.notes.iter().find(|n| n.file == file).map_or(Heard::Unknown, Heard::Knows)
                        }
                    };
                    (p.as_str(), said)
                })
                .collect();
            settled_owner(&heard).map(str::to_string)
        })
    }

    /// How many of `provider`'s rows have a newer version: rows shown with another provider are
    /// that one's.
    pub fn owned_updates(&self, order: &[String], provider: &str, key: &str, kind: ContentKind) -> usize {
        let files: Vec<String> = self.overviews.with(|m| {
            m.get(&tab(provider, key, kind))
                .map(|o| o.notes.iter().filter(|n| n.update.is_some()).map(|n| n.file.clone()).collect())
                .unwrap_or_default()
        });
        files.iter().filter(|file| self.owner(order, key, kind, file).as_deref() == Some(provider)).count()
    }

    /// The note of project `project_id` when it has a newer version.
    pub fn update_of(
        &self,
        provider: &str,
        key: &str,
        kind: ContentKind,
        project_id: &str,
    ) -> Option<FileNote> {
        self.overviews.with(|m| {
            m.get(&tab(provider, key, kind)).and_then(|o| {
                o.notes.iter().find(|n| n.project_id == project_id && n.update.is_some()).cloned()
            })
        })
    }

    pub fn set_checking(&self, provider: &str, key: &str, kind: ContentKind, on: bool) {
        self.checking.update(|s| {
            if on {
                s.insert(tab(provider, key, kind));
            } else {
                s.remove(&tab(provider, key, kind));
            }
        });
    }

    pub fn checking(&self, provider: &str, key: &str, kind: ContentKind) -> bool {
        self.checking.with(|s| s.contains(&tab(provider, key, kind)))
    }

    /// Asks for one more look at the tab after the one running.
    pub fn look_again(&self, provider: &str, key: &str, kind: ContentKind) {
        self.again.update_untracked(|s| {
            s.insert(tab(provider, key, kind));
        });
    }

    /// Whether another look was asked for meanwhile (once).
    pub fn take_again(&self, provider: &str, key: &str, kind: ContentKind) -> bool {
        self.again.try_update_untracked(|s| s.remove(&tab(provider, key, kind))).unwrap_or(false)
    }

    pub fn ask(&self, ask: UpdateAsk) {
        self.asked.set(Some(ask));
    }

    /// Tracks the asks; takes the one for this provider's tab.
    pub fn take_ask(&self, provider: &str, key: &str, kind: ContentKind) -> Option<UpdateAsk> {
        let mine = self
            .asked
            .with(|a| a.as_ref().is_some_and(|a| a.provider == provider && a.key == key && a.kind == kind));
        if !mine {
            return None;
        }
        let taken = self.asked.get_untracked();
        self.asked.set(None);
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::provider::NewerVersion;

    fn note(file: &str, project: &str, update: Option<&str>) -> FileNote {
        FileNote {
            file: file.into(),
            project_id: project.into(),
            slug: String::new(),
            title: String::new(),
            version_number: "1".into(),
            update: update.map(|v| NewerVersion { version_id: v.into(), version_number: v.into() }),
            url: None,
            icon_url: None,
            installed: false,
        }
    }

    #[test]
    fn a_row_has_no_provider_until_the_ones_before_answered() {
        let owner = Owner::new();
        owner.with(|| {
            let s = store();
            let order = ["modrinth".to_string(), "curseforge".to_string()];
            let notes = Overview { notes: vec![note("late.jar", "L", None)], updates: None };
            s.set_overview("curseforge", "late", ContentKind::Mods, notes.clone());
            assert_eq!(
                s.owner(&order, "late", ContentKind::Mods, "late.jar"),
                None,
                "Modrinth has not answered"
            );
            s.set_overview("modrinth", "late", ContentKind::Mods, Overview::default());
            assert_eq!(
                s.owner(&order, "late", ContentKind::Mods, "late.jar").as_deref(),
                Some("curseforge"),
                "Modrinth does not know it"
            );
        });
        owner.cleanup();
    }

    #[test]
    fn a_file_two_providers_know_is_one_provider_s() {
        let owner = Owner::new();
        owner.with(|| {
            let s = store();
            let order = ["modrinth".to_string(), "curseforge".to_string()];
            let mut pack_file = note("pack.jar", "P", Some("p2"));
            pack_file.installed = true;
            s.set_overview(
                "modrinth",
                "zeta",
                ContentKind::Mods,
                Overview {
                    notes: vec![note("both.jar", "B", Some("b2")), note("pack.jar", "P", Some("p3"))],
                    updates: None,
                },
            );
            s.set_overview(
                "curseforge",
                "zeta",
                ContentKind::Mods,
                Overview {
                    notes: vec![note("both.jar", "7", Some("8")), pack_file, note("cf.jar", "9", Some("10"))],
                    updates: None,
                },
            );
            let of = |file: &str| s.owner(&order, "zeta", ContentKind::Mods, file);
            assert_eq!(of("both.jar").as_deref(), Some("modrinth"), "Modrinth first");
            assert_eq!(of("pack.jar").as_deref(), Some("curseforge"), "the provider that installed it");
            assert_eq!(of("cf.jar").as_deref(), Some("curseforge"), "only CurseForge knows it");
            assert_eq!(of("none.jar"), None);
            let waiting = |provider: &str| s.owned_updates(&order, provider, "zeta", ContentKind::Mods);
            assert_eq!((waiting("modrinth"), waiting("curseforge")), (1, 2), "each counts its own rows only");
        });
        owner.cleanup();
    }

    fn ask(provider: &str) -> UpdateAsk {
        UpdateAsk {
            provider: provider.into(),
            key: "aero".into(),
            kind: ContentKind::Mods,
            note: note("a.jar", "A", Some("a2")),
            name: "A".into(),
        }
    }

    #[test]
    fn a_build_tab_s_overview_names_rows_and_projects() {
        let owner = Owner::new();
        owner.with(|| {
            let s = store();
            let overview = Overview {
                notes: vec![note("a.jar", "A", Some("a2")), note("b.jar.disabled", "B", None)],
                updates: None,
            };
            s.set_overview("modrinth", "aero", ContentKind::Mods, overview);
            assert_eq!(s.note("modrinth", "aero", ContentKind::Mods, "a.jar").unwrap().project_id, "A");
            assert!(s.note("modrinth", "aero", ContentKind::ResourcePacks, "a.jar").is_none(), "another tab");
            assert!(
                s.note("curseforge", "aero", ContentKind::Mods, "a.jar").is_none(),
                "another provider's rows"
            );
            assert!(s.update_of("modrinth", "aero", ContentKind::Mods, "A").is_some());
            assert!(s.update_of("modrinth", "aero", ContentKind::Mods, "B").is_none());
            s.ask(ask("modrinth"));
            assert!(
                s.take_ask("modrinth", "zeta", ContentKind::Mods).is_none(),
                "another build's toolbar leaves it"
            );
            assert!(
                s.take_ask("curseforge", "aero", ContentKind::Mods).is_none(),
                "another provider's toolbar too"
            );
            assert!(s.take_ask("modrinth", "aero", ContentKind::Mods).is_some());
            assert!(s.take_ask("modrinth", "aero", ContentKind::Mods).is_none(), "taken once");
            s.set_checking("modrinth", "aero", ContentKind::Mods, true);
            assert!(s.checking("modrinth", "aero", ContentKind::Mods));
            assert!(!s.take_again("modrinth", "aero", ContentKind::Mods), "nothing asked");
            s.look_again("modrinth", "aero", ContentKind::Mods);
            s.look_again("modrinth", "aero", ContentKind::Mods);
            assert!(s.take_again("modrinth", "aero", ContentKind::Mods), "asked twice, one more look");
            assert!(!s.take_again("modrinth", "aero", ContentKind::Mods));
            assert!(!s.checking("curseforge", "aero", ContentKind::Mods));
        });
        owner.cleanup();
    }
}
