use launcher_shared::{AlertFile, Level};
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::use_i18n;
use ui_kit::{Button, Dialog, DialogFooter, DialogTone, Size, Variant, ipc, use_toasts};

use super::{has_contacts, use_support};
use crate::modules::use_module_parts;
use crate::store::use_store;

#[derive(serde::Serialize)]
struct PathArgs {
    path: String,
}

/// The command that opens `file`: a text file in its program, a folder in the file manager.
pub fn open_command(file: &AlertFile) -> &'static str {
    if file.folder { "open_path" } else { "open_text_file" }
}

/// Modal for backend warnings/errors (`app://alert`).
#[component]
pub fn AlertHost() -> impl IntoView {
    let store = use_store();
    let i18n = use_i18n();
    let toasts = use_toasts();
    let help = use_support();
    let parts = use_module_parts();
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(store.alert.get().is_some()));
    let contacts = has_contacts();
    let open_file = move |file: AlertFile| {
        let command = open_command(&file);
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<_, ()>(command, &PathArgs { path: file.path }).await {
                toasts.show(Level::Error, i18n.error(&e), None);
            }
        });
    };
    // The files the alert opens (a crash's report and logs), one button each.
    let files = move || {
        let files = store.alert.get().map(|a| a.files).unwrap_or_default();
        (!files.is_empty()).then(|| {
            view! {
                <div class="alert-files">
                    {files
                        .into_iter()
                        .map(|file| {
                            let icon = if file.folder { "folder_open" } else { "description" };
                            let label = i18n.text(&file.label);
                            view! {
                                <Button
                                    variant=Variant::Secondary
                                    size=Size::Sm
                                    icon=icon
                                    on_click=move |_| open_file(file.clone())
                                >
                                    {label}
                                </Button>
                            }
                        })
                        .collect_view()}
                </div>
            }
        })
    };
    view! {
        <Dialog
            open=open
            title=Signal::derive(move || store.alert.get().map(|a| i18n.text(&a.title)).unwrap_or_default())
            icon="warning_amber"
            tone=DialogTone::Warning
            on_close=Callback::new(move |_| store.alert.set(None))
        >
            <DialogFooter slot>
                // Modules' buttons (a report) for an alert the user can report.
                {move || {
                    store.alert.get().filter(|a| a.allow_report).map(|alert| {
                        parts.with(|p| p.alert_actions.iter().map(|a| (a.view)(alert.clone())).collect_view())
                    })
                }}
                {move || contacts.get().then(|| view! {
                    <Button variant=Variant::Secondary icon="support_agent" outlined=true on_click=move |_| help.open()>
                        {move || i18n.t("support_title")}
                    </Button>
                })}
                <Button variant=Variant::Ghost on_click=move |_| store.alert.set(None)>{move || i18n.t("close")}</Button>
            </DialogFooter>
            <p style="margin:0">{move || store.alert.get().map(|a| i18n.text(&a.message)).unwrap_or_default()}</p>
            {files}
        </Dialog>
    }
}

#[cfg(test)]
mod tests {
    use launcher_shared::Text;

    use super::*;

    #[test]
    fn a_folder_opens_in_the_file_manager_and_a_file_in_its_program() {
        let file = |folder| AlertFile { label: Text::key("x"), path: "p".into(), folder };
        assert_eq!(open_command(&file(true)), "open_path");
        assert_eq!(open_command(&file(false)), "open_text_file");
    }
}
