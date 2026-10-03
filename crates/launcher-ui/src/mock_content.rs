//! Browser-preview backend for a build's content: the same sample mods, resource packs
//! and shader packs for every build; mods are unsupported for the `vanilna…` builds.

use std::collections::HashMap;

use launcher_shared::{
    AppError, ContentItem, ContentKind, ContentList, ErrorCode, Level, ScreenshotDto, Text,
};
use serde_json::Value;

use crate::mock_builds::{arg, to_value, toast};

fn item(file: &str, size: u64, folder: bool) -> ContentItem {
    ContentItem {
        file: file.into(),
        filename: file.strip_suffix(".disabled").unwrap_or(file).into(),
        size,
        enabled: !file.ends_with(".disabled"),
        folder,
        toggle_supported: true,
        name: None,
        version: None,
        description: None,
        mod_id: None,
        has_backup: false,
    }
}

fn mod_item(file: &str, size: u64, id: &str, name: &str, version: &str, description: &str) -> ContentItem {
    ContentItem {
        name: Some(name.into()),
        version: Some(version.into()),
        description: Some(description.into()),
        mod_id: Some(id.into()),
        ..item(file, size, false)
    }
}

trait WithBackup {
    fn with_backup(self) -> Self;
}

impl WithBackup for ContentItem {
    fn with_backup(self) -> Self {
        ContentItem { has_backup: true, ..self }
    }
}

fn sample(kind: ContentKind) -> Vec<ContentItem> {
    match kind {
        ContentKind::Mods => vec![
            item("broken-mod.jar", 12_288, false),
            mod_item(
                "iris-fabric-1.8.0+mc1.21.1.jar",
                2_621_440,
                "iris",
                "Iris",
                "1.8.0+mc1.21.1",
                "A modern shader pack loader for Minecraft intended to be compatible with existing OptiFine shader packs",
            ),
            mod_item(
                "lithium-fabric-0.14.3+mc1.21.1.jar.disabled",
                723_000,
                "lithium",
                "Lithium",
                "0.14.3+mc1.21.1",
                "No-compromises game logic optimization mod",
            ),
            mod_item(
                "sodium-fabric-0.6.0+mc1.21.1.jar",
                1_153_433,
                "sodium",
                "Sodium",
                "0.6.0+mc1.21.1",
                "The fastest and most compatible rendering optimization mod for Minecraft",
            )
            .with_backup(),
        ],
        ContentKind::ResourcePacks => vec![
            item("Faithful 32x.zip", 8_912_896, false),
            ContentItem { enabled: false, ..item("Кастомні текстури", 3_145_728, true) },
        ],
        ContentKind::ShaderPacks => vec![
            ContentItem { enabled: false, ..item("BSL_v8.2.09.zip", 1_048_576, false) },
            item("ComplementaryReimagined_r5.2.2.zip", 2_097_152, false),
        ],
    }
}

fn shot(name: &str, size: u64, minutes_ago: u64, color: &str) -> ScreenshotDto {
    let src = format!(
        "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 9'%3E%3Crect width='16' height='9' fill='%23{color}'/%3E%3C/svg%3E"
    );
    ScreenshotDto {
        name: name.into(),
        size,
        modified_ms: Some(1_790_000_000_000 - minutes_ago * 60_000),
        thumb: src.clone(),
        src,
    }
}

fn sample_shots() -> Vec<ScreenshotDto> {
    vec![
        shot("2026-09-21_18.04.12.png", 2_457_600, 5, "17c3a2"),
        shot("2026-09-21_17.40.03.png", 3_145_728, 29, "2db8da"),
        shot("2026-09-20_21.12.55.jpg", 1_048_576, 1_260, "f2b544"),
    ]
}

fn not_found(file: &str) -> AppError {
    AppError::new(ErrorCode::NotFound, "mock").with_param("path", file)
}

fn announce((list, message): (ContentList, Option<Text>)) -> Result<Value, AppError> {
    if let Some(message) = message {
        toast(Level::Info, message);
    }
    to_value(list)
}

#[derive(Default)]
pub struct MockContent {
    lists: HashMap<(String, ContentKind), Vec<ContentItem>>,
    shots: HashMap<String, Vec<ScreenshotDto>>,
}

impl MockContent {
    pub fn screenshots(&mut self, key: &str) -> Vec<ScreenshotDto> {
        self.shots.entry(key.to_string()).or_insert_with(sample_shots).clone()
    }

    pub fn delete_screenshot(
        &mut self,
        key: &str,
        name: &str,
    ) -> Result<(Vec<ScreenshotDto>, Text), AppError> {
        self.screenshots(key);
        let shots = self.shots.get_mut(key).expect("listed above");
        let at = shots.iter().position(|s| s.name == name).ok_or_else(|| not_found(name))?;
        shots.remove(at);
        Ok((self.screenshots(key), Text::key("screenshot_deleted").param("name", name)))
    }

    pub fn new() -> MockContent {
        MockContent::default()
    }

    fn supported(key: &str, kind: ContentKind) -> bool {
        kind != ContentKind::Mods || !key.starts_with("vanilna")
    }

    fn items(&mut self, key: &str, kind: ContentKind) -> &mut Vec<ContentItem> {
        let supported = Self::supported(key, kind);
        self.lists
            .entry((key.to_string(), kind))
            .or_insert_with(|| if supported { sample(kind) } else { Vec::new() })
    }

    pub fn list(&mut self, key: &str, kind: ContentKind) -> ContentList {
        ContentList { kind, supported: Self::supported(key, kind), items: self.items(key, kind).clone() }
    }

    /// Switches `file` to `enable`; an item already so stays (like the backend).
    pub fn toggle(
        &mut self,
        key: &str,
        kind: ContentKind,
        file: &str,
        enable: bool,
    ) -> Result<(ContentList, Option<Text>), AppError> {
        let items = self.items(key, kind);
        let at = items.iter().position(|i| i.file == file).ok_or_else(|| not_found(file))?;
        if items[at].enabled == enable {
            return Ok((self.list(key, kind), None));
        }
        let on = enable;
        if kind == ContentKind::ShaderPacks && on {
            items.iter_mut().for_each(|i| i.enabled = false);
        }
        let item = &mut items[at];
        item.enabled = on;
        if kind == ContentKind::Mods {
            item.file = if on { item.filename.clone() } else { format!("{}.disabled", item.filename) };
        }
        let name = item.name.clone().unwrap_or_else(|| item.filename.clone());
        Ok((self.list(key, kind), Some(Text::key(kind.toggled_key(on)).param("name", name))))
    }

    pub fn restore(
        &mut self,
        key: &str,
        kind: ContentKind,
        file: &str,
    ) -> Result<(ContentList, Option<Text>), AppError> {
        let items = self.items(key, kind);
        let item = items
            .iter_mut()
            .find(|i| i.file == file && i.has_backup)
            .ok_or_else(|| AppError::new(ErrorCode::BackupNotFound, format!("no backup of {file}")))?;
        item.has_backup = false;
        let name = item.name.clone().unwrap_or_else(|| item.filename.clone());
        Ok((self.list(key, kind), Some(Text::key("mod_restored").param("name", name))))
    }

    pub fn delete(
        &mut self,
        key: &str,
        kind: ContentKind,
        file: &str,
    ) -> Result<(ContentList, Option<Text>), AppError> {
        let items = self.items(key, kind);
        let at = items.iter().position(|i| i.file == file).ok_or_else(|| not_found(file))?;
        let gone = items.remove(at);
        let name = gone.name.unwrap_or(gone.filename);
        Ok((self.list(key, kind), Some(Text::key(kind.deleted_key()).param("name", name))))
    }

    /// Answers `cmd`, or `None` when it is not a content command.
    pub fn handle(&mut self, cmd: &str, args: &Value) -> Option<Result<Value, AppError>> {
        let kind = serde_json::from_value::<ContentKind>(args["kind"].clone()).unwrap_or(ContentKind::Mods);
        let key = arg(args, "key");
        let result = match cmd {
            "content_list" => to_value(self.list(key, kind)),
            "content_toggle" => {
                let enable = args["enable"].as_bool().unwrap_or(true);
                self.toggle(key, kind, arg(args, "file"), enable).and_then(announce)
            }
            "content_delete" => self.delete(key, kind, arg(args, "file")).and_then(announce),
            "content_restore" => self.restore(key, kind, arg(args, "file")).and_then(announce),
            "content_open_dir" => Ok(Value::Null),
            "screenshots_list" => to_value(self.screenshots(key)),
            "screenshot_delete" => {
                self.delete_screenshot(key, arg(args, "name")).and_then(|(shots, message)| {
                    toast(Level::Info, message);
                    to_value(shots)
                })
            }
            "screenshot_open" | "screenshots_open_dir" => Ok(Value::Null),
            _ => return None,
        };
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mod_with_a_backup_restores_once() {
        let mut m = MockContent::new();
        let sodium = "sodium-fabric-0.6.0+mc1.21.1.jar";
        let has = |m: &mut MockContent| {
            m.list("aeronautics", ContentKind::Mods).items.iter().any(|i| i.file == sodium && i.has_backup)
        };
        assert!(has(&mut m));
        let (_, message) = m.restore("aeronautics", ContentKind::Mods, sodium).unwrap();
        assert_eq!(message, Some(Text::key("mod_restored").param("name", "Sodium")));
        assert!(!has(&mut m));
        let again = m.restore("aeronautics", ContentKind::Mods, sodium).unwrap_err();
        assert_eq!(again.code, ErrorCode::BackupNotFound);
    }

    #[test]
    fn content_switches_and_deletes_like_the_backend() {
        let mut m = MockContent::new();
        assert!(!m.list("vanilna_1_20_1", ContentKind::Mods).supported);
        let mods = m.list("aeronautics", ContentKind::Mods);
        assert!(mods.supported && mods.items.iter().any(|i| i.file.ends_with(".jar.disabled") && !i.enabled));
        let (after, _) =
            m.toggle("aeronautics", ContentKind::Mods, "sodium-fabric-0.6.0+mc1.21.1.jar", false).unwrap();
        assert!(
            after.items.iter().any(|i| i.file == "sodium-fabric-0.6.0+mc1.21.1.jar.disabled" && !i.enabled)
        );
        let (shaders, _) =
            m.toggle("aeronautics", ContentKind::ShaderPacks, "BSL_v8.2.09.zip", true).unwrap();
        let on: Vec<&str> = shaders.items.iter().filter(|i| i.enabled).map(|i| i.file.as_str()).collect();
        assert_eq!(on, ["BSL_v8.2.09.zip"], "one shader pack at a time");
        let (packs, _) = m.delete("aeronautics", ContentKind::ResourcePacks, "Faithful 32x.zip").unwrap();
        assert_eq!(packs.items.len(), 1);
        assert_eq!(
            m.delete("aeronautics", ContentKind::Mods, "../x.jar").unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn asking_for_the_current_state_changes_nothing() {
        let mut m = MockContent::new();
        let before = m.list("aeronautics", ContentKind::ShaderPacks);
        let (after, message) = m
            .toggle("aeronautics", ContentKind::ShaderPacks, "ComplementaryReimagined_r5.2.2.zip", true)
            .unwrap();
        assert_eq!((after, message), (before, None));
    }

    #[test]
    fn screenshots_are_listed_and_deleted_like_the_backend() {
        let mut m = MockContent::new();
        let shots = m.screenshots("aeronautics");
        assert_eq!(shots.len(), 3);
        assert!(shots.windows(2).all(|w| w[0].modified_ms >= w[1].modified_ms), "newest first");
        assert!(shots.iter().all(|s| s.src.starts_with("data:image/svg+xml,")));
        let (left, _) = m.delete_screenshot("aeronautics", &shots[0].name).unwrap();
        assert_eq!(left.len(), 2);
        assert_eq!(m.delete_screenshot("aeronautics", "../x.png").unwrap_err().code, ErrorCode::NotFound);
    }
}
