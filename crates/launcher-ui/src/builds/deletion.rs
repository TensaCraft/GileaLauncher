//! What deleting a build removes with it, alike from the Delete tab and the Delete dialog: its game
//! folder and each module's part (world backups), all on by default (owner, 2026-10-30).

use launcher_shared::Level;
use leptos::prelude::*;
use leptos::task::spawn_local;
use ui_kit::i18n::I18nCtx;
use ui_kit::module::DeleteOption;
use ui_kit::{Toasts, ipc};

use super::actions::BuildActions;

/// The choices of one delete form: the folder, and the modules' parts with a switch each.
#[derive(Clone, Copy)]
pub struct DeleteChoices {
    pub files: RwSignal<bool>,
    pub options: RwSignal<Vec<(DeleteOption, RwSignal<bool>)>>,
    owner: StoredValue<Option<Owner>>,
}

impl DeleteChoices {
    pub fn new() -> DeleteChoices {
        DeleteChoices {
            files: RwSignal::new(true),
            options: RwSignal::new(Vec::new()),
            owner: StoredValue::new(Owner::current()),
        }
    }

    /// Everything on again, with the parts `offered` now (the modules whose backend is there).
    pub fn reset(&self, offered: Vec<DeleteOption>) {
        self.files.set(true);
        let make = || RwSignal::new(true);
        let options = offered
            .into_iter()
            .map(|option| (option, self.owner.with_value(|o| o.as_ref().map_or_else(make, |o| o.with(make)))))
            .collect();
        self.options.set(options);
    }

    /// The modules' parts switched on.
    pub fn picked(&self) -> Vec<DeleteOption> {
        self.options.with_untracked(|all| {
            all.iter().filter(|(_, on)| on.get_untracked()).map(|(option, _)| *option).collect()
        })
    }
}

/// Deletes build `key` as `choices` say: the modules' parts first (one that fails keeps the build
/// for another try), then the build, with its folder while `choices.files` is on.
pub fn delete_build(
    choices: DeleteChoices,
    key: String,
    actions: BuildActions,
    toasts: Toasts,
    i18n: I18nCtx,
) {
    let (files, picked) = (choices.files.get_untracked(), choices.picked());
    spawn_local(async move {
        for option in picked {
            let args = serde_json::json!({ "key": key });
            if let Err(e) =
                ipc::module_invoke::<_, serde_json::Value>(option.module, option.command, &args).await
            {
                toasts.show(Level::Error, i18n.error(&e), None);
                return;
            }
        }
        actions.delete(key, files);
    });
}

impl Default for DeleteChoices {
    fn default() -> Self {
        DeleteChoices::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(module: &'static str) -> DeleteOption {
        DeleteOption { module, label: "label", desc: "desc", command: "delete_build" }
    }

    #[test]
    fn every_part_is_deleted_unless_switched_off() {
        let owner = Owner::new();
        owner.with(|| {
            let choices = DeleteChoices::new();
            choices.reset(vec![option("backups"), option("other")]);
            assert!(choices.files.get_untracked(), "the folder goes by default");
            let modules = |c: &DeleteChoices| c.picked().iter().map(|o| o.module).collect::<Vec<_>>();
            assert_eq!(modules(&choices), ["backups", "other"], "each module's part too");
            choices.files.set(false);
            choices.options.with_untracked(|all| all[0].1.set(false));
            assert_eq!(modules(&choices), ["other"]);
            choices.reset(vec![option("backups")]);
            assert!(choices.files.get_untracked(), "a new form starts all on again");
            assert_eq!(modules(&choices), ["backups"]);
        });
    }
}
