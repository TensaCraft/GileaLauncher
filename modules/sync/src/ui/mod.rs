//! The sync module's UI: the «Синхронізація» page, a build's tab, and the mark on cards of
//! builds in a group.

mod api;
mod badge;
mod card;
mod editor;
mod group;
mod model;
mod page;
mod state;
mod tab;

use leptos::prelude::*;
use ui_kit::TagTone;
use ui_kit::module::{ModuleBuildBadge, ModulePage, ModuleTab, UiModule};

pub struct SyncUi;

impl UiModule for SyncUi {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        match lang {
            "uk_UA" => Some(include_str!("../../locales/uk_UA.json")),
            "en_US" => Some(include_str!("../../locales/en_US.json")),
            _ => None,
        }
    }

    fn pages(&self) -> Vec<ModulePage> {
        vec![ModulePage {
            module: crate::ID,
            path: "/sync",
            icon: "sync",
            label: "sync_nav",
            view: || view! { <page::SyncPage /> }.into_any(),
        }]
    }

    fn content_tabs(&self) -> Vec<ModuleTab> {
        vec![ModuleTab {
            module: crate::ID,
            id: "sync",
            icon: "sync",
            label: "sync_content_tab",
            tone: TagTone::Info,
            after: "settings",
            view: |key| view! { <tab::SyncTab key=key /> }.into_any(),
        }]
    }

    fn build_badges(&self) -> Vec<ModuleBuildBadge> {
        vec![ModuleBuildBadge {
            module: crate::ID,
            view: |key| view! { <badge::SyncBadge key=key /> }.into_any(),
        }]
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(SyncUi)
}
