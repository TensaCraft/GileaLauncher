//! The 3D viewer: the vendored skinview3d bundle (`assets/vendor/skinview3d`), loaded once, on
//! the page's first visit; the 2D front view stands in when it or WebGL is missing.

use std::cell::RefCell;

use js_sys::{Array, Function, Object, Promise, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::Icon;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlCanvasElement;

const SCRIPT: &str = "/vendor/skinview3d/skinview3d.bundle.js";
const GLOBAL: &str = "skinview3d";
pub const WIDTH: u32 = 288;
pub const HEIGHT: u32 = 400;
/// How fast the figure walks: a stroll.
const WALK_SPEED: f64 = 0.55;

thread_local! {
    /// The script's load, shared by every viewer that waits for it.
    static LOADING: RefCell<Option<Promise>> = const { RefCell::new(None) };
}

/// `window.skinview3d`, once the script has run.
fn library() -> Option<JsValue> {
    let window: JsValue = web_sys::window()?.into();
    let lib = Reflect::get(&window, &GLOBAL.into()).ok()?;
    (!lib.is_undefined() && !lib.is_null()).then_some(lib)
}

/// A `<script>` for the bundle: `resolve` when it has run, `reject` when it cannot load.
fn add_script(resolve: &Function, reject: &Function) -> Result<(), JsValue> {
    let document = web_sys::window().and_then(|w| w.document()).ok_or(JsValue::NULL)?;
    let script = document.create_element("script")?.dyn_into::<web_sys::HtmlScriptElement>()?;
    script.set_src(SCRIPT);
    script.set_onload(Some(resolve));
    script.set_onerror(Some(reject));
    document.body().ok_or(JsValue::NULL)?.append_child(&script)?;
    Ok(())
}

fn load_script() -> Promise {
    Promise::new(&mut |resolve: Function, reject: Function| {
        if let Err(e) = add_script(&resolve, &reject) {
            let _ = reject.call1(&JsValue::NULL, &e);
        }
    })
}

/// Loads the viewer's script once; `Err` when it cannot be had.
pub async fn ensure_script() -> Result<(), ()> {
    if library().is_some() {
        return Ok(());
    }
    let promise = LOADING.with(|l| l.borrow_mut().get_or_insert_with(load_script).clone());
    let loaded = JsFuture::from(promise).await.is_ok() && library().is_some();
    if !loaded {
        // The next visit tries again.
        LOADING.with(|l| l.borrow_mut().take());
        return Err(());
    }
    Ok(())
}

fn call(target: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let function: Function = Reflect::get(target, &method.into())?.dyn_into()?;
    function.apply(target, &args.iter().collect::<Array>())
}

/// A texture that fails to load leaves the figure as it was, quietly.
fn quietly(answer: Result<JsValue, JsValue>) {
    if let Ok(promise) = answer {
        let _ = call(&promise, "catch", &[Closure::once_into_js(|_: JsValue| {})]);
    }
}

/// A skinview3d `SkinViewer` on a canvas; disposed when dropped.
pub struct SkinView {
    viewer: JsValue,
}

impl SkinView {
    pub fn new(canvas: &HtmlCanvasElement, width: u32, height: u32) -> Result<SkinView, JsValue> {
        let lib = library().ok_or(JsValue::NULL)?;
        let make: Function = Reflect::get(&lib, &"SkinViewer".into())?.dyn_into()?;
        let options = Object::new();
        Reflect::set(&options, &"canvas".into(), canvas)?;
        Reflect::set(&options, &"width".into(), &width.into())?;
        Reflect::set(&options, &"height".into(), &height.into())?;
        // Throws without WebGL.
        let viewer = Reflect::construct(&make, &Array::of1(&options))?;
        // Turned with the mouse, nothing else: the page scrolls as usual over it.
        if let Ok(controls) = Reflect::get(&viewer, &"controls".into()) {
            let _ = Reflect::set(&controls, &"enableZoom".into(), &false.into());
            let _ = Reflect::set(&controls, &"enablePan".into(), &false.into());
        }
        let _ = Reflect::set(&viewer, &"zoom".into(), &0.82.into());
        Ok(SkinView { viewer })
    }

    /// Wears `data_url` (`None`: no skin) on the classic or the slim model.
    pub fn set_skin(&self, data_url: Option<&str>, slim: bool) {
        let options = Object::new();
        let _ = Reflect::set(&options, &"model".into(), &(if slim { "slim" } else { "default" }).into());
        let skin = data_url.map(JsValue::from).unwrap_or(JsValue::NULL);
        quietly(call(&self.viewer, "loadSkin", &[skin, options.into()]));
    }

    /// Wears cape `data_url`, or none.
    pub fn set_cape(&self, data_url: Option<&str>) {
        let cape = data_url.map(JsValue::from).unwrap_or(JsValue::NULL);
        quietly(call(&self.viewer, "loadCape", &[cape]));
    }

    /// Walks, or stands still.
    pub fn set_walking(&self, on: bool) {
        let animation = on
            .then(|| {
                let lib = library()?;
                let walk: Function = Reflect::get(&lib, &"WalkingAnimation".into()).ok()?.dyn_into().ok()?;
                let animation = Reflect::construct(&walk, &Array::new()).ok()?;
                let _ = Reflect::set(&animation, &"speed".into(), &WALK_SPEED.into());
                Some(animation)
            })
            .flatten()
            .unwrap_or(JsValue::NULL);
        let _ = Reflect::set(&self.viewer, &"animation".into(), &animation);
    }
}

impl Drop for SkinView {
    fn drop(&mut self) {
        let _ = call(&self.viewer, "dispose", &[]);
    }
}

/// The figure: `skin` (its picture, slim) and `cape` in 3D, turning with the mouse, walking while
/// `walking`; `flat`, a front view, when there is no 3D.
#[component]
pub fn SkinStage(
    #[prop(into)] skin: Signal<Option<(String, bool)>>,
    #[prop(into)] cape: Signal<Option<String>>,
    #[prop(into)] walking: Signal<bool>,
    #[prop(into)] flat: Signal<Option<String>>,
) -> impl IntoView {
    let canvas = NodeRef::<leptos::html::Canvas>::new();
    let failed = RwSignal::new(false);
    let ready = RwSignal::new(false);
    let shown = StoredValue::new_local(None::<SkinView>);
    Effect::new(move |started: Option<bool>| {
        if started == Some(true) {
            return true;
        }
        let Some(element) = canvas.get() else { return false };
        spawn_local(async move {
            let made = match ensure_script().await {
                Ok(()) => SkinView::new(&element, WIDTH, HEIGHT).ok(),
                Err(()) => None,
            };
            match made {
                // The page may be gone by now: the viewer is then dropped (and disposed) here.
                Some(made) => {
                    if shown.try_set_value(Some(made)).is_none() {
                        let _ = ready.try_set(true);
                    }
                }
                None => {
                    let _ = failed.try_set(true);
                }
            }
        });
        true
    });
    let with_view = move |act: &dyn Fn(&SkinView)| {
        let _ = shown.try_with_value(|v| v.as_ref().map(act));
    };
    Effect::new(move |_| {
        let skin = skin.get();
        if ready.get() {
            with_view(&|v| {
                v.set_skin(skin.as_ref().map(|(url, _)| url.as_str()), skin.as_ref().is_some_and(|s| s.1))
            });
        }
    });
    Effect::new(move |_| {
        let cape = cape.get();
        if ready.get() {
            with_view(&|v| v.set_cape(cape.as_deref()));
        }
    });
    Effect::new(move |_| {
        let walking = walking.get();
        if ready.get() {
            with_view(&|v| v.set_walking(walking));
        }
    });
    view! {
        <div class="skins-stage__view">
            <canvas
                node_ref=canvas
                class="skins-stage__canvas"
                class:is-hidden=move || failed.get()
                width=WIDTH
                height=HEIGHT
            ></canvas>
            {move || failed.get().then(|| match flat.get() {
                Some(src) => view! { <img class="skins-stage__flat" src=src alt="" /> }.into_any(),
                None => view! { <div class="skins-stage__none"><Icon name="checkroom" outlined=true /></div> }.into_any(),
            })}
        </div>
    }
}
