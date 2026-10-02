//! JSON helpers with the original launcher's formatting rules.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use super::atomic::atomic_write;

#[derive(Debug)]
pub enum JsonRead {
    Missing,
    Object(Map<String, Value>),
    Invalid(String),
}

/// Pretty JSON with `indent` spaces; non-ASCII characters are written as-is.
pub fn to_json_string(value: &Value, indent: usize) -> String {
    let indent_bytes = vec![b' '; indent];
    let formatter = serde_json::ser::PrettyFormatter::with_indent(&indent_bytes);
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut ser).expect("serializing a serde_json::Value cannot fail");
    String::from_utf8(out).expect("serde_json produces UTF-8")
}

pub fn write_json_file(path: &Path, value: &Value, indent: usize) -> io::Result<()> {
    atomic_write(path, to_json_string(value, indent).as_bytes())
}

/// Copies an unreadable file to `<name>.corrupt-<unix seconds>[-n]` before it is replaced.
pub fn backup_corrupt_file(path: &Path) -> io::Result<std::path::PathBuf> {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut backup = path.with_file_name(format!("{name}.corrupt-{secs}"));
    let mut n = 1;
    while backup.exists() {
        backup = path.with_file_name(format!("{name}.corrupt-{secs}-{n}"));
        n += 1;
    }
    fs::copy(path, &backup)?;
    Ok(backup)
}

/// Reads a JSON object. Missing file -> `Missing`; unreadable content -> `Invalid` (never an error).
pub fn read_json_object(path: &Path) -> io::Result<JsonRead> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(JsonRead::Missing),
        Err(e) => return Err(e),
    };
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Ok(JsonRead::Invalid("file is not valid UTF-8".into())),
    };
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(map)) => Ok(JsonRead::Object(map)),
        Ok(other) => Ok(JsonRead::Invalid(format!("expected a JSON object, found {}", type_name(&other)))),
        Err(e) => Ok(JsonRead::Invalid(format!("invalid JSON: {e}"))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pretty_json_uses_requested_indent_and_keeps_unicode() {
        let s = to_json_string(&json!({"lang": "uk_UA", "назва": "Світ"}), 4);
        assert!(s.contains("\n    \"lang\": \"uk_UA\""));
        assert!(s.contains("Світ"));
        assert!(!s.contains("\\u"));
    }

    #[test]
    fn read_json_object_classifies_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.json");
        assert!(matches!(read_json_object(&p).unwrap(), JsonRead::Missing));
        for bad in [&b""[..], b"null", b"[1,2]", b"{", b"\xff\xfe{}"] {
            fs::write(&p, bad).unwrap();
            assert!(matches!(read_json_object(&p).unwrap(), JsonRead::Invalid(_)), "{bad:?}");
        }
        fs::write(&p, br#"{"a": 1}"#).unwrap();
        match read_json_object(&p).unwrap() {
            JsonRead::Object(map) => assert_eq!(map["a"], json!(1)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn write_json_file_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.json");
        write_json_file(&p, &json!({"k": "v"}), 2).unwrap();
        assert!(matches!(read_json_object(&p).unwrap(), JsonRead::Object(_)));
    }
}
