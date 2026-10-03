mod alert;
pub mod browser;
mod clipboard;
mod footer;
mod header;
mod ops;
mod quit;
pub mod sidebar;
mod support;
mod window_bar;

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};
use ui_kit::i18n::{I18nCtx, use_i18n};
use ui_kit::{Toasts, ipc, use_toasts};

pub use alert::AlertHost;
pub use clipboard::copy_text;
pub use header::{PageHeader, provide_header, use_header};
pub use quit::QuitConfirm;
pub use support::{SupportDialog, has_contacts, provide_support, use_support};

use crate::builds::launch::use_launch_flow;
use crate::store::use_store;
use footer::Footer;
use header::Header;
use sidebar::Sidebar;
use window_bar::{WindowControls, provide_window_drag};

#[derive(serde::Serialize)]
struct UrlArgs {
    url: String,
}

/// Opens https links in the system browser; failures become a toast.
/// Create it during component setup (`use_url_opener`), then call `open` from handlers.
#[derive(Clone, Copy)]
pub struct UrlOpener {
    toasts: Toasts,
    i18n: I18nCtx,
}

impl UrlOpener {
    pub fn open(&self, url: String) {
        let Self { toasts, i18n } = *self;
        spawn_local(async move {
            if ipc::invoke::<_, ()>("open_url", &UrlArgs { url }).await.is_err() {
                toasts.show(Level::Warning, i18n.t("support_open_failed"), None);
            }
        });
    }
}

pub fn use_url_opener() -> UrlOpener {
    UrlOpener { toasts: use_toasts(), i18n: use_i18n() }
}

#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let toasts = use_toasts();
    let location = use_location();
    let navigate = use_navigate();
    let in_setup = move || location.pathname.get() == "/setup";

    // Single place that talks to the router: pages call `store.go(path)`.
    Effect::new(move |_| {
        if let Some(path) = store.nav_to.get() {
            store.nav_to.set(None);
            navigate(&path, Default::default());
        }
    });

    Effect::new(move |_| {
        if let Some(setup) = store.setup.get()
            && setup.should_open
            && location.pathname.get_untracked() != "/setup"
        {
            store.go("/setup");
        }
    });

    // `--launch-version`: once the builds are known, Play the build it names.
    let flow = use_launch_flow();
    Effect::new(move |_| {
        if !store.builds_loaded.get() {
            return;
        }
        if let Some(id) = store.pending_launch.get() {
            store.pending_launch.set(None);
            if location.pathname.get_untracked() != "/setup" && !flow.start_external(&id) {
                toasts.show(Level::Warning, i18n.t("launcher_shortcut_failed"), Some(id));
            }
        }
    });

    // The width the content keeps for its scroll bar (`scrollbar-gutter: stable`), measured: the
    // stylesheet takes it from the right padding. Again on resize — another screen may draw
    // scroll bars at another width.
    let content = NodeRef::<leptos::html::Div>::new();
    let gutter = RwSignal::new(0);
    let measure = move || {
        if let Some(el) = content.get_untracked() {
            gutter.set((el.offset_width() - el.client_width()).max(0));
        }
    };
    Effect::new(move |_| {
        if content.get().is_some() {
            measure();
        }
    });
    let resize = window_event_listener(leptos::ev::resize, move |_| measure());
    on_cleanup(move || resize.remove());

    let drag = provide_window_drag();

    view! {
        <div
            class="launcher-app"
            class:is-compact=move || store.settings.get().compact_sidebar
            on:mousedown=move |ev| drag.press(ev)
            on:mousemove=move |ev| drag.moved(ev)
            on:mouseup=move |_| drag.release()
            on:dblclick=move |ev| drag.double(ev)
        >
            <Show when=move || !in_setup()>
                <Sidebar />
            </Show>
            <main class="main">
                <Header />
                <div class="content" node_ref=content style=move || format!("--gutter: {}px", gutter.get())>
                    {children()}
                </div>
                <Footer />
            </main>
            <WindowControls />
        </div>
    }
}
