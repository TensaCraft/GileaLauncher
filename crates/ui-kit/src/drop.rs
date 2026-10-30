//! Files dropped on the window from the system. The window takes no file drop of its own (the
//! build list's drag to reorder needs the page's), so a `DropZone` reads what lands on it and the
//! launcher keeps each file under its cache (`drop_stage`): the page gets their paths.

use launcher_shared::{AppError, ErrorCode, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{DragEvent, File};

use crate::components::{Icon, use_toasts};
use crate::i18n::use_i18n;
use crate::ipc;

/// The header that names a dropped file, percent-encoded: a header takes ASCII only.
pub const NAME_HEADER: &str = "x-drop-name";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke_raw(cmd: &str, body: JsValue, options: JsValue) -> Result<JsValue, JsValue>;
}

/// The endings of the files a zone takes, lower case and without the dot: fixed (`&["jar"]`) or
/// following what the page offers.
#[derive(Debug, Clone, Copy)]
pub struct Endings(Signal<Vec<String>>);

impl<const N: usize> From<&'static [&'static str; N]> for Endings {
    fn from(endings: &'static [&'static str; N]) -> Endings {
        Endings::from(&endings[..])
    }
}

impl From<&'static [&'static str]> for Endings {
    fn from(endings: &'static [&'static str]) -> Endings {
        Endings(Signal::stored(endings.iter().map(|e| e.to_string()).collect()))
    }
}

impl From<Signal<Vec<String>>> for Endings {
    fn from(endings: Signal<Vec<String>>) -> Endings {
        Endings(endings)
    }
}

/// Whether file `name` ends with one of `endings` (lower case, without the dot), in any case.
pub fn takes(name: &str, endings: &[impl AsRef<str>]) -> bool {
    let name = name.to_ascii_lowercase();
    endings.iter().any(|ending| {
        name.strip_suffix(ending.as_ref())
            .and_then(|rest| rest.strip_suffix('.'))
            .is_some_and(|stem| !stem.is_empty())
    })
}

/// Whether the drag carries files from the system, not one of the page's own drags.
fn carries_files(ev: &DragEvent) -> bool {
    ev.data_transfer()
        .is_some_and(|data| data.types().iter().any(|t| t.as_string().as_deref() == Some("Files")))
}

/// The largest file a drop takes: the page holds it whole while it hands it over.
const MAX_BYTES: f64 = 2.0 * 1024.0 * 1024.0 * 1024.0;

/// Whether a file of `size` bytes can be dropped: something, and no more than `MAX_BYTES`.
pub fn fits(size: f64) -> bool {
    size > 0.0 && size <= MAX_BYTES
}

fn dropped(ev: &DragEvent) -> Vec<File> {
    let Some(list) = ev.data_transfer().and_then(|data| data.files()) else { return Vec::new() };
    (0..list.length()).filter_map(|i| list.get(i)).collect()
}

/// Keeps `file` under the launcher's cache and gives its path (in the browser preview the mock
/// answers with a made-up one).
pub async fn stage(file: &File) -> Result<String, AppError> {
    let name = file.name();
    if !fits(file.size()) {
        return Err(AppError::new(ErrorCode::InvalidInput, format!("{name} is too large to drop"))
            .with_param("name", name));
    }
    let buffer = JsFuture::from(file.array_buffer())
        .await
        .map_err(|e| AppError::internal(format!("{name}: {e:?}")))?;
    let bytes = js_sys::Uint8Array::new(&buffer);
    if !ipc::is_tauri() {
        let args = serde_json::json!({ "name": name, "size": bytes.length() });
        return ipc::invoke("drop_stage", &args).await;
    }
    let headers = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&headers, &NAME_HEADER.into(), &js_sys::encode_uri_component(&name));
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"headers".into(), &headers);
    match tauri_invoke_raw("drop_stage", bytes.into(), options.into()).await {
        Ok(path) => path.as_string().ok_or_else(|| AppError::internal("drop_stage: no path")),
        Err(err) => Err(serde_wasm_bindgen::from_value::<AppError>(err.clone())
            .unwrap_or_else(|_| AppError::internal(format!("drop_stage: {err:?}")))),
    }
}

/// Keeps a file dropped outside every zone from being opened by the window: the web view would
/// show it in place of the launcher. Called once.
pub fn guard_window() {
    let Some(window) = web_sys::window() else { return };
    for kind in ["dragover", "drop"] {
        let listener = Closure::<dyn FnMut(DragEvent)>::new(|ev: DragEvent| {
            // A zone that takes the drag has said so already.
            if carries_files(&ev) && !ev.default_prevented() {
                ev.prevent_default();
                if let Some(data) = ev.data_transfer() {
                    data.set_drop_effect("none");
                }
            }
        });
        let _ = window.add_event_listener_with_callback(kind, listener.as_ref().unchecked_ref());
        listener.forget();
    }
}

/// Takes files dropped on `children` from the system: while some are dragged over, a veil says
/// `label`; those with one of `accept`'s endings are kept (`stage`) and handed to `on_drop` by
/// path, in the order dropped. A drop of none of them says so.
#[component]
pub fn DropZone(
    /// The endings taken, lower case and without the dot (`&["jar"]`); none: the zone takes no drop.
    #[prop(into)]
    accept: Endings,
    #[prop(into)] label: Signal<String>,
    on_drop: Callback<Vec<String>>,
    /// While true, a drop here is left to the window, which takes none.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let i18n = use_i18n();
    let toasts = use_toasts();
    // `dragenter` and `dragleave` come for each child crossed: the veil shows while more came in
    // than left.
    let depth = RwSignal::new(0_i32);
    let Endings(accept) = accept;
    let wants = move |ev: &DragEvent| {
        !disabled.get_untracked() && accept.with_untracked(|a| !a.is_empty()) && carries_files(ev)
    };
    let on_enter = move |ev: DragEvent| {
        if wants(&ev) {
            ev.prevent_default();
            depth.update(|d| *d += 1);
        }
    };
    let on_over = move |ev: DragEvent| {
        if wants(&ev) {
            ev.prevent_default();
            if let Some(data) = ev.data_transfer() {
                data.set_drop_effect("copy");
            }
        }
    };
    let on_leave = move |ev: DragEvent| {
        if wants(&ev) {
            depth.update(|d| *d = (*d - 1).max(0));
        }
    };
    let on_drop_files = move |ev: DragEvent| {
        depth.set(0);
        // A zone inside this one has taken it already.
        if !wants(&ev) || ev.default_prevented() {
            return;
        }
        ev.prevent_default();
        let (taken, skipped): (Vec<File>, Vec<File>) =
            dropped(&ev).into_iter().partition(|file| accept.with_untracked(|a| takes(&file.name(), a)));
        if taken.is_empty() {
            if !skipped.is_empty() {
                toasts.show(Level::Warning, i18n.t("drop_not_taken"), None);
            }
            return;
        }
        spawn_local(async move {
            let mut paths = Vec::new();
            for file in &taken {
                match stage(file).await {
                    Ok(path) => paths.push(path),
                    Err(e) => toasts.show(Level::Error, i18n.t("drop_failed"), Some(i18n.error(&e))),
                }
            }
            // The page may be gone by now.
            if !paths.is_empty() {
                let _ = on_drop.try_run(paths);
            }
        });
    };
    view! {
        <div
            class=format!("drop-zone {class}")
            class:is-over=Signal::derive(move || depth.get() > 0)
            on:dragenter=on_enter
            on:dragover=on_over
            on:dragleave=on_leave
            on:drop=on_drop_files
        >
            {children()}
            <div class="drop-zone__veil" aria-hidden="true">
                <div class="drop-zone__label">
                    <Icon name="file_download" size=22 />
                    <span>{move || label.get()}</span>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_takes_files_by_their_ending() {
        assert!(takes("sodium-0.6.jar", &["jar"]));
        assert!(takes("Pack.MRPACK", &["mrpack", "zip"]));
        assert!(takes("faithful.zip", &["mrpack", "zip"]));
        assert!(!takes("skin.png.txt", &["png"]));
        assert!(!takes(".jar", &["jar"]), "an ending alone names no file");
        assert!(!takes("modjar", &["jar"]));
        assert!(!takes("anything.png", &[] as &[&str]));
        assert!(takes("Pack.zip", &["mrpack".to_string(), "zip".to_string()]), "endings a page gives");
    }

    #[test]
    fn a_drop_is_something_and_at_most_two_gigabytes() {
        assert!(fits(1.0) && fits(MAX_BYTES));
        assert!(!fits(0.0), "an empty file is nothing to add");
        assert!(!fits(MAX_BYTES + 1.0));
    }
}
