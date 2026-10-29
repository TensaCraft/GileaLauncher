//! The skins module's UI: the «Скіни» page.

mod api;
mod capes;
mod library;
mod model;
mod page;
mod viewer;

use leptos::prelude::*;
use ui_kit::module::{ModulePage, UiModule};

pub struct SkinsUi;

impl UiModule for SkinsUi {
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
            path: "/skins",
            icon: "checkroom",
            label: "skins_nav",
            view: || view! { <page::SkinsPage /> }.into_any(),
        }]
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(SkinsUi)
}
