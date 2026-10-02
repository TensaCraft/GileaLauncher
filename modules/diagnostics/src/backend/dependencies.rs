//! Which mod needs which missing mod, read from the loaders' own words (missing-dependency
//! parsing): NeoForge's table, Quilt's lines and the generic Fabric form.

use std::sync::LazyLock;

use regex::Regex;

static NEOFORGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)mod id:\s*'(?P<dependency>[a-z0-9_.:+-]+)'\s*,\s*requested by:\s*'(?P<mod>[a-z0-9_.:+-]+)'.*?actual version:\s*'\[missing\]'")
        .expect("the NeoForge pattern")
});
static QUILT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<mod>.+?)\s+requires\s+(?P<requirement>.+which is missing!)\s*$")
        .expect("the Quilt pattern")
});
static GENERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\bmod\s+(?:["'](?P<quoted_mod>[^"'\r\n]+)["'](?:\s+\((?P<mod_id>[a-z0-9_.:+-]+)\))?|(?P<plain_mod>[a-z0-9_.:+-]+))(?:\s+\([^)\r\n]+\))?(?:\s+[0-9][^\s,;]*)?\s+requires\s+(?P<requirement>[^\r\n]+)"#,
    )
    .expect("the generic pattern")
});
static OF_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bof\s+(?:mod\s+)?["'][^"'\r\n]+["']\s*\((?P<dependency>[a-z0-9_.:+-]+)\)"#)
        .expect("of id")
});
static OF_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bof\s+(?:mod\s+)?(?:["'](?P<quoted>[^"'\r\n]+)["']|(?P<plain>[a-z0-9_.:+-]+))"#)
        .expect("of name")
});
static LEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)^\s*['"]?(?P<dependency>[a-z0-9_.:+-]+)"#).expect("leading id"));

/// An id as the logs name it: no surrounding quotes, dots or colons; lower case.
pub fn clean_id(raw: &str) -> String {
    raw.trim_matches(|c| " '\".,;:".contains(c)).to_lowercase()
}

/// An id made fit for a finding's id; the names of Fabric API are one.
pub fn finding_id_part(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let mut out = String::new();
    let mut replaced = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            out.push(c);
            replaced = false;
        } else if !replaced {
            out.push('_');
            replaced = true;
        }
    }
    let part = out.trim_matches(['.', '_', '-']).to_string();
    match part.as_str() {
        "" => "unknown".into(),
        "fabric-api" | "fabric_api" | "fabricapi" => "fabric_api".into(),
        _ => part,
    }
}

/// The dependency a requirement names: `of 'Name' (id)`, else `of name`, else its first word.
fn dependency_of(requirement: &str) -> Option<String> {
    if let Some(c) = OF_ID.captures(requirement) {
        return Some(clean_id(&c["dependency"]));
    }
    if let Some(c) = OF_NAME.captures(requirement) {
        let named = c.name("quoted").or_else(|| c.name("plain")).map(|m| m.as_str()).unwrap_or_default();
        if !named.is_empty() && !named.contains(' ') {
            return Some(clean_id(named));
        }
    }
    let first = clean_id(&LEADING.captures(requirement)?["dependency"]);
    (!first.is_empty() && first != "any" && first != "version").then_some(first)
}

/// A Quilt line starts with the mod's name; a line that starts with "mod" (after a list marker)
/// is the generic form's.
fn names_a_mod_first(line: &str) -> bool {
    let rest = line.trim_start().trim_start_matches(['-', '*']).trim_start().to_lowercase();
    rest.strip_prefix("mod")
        .is_some_and(|after| after.starts_with(|c: char| c.is_whitespace() || c == '\'' || c == '"'))
}

/// `(mod, dependency)` for each missing dependency the logs name, each dependency once.
pub fn missing(raw: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let put =
        |found: &mut Vec<(String, String)>, module: String, dependency: String, replace: bool| match found
            .iter()
            .position(|(_, d)| *d == dependency)
        {
            Some(i) if replace => found[i] = (module, dependency),
            Some(_) => {}
            None => found.push((module, dependency)),
        };
    for c in NEOFORGE.captures_iter(raw) {
        put(&mut found, clean_id(&c["mod"]), clean_id(&c["dependency"]), true);
    }
    for line in raw.lines().filter(|l| !names_a_mod_first(l)) {
        if let Some(c) = QUILT.captures(line)
            && let Some(dependency) = dependency_of(&c["requirement"])
        {
            put(&mut found, clean_id(&c["mod"]), dependency, false);
        }
    }
    for c in GENERIC.captures_iter(raw) {
        let module = c.name("mod_id").or_else(|| c.name("quoted_mod")).or_else(|| c.name("plain_mod"));
        let (Some(module), Some(dependency)) = (module, dependency_of(&c["requirement"])) else { continue };
        let requirement = c["requirement"].to_lowercase();
        let marked = requirement.contains("missing") || requirement.contains("not installed");
        let currently = Regex::new(&format!(
            r#"(?im)\bcurrently,\s+(?:mod\s+)?['"]?{}['"]?\s+is\s+not\s+installed\b"#,
            regex::escape(&dependency)
        ))
        .is_ok_and(|r| r.is_match(raw));
        if marked || currently {
            put(&mut found, clean_id(module.as_str()), dependency, false);
        }
    }
    found
}
