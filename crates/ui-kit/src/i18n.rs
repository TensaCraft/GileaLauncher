//! Translations: core dictionary + module dictionaries, per-key fallback to English.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use launcher_shared::{AppError, Text};
use leptos::prelude::*;

use crate::module::UiModule;

#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    map: HashMap<String, String>,
}

impl Dictionary {
    /// Merges a flat JSON object; later values override earlier ones. Non-string values are ignored.
    pub fn merge_json(&mut self, raw: &str) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        let obj = value.as_object().ok_or("translation file must be a JSON object")?;
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                self.map.insert(k.clone(), s.to_string());
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.map.keys()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn is_param_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replaces `{name}` placeholders in a single pass; unknown placeholders stay as-is.
pub fn interpolate(template: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if is_param_name(&after[..end]) && params.contains_key(&after[..end]) => {
                out.push_str(&params[&after[..end]]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone)]
pub struct I18n {
    lang: String,
    primary: Dictionary,
    fallback: Dictionary,
}

impl I18n {
    pub fn new(lang: impl Into<String>, primary: Dictionary, fallback: Dictionary) -> Self {
        Self { lang: lang.into(), primary, fallback }
    }

    pub fn lang(&self) -> &str {
        &self.lang
    }

    fn raw(&self, key: &str) -> String {
        self.primary.get(key).or_else(|| self.fallback.get(key)).unwrap_or(key).to_string()
    }

    pub fn t(&self, key: &str) -> String {
        self.raw(key)
    }

    pub fn tp(&self, key: &str, params: &[(&str, String)]) -> String {
        let map: BTreeMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        interpolate(&self.raw(key), &map)
    }

    pub fn text(&self, text: &Text) -> String {
        match text {
            Text::Key { key, params } => interpolate(&self.raw(key), params),
            Text::Raw { text } => text.clone(),
        }
    }

    /// The translated error; a missing `{error}` parameter shows the error's detail.
    pub fn error(&self, err: &AppError) -> String {
        let mut params = err.params.clone();
        params.entry("error".to_string()).or_insert_with(|| err.detail.clone());
        interpolate(&self.raw(err.code.i18n_key()), &params)
    }
}

pub fn build_i18n(
    lang: &str,
    core: impl Fn(&str) -> Option<&'static str>,
    modules: &[Box<dyn UiModule>],
) -> I18n {
    let load = |l: &str| {
        let mut d = Dictionary::default();
        if let Some(raw) = core(l) {
            let _ = d.merge_json(raw);
        }
        for m in modules {
            if let Some(raw) = m.locale_json(l) {
                let _ = d.merge_json(raw);
            }
        }
        d
    };
    let fallback = if lang == "en_US" { Dictionary::default() } else { load("en_US") };
    I18n::new(lang, load(lang), fallback)
}

/// Reactive translation handle provided through context.
#[derive(Clone, Copy)]
pub struct I18nCtx {
    current: RwSignal<Arc<I18n>>,
}

impl I18nCtx {
    pub fn set(&self, i18n: I18n) {
        self.current.set(Arc::new(i18n));
    }

    pub fn lang(&self) -> String {
        self.current.with(|i| i.lang().to_string())
    }

    pub fn t(&self, key: &str) -> String {
        self.current.with(|i| i.t(key))
    }

    pub fn tp(&self, key: &str, params: &[(&str, String)]) -> String {
        self.current.with(|i| i.tp(key, params))
    }

    pub fn text(&self, text: &Text) -> String {
        self.current.with(|i| i.text(text))
    }

    pub fn error(&self, err: &AppError) -> String {
        self.current.with(|i| i.error(err))
    }
}

pub fn provide_i18n(i18n: I18n) -> I18nCtx {
    let ctx = I18nCtx { current: RwSignal::new(Arc::new(i18n)) };
    provide_context(ctx);
    ctx
}

pub fn use_i18n() -> I18nCtx {
    expect_context::<I18nCtx>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_shared::ErrorCode;

    fn dict(raw: &str) -> Dictionary {
        let mut d = Dictionary::default();
        d.merge_json(raw).unwrap();
        d
    }

    #[test]
    fn falls_back_to_english_then_key() {
        let i = I18n::new("uk_UA", dict(r#"{"a":"Привіт"}"#), dict(r#"{"a":"Hi","b":"Only en"}"#));
        assert_eq!(i.t("a"), "Привіт");
        assert_eq!(i.t("b"), "Only en");
        assert_eq!(i.t("missing_key"), "missing_key");
    }

    #[test]
    fn interpolation_keeps_unknown_and_does_not_reexpand() {
        let mut p = BTreeMap::new();
        p.insert("version".to_string(), "{name}".to_string());
        p.insert("name".to_string(), "X".to_string());
        assert_eq!(interpolate("Build {version} by {name}, {missing}", &p), "Build {name} by X, {missing}");
        assert_eq!(interpolate("unclosed {brace", &p), "unclosed {brace");
    }

    #[test]
    fn text_and_error_are_translated() {
        let i = I18n::new(
            "en_US",
            dict(
                r#"{"version_starting":"{version} is starting","invalid_directory_path":"Invalid directory path"}"#,
            ),
            Dictionary::default(),
        );
        assert_eq!(i.text(&Text::key("version_starting").param("version", "1.21")), "1.21 is starting");
        assert_eq!(i.text(&Text::raw("file.jar")), "file.jar");
        let e = AppError::new(ErrorCode::InvalidDirectoryPath, "x");
        assert_eq!(i.error(&e), "Invalid directory path");
    }

    #[test]
    fn errors_without_an_error_param_show_their_detail() {
        let i = I18n::new(
            "en_US",
            dict(r#"{"download_error":"Error downloading file: {error}"}"#),
            Dictionary::default(),
        );
        let bare = AppError::new(ErrorCode::DownloadFailed, "timed out");
        assert_eq!(i.error(&bare), "Error downloading file: timed out");
        let given = AppError::new(ErrorCode::DownloadFailed, "x").with_param("error", "HTTP 404");
        assert_eq!(i.error(&given), "Error downloading file: HTTP 404");
    }

    #[test]
    fn modules_extend_and_override_core_dictionary() {
        struct M;
        impl UiModule for M {
            fn id(&self) -> &'static str {
                "m"
            }
            fn locale_json(&self, lang: &str) -> Option<&'static str> {
                (lang == "uk_UA").then_some(r#"{"module_m_name":"Модуль","a":"Перекрито"}"#)
            }
        }
        let core = |lang: &str| match lang {
            "uk_UA" => Some(r#"{"a":"Ядро"}"#),
            "en_US" => Some(r#"{"a":"Core","z":"Zed"}"#),
            _ => None,
        };
        let modules: Vec<Box<dyn UiModule>> = vec![Box::new(M)];
        let i = build_i18n("uk_UA", core, &modules);
        assert_eq!(i.t("a"), "Перекрито");
        assert_eq!(i.t("module_m_name"), "Модуль");
        assert_eq!(i.t("z"), "Zed");
    }

    #[test]
    fn shipped_locales_have_identical_keys_and_no_old_brand() {
        let uk = dict(include_str!("../../../assets/langs/uk_UA.json"));
        let en = dict(include_str!("../../../assets/langs/en_US.json"));
        let mut uk_keys: Vec<_> = uk.keys().collect();
        let mut en_keys: Vec<_> = en.keys().collect();
        uk_keys.sort();
        en_keys.sort();
        assert_eq!(uk_keys, en_keys);
        assert!(uk.len() > 600);
        assert!(!include_str!("../../../assets/langs/uk_UA.json").contains("TensaLauncher"));
    }

    #[test]
    fn game_install_texts_are_translated() {
        let uk = dict(include_str!("../../../assets/langs/uk_UA.json"));
        let en = dict(include_str!("../../../assets/langs/en_US.json"));
        for key in [
            "installing_java_runtime",
            "installing_minecraft_version",
            "repairing_minecraft_version",
            "installation_complete",
            "version_install_success",
            "version_install_error",
        ] {
            for d in [&uk, &en] {
                assert!(d.get(key).is_some(), "missing {key}");
            }
        }
        assert!(uk.get("installing_java_runtime").is_some_and(|t| t.contains("{component}")));
        assert!(en.get("java_runtime_install_failed").is_some_and(|t| t.contains("{error}")));
    }

    #[test]
    fn every_error_code_and_update_key_is_translated() {
        let uk = dict(include_str!("../../../assets/langs/uk_UA.json"));
        let en = dict(include_str!("../../../assets/langs/en_US.json"));
        let update_keys = [
            "update_dev_mode_disabled",
            "update_applied_toast",
            "update_source_test_server",
            "update_channel_stable",
            "update_channel_beta",
            "update_restart_now",
            "update_restart_later",
            "update_downloading_version",
            "update_notes_empty",
            "update_notes_title",
            "update_install",
            "launcher_update_status_checked_at",
            "launcher_update_status_ready",
            "launcher_update_status_downloading",
            "version_stop",
            "version_running_badge",
            "version_copy_default_name",
            "version_delete_confirm_title",
            "version_create_error",
            "version_create_retry",
            "version_create_release_date",
            "default_max_ram_auto",
            "gpu_mode_default_label",
            "window_size_label",
            "window_size_desc",
            "window_size_fullscreen",
            "window_size_maximized",
            "window_size_custom",
            "window_size_width",
            "window_size_height",
            "window_size_apply",
            "window_size_invalid",
        ];
        for d in [&uk, &en] {
            for code in launcher_shared::ErrorCode::ALL {
                assert!(d.get(code.i18n_key()).is_some(), "missing {}", code.i18n_key());
            }
            for key in update_keys {
                assert!(d.get(key).is_some(), "missing {key}");
            }
        }
        let account_keys = [
            "xbox_account_missing",
            "xbox_child_account",
            "xbox_unavailable",
            "minecraft_not_owned",
            "credential_encryption_unavailable",
            "profile_name_invalid",
            "profile_exists",
            "microsoft_auth_device_code_title",
            "launch_profile_select_message",
            "profile_required_message",
        ];
        for d in [&uk, &en] {
            for key in account_keys {
                assert!(d.get(key).is_some(), "missing {key}");
            }
        }
        assert!(uk.get("profile_exists").unwrap().contains("{name}"));
        for d in [&uk, &en] {
            let text = d.get("not_enough_space").expect("not_enough_space");
            for param in ["{path}", "{required}", "{available}"] {
                assert!(text.contains(param), "{param} in {text}");
            }
        }
        assert!(uk.get("update_ready_message").unwrap().contains("{version}"));
    }
}
