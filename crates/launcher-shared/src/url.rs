//! Percent-encoding of the launcher's own URLs (page queries, the screenshot scheme).

/// `raw` with unreserved characters as they are and everything else as `%XX` of its UTF-8.
pub fn url_escape(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `url_escape` undone; `None` for a broken `%XX` or bytes that are not UTF-8.
pub fn url_unescape(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_survive_the_round_trip() {
        for raw in ["aero", "Моя збірка", "2026-09-27_16.55.46.png", "a b+c%d/e"] {
            assert_eq!(url_unescape(&url_escape(raw)).as_deref(), Some(raw));
        }
        assert_eq!(url_escape("a b/ї"), "a%20b%2F%D1%97");
        for bad in ["%", "%4", "%zz", "%FF"] {
            assert_eq!(url_unescape(bad), None, "{bad}");
        }
    }
}
