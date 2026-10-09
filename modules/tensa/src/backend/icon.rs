//! The server's pictures with their version in the address: a picture the server replaces under
//! the same address gets a new address too, so the window fetches it instead of showing the copy
//! it kept.

use reqwest::Url;

use super::api::TensaApi;

/// The query key the launcher adds.
const KEY: &str = "iv";

/// A picture on the network (the server's), not one the player picked (kept as its bytes).
pub fn is_address(raw: &str) -> bool {
    let raw = raw.trim();
    raw.starts_with("https://") || raw.starts_with("http://")
}

/// The query pairs of `url` but the version the launcher added.
fn other_pairs(url: &Url) -> Vec<(String, String)> {
    url.query_pairs().filter(|(k, _)| k != KEY).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

/// `url` with version `version` in its query, replacing one added before.
pub fn versioned(url: &str, version: &str) -> String {
    let Ok(mut parsed) = Url::parse(url.trim()) else { return url.to_string() };
    let kept = other_pairs(&parsed);
    parsed.query_pairs_mut().clear().extend_pairs(kept).append_pair(KEY, version);
    parsed.to_string()
}

/// The server's picture `image` as the window should load it: with its version when it is an
/// address the server tells one for, else as it is.
pub async fn current(api: &TensaApi, image: Option<&str>) -> Option<String> {
    let image = image?;
    if !is_address(image) {
        return Some(image.to_string());
    }
    Some(match api.image_version(image).await {
        Some(version) => versioned(image, &version),
        None => image.to_string(),
    })
}

/// `url` without the version the launcher added.
pub fn unversioned(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url.trim()) else { return url.trim().to_string() };
    let kept = other_pairs(&parsed);
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_rides_in_the_query_and_comes_off_again() {
        let plain = "https://gigabait.uk/storage/launcher/icons/aero.png";
        let first = versioned(plain, "aaa-1");
        assert_eq!(first, format!("{plain}?iv=aaa-1"));
        assert_eq!(versioned(&first, "bbb-2"), format!("{plain}?iv=bbb-2"), "the old version is replaced");
        assert_eq!(unversioned(&first), plain);
        assert_eq!(unversioned(plain), plain);
        let own = "https://cdn.example/a.png?size=64";
        assert_eq!(versioned(own, "x"), "https://cdn.example/a.png?size=64&iv=x");
        assert_eq!(unversioned(&versioned(own, "x")), own, "the server's own query stays");
    }

    #[test]
    fn only_network_pictures_are_the_server_s() {
        assert!(is_address("https://gigabait.uk/a.png") && is_address(" http://127.0.0.1/a.png"));
        assert!(!is_address("iVBORw0KGgoAAA") && !is_address("data:image/png;base64,AA") && !is_address(""));
    }
}
