//! A catalog entry read as a server build: its id, what it shows, what it runs,
//! where its files are and what it asks of the build.

use serde_json::{Map, Value};

/// A flag as the server writes it: a bool, a number other than zero, `1`/`true`/`yes`/`y`/`on`
/// (any case), or else anything that is not empty.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => ["1", "true", "yes", "y", "on"].contains(&text.trim().to_lowercase().as_str()),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        Value::Null => false,
    }
}

/// A string that is not blank, trimmed.
fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
}

/// The first of `keys` in `map` that is a string that is not blank.
fn first(map: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| text(map.get(*key)))
}

/// The keys the server may name its force-update endpoint with, the first one first.
const FORCE_ENDPOINT_KEYS: [&str; 5] = [
    "force_update_endpoint",
    "forceUpdateEndpoint",
    "force_update_url",
    "forceUpdateUrl",
    "force-update_endpoint",
];

/// Java arguments as a list, or as a string of lines; blank ones go.
fn arguments(value: &Value) -> Option<Vec<String>> {
    let list = match value {
        Value::Array(items) => items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>(),
        Value::String(lines) => lines.lines().map(str::to_string).collect(),
        _ => return None,
    };
    Some(list.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).map(str::to_string).collect())
}

/// A server port, or Minecraft's own when it is missing or out of range.
fn port(value: Option<&Value>) -> u16 {
    let number = match value {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    number.filter(|p| (1..=65535).contains(p)).map_or(25565, |p| p as u16)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub image: Option<String>,
    pub minecraft: Option<String>,
    /// `fabric`, `forge`, `neoforge` or `quilt`.
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    /// The build follows the server's files before every launch.
    pub force_update: bool,
    pub files_endpoint: Option<String>,
    pub force_endpoint: Option<String>,
    pub preserve_rules: Option<Value>,
    /// Settings of the build the server overrides at every sync.
    pub forced_fields: Vec<String>,
    pub server: Option<(String, u16)>,
    pub gpu: Option<String>,
    pub options: Map<String, Value>,
    /// Java arguments; an empty list clears the build's own.
    pub jvm_arguments: Option<Vec<String>>,
    pub raw: Value,
}

impl Pack {
    /// The build `entry` describes: none without a `client` object or an id.
    pub fn from_value(entry: &Value) -> Option<Pack> {
        let client = entry.get("client").filter(|c| c.is_object())?;
        let id = text(client.get("id"))
            .or_else(|| first(entry, &["slug", "name"]))
            .or_else(|| first(client, &["name", "ver_id"]))?;
        let name = text(entry.get("title"))
            .or_else(|| text(client.get("name")))
            .or_else(|| text(entry.get("name")))
            .unwrap_or_else(|| id.clone());
        let description = first(entry, &["description", "summary", "short_description"])
            .or_else(|| first(client, &["description", "summary", "short_description"]));
        let force_endpoint = first(client, &FORCE_ENDPOINT_KEYS);
        let force_update = match client.get("force_update") {
            Some(flag) => truthy(flag),
            None => force_endpoint.is_some(),
        };
        let options = client.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
        let jvm_arguments =
            [client.get("jvm_arguments"), options.get("jvm_arguments"), options.get("jvmArguments")]
                .into_iter()
                .flatten()
                .find_map(arguments);
        Some(Pack {
            name,
            description,
            image: text(entry.get("image")).or_else(|| text(client.get("image"))),
            minecraft: first(client, &["minecraft_version", "version"])
                .or_else(|| text(entry.get("minecraft_version"))),
            loader: first(client, &["loader_id", "loader"]).map(|l| l.to_lowercase()),
            loader_version: text(client.get("loader_version")),
            force_update,
            files_endpoint: first(client, &["files_endpoint", "endpoint"]),
            force_endpoint,
            preserve_rules: client.get("preserve_rules").cloned(),
            forced_fields: client
                .get("force_update_profile_fields")
                .and_then(Value::as_array)
                .map(|fields| fields.iter().filter_map(|f| text(Some(f))).collect())
                .unwrap_or_default(),
            server: text(client.get("server_host")).map(|host| (host, port(client.get("server_port")))),
            gpu: text(client.get("gpu_preference")),
            options,
            jvm_arguments,
            raw: entry.clone(),
            id,
        })
    }
}

/// The catalog's build `needle` names (its id, slug, name, client id or client name; case and
/// surrounding spaces aside).
pub fn find(packs: &[Value], needle: &str) -> Option<Pack> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    packs.iter().filter_map(Pack::from_value).find(|pack| {
        let client = pack.raw.get("client").unwrap_or(&Value::Null);
        [
            Some(pack.id.clone()),
            text(pack.raw.get("slug")),
            text(pack.raw.get("name")),
            text(client.get("id")),
            text(client.get("name")),
        ]
        .into_iter()
        .flatten()
        .any(|name| name.to_lowercase() == needle)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn pack(entry: Value) -> Pack {
        Pack::from_value(&entry).expect("a server build")
    }

    #[test]
    fn a_pack_needs_a_client_and_an_id() {
        assert_eq!(Pack::from_value(&json!({"name": "Aero"})), None);
        assert_eq!(Pack::from_value(&json!({"client": "aero"})), None);
        assert_eq!(Pack::from_value(&json!({"client": {"id": "  "}})), None);
        assert_eq!(pack(json!({"client": {"id": " aero "}})).id, "aero");
    }

    #[test]
    fn the_id_falls_back_in_order() {
        let full = json!({"slug": "s", "name": "n", "client": {"id": "c", "name": "cn", "ver_id": "v"}});
        assert_eq!(pack(full).id, "c");
        assert_eq!(pack(json!({"slug": "s", "name": "n", "client": {"name": "cn"}})).id, "s");
        assert_eq!(pack(json!({"name": "n", "client": {"name": "cn"}})).id, "n");
        assert_eq!(pack(json!({"client": {"name": "cn", "ver_id": "v"}})).id, "cn");
        assert_eq!(pack(json!({"client": {"ver_id": "v"}})).id, "v");
    }

    #[test]
    fn names_descriptions_and_images_fall_back() {
        let p = pack(json!({"title": "T", "name": "N", "client": {"id": "c", "name": "CN"}}));
        assert_eq!(p.name, "T");
        assert_eq!(pack(json!({"name": "N", "client": {"id": "c", "name": "CN"}})).name, "CN");
        assert_eq!(pack(json!({"name": "N", "client": {"id": "c"}})).name, "N");
        assert_eq!(pack(json!({"client": {"id": "c"}})).name, "c");
        let described = |entry: Value| pack(entry).description;
        assert_eq!(
            described(json!({"summary": "S", "client": {"id": "c", "description": "CD"}})),
            Some("S".into())
        );
        assert_eq!(described(json!({"client": {"id": "c", "short_description": "CS"}})), Some("CS".into()));
        assert_eq!(described(json!({"client": {"id": "c"}})), None);
        assert_eq!(pack(json!({"client": {"id": "c", "image": "i.png"}})).image, Some("i.png".into()));
        assert_eq!(
            pack(json!({"image": "top.png", "client": {"id": "c", "image": "i.png"}})).image,
            Some("top.png".into())
        );
    }

    #[test]
    fn what_it_runs_comes_from_its_client() {
        let p = pack(json!({"minecraft_version": "1.20.1", "client": {
            "id": "c", "version": "1.21", "loader": "NeoForge", "loader_version": "21.1.77"
        }}));
        assert_eq!(
            (p.minecraft.as_deref(), p.loader.as_deref(), p.loader_version.as_deref()),
            (Some("1.21"), Some("neoforge"), Some("21.1.77"))
        );
        let q = pack(json!({"minecraft_version": "1.20.1", "client": {
            "id": "c", "minecraft_version": "1.21.1", "loader_id": "fabric", "loader": "forge"
        }}));
        assert_eq!((q.minecraft.as_deref(), q.loader.as_deref()), (Some("1.21.1"), Some("fabric")));
        assert_eq!(
            pack(json!({"minecraft_version": "1.20.1", "client": {"id": "c"}})).minecraft,
            Some("1.20.1".into())
        );
    }

    #[test]
    fn force_update_follows_the_flag_or_a_force_endpoint() {
        assert!(pack(json!({"client": {"id": "c", "force_update": "Yes"}})).force_update);
        assert!(
            !pack(json!({"client": {"id": "c", "force_update": "no", "force_update_url": "u"}})).force_update
        );
        assert!(pack(json!({"client": {"id": "c", "forceUpdateEndpoint": "u"}})).force_update);
        assert!(!pack(json!({"client": {"id": "c"}})).force_update);
    }

    #[test]
    fn the_force_endpoint_takes_its_aliases_in_order() {
        let p = pack(
            json!({"client": {"id": "c", "forceUpdateUrl": "b", "force_update_url": "a", "force-update_endpoint": "z"}}),
        );
        assert_eq!(p.force_endpoint.as_deref(), Some("a"));
        assert_eq!(
            pack(json!({"client": {"id": "c", "force-update_endpoint": "z"}})).force_endpoint.as_deref(),
            Some("z")
        );
        let f = pack(json!({"client": {"id": "c", "endpoint": "e"}}));
        assert_eq!(f.files_endpoint.as_deref(), Some("e"));
        let g = pack(json!({"client": {"id": "c", "files_endpoint": "f", "endpoint": "e"}}));
        assert_eq!(g.files_endpoint.as_deref(), Some("f"));
    }

    #[test]
    fn jvm_arguments_come_as_a_list_or_lines() {
        let listed = pack(json!({"client": {"id": "c", "jvm_arguments": [" -Xss2m ", "", 5, "-Da=b"]}}));
        assert_eq!(listed.jvm_arguments, Some(vec!["-Xss2m".into(), "-Da=b".into()]));
        let lines = pack(json!({"client": {"id": "c", "options": {"jvmArguments": "-Xss2m\n\n -Da=b "}}}));
        assert_eq!(lines.jvm_arguments, Some(vec!["-Xss2m".into(), "-Da=b".into()]));
        let cleared = pack(json!({"client": {"id": "c", "jvm_arguments": []}}));
        assert_eq!(cleared.jvm_arguments, Some(Vec::new()), "an empty list clears them");
        assert_eq!(pack(json!({"client": {"id": "c"}})).jvm_arguments, None);
    }

    #[test]
    fn the_server_address_and_the_rest_are_kept() {
        let p = pack(json!({"client": {
            "id": "c", "server_host": " play.example ", "server_port": "70000",
            "gpu_preference": "discrete", "options": {"a": 1}, "preserve_rules": [{"type": "file", "path": "x"}],
            "force_update_profile_fields": [" server ", 3, "jvm"]
        }}));
        assert_eq!(p.server, Some(("play.example".into(), 25565)), "a bad port is the default");
        assert_eq!(
            pack(json!({"client": {"id": "c", "server_host": "h", "server_port": 25570}})).server,
            Some(("h".into(), 25570))
        );
        assert_eq!(pack(json!({"client": {"id": "c", "server_port": 25570}})).server, None);
        assert_eq!(p.gpu.as_deref(), Some("discrete"));
        assert_eq!(p.options.get("a"), Some(&json!(1)));
        assert_eq!(p.preserve_rules, Some(json!([{"type": "file", "path": "x"}])));
        assert_eq!(p.forced_fields, ["server", "jvm"]);
    }

    #[test]
    fn find_matches_any_name_ignoring_case_and_spaces() {
        let packs = vec![
            json!({"name": "Other", "client": {"id": "other"}}),
            json!({"slug": "aero-pack", "name": "Aero Pack", "client": {"id": "aero", "name": "Aeronautics"}}),
        ];
        for needle in ["aero", " AERO-PACK ", "aero pack", "aeronautics"] {
            assert_eq!(find(&packs, needle).map(|p| p.id), Some("aero".into()), "{needle}");
        }
        assert_eq!(find(&packs, "nope"), None);
        assert_eq!(find(&packs, "  "), None);
    }

    #[test]
    fn truthy_reads_flags_like_the_original() {
        for yes in [
            json!(true),
            json!(1),
            json!(0.5),
            json!("1"),
            json!(" Yes "),
            json!("on"),
            json!("Y"),
            json!([0]),
            json!({"a": 1}),
        ] {
            assert!(truthy(&yes), "{yes}");
        }
        for no in [
            json!(false),
            json!(0),
            json!("no"),
            json!("maybe"),
            json!(""),
            json!(null),
            json!([]),
            json!({}),
        ] {
            assert!(!truthy(&no), "{no}");
        }
    }
}
