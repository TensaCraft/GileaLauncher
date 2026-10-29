//! The module's commands (`module_invoke("skins", …)`), a profile's head, and a reload after any
//! change (`CHANGED_EVENT`, `app://profiles`).

use std::cell::Cell;

use launcher_shared::{AppError, names};
use leptos::prelude::*;
use serde::Serialize;
use serde_json::Value;
use ui_kit::ipc;

use crate::dto::{
    Account, AccountState, ApplyArgs, CHANGED_EVENT, CapeArgs, IdArgs, ImportArgs, LibrarySkin, ProfileArgs,
    RenameArgs, SkinVariant, VariantArgs,
};

pub async fn accounts() -> Result<Vec<Account>, AppError> {
    ipc::module_invoke(crate::ID, "accounts", &Value::Null).await
}

pub async fn state(profile: &str) -> Result<AccountState, AppError> {
    ipc::module_invoke(crate::ID, "state", &ProfileArgs { profile: profile.into() }).await
}

pub async fn library() -> Result<Vec<LibrarySkin>, AppError> {
    ipc::module_invoke(crate::ID, "library", &Value::Null).await
}

pub async fn skin(id: &str) -> Result<String, AppError> {
    ipc::module_invoke(crate::ID, "skin", &IdArgs { id: id.into() }).await
}

pub async fn import(args: ImportArgs) -> Result<LibrarySkin, AppError> {
    ipc::module_invoke(crate::ID, "import", &args).await
}

pub async fn rename(id: String, name: String) -> Result<LibrarySkin, AppError> {
    ipc::module_invoke(crate::ID, "rename", &RenameArgs { id, name }).await
}

pub async fn set_variant(id: String, variant: SkinVariant) -> Result<LibrarySkin, AppError> {
    ipc::module_invoke(crate::ID, "set_variant", &VariantArgs { id, variant }).await
}

pub async fn delete(id: String) -> Result<Value, AppError> {
    ipc::module_invoke(crate::ID, "delete", &IdArgs { id }).await
}

pub async fn apply(profile: String, id: String) -> Result<AccountState, AppError> {
    ipc::module_invoke(crate::ID, "apply", &ApplyArgs { profile, id }).await
}

pub async fn set_cape(profile: String, cape: Option<String>) -> Result<AccountState, AppError> {
    ipc::module_invoke(crate::ID, "set_cape", &CapeArgs { profile, cape }).await
}

#[derive(Serialize)]
struct KeyArgs<'a> {
    key: &'a str,
}

/// Profile `key`'s head for an `<img src>`.
pub async fn avatar(key: &str) -> Option<String> {
    ipc::invoke::<_, Option<String>>("profile_avatar", &KeyArgs { key }).await.ok().flatten()
}

thread_local! {
    /// Grows after each change of the library, a skin or a cape (`CHANGED_EVENT`).
    static CHANGES: ArcRwSignal<u64> = ArcRwSignal::new(0);
    /// Grows after each change of the profiles (a sign-in, a profile gone).
    static PROFILES: ArcRwSignal<u64> = ArcRwSignal::new(0);
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

fn listen() {
    if !LISTENING.with(|l| l.replace(true)) {
        ipc::listen::<Value>(CHANGED_EVENT, |_| CHANGES.with(|r| r.update(|n| *n += 1)));
        ipc::listen::<Value>(names::PROFILES, |_| PROFILES.with(|r| r.update(|n| *n += 1)));
    }
}

/// Changes of the library, a skin or a cape, counted: the library loads again.
pub fn changes() -> ArcRwSignal<u64> {
    listen();
    CHANGES.with(Clone::clone)
}

/// Changes of the profiles, counted: the accounts load again.
pub fn profile_changes() -> ArcRwSignal<u64> {
    listen();
    PROFILES.with(Clone::clone)
}
