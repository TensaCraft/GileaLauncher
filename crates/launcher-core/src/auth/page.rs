//! The page the browser shows after the Microsoft redirect.

use serde_json::Value;

const UK: &str = include_str!("../../../../assets/langs/uk_UA.json");
const EN: &str = include_str!("../../../../assets/langs/en_US.json");

/// Launcher text for `key`: the chosen language, then English, then the key itself.
fn text(lang: &str, key: &str) -> String {
    let pick =
        |source: &str| serde_json::from_str::<Value>(source).ok()?.get(key)?.as_str().map(str::to_string);
    let primary = if lang == "uk_UA" { UK } else { EN };
    pick(primary).or_else(|| pick(EN)).unwrap_or_else(|| key.to_string())
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `error: None` is the success page; otherwise the (escaped) error text is shown.
pub fn callback_page(lang: &str, error: Option<&str>) -> String {
    let (title, message) = match error {
        None => (text(lang, "auth_success"), text(lang, "microsoft_auth_browser_success")),
        Some(detail) => (text(lang, "auth_error"), detail.to_string()),
    };
    let html_lang = if lang == "uk_UA" { "uk" } else { "en" };
    let tab = escape(&text(lang, "app_title"));
    format!(
        "<!doctype html><html lang=\"{html_lang}\"><head><meta charset=\"utf-8\"><title>{tab}</title>\
<style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#07150f;color:#e6f6f2;\
font-family:\"Segoe UI\",system-ui,sans-serif}}main{{max-width:520px;padding:32px;text-align:center}}\
h1{{font-size:22px;margin:0 0 12px}}p{{color:#bed7d2;line-height:1.5}}</style></head>\
<body><main><h1>{}</h1><p>{}</p></main></body></html>",
        escape(&title),
        escape(&message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_is_escaped() {
        let page = callback_page("en_US", Some("<script>alert('x')</script>"));
        assert!(page.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"), "{page}");
        assert!(!page.contains("<script>"));
    }

    #[test]
    fn the_page_names_no_brand() {
        let page = callback_page("en_US", None);
        assert!(page.contains("<title>Minecraft launcher</title>"), "{page}");
        assert!(page.contains("return to the launcher"), "{page}");
    }

    #[test]
    fn success_page_follows_the_language() {
        assert!(callback_page("uk_UA", None).contains(&escape(&text("uk_UA", "auth_success"))));
        assert!(callback_page("de_DE", None).contains(&escape(&text("en_US", "auth_success"))));
        assert_eq!(text("en_US", "no_such_key_here"), "no_such_key_here");
    }
}
