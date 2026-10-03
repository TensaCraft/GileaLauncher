//! The `shot` scheme: how the content page shows a build's screenshots — only files its
//! screenshot list has, never an arbitrary path.

use launcher_core::content::screenshots::image_type;
use launcher_shared::url::{url_escape, url_unescape};
use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

use crate::commands::AppState;

pub const SCHEME: &str = "shot";

/// The page's URL of screenshot `name` of build `key`; `modified_ms` keeps a changed file fresh.
pub fn screenshot_url(key: &str, name: &str, modified_ms: Option<u64>) -> String {
    let base = if cfg!(any(windows, target_os = "android")) {
        format!("http://{SCHEME}.localhost")
    } else {
        format!("{SCHEME}://localhost")
    };
    format!("{base}/{}/{}?v={}", url_escape(key), url_escape(name), modified_ms.unwrap_or(0))
}

/// The page's URL of the thumbnail of screenshot `name` of build `key` (`Thumbs`).
pub fn thumbnail_url(key: &str, name: &str, modified_ms: Option<u64>) -> String {
    format!("{}&thumb=1", screenshot_url(key, name, modified_ms))
}

/// Whether a request's query asks for the thumbnail.
pub fn wants_thumb(query: Option<&str>) -> bool {
    query.is_some_and(|q| q.split('&').any(|part| part == "thumb=1"))
}

/// The build key and file name of a request path `/<key>/<name>`.
pub fn parse_path(path: &str) -> Option<(String, String)> {
    let (key, name) = path.strip_prefix('/')?.split_once('/')?;
    Some((url_unescape(key)?, url_unescape(name)?))
}

fn status(code: u16) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap_or_default()
}

/// The picture a request names (blocking): 404 for anything the build's list does not have. A
/// thumbnail that cannot be made is answered with the picture itself.
pub fn respond(app: &AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(state) = app.try_state::<AppState>() else { return status(503) };
    let Some((key, name)) = parse_path(request.uri().path()) else { return status(400) };
    let Ok(shot) = state.core.content.screenshot(&key, &name) else { return status(404) };
    let thumb = wants_thumb(request.uri().query())
        .then(|| state.core.content.screenshot_thumb(&key, &name))
        .and_then(|made| made.inspect_err(|e| tracing::debug!("No thumbnail: {}", e.detail)).ok());
    let (path, kind) = match &thumb {
        Some(path) => (path.as_path(), "image/jpeg"),
        None => (shot.path.as_path(), image_type(&shot.name)),
    };
    match std::fs::read(path) {
        Ok(bytes) => Response::builder()
            .header("Content-Type", kind)
            .header("Cache-Control", "max-age=3600")
            .body(bytes)
            .unwrap_or_default(),
        Err(_) => status(404),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_urls_round_trip() {
        let url = screenshot_url("моя збірка", "2026 09+1.png", Some(5));
        let prefix = if cfg!(windows) { "http://shot.localhost" } else { "shot://localhost" };
        let rest = url.strip_prefix(prefix).unwrap();
        assert_eq!(rest, "/%D0%BC%D0%BE%D1%8F%20%D0%B7%D0%B1%D1%96%D1%80%D0%BA%D0%B0/2026%2009%2B1.png?v=5");
        let path = rest.split('?').next().unwrap();
        assert_eq!(parse_path(path), Some(("моя збірка".to_string(), "2026 09+1.png".to_string())));
        assert_eq!(screenshot_url("a", "b.png", None).rsplit('?').next(), Some("v=0"));
        for bad in ["", "/", "/only", "no-slash/x", "/a/%zz", "/%D0/x.png"] {
            assert_eq!(parse_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_thumbnail_is_asked_by_its_own_url() {
        let thumb = thumbnail_url("a", "b.png", Some(7));
        assert_eq!(thumb, format!("{}&thumb=1", screenshot_url("a", "b.png", Some(7))));
        let query = thumb.split_once('?').map(|(_, q)| q);
        assert!(wants_thumb(query));
        assert!(!wants_thumb(Some("v=7")) && !wants_thumb(None) && !wants_thumb(Some("v=1&thumb=0")));
    }
}
