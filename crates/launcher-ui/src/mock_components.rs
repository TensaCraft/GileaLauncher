//! Browser-preview backend for the Components page.

use launcher_shared::{
    AppError, ComponentDto, ComponentsSnapshot, ErrorCode, Level, LoaderKind, Text, VerifyOutcome,
};
use serde_json::{Value, json};

use crate::mock_builds::{arg, to_value, toast};

fn component(
    id: &str,
    loader: LoaderKind,
    mc: &str,
    lv: Option<&str>,
    megabytes: u64,
    used_by: &[&str],
) -> ComponentDto {
    ComponentDto {
        id: id.into(),
        loader: Some(loader),
        minecraft: Some(mc.into()),
        loader_version: lv.map(str::to_string),
        size: megabytes * 1024 * 1024,
        modified_ms: Some(1_790_000_000_000),
        used_by: used_by.iter().map(|s| s.to_string()).collect(),
        base_for: Vec::new(),
    }
}

fn not_found(id: &str) -> AppError {
    AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", id)
}

pub struct MockComponents {
    components: Vec<ComponentDto>,
}

impl MockComponents {
    pub fn new() -> MockComponents {
        MockComponents {
            components: vec![
                component("1.21.1", LoaderKind::Minecraft, "1.21.1", None, 27, &["Aeronautics"]),
                component(
                    "fabric-loader-0.16.9-1.21.1",
                    LoaderKind::Fabric,
                    "1.21.1",
                    Some("0.16.9"),
                    1,
                    &[],
                ),
                component("neoforge-21.1.209", LoaderKind::NeoForge, "1.21.1", Some("21.1.209"), 31, &[]),
                component("1.20.1", LoaderKind::Minecraft, "1.20.1", None, 24, &["Ванільна 1.20.1"]),
            ],
        }
    }

    /// Newest Minecraft first, each base listing what inherits from it.
    pub fn snapshot(&self) -> ComponentsSnapshot {
        let mut components = self.components.clone();
        for c in &mut components {
            let base_for = self
                .components
                .iter()
                .filter(|other| {
                    other.loader != Some(LoaderKind::Minecraft)
                        && other.minecraft.as_deref() == Some(c.id.as_str())
                })
                .map(|other| other.id.clone())
                .collect();
            c.base_for = if c.loader == Some(LoaderKind::Minecraft) { base_for } else { Vec::new() };
        }
        components.sort_by(|a, b| b.minecraft.cmp(&a.minecraft).then_with(|| a.id.cmp(&b.id)));
        ComponentsSnapshot { components }
    }

    pub fn install(&mut self, kind: LoaderKind, mc: &str, lv: Option<&str>) -> Result<(), AppError> {
        let id = match (kind, lv) {
            (LoaderKind::Minecraft, _) => mc.to_string(),
            (kind, Some(lv)) => kind.component_id(mc, lv),
            (_, None) => return Err(AppError::new(ErrorCode::InvalidInput, "mock").with_param("version", mc)),
        };
        if !self.components.iter().any(|c| c.id == mc) {
            self.components.push(component(mc, LoaderKind::Minecraft, mc, None, 25, &[]));
        }
        if !self.components.iter().any(|c| c.id == id) {
            self.components.push(component(&id, kind, mc, lv, 2, &[]));
        }
        Ok(())
    }

    /// NeoForge "needs" a repair, everything else is intact.
    pub fn verify(&self, id: &str) -> Result<VerifyOutcome, AppError> {
        let c = self.components.iter().find(|c| c.id == id).ok_or_else(|| not_found(id))?;
        Ok(if c.loader == Some(LoaderKind::NeoForge) {
            VerifyOutcome::Repaired
        } else {
            VerifyOutcome::Intact
        })
    }

    pub fn delete(&mut self, id: &str) -> Result<(), AppError> {
        let before = self.components.len();
        self.components.retain(|c| c.id != id);
        if self.components.len() == before { Err(not_found(id)) } else { Ok(()) }
    }

    pub fn handle(&mut self, cmd: &str, args: &Value) -> Option<Result<Value, AppError>> {
        let id = arg(args, "id").to_string();
        let done = |key: &str| toast(Level::Success, Text::key(key).param("version", id.clone()));
        let result = match cmd {
            "components_list" => to_value(self.snapshot()),
            "component_install" => {
                let kind: LoaderKind =
                    serde_json::from_value(args["loader"].clone()).unwrap_or(LoaderKind::Minecraft);
                let mc = arg(args, "mc").to_string();
                let lv = args["loaderVersion"].as_str().map(str::to_string);
                let installed = match &lv {
                    Some(lv) if kind != LoaderKind::Minecraft => kind.component_id(&mc, lv),
                    _ => mc.clone(),
                };
                self.install(kind, &mc, lv.as_deref()).and_then(|()| {
                    toast(
                        Level::Success,
                        Text::key("minecraft_components_install_complete").param("version", installed),
                    );
                    to_value(self.snapshot())
                })
            }
            "component_verify" => self.verify(&id).and_then(|outcome| {
                done(match outcome {
                    VerifyOutcome::Intact => "minecraft_components_verify_ok",
                    VerifyOutcome::Repaired => "minecraft_components_repair_complete",
                });
                to_value(outcome)
            }),
            "component_reinstall" => self.verify(&id).map(|_| {
                done("minecraft_components_reinstall_complete");
                Value::Null
            }),
            "component_delete" => self.delete(&id).and_then(|()| {
                done("minecraft_components_delete_complete");
                to_value(self.snapshot())
            }),
            "component_open_dir" => self.verify(&id).map(|_| json!(null)),
            _ => return None,
        };
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_listed_installed_verified_and_deleted() {
        let mut m = MockComponents::new();
        assert_eq!(m.snapshot().components.len(), 4);
        m.install(LoaderKind::Quilt, "1.21.1", Some("0.26.4")).unwrap();
        let ids: Vec<String> = m.snapshot().components.into_iter().map(|c| c.id).collect();
        assert!(ids.contains(&"quilt-loader-0.26.4-1.21.1".to_string()));
        assert!(m.snapshot().components[0].base_for.contains(&"quilt-loader-0.26.4-1.21.1".to_string()));
        assert_eq!(m.install(LoaderKind::Quilt, "1.21.1", None).unwrap_err().code, ErrorCode::InvalidInput);
        assert_eq!(m.verify("1.20.1").unwrap(), VerifyOutcome::Intact);
        assert_eq!(m.verify("neoforge-21.1.209").unwrap(), VerifyOutcome::Repaired);
        assert_eq!(m.verify("ghost").unwrap_err().code, ErrorCode::VersionNotFound);
        m.delete("fabric-loader-0.16.9-1.21.1").unwrap();
        assert!(!m.snapshot().components[0].base_for.iter().any(|id| id.starts_with("fabric")));
        assert_eq!(m.delete("ghost").unwrap_err().code, ErrorCode::VersionNotFound);
    }
}
