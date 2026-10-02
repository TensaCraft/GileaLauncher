//! Template of a new optional module (same layout as the built-in module skeletons).

use anyhow::{Result, bail};

pub fn validate_id(id: &str) -> Result<()> {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => bail!("module id must start with a lowercase letter"),
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        bail!("module id may contain only lowercase letters and digits");
    }
    Ok(())
}

fn struct_name(id: &str) -> String {
    let mut chars = id.chars();
    let first = chars.next().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
    format!("{first}{}", chars.collect::<String>())
}

const CARGO: &str = r#"[package]
name = "module-{{id}}"
version.workspace = true
edition.workspace = true
license.workspace = true

[features]
backend = ["dep:launcher-core"]
ui = ["dep:ui-kit"]

[dependencies]
launcher-shared.workspace = true
serde_json.workspace = true
launcher-core = { workspace = true, optional = true }
ui-kit = { workspace = true, optional = true }
"#;

const LIB: &str = r#"//! Launcher optional module `{{id}}`.

pub const ID: &str = "{{id}}";

#[cfg(feature = "backend")]
pub mod backend;
#[cfg(feature = "ui")]
pub mod ui;
"#;

const BACKEND: &str = r#"use launcher_core::modules::Module;

pub struct {{Struct}}Module;

impl Module for {{Struct}}Module {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

pub fn module() -> Box<dyn Module> {
    Box::new({{Struct}}Module)
}
"#;

const UI: &str = r#"use ui_kit::module::UiModule;

pub struct {{Struct}}Ui;

impl UiModule for {{Struct}}Ui {
    fn id(&self) -> &'static str {
        crate::ID
    }

    fn locale_json(&self, lang: &str) -> Option<&'static str> {
        match lang {
            "uk_UA" => Some(include_str!("../locales/uk_UA.json")),
            "en_US" => Some(include_str!("../locales/en_US.json")),
            _ => None,
        }
    }
}

pub fn ui_module() -> Box<dyn UiModule> {
    Box::new({{Struct}}Ui)
}
"#;

const LOCALE: &str =
    "{\n    \"module_{{id}}_name\": \"{{Struct}}\",\n    \"module_{{id}}_desc\": \"{{Struct}}\"\n}\n";

/// Returns `(relative path, content)` pairs for `modules/<id>/`.
pub fn render(id: &str) -> Vec<(String, String)> {
    let fill = |t: &str| t.replace("{{id}}", id).replace("{{Struct}}", &struct_name(id));
    vec![
        ("Cargo.toml".into(), fill(CARGO)),
        ("src/lib.rs".into(), fill(LIB)),
        ("src/backend.rs".into(), fill(BACKEND)),
        ("src/ui.rs".into(), fill(UI)),
        ("locales/uk_UA.json".into(), fill(LOCALE)),
        ("locales/en_US.json".into(), fill(LOCALE)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_module_ids() {
        assert!(validate_id("stats").is_ok());
        assert!(validate_id("my2").is_ok());
        for bad in ["", "2x", "Big", "with-dash", "a b"] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn renders_placeholders() {
        let files = render("stats");
        let cargo = files.iter().find(|(p, _)| p == "Cargo.toml").unwrap();
        assert!(cargo.1.contains("name = \"module-stats\""));
        let backend = files.iter().find(|(p, _)| p == "src/backend.rs").unwrap();
        assert!(backend.1.contains("pub struct StatsModule;"));
        assert!(files.iter().all(|(_, body)| !body.contains("{{")));
    }
}
