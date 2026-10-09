//! Browser-preview backend for builds, the Minecraft catalog, Java and memory.
//! `?builds=none` starts without builds, `?catalog=error` makes the catalog fail and
//! `?launch=noprofile` answers Play with `no_profile`.

use std::collections::HashMap;

use launcher_shared::{
    AppError, BuildDto, BuildSettingsDto, BuildSettingsUpdate, BuildsSnapshot, CatalogVersion, ErrorCode,
    GameEvent, GameState, JavaEntry, JavaList, Level, LoaderBuild, LoaderKind, LoaderOption, MemoryInfo,
    Text, Toast, names,
};
use serde_json::{Value, json};
use ui_kit::ipc;

use crate::builds::{NameProblem, check_new_name};
fn mock_build(key: &str, name: &str, version: &str) -> BuildDto {
    BuildDto {
        key: key.into(),
        version_id: key.into(),
        name: name.into(),
        version: Some(version.into()),
        loader: Some(version.into()),
        client: Some("Minecraft".into()),
        loader_version: None,
        game_dir: format!("C:\\Users\\Player\\AppData\\Roaming\\Launcher\\games\\{key}"),
        image: None,
        description: String::new(),
        running: false,
        profile: None,
    }
}

fn folder_id(name: &str, taken: &[BuildDto]) -> String {
    let base: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let base = if base.trim_matches('_').is_empty() { "build".to_string() } else { base };
    let used = |id: &str| taken.iter().any(|b| b.key == id);
    if !used(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}_{n}")).find(|id| !used(id)).expect("a free id exists")
}

pub(crate) fn arg<'a>(args: &'a Value, name: &str) -> &'a str {
    args[name].as_str().unwrap_or_default()
}

pub(crate) fn to_value<T: serde::Serialize>(value: T) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|e| AppError::internal(e.to_string()))
}

pub(crate) fn toast(level: Level, title: Text) {
    let toast = Toast { id: 0, level, title, message: None, action: None };
    if let Ok(value) = serde_json::to_value(toast) {
        ipc::emit_mock(names::TOAST, value);
    }
}

pub struct MockBuilds {
    builds: Vec<BuildDto>,
    custom_java: Vec<JavaEntry>,
    settings: HashMap<String, BuildSettingsDto>,
    catalog_fails: bool,
    no_profile: bool,
    /// The order the user dragged the builds into.
    order: Vec<String>,
}

const MOCK_LOADER_GAMES: [&str; 4] = ["1.21.1", "1.21", "1.20.6", "1.20.1"];
const MOCK_ICON: &str = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxNiI+PHJlY3Qgd2lkdGg9IjE2IiBoZWlnaHQ9IjE2IiBmaWxsPSIjMTdjM2EyIi8+PC9zdmc+";

impl MockBuilds {
    pub fn new(empty: bool, catalog_fails: bool) -> MockBuilds {
        let builds = if empty {
            Vec::new()
        } else {
            // Aeronautics runs Fabric (its mock content is Fabric mods and shader packs).
            let aeronautics = BuildDto {
                client: Some("Fabric".into()),
                loader: Some("fabric-loader-0.16.9-1.21.1".into()),
                loader_version: Some("0.16.9".into()),
                ..mock_build("aeronautics", "Aeronautics", "1.21.1")
            };
            vec![aeronautics, mock_build("vanilna_1_20_1", "Ванільна 1.20.1", "1.20.1")]
        };
        MockBuilds {
            builds,
            custom_java: Vec::new(),
            settings: HashMap::new(),
            catalog_fails,
            no_profile: false,
            order: Vec::new(),
        }
    }

    pub fn with_no_profile(mut self, no_profile: bool) -> MockBuilds {
        self.no_profile = no_profile;
        self
    }

    pub fn settings(&self, key: &str) -> Result<BuildSettingsDto, AppError> {
        if let Some(saved) = self.settings.get(key) {
            return Ok(saved.clone());
        }
        let build =
            self.builds.iter().find(|b| b.key == key).ok_or_else(|| {
                AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", key)
            })?;
        Ok(BuildSettingsDto {
            key: build.key.clone(),
            name: build.name.clone(),
            image: build.image.clone(),
            component: build.loader.clone(),
            java_path: None,
            auto_java: Some("C:\\Users\\Player\\AppData\\Roaming\\Launcher\\minecraft\\runtime\\java-runtime-delta\\bin\\javaw.exe".into()),
            gpu_mode: "auto".into(),
            max_ram_gb: None,
            jvm_arguments: Vec::new(),
            server_host: String::new(),
            server_port: None,
            profile: build.profile.clone(),
        })
    }

    pub fn save_settings(
        &mut self,
        key: &str,
        update: &BuildSettingsUpdate,
    ) -> Result<BuildSettingsDto, AppError> {
        let mut settings = self.settings(key)?;
        let name = update.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::new(ErrorCode::VersionNameEmpty, "mock"));
        }
        if self.builds.iter().any(|b| b.key != key && b.name.to_lowercase() == name.to_lowercase()) {
            return Err(AppError::new(ErrorCode::VersionExists, "mock").with_param("name", &name));
        }
        let port = update.server_port.trim();
        let server_port = match port {
            "" => None,
            raw => Some(raw.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| {
                AppError::new(ErrorCode::InvalidInput, "mock").with_param("error", "invalid_port")
            })?),
        };
        settings.name = name.clone();
        settings.component = update.component.clone();
        settings.java_path = update.java_path.clone();
        settings.gpu_mode = update.gpu_mode.clone();
        settings.max_ram_gb = update.max_ram_gb;
        settings.jvm_arguments = update.jvm_arguments.clone();
        settings.server_host = update.server_host.trim().to_string();
        settings.server_port = server_port.filter(|_| !settings.server_host.is_empty());
        settings.profile = update.profile.clone().filter(|p| !p.trim().is_empty());
        if update.remove_image {
            settings.image = None;
        }
        if update.image_path.is_some() {
            settings.image = Some(MOCK_ICON.to_string());
        }
        if let Some(build) = self.builds.iter_mut().find(|b| b.key == key) {
            build.name = name;
            build.image = settings.image.clone();
            build.profile = settings.profile.clone();
            if let Some(component) = &settings.component {
                build.loader = Some(component.clone());
            }
        }
        self.settings.insert(key.to_string(), settings.clone());
        Ok(settings)
    }

    pub fn change_component(
        &mut self,
        key: &str,
        kind: LoaderKind,
        mc: &str,
        lv: Option<&str>,
    ) -> Result<BuildSettingsDto, AppError> {
        let id = match (kind, lv) {
            (LoaderKind::Minecraft, _) => mc.to_string(),
            (kind, Some(lv)) => kind.component_id(mc, lv),
            (_, None) => return Err(AppError::new(ErrorCode::InvalidInput, "mock").with_param("version", mc)),
        };
        let mut settings = self.settings(key)?;
        let build = self.builds.iter_mut().find(|b| b.key == key).expect("settings found it");
        build.loader = Some(id.clone());
        build.version = Some(mc.to_string());
        build.client = Some(kind.display_name().to_string());
        build.loader_version = lv.filter(|_| kind != LoaderKind::Minecraft).map(str::to_string);
        settings.component = Some(id);
        self.settings.insert(key.to_string(), settings.clone());
        Ok(settings)
    }

    pub fn snapshot(&self) -> BuildsSnapshot {
        let mut builds = self.builds.clone();
        let position = |key: &str| self.order.iter().position(|k| k == key).unwrap_or(usize::MAX);
        builds.sort_by_key(|b| (position(&b.key), b.name.to_lowercase()));
        BuildsSnapshot { builds }
    }

    fn checked_name(&self, raw: &str) -> Result<String, AppError> {
        check_new_name(raw, &self.builds).map_err(|problem| match problem {
            NameProblem::Empty => AppError::new(ErrorCode::VersionNameEmpty, "mock"),
            NameProblem::Taken => {
                AppError::new(ErrorCode::VersionExists, "mock").with_param("name", raw.trim())
            }
        })
    }

    pub fn create(&mut self, name: &str, version: &str) -> Result<BuildDto, AppError> {
        let name = self.checked_name(name)?;
        let build = mock_build(&folder_id(&name, &self.builds), &name, version);
        self.builds.push(build.clone());
        Ok(build)
    }

    /// Makes build `key` one of a module's (its client), as a module's install does.
    #[cfg_attr(not(feature = "mod-tensa"), allow(dead_code))]
    pub fn set_client(&mut self, key: &str, client: &str) {
        if let Some(build) = self.builds.iter_mut().find(|b| b.key == key) {
            build.client = Some(client.into());
        }
    }

    pub fn create_loader(
        &mut self,
        name: &str,
        kind: LoaderKind,
        mc: &str,
        lv: &str,
    ) -> Result<BuildDto, AppError> {
        let mut build = self.create(name, mc)?;
        let stored = self.builds.iter_mut().find(|b| b.key == build.key).expect("just created");
        stored.client = Some(kind.display_name().into());
        stored.loader = Some(kind.component_id(mc, lv));
        stored.loader_version = Some(lv.into());
        build = stored.clone();
        Ok(build)
    }

    /// Fabric and Quilt share builds across releases; Forge and NeoForge have their own per
    /// release (Forge with a recommended one); prerelease builds join with `unstable`.
    pub fn loader_catalog(&self, kind: LoaderKind, unstable: bool) -> Vec<LoaderOption> {
        let row =
            |mc: &str, stable: &[&str], beta: Option<&str>, default: Option<&str>| -> Option<LoaderOption> {
                let mut builds: Vec<LoaderBuild> =
                    stable.iter().map(|v| LoaderBuild { version: v.to_string(), stable: true }).collect();
                if let Some(beta) = beta.filter(|_| unstable) {
                    builds.insert(0, LoaderBuild { version: beta.into(), stable: false });
                }
                let default_version =
                    default.map(str::to_string).or_else(|| builds.first().map(|b| b.version.clone()))?;
                Some(LoaderOption { mc: mc.into(), snapshot: false, builds, default_version })
            };
        let rows = match kind {
            LoaderKind::Forge => vec![
                row("1.21.1", &["52.1.16", "52.1.0"], None, Some("52.1.0")),
                row("1.20.1", &["47.4.23", "47.4.10"], None, Some("47.4.10")),
                row("1.12.2", &["14.23.5.2864", "14.23.5.2859"], None, Some("14.23.5.2859")),
            ],
            LoaderKind::NeoForge => vec![
                row("1.21.4", &[], Some("21.4.0-beta"), None),
                row("1.21.1", &["21.1.209", "21.1.77"], Some("21.1.210-beta"), Some("21.1.209")),
            ],
            LoaderKind::Quilt => MOCK_LOADER_GAMES
                .iter()
                .map(|mc| row(mc, &["0.26.4", "0.26.3"], Some("0.27.0-beta.1"), None))
                .collect(),
            _ => MOCK_LOADER_GAMES
                .iter()
                .map(|mc| row(mc, &["0.16.9", "0.16.8"], Some("0.17.0-beta.1"), None))
                .collect(),
        };
        rows.into_iter().flatten().collect()
    }

    pub fn copy(&mut self, key: &str, name: &str) -> Result<BuildDto, AppError> {
        let name = self.checked_name(name)?;
        let source =
            self.builds.iter().find(|b| b.key == key).cloned().ok_or_else(|| {
                AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", key)
            })?;
        let mut copy = source;
        copy.key = folder_id(&name, &self.builds);
        copy.version_id = copy.key.clone();
        copy.name = name;
        copy.running = false;
        self.builds.push(copy.clone());
        Ok(copy)
    }

    pub fn delete(&mut self, key: &str, delete_files: bool) -> Result<BuildsSnapshot, AppError> {
        let build =
            self.builds.iter().find(|b| b.key == key).ok_or_else(|| {
                AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", key)
            })?;
        if delete_files && build.running {
            return Err(AppError::new(ErrorCode::BuildRunning, "mock").with_param("version", &build.name));
        }
        self.builds.retain(|b| b.key != key);
        Ok(self.snapshot())
    }

    /// Marks `key` running or stopped; returns its name.
    pub fn set_running(&mut self, key: &str, running: bool) -> Option<String> {
        let build = self.builds.iter_mut().find(|b| b.key == key)?;
        build.running = running;
        Some(build.name.clone())
    }

    /// Releases 1.21.1 … 1.0 (and a snapshot before every x.y.0 when asked), newest first.
    pub fn catalog(&self, snapshots: bool) -> Result<Vec<CatalogVersion>, AppError> {
        if self.catalog_fails {
            return Err(AppError::new(ErrorCode::DownloadFailed, "mock")
                .with_param("error", "the catalog is offline"));
        }
        let mut list = Vec::new();
        for minor in (0..=21u32).rev() {
            for patch in (0..=4u32).rev() {
                let id = if patch == 0 { format!("1.{minor}") } else { format!("1.{minor}.{patch}") };
                let release_time = Some(format!("20{:02}-0{}-15T10:00:00+00:00", 11 + minor / 2, 1 + patch));
                list.push(CatalogVersion { id, kind: "release".into(), release_time });
            }
            if snapshots {
                list.push(CatalogVersion {
                    id: format!("{}w14a", 11 + minor),
                    kind: "snapshot".into(),
                    release_time: None,
                });
            }
        }
        Ok(list)
    }

    pub fn java_list(&self) -> JavaList {
        let found = |label: &str, path: &str| JavaEntry { label: label.into(), path: path.into() };
        JavaList {
            launcher: vec![
                found(
                    "Launcher Java 21.0.7 (java-runtime-delta)",
                    "C:\\Users\\Player\\AppData\\Roaming\\Launcher\\runtime\\java-runtime-delta\\bin\\javaw.exe",
                ),
                found(
                    "Eclipse Adoptium Java 21.0.3",
                    "C:\\Program Files\\Eclipse Adoptium\\jdk-21.0.3\\bin\\javaw.exe",
                ),
                found("Java 1.8.0_402", "C:\\Program Files\\Java\\jre1.8.0_402\\bin\\javaw.exe"),
            ],
            custom: self.custom_java.clone(),
        }
    }

    pub fn java_add(&mut self, label: &str, path: &str) -> Result<JavaList, AppError> {
        let path = path.trim().trim_matches('"').trim();
        let file = path.rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
        if !["java", "java.exe", "javaw", "javaw.exe", "minecraftjava.exe"].contains(&file.as_str()) {
            return Err(AppError::new(ErrorCode::InvalidJavaExecutable, "mock").with_param("path", path));
        }
        let label = match label.trim() {
            "" => path.rsplit(['/', '\\']).nth(2).unwrap_or("Java").to_string(),
            given => given.to_string(),
        };
        self.custom_java.retain(|e| e.path.to_lowercase() != path.to_lowercase());
        self.custom_java.push(JavaEntry { label, path: path.to_string() });
        Ok(self.java_list())
    }

    pub fn java_remove(&mut self, path: &str) -> JavaList {
        self.custom_java.retain(|e| e.path.to_lowercase() != path.to_lowercase());
        self.java_list()
    }

    pub fn java_scan(&mut self) -> JavaList {
        for found in self.java_list().launcher {
            if !self.custom_java.iter().any(|e| e.path.to_lowercase() == found.path.to_lowercase()) {
                self.custom_java.push(found);
            }
        }
        self.java_list()
    }

    /// The name of build `key`.
    #[cfg(feature = "mod-tensa")]
    pub fn name_of(&self, key: &str) -> Option<String> {
        self.builds.iter().find(|b| b.key == key).map(|b| b.name.clone())
    }

    pub fn emit_builds(&self) {
        if let Ok(value) = serde_json::to_value(self.snapshot()) {
            ipc::emit_mock(names::BUILDS, value);
        }
    }

    fn emit_game(&self, key: &str, name: String, state: GameState) {
        if let Ok(value) = serde_json::to_value(GameEvent { build_key: key.into(), build_name: name, state })
        {
            ipc::emit_mock(names::GAME, value);
        }
    }

    /// Answers `cmd`, or `None` when it is not a build, catalog, Java or memory command.
    pub fn handle(&mut self, cmd: &str, args: &Value) -> Option<Result<Value, AppError>> {
        let result = match cmd {
            "builds_list" => to_value(self.snapshot()),
            "builds_reorder" => {
                self.order = serde_json::from_value(args["keys"].clone()).unwrap_or_default();
                let snapshot = self.snapshot();
                self.emit_builds();
                to_value(snapshot)
            }
            "build_create_vanilla" => self.create(arg(args, "name"), arg(args, "version")).and_then(|b| {
                self.emit_builds();
                toast(Level::Success, Text::key("version_install_success").param("version", &b.name));
                to_value(b)
            }),
            "build_copy" => self.copy(arg(args, "key"), arg(args, "name")).and_then(|b| {
                self.emit_builds();
                toast(Level::Success, Text::key("version_copy_success").param("version", &b.name));
                to_value(b)
            }),
            "build_delete" => {
                self.delete(arg(args, "key"), args["deleteFiles"].as_bool().unwrap_or(true)).and_then(|s| {
                    self.emit_builds();
                    toast(Level::Success, Text::key("version_deleted"));
                    to_value(s)
                })
            }
            "build_launch" if self.no_profile => Err(AppError::new(ErrorCode::NoProfile, "mock")),
            "build_launch" => {
                let key = arg(args, "key").to_string();
                match self.set_running(&key, true) {
                    Some(name) => {
                        toast(Level::Info, Text::key("version_starting").param("version", &name));
                        self.emit_game(&key, name.clone(), GameState::Started { pid: 4242 });
                        self.emit_game(&key, name, GameState::Running { close_launcher: false });
                        self.emit_builds();
                        Ok(json!(4242))
                    }
                    None => {
                        Err(AppError::new(ErrorCode::VersionNotFound, "mock").with_param("version", &key))
                    }
                }
            }
            "build_stop" => {
                let key = arg(args, "key").to_string();
                let stopped = self.set_running(&key, false);
                if let Some(name) = stopped.clone() {
                    self.emit_game(&key, name, GameState::Exited { code: Some(0) });
                    self.emit_builds();
                }
                Ok(json!(usize::from(stopped.is_some())))
            }
            "build_open_dir" => Ok(Value::Null),
            "build_shortcut" => {
                toast(Level::Success, Text::key("desktop_shortcut_created"));
                Ok(json!("C:\\Users\\Player\\Desktop\\Aeronautics.lnk"))
            }
            "catalog_minecraft" => {
                self.catalog(args["snapshots"].as_bool().unwrap_or(false)).and_then(to_value)
            }
            "catalog_loader" => {
                let kind: LoaderKind =
                    serde_json::from_value(args["loader"].clone()).unwrap_or(LoaderKind::Fabric);
                to_value(self.loader_catalog(kind, args["unstable"].as_bool().unwrap_or(false)))
            }
            "build_create_loader" => {
                let kind: LoaderKind =
                    serde_json::from_value(args["loader"].clone()).unwrap_or(LoaderKind::Fabric);
                self.create_loader(arg(args, "name"), kind, arg(args, "mc"), arg(args, "loaderVersion"))
                    .and_then(|b| {
                        self.emit_builds();
                        toast(Level::Success, Text::key("version_install_success").param("version", &b.name));
                        to_value(b)
                    })
            }
            "memory_info" => {
                to_value(MemoryInfo { total_gb: 16, min_heap_gb: 1, max_heap_gb: 14, recommended_heap_gb: 6 })
            }
            "java_list" => to_value(self.java_list()),
            "java_add" => self.java_add(arg(args, "label"), arg(args, "path")).and_then(|list| {
                toast(Level::Success, Text::key("custom_java_added"));
                to_value(list)
            }),
            "java_remove" => to_value(self.java_remove(arg(args, "path"))),
            "java_scan" => {
                let before = self.custom_java.len();
                let list = self.java_scan();
                let added = list.custom.len() - before;
                let title = if added == 0 {
                    Text::key("custom_java_scan_no_new")
                } else {
                    Text::key("custom_java_scan_added").param("count", added.to_string())
                };
                toast(if added == 0 { Level::Info } else { Level::Success }, title);
                to_value(list)
            }
            "build_settings_get" => self.settings(arg(args, "key")).and_then(to_value),
            "build_settings_save" => {
                let update: BuildSettingsUpdate =
                    serde_json::from_value(args["update"].clone()).unwrap_or_default();
                self.save_settings(arg(args, "key"), &update).and_then(|saved| {
                    self.emit_builds();
                    toast(Level::Success, Text::key("version_updated"));
                    to_value(saved)
                })
            }
            "build_change_component" => {
                let kind: LoaderKind =
                    serde_json::from_value(args["loader"].clone()).unwrap_or(LoaderKind::Minecraft);
                let mc = arg(args, "mc").to_string();
                let lv = args["loaderVersion"].as_str().map(str::to_string);
                self.change_component(arg(args, "key"), kind, &mc, lv.as_deref()).and_then(|settings| {
                    self.emit_builds();
                    toast(
                        Level::Success,
                        Text::key("version_component_install_complete")
                            .param("loader", kind.display_name())
                            .param("version", mc),
                    );
                    to_value(settings)
                })
            }
            "pick_image_file" => Ok(json!("C:\\Users\\Player\\Pictures\\icon.png")),
            "pick_java_file" => Ok(json!("C:\\Program Files\\Java\\jdk-21\\bin\\javaw.exe")),
            _ => return None,
        };
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_are_created_copied_and_deleted_like_the_backend() {
        let mut m = MockBuilds::new(false, false);
        assert_eq!(m.snapshot().builds.len(), 2);
        let created = m.create(" Нова ", "1.21.1").unwrap();
        assert_eq!((created.name.as_str(), created.version.as_deref()), ("Нова", Some("1.21.1")));
        assert_eq!(m.create("НОВА", "1.21.1").unwrap_err().code, ErrorCode::VersionExists);
        assert_eq!(m.create("  ", "1.21.1").unwrap_err().code, ErrorCode::VersionNameEmpty);
        let copy = m.copy(&created.key, "Нова (копія)").unwrap();
        assert_ne!(copy.key, created.key);
        assert_eq!(m.copy("ghost", "X").unwrap_err().code, ErrorCode::VersionNotFound);
        m.set_running(&copy.key, true);
        assert_eq!(m.delete(&copy.key, true).unwrap_err().code, ErrorCode::BuildRunning);
        assert!(m.delete(&copy.key, false).is_ok());
        let names: Vec<String> = m.snapshot().builds.into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["Aeronautics", "Ванільна 1.20.1", "Нова"], "sorted by name, ignoring case");
    }

    #[test]
    fn the_catalog_pages_and_fails_on_request() {
        let m = MockBuilds::new(false, false);
        let releases = m.catalog(false).unwrap();
        assert!(releases.len() > 80, "enough rows for Load more");
        assert!(releases.iter().all(|v| v.kind == "release"));
        assert!(m.catalog(true).unwrap().iter().any(|v| v.kind == "snapshot"));
        assert_eq!(MockBuilds::new(false, true).catalog(false).unwrap_err().code, ErrorCode::DownloadFailed);
        assert!(MockBuilds::new(true, false).snapshot().builds.is_empty());
    }

    #[test]
    fn custom_java_is_validated_and_scanned() {
        let mut m = MockBuilds::new(false, false);
        assert_eq!(
            m.java_add("x", "C:/Windows/notepad.exe").unwrap_err().code,
            ErrorCode::InvalidJavaExecutable
        );
        let list = m.java_add("", "C:/jdk-21/bin/javaw.exe").unwrap();
        assert_eq!(list.custom.len(), 1);
        assert_eq!(m.java_add("Моя", "C:/jdk-21/bin/javaw.exe").unwrap().custom[0].label, "Моя");
        assert_eq!(m.java_remove("c:/JDK-21/bin/javaw.exe").custom.len(), 0);
        assert_eq!(m.java_scan().custom.len(), m.java_list().launcher.len());
        assert!(m.handle("memory_info", &json!({})).unwrap().is_ok());
        assert!(m.handle("no_such_command", &json!({})).is_none());
    }

    #[test]
    fn loader_builds_are_offered_and_created() {
        let mut m = MockBuilds::new(false, false);
        let stable = m.loader_catalog(LoaderKind::Fabric, false);
        assert!(!stable.is_empty() && stable.iter().all(|o| o.builds.iter().all(|b| b.stable)));
        assert!(m.loader_catalog(LoaderKind::Quilt, true)[0].builds.iter().any(|b| !b.stable));
        let build = m.create_loader("Fabric 1.21.1", LoaderKind::Fabric, "1.21.1", "0.16.9").unwrap();
        assert_eq!(build.client.as_deref(), Some("Fabric"));
        assert_eq!(build.loader.as_deref(), Some("fabric-loader-0.16.9-1.21.1"));
        assert_eq!(build.loader_version.as_deref(), Some("0.16.9"));
    }

    #[test]
    fn forge_and_neoforge_offer_builds_per_release() {
        let m = MockBuilds::new(false, false);
        let forge = m.loader_catalog(LoaderKind::Forge, false);
        assert_eq!(
            (forge[1].mc.as_str(), forge[1].default_version.as_str(), forge[1].builds.len()),
            ("1.20.1", "47.4.10", 2),
            "the recommended build is the default"
        );
        assert_eq!(m.loader_catalog(LoaderKind::NeoForge, false).len(), 1, "1.21.4 has only a beta");
        let neo = m.loader_catalog(LoaderKind::NeoForge, true);
        assert_eq!(
            (neo[0].default_version.as_str(), neo[1].default_version.as_str()),
            ("21.4.0-beta", "21.1.209")
        );
    }

    #[test]
    fn build_settings_are_read_and_saved_like_the_backend() {
        let mut m = MockBuilds::new(false, false);
        let key = m.snapshot().builds[0].key.clone();
        let settings = m.settings(&key).unwrap();
        assert_eq!((settings.gpu_mode.as_str(), settings.java_path.as_ref()), ("auto", None));
        let mut update = BuildSettingsUpdate {
            name: "Нова назва".into(),
            component: settings.component.clone(),
            gpu_mode: "igpu".into(),
            max_ram_gb: Some(6),
            ..BuildSettingsUpdate::default()
        };
        let saved = m.save_settings(&key, &update).unwrap();
        assert_eq!(
            (saved.name.as_str(), saved.gpu_mode.as_str(), saved.max_ram_gb),
            ("Нова назва", "igpu", Some(6))
        );
        assert!(m.snapshot().builds.iter().any(|b| b.name == "Нова назва"));
        update.server_port = "abc".into();
        update.server_host = "a".into();
        assert_eq!(m.save_settings(&key, &update).unwrap_err().code, ErrorCode::InvalidInput);
        update.server_port.clear();
        update.name = m.snapshot().builds.iter().find(|b| b.key != key).unwrap().name.to_uppercase();
        assert_eq!(m.save_settings(&key, &update).unwrap_err().code, ErrorCode::VersionExists);
    }

    #[test]
    fn a_build_changes_its_component_in_the_mock() {
        let mut m = MockBuilds::new(false, false);
        let key = m.snapshot().builds[0].key.clone();
        let changed = m.change_component(&key, LoaderKind::Fabric, "1.21.1", Some("0.16.9")).unwrap();
        assert_eq!(changed.component.as_deref(), Some("fabric-loader-0.16.9-1.21.1"));
        let build = m.snapshot().builds.into_iter().find(|b| b.key == key).unwrap();
        assert_eq!(
            (build.client.as_deref(), build.loader_version.as_deref()),
            (Some("Fabric"), Some("0.16.9"))
        );
        assert_eq!(
            m.change_component(&key, LoaderKind::Forge, "1.21.1", None).unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
}
