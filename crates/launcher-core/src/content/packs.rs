//! The game's own files that say which packs are on: `options.txt`
//! (`resourcePacks:["file/<name>",…]`) and Iris' `config/iris.properties`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use launcher_shared::ContentKind;
use serde_json::Value;

use crate::storage::atomic::atomic_write_text;

pub const RESOURCE_PACKS: &str = "resourcePacks";
pub const INCOMPATIBLE_RESOURCE_PACKS: &str = "incompatibleResourcePacks";

/// Minecraft before 1.13 lists resource packs in `options.txt` by their bare name, later ones as
/// `file/<name>`.
pub fn legacy_pack_names(minecraft: Option<&str>) -> bool {
    let mut parts = minecraft.unwrap_or_default().split('.');
    parts.next() == Some("1")
        && parts.next().and_then(|minor| minor.parse::<u32>().ok()).is_some_and(|m| m < 13)
}

/// How `options.txt` names resource pack `filename`.
pub fn resourcepack_entry(filename: &str, legacy: bool) -> String {
    if legacy { filename.to_string() } else { format!("file/{filename}") }
}

/// The file's lines; a missing file has none. Any other failure is an error, so a file that
/// cannot be read is never rewritten from nothing.
fn lines_of(path: &Path) -> io::Result<Vec<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).lines().map(str::to_string).collect()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// A `.properties` text as `java.util.Properties` reads it: `\uXXXX` (UTF-16 units), `\t`,
/// `\n`, `\r`, `\f`, and `\x` for any other `x`.
fn unescape(raw: &str) -> String {
    fn flush(units: &mut Vec<u16>, out: &mut String) {
        out.extend(char::decode_utf16(units.drain(..)).map(|c| c.unwrap_or('\u{fffd}')));
    }
    let mut out = String::new();
    let mut units: Vec<u16> = Vec::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            flush(&mut units, &mut out);
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u16::from_str_radix(&hex, 16) {
                    Ok(unit) if hex.len() == 4 => units.push(unit),
                    _ => {
                        flush(&mut units, &mut out);
                        out.push_str(&hex);
                    }
                }
            }
            Some(other) => {
                flush(&mut units, &mut out);
                out.push(match other {
                    't' => '\t',
                    'n' => '\n',
                    'r' => '\r',
                    'f' => '\u{c}',
                    x => x,
                });
            }
            None => {}
        }
    }
    flush(&mut units, &mut out);
    out
}

/// A value as `java.util.Properties.store` writes it (the file is ISO-8859-1): `\`, `=`, `:`,
/// `#`, `!` and a leading space escaped, anything outside printable ASCII as `\uXXXX`.
fn escape(value: &str) -> String {
    let mut out = String::new();
    for (i, c) in value.chars().enumerate() {
        match c {
            ' ' if i == 0 => out.push_str("\\ "),
            '\\' | '=' | ':' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    out
}

/// The JSON list after `key:` on the first such line; missing or unreadable is empty.
pub fn read_options_list(path: &Path, key: &str) -> Vec<String> {
    let prefix = format!("{key}:");
    let lines = lines_of(path).unwrap_or_default();
    let Some(raw) = lines.into_iter().find_map(|line| line.strip_prefix(&prefix).map(str::to_string)) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(raw.trim()) {
        Ok(Value::Array(items)) => {
            items.into_iter().map(|v| if let Value::String(s) = v { s } else { v.to_string() }).collect()
        }
        _ => Vec::new(),
    }
}

/// Sets the `key:` line to `values` as compact JSON: the first such line is replaced, others
/// dropped, and the line added when missing; every other line stays.
pub fn write_options_list(path: &Path, key: &str, values: &[String]) -> io::Result<()> {
    let prefix = format!("{key}:");
    let replacement = format!("{prefix}{}", serde_json::to_string(values).map_err(io::Error::other)?);
    let mut output = Vec::new();
    let mut replaced = false;
    for line in lines_of(path)? {
        if !line.starts_with(&prefix) {
            output.push(line);
        } else if !replaced {
            output.push(replacement.clone());
            replaced = true;
        }
    }
    if !replaced {
        output.push(replacement);
    }
    atomic_write_text(path, &format!("{}\n", output.join("\n")))
}

/// Drops `entry` from the `key:` list; nothing is written when the list does not have it.
pub fn remove_options_entry(path: &Path, key: &str, entry: &str) -> io::Result<()> {
    let mut values = read_options_list(path, key);
    let before = values.len();
    values.retain(|v| v != entry);
    if values.len() == before { Ok(()) } else { write_options_list(path, key, &values) }
}

/// Where the game lists which packs of `kind` are on, relative to the build's folder.
pub fn listing_file(kind: ContentKind) -> Option<&'static str> {
    match kind {
        ContentKind::ResourcePacks => Some("options.txt"),
        ContentKind::ShaderPacks => Some("config/iris.properties"),
        ContentKind::Mods => None,
    }
}

/// Whether listing `path` names a pack `renames` renames (`(old, new)` file names).
pub fn lists_any(path: &Path, kind: ContentKind, legacy: bool, renames: &[(String, String)]) -> bool {
    match kind {
        ContentKind::ResourcePacks => [RESOURCE_PACKS, INCOMPATIBLE_RESOURCE_PACKS].iter().any(|key| {
            let listed = read_options_list(path, key);
            renames.iter().any(|(old, _)| listed.contains(&resourcepack_entry(old, legacy)))
        }),
        ContentKind::ShaderPacks => read_properties(path)
            .get("shaderPack")
            .is_some_and(|current| renames.iter().any(|(old, _)| old == current)),
        ContentKind::Mods => false,
    }
}

/// Renames the packs `renames` renames where listing `path` names them — an updated pack keeps
/// its place and stays on (or off). Whether it changed.
pub fn rename_listed(
    path: &Path,
    kind: ContentKind,
    legacy: bool,
    renames: &[(String, String)],
) -> io::Result<bool> {
    match kind {
        ContentKind::ResourcePacks => {
            let mut changed = false;
            for key in [RESOURCE_PACKS, INCOMPATIBLE_RESOURCE_PACKS] {
                let mut listed = read_options_list(path, key);
                let mut hit = false;
                for entry in listed.iter_mut() {
                    if let Some((_, new)) =
                        renames.iter().find(|(old, _)| *entry == resourcepack_entry(old, legacy))
                    {
                        *entry = resourcepack_entry(new, legacy);
                        hit = true;
                    }
                }
                if hit {
                    write_options_list(path, key, &listed)?;
                    changed = true;
                }
            }
            Ok(changed)
        }
        ContentKind::ShaderPacks => {
            let current = read_properties(path).get("shaderPack").cloned();
            match current.and_then(|c| renames.iter().find(|(old, _)| *old == c)) {
                Some((_, new)) => write_properties(path, &[("shaderPack", new)]).map(|()| true),
                None => Ok(false),
            }
        }
        ContentKind::Mods => Ok(false),
    }
}

/// `key=value` pairs (split at the first `=`, trimmed, then unescaped as Java does); comments and
/// blank lines skipped; an unreadable file has none.
pub fn read_properties(path: &Path) -> HashMap<String, String> {
    lines_of(path)
        .unwrap_or_default()
        .iter()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (unescape(k.trim()), unescape(v.trim())))
        .collect()
}

/// Sets `updates` (values escaped as Java stores them) where their keys stand, adds the missing ones
/// at the end and keeps comments, blank lines and other keys.
pub fn write_properties(path: &Path, updates: &[(&str, &str)]) -> io::Result<()> {
    let mut pending: Vec<(&str, &str)> = updates.to_vec();
    let mut output = Vec::new();
    for line in lines_of(path)? {
        let key = (!line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .then(|| line.split_once('=').map(|(k, _)| k.trim().to_string()))
            .flatten();
        match key.and_then(|k| pending.iter().position(|(p, _)| *p == k)) {
            Some(at) => {
                let (k, v) = pending.remove(at);
                output.push(format!("{k}={}", escape(v)));
            }
            None => output.push(line),
        }
    }
    output.extend(pending.into_iter().map(|(k, v)| format!("{k}={}", escape(v))));
    atomic_write_text(path, &format!("{}\n", output.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_lists_change_only_their_line() {
        let dir = tempfile::tempdir().unwrap();
        let options = dir.path().join("options.txt");
        assert!(read_options_list(&options, RESOURCE_PACKS).is_empty(), "no file");
        fs::write(&options, "version:3953\r\nresourcePacks:[\"vanilla\",\"file/A.zip\"]\nlang:uk_ua\nresourcePacks:[\"old\"]\n").unwrap();
        assert_eq!(
            read_options_list(&options, RESOURCE_PACKS),
            ["vanilla", "file/A.zip"],
            "the first line counts"
        );
        let values = vec!["vanilla".to_string(), "file/Б пак.zip".to_string()];
        write_options_list(&options, RESOURCE_PACKS, &values).unwrap();
        assert_eq!(
            fs::read_to_string(&options).unwrap(),
            "version:3953\nresourcePacks:[\"vanilla\",\"file/Б пак.zip\"]\nlang:uk_ua\n"
        );
        write_options_list(&options, INCOMPATIBLE_RESOURCE_PACKS, &["file/x.zip".to_string()]).unwrap();
        assert!(
            fs::read_to_string(&options)
                .unwrap()
                .ends_with("lang:uk_ua\nincompatibleResourcePacks:[\"file/x.zip\"]\n")
        );
        remove_options_entry(&options, INCOMPATIBLE_RESOURCE_PACKS, "file/x.zip").unwrap();
        assert!(read_options_list(&options, INCOMPATIBLE_RESOURCE_PACKS).is_empty());
        let missing = dir.path().join("none").join("options.txt");
        remove_options_entry(&missing, RESOURCE_PACKS, "file/x.zip").unwrap();
        assert!(!missing.exists(), "nothing to remove writes nothing");
        fs::write(&options, "resourcePacks:not json\n").unwrap();
        assert!(read_options_list(&options, RESOURCE_PACKS).is_empty());
    }

    #[test]
    fn properties_keep_comments_and_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("iris.properties");
        write_properties(&path, &[("enableShaders", "true"), ("shaderPack", "BSL.zip")]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "enableShaders=true\nshaderPack=BSL.zip\n");
        fs::write(&path, "#Iris\n\nmaxShadowRenderDistance=32\n enableShaders = true\nshaderPack=BSL.zip\n")
            .unwrap();
        let read = read_properties(&path);
        assert_eq!((read["enableShaders"].as_str(), read["shaderPack"].as_str()), ("true", "BSL.zip"));
        write_properties(&path, &[("enableShaders", "false"), ("colorSpace", "SRGB")]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "#Iris\n\nmaxShadowRenderDistance=32\nenableShaders=false\nshaderPack=BSL.zip\ncolorSpace=SRGB\n"
        );
    }

    #[test]
    fn legacy_names_belong_to_old_minecraft() {
        for (mc, legacy) in [
            (Some("1.12.2"), true),
            (Some("1.8.9"), true),
            (Some("1.13"), false),
            (Some("1.21.1"), false),
            (Some("24w33a"), false),
            (None, false),
        ] {
            assert_eq!(legacy_pack_names(mc), legacy, "{mc:?}");
        }
        assert_eq!(
            (resourcepack_entry("A.zip", false), resourcepack_entry("A.zip", true)),
            ("file/A.zip".into(), "A.zip".into())
        );
    }

    #[test]
    fn properties_are_stored_the_way_java_reads_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iris.properties");
        write_properties(&path, &[("shaderPack", "Шейдер!.zip"), ("note", " a=b:c#\\")]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "shaderPack=\\u0428\\u0435\\u0439\\u0434\\u0435\\u0440\\!.zip\nnote=\\ a\\=b\\:c\\#\\\\\n"
        );
        let read = read_properties(&path);
        assert_eq!((read["shaderPack"].as_str(), read["note"].as_str()), ("Шейдер!.zip", " a=b:c#\\"));
        fs::write(&path, "shaderPack=Pack\\!.zip\nemoji=\\uD83D\\uDE00\n").unwrap();
        let read = read_properties(&path);
        assert_eq!((read["shaderPack"].as_str(), read["emoji"].as_str()), ("Pack!.zip", "😀"));
    }

    #[cfg(windows)]
    #[test]
    fn an_unreadable_file_is_never_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let options = dir.path().join("options.txt");
        fs::write(&options, "key_key.jump:key.keyboard.space\nresourcePacks:[]\n").unwrap();
        // Held without read sharing, but open to delete — as some tools do.
        let held = fs::OpenOptions::new().read(true).share_mode(0x4).open(&options).unwrap();
        assert!(write_options_list(&options, RESOURCE_PACKS, &["file/A.zip".to_string()]).is_err());
        assert!(write_properties(&options, &[("a", "b")]).is_err());
        drop(held);
        assert_eq!(
            fs::read_to_string(&options).unwrap(),
            "key_key.jump:key.keyboard.space\nresourcePacks:[]\n"
        );
    }
}
