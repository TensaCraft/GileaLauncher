/// Puts `text` on the clipboard; false when the browser refused.
pub async fn copy_text(text: String) -> bool {
    let Some(window) = web_sys::window() else { return false };
    let promise = window.navigator().clipboard().write_text(&text);
    wasm_bindgen_futures::JsFuture::from(promise).await.is_ok()
}
