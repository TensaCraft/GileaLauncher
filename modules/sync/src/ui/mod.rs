use ui_kit::module::UiModule;

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
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new(SyncUi)
}
