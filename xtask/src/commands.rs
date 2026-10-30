//! Every command a page asks the app for by name is one the app registers (`generate_handler!`)
//! and one the browser preview answers (`crates/launcher-ui/src/mock*.rs`): a name typed wrong
//! fails here, not in the player's hands.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::brand::repo_files;

/// Where the app lists its commands.
const HANDLERS: &str = "crates/launcher-app/src/lib.rs";

/// The names a page asks for by a literal: `invoke("x"`, `invoke::<..>("x"`, `call::<..>("x")`,
/// `tauri_invoke_raw("x"`.
pub fn asked(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in ["invoke", "call", "tauri_invoke_raw"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(word) {
            let start = from + at;
            from = start + word.len();
            if text[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let mut rest = &text[from..];
            if let Some(after) = rest.strip_prefix("::<") {
                // The turbofish ends at the first `>(` of its line.
                let line = &after[..after.find('\n').unwrap_or(after.len())];
                let Some(close) = line.find(">(") else { continue };
                rest = &after[close + 1..];
            }
            let Some(literal) = rest.strip_prefix('(').map(str::trim_start).and_then(|a| a.strip_prefix('"'))
            else {
                continue;
            };
            let name = &literal[..literal.find('"').unwrap_or(0)];
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                found.push(name.to_string());
            }
        }
    }
    found
}

/// The commands `generate_handler![…]` in `text` lists, by their own names.
pub fn registered(text: &str) -> BTreeSet<String> {
    let Some(start) = text.find("generate_handler![") else { return BTreeSet::new() };
    let list = &text[start + "generate_handler![".len()..];
    let list = &list[..list.find(']').unwrap_or(list.len())];
    list.split(',')
        .map(|entry| entry.trim().rsplit("::").next().unwrap_or_default().to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Whether the preview's `mock` text answers command `name`: by its name, or by a prefix it
/// hands on (`cmd.starts_with("provider_")`).
pub fn mocked(mock: &str, name: &str) -> bool {
    if mock.contains(&format!("\"{name}\"")) {
        return true;
    }
    mock.match_indices("starts_with(\"").any(|(at, marker)| {
        let prefix = &mock[at + marker.len()..];
        let prefix = &prefix[..prefix.find('"').unwrap_or(0)];
        !prefix.is_empty() && name.starts_with(prefix)
    })
}

fn is_page(rel: &str) -> bool {
    let page = rel.starts_with("crates/launcher-ui/src/")
        || rel.starts_with("crates/ui-kit/src/")
        || (rel.starts_with("modules/") && rel.contains("/src/ui/"));
    let file = rel.rsplit('/').next().unwrap_or(rel);
    page && rel.ends_with(".rs") && !file.starts_with("mock")
}

pub fn offenders(root: &Path) -> Vec<String> {
    let files = repo_files(root);
    let read = |rel: &str| fs::read_to_string(root.join(rel)).unwrap_or_default();
    let handlers = registered(&read(HANDLERS));
    let mock: String = files
        .iter()
        .filter(|r| {
            r.starts_with("crates/launcher-ui/src/")
                && r.rsplit('/').next().is_some_and(|f| f.starts_with("mock"))
        })
        .map(|r| read(r))
        .collect();
    let mut found = Vec::new();
    for rel in files.iter().filter(|r| is_page(r)) {
        for name in asked(&read(rel)) {
            if !handlers.contains(&name) {
                found.push(format!("{rel}: asks for `{name}`, which the app does not register"));
            }
            if !mocked(&mock, &name) {
                found.push(format!("{rel}: asks for `{name}`, which the browser preview does not answer"));
            }
        }
    }
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_s_literal_command_names_are_found() {
        let text = r#"
            ipc::invoke::<_, ()>("image_file_check", &args).await;
            ipc::call::<Vec<Option<String>>>("builds_list").await;
            invoke("content_add", &args);
            tauri_invoke_raw("drop_stage", bytes, options);
            ipc::module_invoke(crate::ID, "import", &args);
            change("content_toggle", item);
            recall("not_a_call");
        "#;
        assert_eq!(asked(text), ["image_file_check", "content_add", "builds_list", "drop_stage"]);
    }

    #[test]
    fn registered_and_mocked_names_are_read() {
        let lib = "tauri::generate_handler![\n    commands::app_info,\n    content_commands::content_add,\n]";
        assert_eq!(registered(lib), BTreeSet::from(["app_info".to_string(), "content_add".to_string()]));
        let mock = r#""content_add" => x, cmd if cmd.starts_with("provider_") => y"#;
        assert!(mocked(mock, "content_add") && mocked(mock, "provider_search"));
        assert!(!mocked(mock, "content"), "a name is matched whole");
    }

    #[test]
    fn every_command_a_page_asks_for_is_registered_and_answered_in_the_preview() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(offenders(root), Vec::<String>::new());
    }
}
