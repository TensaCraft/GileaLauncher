use serde::{Deserialize, Serialize};

use crate::Text;

pub const SUPPORTED_LANGS: &[&str] = &["uk_UA", "en_US"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsInfo {
    pub app_state_dir: String,
    pub minecraft_dir: String,
    pub cache_dir: String,
    pub log_dir: String,
    /// The launcher log file itself.
    #[serde(default)]
    pub log_file: String,
}

/// A launcher log record's level (`tracing` levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// One record of the launcher log; the lines after its first belong to it too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// `2026-09-27 12:00:00`; empty for text that came before any record.
    pub time: String,
    pub level: Option<LogLevel>,
    pub message: String,
}

/// The newest records of the launcher log, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogView {
    pub file: String,
    pub entries: Vec<LogEntry>,
    /// Older records left out.
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub profile: String,
    pub dev_mode: bool,
    pub os: String,
    pub support_url: Option<String>,
    /// Where bugs and suggestions go (the repository's issues).
    #[serde(default)]
    pub issues_url: Option<String>,
    pub updates_configured: bool,
    /// `None` when updates come from api.github.com, otherwise the (test) API address.
    pub update_source: Option<String>,
    pub modules: Vec<ModuleInfo>,
    /// The content providers this build has, in module order.
    #[serde(default)]
    pub providers: Vec<crate::provider::ProviderInfo>,
    pub paths: PathsInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickSound {
    TypewriterSoftClick,
    #[default]
    GateLatchClick,
    PlasticBubbleClick,
    // Made by `tools/sounds/generate.py`.
    SoftPop,
    GlassTap,
    WaterDrop,
    MechanicalKey,
    DigitalBlip,
    WoodTap,
    CosmicZap,
}

impl ClickSound {
    pub const ALL: [ClickSound; 10] = [
        ClickSound::TypewriterSoftClick,
        ClickSound::GateLatchClick,
        ClickSound::PlasticBubbleClick,
        ClickSound::SoftPop,
        ClickSound::GlassTap,
        ClickSound::WaterDrop,
        ClickSound::MechanicalKey,
        ClickSound::DigitalBlip,
        ClickSound::WoodTap,
        ClickSound::CosmicZap,
    ];

    pub fn as_config_str(self) -> &'static str {
        match self {
            ClickSound::TypewriterSoftClick => "typewriter_soft_click",
            ClickSound::GateLatchClick => "gate_latch_click",
            ClickSound::PlasticBubbleClick => "plastic_bubble_click",
            ClickSound::SoftPop => "soft_pop",
            ClickSound::GlassTap => "glass_tap",
            ClickSound::WaterDrop => "water_drop",
            ClickSound::MechanicalKey => "mechanical_key",
            ClickSound::DigitalBlip => "digital_blip",
            ClickSound::WoodTap => "wood_tap",
            ClickSound::CosmicZap => "cosmic_zap",
        }
    }

    pub fn from_config_str(value: &str) -> Self {
        Self::ALL.into_iter().find(|s| s.as_config_str() == value).unwrap_or_default()
    }

    /// Translation key of the human label.
    pub fn label_key(self) -> &'static str {
        match self {
            ClickSound::TypewriterSoftClick => "click_sound_typewriter",
            ClickSound::GateLatchClick => "click_sound_gate_latch",
            ClickSound::PlasticBubbleClick => "click_sound_plastic_bubble",
            ClickSound::SoftPop => "click_sound_soft_pop",
            ClickSound::GlassTap => "click_sound_glass_tap",
            ClickSound::WaterDrop => "click_sound_water_drop",
            ClickSound::MechanicalKey => "click_sound_mechanical_key",
            ClickSound::DigitalBlip => "click_sound_digital_blip",
            ClickSound::WoodTap => "click_sound_wood_tap",
            ClickSound::CosmicZap => "click_sound_cosmic_zap",
        }
    }

    /// File name under `assets/sounds/`.
    pub fn file_name(self) -> &'static str {
        match self {
            ClickSound::TypewriterSoftClick => "typewriter-soft-click.wav",
            ClickSound::GateLatchClick => "gate-latch-click.wav",
            ClickSound::PlasticBubbleClick => "plastic-bubble-click.wav",
            ClickSound::SoftPop => "soft-pop-click.wav",
            ClickSound::GlassTap => "glass-tap-click.wav",
            ClickSound::WaterDrop => "water-drop-click.wav",
            ClickSound::MechanicalKey => "mechanical-key-click.wav",
            ClickSound::DigitalBlip => "digital-blip-click.wav",
            ClickSound::WoodTap => "wood-tap-click.wav",
            ClickSound::CosmicZap => "cosmic-zap-click.wav",
        }
    }
}

/// What the launcher does once the game it started runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameStartAction {
    /// Stays as it is.
    #[default]
    Nothing,
    /// Quits.
    Close,
    /// Hides into an icon in the system tray.
    Tray,
}

impl GameStartAction {
    pub fn as_config_str(self) -> &'static str {
        match self {
            GameStartAction::Nothing => "nothing",
            GameStartAction::Close => "close",
            GameStartAction::Tray => "tray",
        }
    }

    pub fn from_config_str(raw: &str) -> Option<GameStartAction> {
        match raw {
            "nothing" => Some(GameStartAction::Nothing),
            "close" => Some(GameStartAction::Close),
            "tray" => Some(GameStartAction::Tray),
            _ => None,
        }
    }
}

/// What the launcher's own title bar asks of its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowAction {
    /// Nothing: only whether the window fills the screen.
    Look,
    Minimize,
    /// A window filling the screen back to its size, any other maximized.
    Maximize,
    Close,
    /// Hidden into an icon in the system tray.
    Tray,
    /// Moved with the mouse, its button held down.
    Drag,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SettingsSnapshot {
    pub lang: String,
    pub auto_update: bool,
    pub include_beta_updates: bool,
    /// What the launcher does once the game it started runs.
    #[serde(default)]
    pub on_game_start: GameStartAction,
    pub ask_profile_on_launch: bool,
    pub compact_sidebar: bool,
    pub click_sound_enabled: bool,
    pub click_sound: ClickSound,
    pub minecraft_dir: String,
    pub minecraft_dir_is_default: bool,
    /// The folder the launcher uses when none is chosen ("Default paths" fills it in).
    #[serde(default)]
    pub default_minecraft_dir: String,
    /// `default_max_ram_gb` in GiB; `None` means the amount recommended for this computer.
    #[serde(default)]
    pub default_max_ram_gb: Option<u64>,
    /// `gpu_mode_default` for new builds: `auto`, `igpu` or `dgpu`.
    #[serde(default)]
    pub gpu_mode_default: String,
    /// `window_size`: `fullscreen`, `maximized` or `{w}x{h}`.
    #[serde(default)]
    pub window_size: String,
    /// How many builds Home's «Продовжити гру» shows (0 hides it; at most `recent::RECENT_MOST`).
    #[serde(default = "crate::recent::recent_default")]
    pub home_recent_builds: u8,
    /// When «Продовжити гру» was cleared (ms since the epoch): builds played before stay out.
    #[serde(default)]
    pub home_recent_cleared_ms: Option<u64>,
    /// How many changes were saved before this snapshot (this run): a later answer has a higher
    /// one.
    #[serde(default)]
    pub revision: u64,
}

impl SettingsSnapshot {
    /// This snapshot may replace `shown`: it was taken after it (answers to changes saved side by
    /// side may arrive in any order).
    pub fn replaces(&self, shown: &SettingsSnapshot) -> bool {
        self.revision >= shown.revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "key", content = "value", rename_all = "snake_case")]
pub enum SettingUpdate {
    Lang(String),
    AutoUpdate(bool),
    IncludeBetaUpdates(bool),
    OnGameStart(GameStartAction),
    AskProfileOnLaunch(bool),
    CompactSidebar(bool),
    ClickSoundEnabled(bool),
    ClickSound(ClickSound),
    /// `None` returns to the recommended amount.
    DefaultMaxRamGb(Option<u64>),
    GpuModeDefault(String),
    WindowSize(String),
    HomeRecentBuilds(u8),
    /// `true` clears «Продовжити гру» now; `false` gives its history back.
    HomeRecentClear(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupState {
    pub should_open: bool,
    pub lang: String,
    pub app_state_dir: String,
    pub default_app_state_dir: String,
    pub derived_minecraft_dir: String,
    pub derived_backups_dir: String,
    pub issues: Vec<Text>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPlan {
    pub lang: String,
    pub app_state_dir: String,
}

/// Folders the wizard will really use for a chosen launcher-data folder (computed by the backend).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPreview {
    pub app_state_dir: String,
    pub minecraft_dir: String,
    pub backups_dir: String,
}

/// One build as the UI lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDto {
    pub key: String,
    /// The id `--launch-version` and desktop shortcuts use.
    pub version_id: String,
    pub name: String,
    pub version: Option<String>,
    pub loader: Option<String>,
    pub client: Option<String>,
    pub loader_version: Option<String>,
    pub game_dir: String,
    pub image: Option<String>,
    pub description: String,
    /// A game of this build is running.
    pub running: bool,
    /// Its own account (a profile key): Play starts with it and asks no account.
    #[serde(default)]
    pub profile: Option<String>,
}

/// Every build in the order the user put them (the rest by name) (`app://builds`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildsSnapshot {
    pub builds: Vec<BuildDto>,
}

/// A Minecraft version the "Create build" page offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogVersion {
    pub id: String,
    /// `release` or `snapshot`.
    pub kind: String,
    pub release_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaEntry {
    pub label: String,
    pub path: String,
}

/// Java found on this computer (`launcher`) and the user's own list (`custom`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaList {
    pub launcher: Vec<JavaEntry>,
    pub custom: Vec<JavaEntry>,
}

/// What the memory slider may offer on this computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_gb: u64,
    pub min_heap_gb: u64,
    pub max_heap_gb: u64,
    pub recommended_heap_gb: u64,
}

/// The smallest and largest window sizes (logical pixels).
pub const WINDOW_MIN: (u32, u32) = (960, 600);
pub const WINDOW_MAX: (u32, u32) = (7680, 4320);
/// Sizes offered in Settings → Interface.
pub const WINDOW_PRESETS: [(u32, u32); 4] = [(1200, 740), (1366, 800), (1600, 960), (1920, 1080)];
/// The window's size until the user picks another (one of `WINDOW_PRESETS`).
pub const WINDOW_DEFAULT: (u32, u32) = WINDOW_PRESETS[1];

/// How big the launcher window is (Settings → Interface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSize {
    Fullscreen,
    Maximized,
    Size { width: u32, height: u32 },
}

impl Default for WindowSize {
    fn default() -> WindowSize {
        WindowSize::Size { width: WINDOW_DEFAULT.0, height: WINDOW_DEFAULT.1 }
    }
}

impl WindowSize {
    /// A size within `WINDOW_MIN..=WINDOW_MAX`.
    pub fn sized(width: u32, height: u32) -> Option<WindowSize> {
        let fits =
            (WINDOW_MIN.0..=WINDOW_MAX.0).contains(&width) && (WINDOW_MIN.1..=WINDOW_MAX.1).contains(&height);
        fits.then_some(WindowSize::Size { width, height })
    }

    /// `fullscreen`, `maximized` or `{w}x{h}` (ignoring case and spaces).
    pub fn parse(raw: &str) -> Option<WindowSize> {
        match raw.trim().to_lowercase().as_str() {
            "fullscreen" => Some(WindowSize::Fullscreen),
            "maximized" => Some(WindowSize::Maximized),
            other => {
                let (w, h) = other.split_once('x')?;
                WindowSize::sized(w.trim().parse().ok()?, h.trim().parse().ok()?)
            }
        }
    }

    pub fn as_config_str(&self) -> String {
        match self {
            WindowSize::Fullscreen => "fullscreen".to_string(),
            WindowSize::Maximized => "maximized".to_string(),
            WindowSize::Size { width, height } => format!("{width}x{height}"),
        }
    }
}

/// A mod loader, or plain Minecraft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Minecraft,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    pub const ALL: [LoaderKind; 5] = [
        LoaderKind::Minecraft,
        LoaderKind::Fabric,
        LoaderKind::Quilt,
        LoaderKind::Forge,
        LoaderKind::NeoForge,
    ];

    /// The client name builds show.
    pub fn display_name(self) -> &'static str {
        match self {
            LoaderKind::Minecraft => "Minecraft",
            LoaderKind::Fabric => "Fabric",
            LoaderKind::Quilt => "Quilt",
            LoaderKind::Forge => "Forge",
            LoaderKind::NeoForge => "NeoForge",
        }
    }

    /// The installed version id of loader build `loader_version` for Minecraft `mc`.
    pub fn component_id(self, mc: &str, loader_version: &str) -> String {
        match self {
            LoaderKind::Minecraft => mc.to_string(),
            LoaderKind::Fabric => format!("fabric-loader-{loader_version}-{mc}"),
            LoaderKind::Quilt => format!("quilt-loader-{loader_version}-{mc}"),
            LoaderKind::Forge => format!("{mc}-forge-{loader_version}"),
            LoaderKind::NeoForge => format!("neoforge-{loader_version}"),
        }
    }

    /// The loader an installed version id belongs to; `None` for plain Minecraft or unknown ids.
    pub fn of_component(id: &str) -> Option<LoaderKind> {
        let id = id.to_lowercase();
        if id.starts_with("fabric-loader-") {
            Some(LoaderKind::Fabric)
        } else if id.starts_with("quilt-loader-") {
            Some(LoaderKind::Quilt)
        } else if id.starts_with("neoforge-") {
            Some(LoaderKind::NeoForge)
        } else if id.contains("-forge-") {
            Some(LoaderKind::Forge)
        } else {
            None
        }
    }

    /// The loader a build's client name means (`Fabric`, `neoforge`…).
    pub fn from_client(client: &str) -> Option<LoaderKind> {
        match client.trim().to_lowercase().as_str() {
            "minecraft" | "vanilla" => Some(LoaderKind::Minecraft),
            "fabric" => Some(LoaderKind::Fabric),
            "quilt" => Some(LoaderKind::Quilt),
            "forge" => Some(LoaderKind::Forge),
            "neoforge" => Some(LoaderKind::NeoForge),
            _ => None,
        }
    }
}

/// One build of a mod loader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderBuild {
    pub version: String,
    pub stable: bool,
}

/// A Minecraft version a loader supports, with the loader builds offered for it (newest first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderOption {
    pub mc: String,
    /// The Minecraft version is a snapshot.
    pub snapshot: bool,
    pub builds: Vec<LoaderBuild>,
    pub default_version: String,
}

/// A loader's Minecraft versions as sent to the window: the builds the first version offers are
/// sent once (`builds`), and every version offering the same ones has an empty list of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderCatalog {
    pub builds: Vec<LoaderBuild>,
    pub options: Vec<LoaderOption>,
}

impl LoaderCatalog {
    pub fn pack(mut options: Vec<LoaderOption>) -> LoaderCatalog {
        let builds = options.first().map(|o| o.builds.clone()).unwrap_or_default();
        for option in &mut options {
            if option.builds == builds {
                option.builds = Vec::new();
            }
        }
        LoaderCatalog { builds, options }
    }

    /// The versions each with its builds again.
    pub fn unpack(self) -> Vec<LoaderOption> {
        let LoaderCatalog { builds, mut options } = self;
        for option in &mut options {
            if option.builds.is_empty() {
                option.builds = builds.clone();
            }
        }
        options
    }
}

/// One installed version in `versions/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentDto {
    pub id: String,
    /// `None` when it inherits from another version but names no loader the launcher knows.
    pub loader: Option<LoaderKind>,
    pub minecraft: Option<String>,
    pub loader_version: Option<String>,
    /// Bytes in `versions/<id>/`.
    pub size: u64,
    /// When its version JSON last changed (Unix milliseconds).
    pub modified_ms: Option<u64>,
    /// Names of the builds that run it or build on it.
    pub used_by: Vec<String>,
    /// Installed components that inherit from it.
    pub base_for: Vec<String>,
}

/// Everything in `versions/`, newest Minecraft first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentsSnapshot {
    pub components: Vec<ComponentDto>,
}

/// What "Verify" found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyOutcome {
    /// Every file checked out.
    Intact,
    /// Something was damaged or missing and has been repaired.
    Repaired,
}

/// What the build settings page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSettingsDto {
    pub key: String,
    pub name: String,
    /// The icon as the page shows it (a `data:` or `https:` URL).
    pub image: Option<String>,
    /// The installed component it runs.
    pub component: Option<String>,
    /// Its own Java (`executablePath`); `None` uses the launcher's.
    pub java_path: Option<String>,
    /// The launcher's Java for its component, when installed.
    pub auto_java: Option<String>,
    pub gpu_mode: String,
    /// Its own memory limit (`-Xmx`, GB); `None` follows Settings → Java.
    pub max_ram_gb: Option<u64>,
    /// JVM arguments besides the memory limit.
    pub jvm_arguments: Vec<String>,
    pub server_host: String,
    pub server_port: Option<u16>,
    /// Its own account (a profile key); `None` follows the launcher's settings.
    #[serde(default)]
    pub profile: Option<String>,
}

/// What "Save" on the build settings page sends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSettingsUpdate {
    pub name: String,
    /// An installed component to run.
    pub component: Option<String>,
    pub java_path: Option<String>,
    pub gpu_mode: String,
    pub max_ram_gb: Option<u64>,
    pub jvm_arguments: Vec<String>,
    pub server_host: String,
    /// As typed: empty or 1–65535.
    pub server_port: String,
    /// A picked icon file to use.
    pub image_path: Option<String>,
    pub remove_image: bool,
    /// Its own account (a profile key); `None`: none of its own.
    #[serde(default)]
    pub profile: Option<String>,
}

/// A build runs mods only with a mod loader (the original's `mods_supported`).
pub fn mods_supported(client: Option<&str>) -> bool {
    let client = client.unwrap_or_default().to_lowercase();
    client != "minecraft"
        && ["fabric", "forge", "neoforge", "quilt", "tensacraft"].iter().any(|l| client.contains(l))
}

/// A kind of installed content on a build's content page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    Mods,
    ResourcePacks,
    ShaderPacks,
}

impl ContentKind {
    pub const ALL: [ContentKind; 3] =
        [ContentKind::Mods, ContentKind::ResourcePacks, ContentKind::ShaderPacks];

    /// Its folder in the build's game folder.
    pub fn folder(self) -> &'static str {
        match self {
            ContentKind::Mods => "mods",
            ContentKind::ResourcePacks => "resourcepacks",
            ContentKind::ShaderPacks => "shaderpacks",
        }
    }

    /// The toast after an item was switched on or off.
    pub fn toggled_key(self, on: bool) -> &'static str {
        match (self, on) {
            (ContentKind::Mods, true) => "mod_enabled",
            (ContentKind::Mods, false) => "mod_disabled",
            (ContentKind::ResourcePacks, true) => "resourcepack_enabled",
            (ContentKind::ResourcePacks, false) => "resourcepack_disabled",
            (ContentKind::ShaderPacks, true) => "shaderpack_enabled",
            (ContentKind::ShaderPacks, false) => "shaderpack_disabled",
        }
    }

    /// The toast after an item was deleted.
    pub fn deleted_key(self) -> &'static str {
        match self {
            ContentKind::Mods => "mod_deleted",
            ContentKind::ResourcePacks => "resourcepack_deleted",
            ContentKind::ShaderPacks => "shaderpack_deleted",
        }
    }
}

/// One installed mod, resource pack or shader pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentItem {
    /// Its name on disk: what a change names.
    pub file: String,
    /// Its name without `.disabled`.
    pub filename: String,
    pub size: u64,
    pub enabled: bool,
    /// A pack kept as a folder.
    pub folder: bool,
    /// Shader packs switch only with Iris installed.
    pub toggle_supported: bool,
    /// What a mod's own metadata says.
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub mod_id: Option<String>,
    /// A mod with a backup of itself in `mods/.backups` to put back.
    #[serde(default)]
    pub has_backup: bool,
}

/// A build's installed content of one kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentList {
    pub kind: ContentKind,
    /// `false` for the mods of a build without a mod loader.
    pub supported: bool,
    pub items: Vec<ContentItem>,
}

/// A screenshot on a build's content page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotDto {
    pub name: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
    /// Where the page loads the picture from (the `shot` scheme).
    pub src: String,
    /// Its thumbnail, 480 pixels wide (the same scheme).
    #[serde(default)]
    pub thumb: String,
}

/// One screenshot of one build (what the Screenshots page acts on several at once).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShotRef {
    pub key: String,
    pub name: String,
}

/// A build's screenshots, newest first (the Screenshots page).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildShots {
    pub key: String,
    pub shots: Vec<ScreenshotDto>,
}

#[cfg(test)]
mod loader_catalog_tests {
    use super::*;

    fn builds(versions: &[&str]) -> Vec<LoaderBuild> {
        versions.iter().map(|v| LoaderBuild { version: v.to_string(), stable: true }).collect()
    }

    fn option(mc: &str, offered: Vec<LoaderBuild>) -> LoaderOption {
        LoaderOption {
            mc: mc.into(),
            snapshot: false,
            default_version: offered[0].version.clone(),
            builds: offered,
        }
    }

    #[test]
    fn builds_every_version_shares_are_sent_once() {
        // Fabric offers its ~200 builds for each of ~900 versions: sent per version, the list
        // came to megabytes. Forge's builds are each version's own and stay with it.
        let shared = builds(&["0.17.2", "0.16.9"]);
        let options = vec![
            option("1.21.1", shared.clone()),
            option("1.20.1", shared.clone()),
            option("1.12.2", builds(&["14.23.5"])),
        ];
        let catalog = LoaderCatalog::pack(options.clone());
        assert_eq!(catalog.builds, shared);
        assert!(catalog.options[0].builds.is_empty() && catalog.options[1].builds.is_empty());
        assert_eq!(catalog.options[2].builds, builds(&["14.23.5"]));
        assert_eq!(catalog.unpack(), options);
        assert_eq!(LoaderCatalog::pack(Vec::new()).unpack(), Vec::new());
    }
}
