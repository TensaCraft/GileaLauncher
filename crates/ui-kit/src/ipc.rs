//! Typed Tauri IPC. Outside Tauri (plain browser via `trunk serve`) calls go to a mock handler.

use std::cell::RefCell;
use std::rc::Rc;

use launcher_shared::AppError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    async fn tauri_listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> Result<JsValue, JsValue>;
}

pub type MockHandler = Rc<dyn Fn(&str, serde_json::Value) -> Result<serde_json::Value, AppError>>;
type MockListener = Rc<dyn Fn(serde_json::Value)>;

thread_local! {
    static MOCK: RefCell<Option<MockHandler>> = const { RefCell::new(None) };
    static MOCK_LISTENERS: RefCell<Vec<(String, MockListener)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Serialize)]
pub struct NoArgs {}

pub fn is_tauri() -> bool {
    web_sys::window()
        .map(|w| js_sys::Reflect::has(&w, &JsValue::from_str("__TAURI__")).unwrap_or(false))
        .unwrap_or(false)
}

pub fn set_mock(handler: MockHandler) {
    MOCK.with(|m| *m.borrow_mut() = Some(handler));
}

/// Delivers an event to mock listeners (browser preview only).
pub fn emit_mock(event: &str, payload: serde_json::Value) {
    let listeners: Vec<MockListener> = MOCK_LISTENERS
        .with(|l| l.borrow().iter().filter(|(e, _)| e == event).map(|(_, f)| f.clone()).collect());
    for f in listeners {
        f(payload.clone());
    }
}

pub async fn invoke<A: Serialize, R: DeserializeOwned>(cmd: &str, args: &A) -> Result<R, AppError> {
    if !is_tauri() {
        let handler = MOCK
            .with(|m| m.borrow().clone())
            .ok_or_else(|| AppError::internal(format!("no backend for {cmd}")))?;
        let value = serde_json::to_value(args).map_err(|e| AppError::internal(e.to_string()))?;
        let out = handler(cmd, value)?;
        return serde_json::from_value(out).map_err(|e| AppError::internal(format!("{cmd}: {e}")));
    }
    let js_args = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| AppError::internal(e.to_string()))?;
    match tauri_invoke(cmd, js_args).await {
        Ok(value) => {
            serde_wasm_bindgen::from_value(value).map_err(|e| AppError::internal(format!("{cmd}: {e}")))
        }
        Err(err) => Err(serde_wasm_bindgen::from_value::<AppError>(err.clone())
            .unwrap_or_else(|_| AppError::internal(format!("{cmd}: {err:?}")))),
    }
}

pub async fn call<R: DeserializeOwned>(cmd: &str) -> Result<R, AppError> {
    invoke(cmd, &NoArgs {}).await
}

#[derive(Serialize)]
struct ModuleCall<'a, A: Serialize> {
    module: &'a str,
    command: &'a str,
    args: &'a A,
}

/// Command `command` of module `module` (`module_invoke`).
pub async fn module_invoke<A: Serialize, R: DeserializeOwned>(
    module: &str,
    command: &str,
    args: &A,
) -> Result<R, AppError> {
    invoke("module_invoke", &ModuleCall { module, command, args }).await
}

/// Subscribes for the lifetime of the app.
pub fn listen<T: DeserializeOwned + 'static>(event: &'static str, handler: impl Fn(T) + 'static) {
    if !is_tauri() {
        let f: MockListener = Rc::new(move |v| {
            if let Ok(t) = serde_json::from_value(v) {
                handler(t);
            }
        });
        MOCK_LISTENERS.with(|l| l.borrow_mut().push((event.to_string(), f)));
        return;
    }
    let closure = Closure::<dyn FnMut(JsValue)>::new(move |ev: JsValue| {
        let payload = js_sys::Reflect::get(&ev, &JsValue::from_str("payload")).unwrap_or(JsValue::NULL);
        match serde_wasm_bindgen::from_value::<T>(payload) {
            Ok(value) => handler(value),
            Err(e) => web_sys::console::warn_1(&format!("bad payload for {event}: {e}").into()),
        }
    });
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = tauri_listen(event, &closure).await {
            web_sys::console::error_1(&e);
        }
        closure.forget();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_call_names_the_module_and_the_command() {
        #[derive(Serialize)]
        struct Args {
            key: &'static str,
        }
        let call = ModuleCall { module: "modrinth", command: "installed", args: &Args { key: "aero" } };
        assert_eq!(
            serde_json::to_value(call).unwrap(),
            serde_json::json!({"module": "modrinth", "command": "installed", "args": {"key": "aero"}})
        );
    }
}
