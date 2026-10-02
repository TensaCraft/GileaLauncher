//! The thirteen rules, in the original's order: each names a crash it knows
//! from the words it leaves in the logs.

use std::sync::LazyLock;

use crate::dto::{Confidence, Finding, FixAction, FixKind, Safety, Severity};
use launcher_shared::Text;
use regex::Regex;

use super::dependencies::{finding_id_part, missing};
use super::{Case, Detector, open_diagnostics};

fn action(id: &str, kind: FixKind, safety: Safety, title: &str) -> FixAction {
    FixAction { id: id.into(), kind, safety, title: Text::key(title) }
}

fn open_mod_manager() -> FixAction {
    action("open_mod_manager", FixKind::OpenModManager, Safety::Safe, "diagnostic_action_open_mod_manager")
}

fn repair_minecraft() -> FixAction {
    action("repair_minecraft", FixKind::Repair, Safety::Confirm, "diagnostic_action_repair")
}

fn retry_sync() -> FixAction {
    action("retry_sync", FixKind::RepairSync, Safety::Confirm, "diagnostic_action_retry_sync")
}

/// A server build's files are synced again; a build of one's own only has its mods to look at.
fn mod_actions(case: &Case) -> Vec<FixAction> {
    let mut actions = vec![open_mod_manager()];
    actions.extend(case.managed.then(retry_sync));
    actions
}

fn sync_actions(case: &Case) -> Vec<FixAction> {
    case.managed.then(retry_sync).into_iter().collect()
}

fn no_actions(_: &Case) -> Vec<FixAction> {
    Vec::new()
}

/// Up to four trimmed lines of the logs that hold one of `markers`, 300 characters each.
fn evidence(case: &Case, markers: &[&str]) -> Vec<String> {
    shown(case.raw.lines().filter(|line| {
        let low = line.to_lowercase();
        markers.iter().any(|m| low.contains(m))
    }))
}

/// At most four lines of at most 300 characters, each once: the same line is often in both the
/// game's log and the launch log.
fn shown<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut shown: Vec<String> = Vec::new();
    for line in lines.map(|l| l.trim().chars().take(300).collect::<String>()) {
        if !line.is_empty() && !shown.contains(&line) {
            shown.push(line);
        }
        if shown.len() == 4 {
            break;
        }
    }
    shown
}

/// Like [`evidence`], but a line that matches more of the marker groups comes first; lines that
/// match as many keep their order in the log.
fn evidence_ranked(case: &Case, groups: &[&[&str]]) -> Vec<String> {
    let mut lines: Vec<(usize, &str)> = case
        .raw
        .lines()
        .filter_map(|line| {
            let low = line.to_lowercase();
            let rank = groups.iter().filter(|g| g.iter().any(|m| low.contains(m))).count();
            (rank > 0).then_some((rank, line))
        })
        .collect();
    lines.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    shown(lines.into_iter().map(|(_, line)| line))
}

/// Lines of at most a thousand characters, lower case beside as written.
fn short_lines(case: &Case) -> impl Iterator<Item = (String, &str)> {
    case.raw.lines().filter(|l| l.chars().count() <= 1000).map(|l| (l.to_lowercase(), l))
}

#[allow(clippy::too_many_arguments)]
fn finding(
    id: impl Into<String>,
    kind: &str,
    key: &str,
    severity: Severity,
    confidence: Confidence,
    priority: u32,
    message: Text,
    evidence: Vec<String>,
    actions: Vec<FixAction>,
    suppresses: Vec<String>,
) -> Finding {
    Finding {
        id: id.into(),
        kind: kind.into(),
        severity,
        confidence,
        priority,
        title: Text::key(format!("launch_diagnostic_{key}_title")),
        message,
        evidence,
        actions,
        suppresses,
    }
}

/// A rule that is a set of words in the logs.
struct Plain {
    id: &'static str,
    kind: &'static str,
    key: &'static str,
    severity: Severity,
    confidence: Confidence,
    priority: u32,
    markers: &'static [&'static str],
    matches: fn(&str) -> bool,
    actions: fn(&Case) -> Vec<FixAction>,
    suppresses: &'static [&'static str],
}

impl Detector for Plain {
    fn detect(&self, case: &Case) -> Vec<Finding> {
        if !(self.matches)(&case.text) {
            return Vec::new();
        }
        vec![finding(
            self.id,
            self.kind,
            self.key,
            self.severity,
            self.confidence,
            self.priority,
            Text::key(format!("launch_diagnostic_{}", self.key)),
            evidence(case, self.markers),
            (self.actions)(case),
            self.suppresses.iter().map(|s| s.to_string()).collect(),
        )]
    }
}

const CREATE_SUPPRESSES: &[&str] = &[
    "runtime.minecraft.missing",
    "mods.missing_dependency",
    "network.channel_mismatch",
    "graphics.initialization",
];

static MODULE_CONFLICT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)modules\s+(?P<first>[a-z0-9_.-]+)\s+and\s+(?P<second>[a-z0-9_.-]+)\s+export package\s+(?P<package>[a-z0-9_.-]+)\s+to module")
        .expect("the module conflict pattern")
});
static CREATE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^\s*Block:\s*Block\{(?P<block>[a-z0-9_.-]+:[a-z0-9_./-]+)\}")
        .expect("the block pattern")
});

/// №3: two modules export one package.
struct ModuleConflict;

impl Detector for ModuleConflict {
    fn detect(&self, case: &Case) -> Vec<Finding> {
        let Some(c) = MODULE_CONFLICT.captures(&case.raw) else { return Vec::new() };
        let suppresses = [&c["first"], &c["second"]]
            .iter()
            .map(|m| format!("mods.missing_dependency.{}", finding_id_part(m)))
            .collect();
        vec![finding(
            "mods.module_conflict",
            "mod_incompatibility",
            "mod_incompatibility",
            Severity::Warning,
            Confidence::Exact,
            120,
            Text::key("launch_diagnostic_mod_incompatibility"),
            evidence(case, &["export package"]),
            mod_actions(case),
            suppresses,
        )]
    }
}

/// №8: Create could not model a block entity; the block, when the report names it.
struct CreateRendering;

impl Detector for CreateRendering {
    fn detect(&self, case: &Case) -> Vec<Finding> {
        let t = &case.text;
        let hit = t.contains("description: rendering block entity")
            && t.contains("bakedmodel.getmodeldata")
            && (t.contains("bakedmodelbuffererimpl") || t.contains("packagerrenderer"));
        if !hit {
            return Vec::new();
        }
        let message = match CREATE_BLOCK.captures(&case.raw) {
            Some(c) => Text::key("launch_diagnostic_create_rendering_block").param("block", &c["block"]),
            None => Text::key("launch_diagnostic_create_rendering"),
        };
        vec![finding(
            "mods.create_block_entity_rendering",
            "mod_rendering_error",
            "create_rendering",
            Severity::Warning,
            Confidence::Exact,
            120,
            message,
            evidence(case, &["rendering block entity", "bakedmodel.getmodeldata", "block{"]),
            vec![open_mod_manager()],
            CREATE_SUPPRESSES.iter().map(|s| s.to_string()).collect(),
        )]
    }
}

/// №9: one finding for each dependency the loader says is missing.
struct MissingDependency;

impl Detector for MissingDependency {
    fn detect(&self, case: &Case) -> Vec<Finding> {
        let t = &case.text;
        const MISSING: [&str; 6] = [
            "not installed",
            "which is missing",
            "is missing",
            "missing or unsupported mandatory dependencies",
            "mandatory dependencies",
            "mod loading has failed",
        ];
        let gate = (t.contains("requires") && MISSING.iter().any(|m| t.contains(m)))
            || (t.contains("currently,") && t.contains("not installed"))
            || (t.contains("mandatory dependencies") && t.contains("actual version: '[missing]'"));
        if !gate {
            return Vec::new();
        }
        missing(&case.raw)
            .into_iter()
            // Minecraft itself missing is rule №1's: the build is repaired, nothing is installed.
            .filter(|(_, dependency)| !dependency.eq_ignore_ascii_case("minecraft"))
            .map(|(module, dependency)| {
                // The loader's own sentence about what is missing, before the lines that only name it.
                let named = dependency.to_lowercase();
                let markers: [&[&str]; 2] = [
                    &[named.as_str()],
                    &["which is missing", "actual version: '[missing]'", "not installed"],
                ];
                finding(
                    format!("mods.missing_dependency.{}", finding_id_part(&dependency)),
                    "missing_mod_dependency",
                    "missing_mod_dependency",
                    Severity::Warning,
                    Confidence::Exact,
                    100,
                    Text::key("launch_diagnostic_missing_mod_dependency")
                        .param("mod", module)
                        .param("dependency", dependency.clone()),
                    evidence_ranked(case, &markers),
                    mod_actions(case),
                    vec!["mods.incompatible.generic".into()],
                )
            })
            .collect()
    }
}

/// №11: the client and the server disagree about a mod's network channel.
struct ChannelMismatch;

impl Detector for ChannelMismatch {
    fn detect(&self, case: &Case) -> Vec<Finding> {
        let hit = |low: &str| {
            low.contains("не вдалося з'єднатися з каналом")
                || low.contains("mismatched mod channel list")
                || ((low.contains("channel") || low.contains("канал"))
                    && ["client", "server", "connection", "connect", "клієнт", "сервер"]
                        .iter()
                        .any(|w| low.contains(w))
                    && [
                        "absent",
                        "missing",
                        "requires",
                        "required",
                        "rejected",
                        "mismatch",
                        "incompatible",
                        "відсут",
                        "необхід",
                    ]
                    .iter()
                    .any(|w| low.contains(w)))
        };
        let lines: Vec<String> = short_lines(case)
            .filter(|(low, _)| hit(low))
            .map(|(_, line)| line.trim().chars().take(300).collect())
            .take(4)
            .collect();
        if lines.is_empty() {
            return Vec::new();
        }
        vec![finding(
            "network.channel_mismatch",
            "network_channel_mismatch",
            "channel_mismatch",
            Severity::Warning,
            Confidence::High,
            70,
            Text::key("launch_diagnostic_channel_mismatch"),
            lines,
            sync_actions(case),
            Vec::new(),
        )]
    }
}

/// №13: a line that says the graphics could not start.
struct GraphicsInitialization;

impl Detector for GraphicsInitialization {
    fn detect(&self, case: &Case) -> Vec<Finding> {
        const SUBJECTS: [&str; 7] = [
            "opengl",
            "buffer storage",
            "persistent mapping",
            "renderer",
            "graphics driver",
            "gpu driver",
            "video driver",
        ];
        const FAILURES: [&str; 9] = [
            "error",
            "exception",
            "failed",
            "failure",
            "crash",
            "incompatible",
            "unsupported",
            "not support",
            "unavailable",
        ];
        let lines: Vec<String> = short_lines(case)
            .filter(|(low, _)| {
                SUBJECTS.iter().any(|s| low.contains(s)) && FAILURES.iter().any(|f| low.contains(f))
            })
            .map(|(_, line)| line.trim().chars().take(300).collect())
            .take(4)
            .collect();
        if lines.is_empty() {
            return Vec::new();
        }
        vec![finding(
            "graphics.initialization",
            "graphics_compatibility",
            "graphics",
            Severity::Warning,
            Confidence::High,
            60,
            Text::key("launch_diagnostic_graphics"),
            lines,
            vec![open_diagnostics()],
            Vec::new(),
        )]
    }
}

/// The thirteen rules, in the original's order.
pub fn rules() -> Vec<Box<dyn Detector>> {
    vec![
        Box::new(Plain {
            id: "runtime.minecraft.missing",
            kind: "missing_minecraft",
            key: "missing_minecraft",
            severity: Severity::Error,
            confidence: Confidence::Exact,
            priority: 100,
            markers: &["mod id: 'minecraft'", "actual version: '[missing]'"],
            matches: |t| t.contains("mod id: 'minecraft'") && t.contains("actual version: '[missing]'"),
            actions: |_| vec![repair_minecraft()],
            suppresses: &[],
        }),
        Box::new(Plain {
            id: "files.locked",
            kind: "locked_file",
            key: "locked_file",
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 110,
            markers: &["winerror 32", "being used by another process", "process cannot access the file"],
            matches: |t| {
                t.contains("winerror 32")
                    || t.contains("being used by another process")
                    || t.contains("process cannot access the file")
            },
            actions: sync_actions,
            suppresses: &[],
        }),
        Box::new(ModuleConflict),
        Box::new(Plain {
            id: "mods.broken_mixin",
            kind: "mod_incompatibility",
            key: "mod_incompatibility",
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 115,
            markers: &["illegalclassloaderror", "illegal classload request", "mixin is missing from"],
            matches: |t| {
                t.contains("illegalclassloaderror")
                    && t.contains("illegal classload request")
                    && t.contains("mixin is missing from")
            },
            actions: mod_actions,
            suppresses: &[],
        }),
        Box::new(Plain {
            id: "mods.player_interaction",
            kind: "mod_interaction_error",
            key: "player_interaction",
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 125,
            markers: &[
                "java.lang.nullpointerexception",
                "this.minecraft.player",
                "multiplayergamemode.ensurehassentcarrieditem",
            ],
            matches: |t| {
                t.contains("java.lang.nullpointerexception")
                    && t.contains("this.minecraft.player")
                    && t.contains("multiplayergamemode.ensurehassentcarrieditem")
            },
            actions: mod_actions,
            suppresses: &["graphics.initialization"],
        }),
        Box::new(Plain {
            id: "mods.ftb_chunks_local_data",
            kind: "ftb_chunks_local_data",
            key: "ftb_chunks",
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 125,
            markers: &["java.util.concurrentmodificationexception", "ftbchunks", "mapmanager.saveallregions"],
            matches: |t| {
                t.contains("java.util.concurrentmodificationexception")
                    && t.contains("ftbchunks")
                    && t.contains("mapmanager.saveallregions")
            },
            actions: no_actions,
            suppresses: &["graphics.initialization"],
        }),
        Box::new(Plain {
            id: "mods.create_configuration_payload",
            kind: "mod_network_initialization_error",
            key: "create_configuration",
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 125,
            markers: &[
                "cannot retrieve the client player during the configuration phase",
                "ponder@",
                "net.createmod",
                "payload: create:",
            ],
            matches: |t| {
                t.contains("cannot retrieve the client player during the configuration phase")
                    && (t.contains("ponder@")
                        || t.contains("net.createmod")
                        || t.contains("payload: create:"))
            },
            actions: mod_actions,
            suppresses: CREATE_SUPPRESSES,
        }),
        Box::new(CreateRendering),
        Box::new(MissingDependency),
        Box::new(Plain {
            id: "mods.incompatible.generic",
            kind: "mod_incompatibility",
            key: "mod_incompatibility",
            severity: Severity::Warning,
            confidence: Confidence::High,
            priority: 80,
            markers: &[
                "incompatible mods found",
                "incompatible mod set",
                "mods are incompatible",
                "neg_hard_dep",
                "negative hard dependency",
                "conflicts with an installed or selected mod",
                "replace mod",
                "breaks",
            ],
            matches: |t| {
                [
                    "incompatible mods found",
                    "incompatible mod set",
                    "mods are incompatible",
                    "neg_hard_dep",
                    "negative hard dependency",
                    "conflicts with an installed or selected mod",
                ]
                .iter()
                .any(|m| t.contains(m))
                    || (t.contains("replace mod") && t.contains("compatible with"))
                    || (t.contains("breaks") && (t.contains("hard_dep") || t.contains("mod")))
            },
            actions: |_| vec![open_mod_manager()],
            suppresses: &[],
        }),
        Box::new(ChannelMismatch),
        Box::new(Plain {
            id: "graphics.vulkan_device",
            kind: "graphics_compatibility",
            key: "vulkan_gpu",
            severity: Severity::Warning,
            confidence: Confidence::Exact,
            priority: 105,
            markers: &["failed to find a suitable gpu", "vulkanmod.vulkan.device.devicemanager"],
            matches: |t| {
                t.contains("failed to find a suitable gpu")
                    && t.contains("vulkanmod.vulkan.device.devicemanager")
            },
            actions: |_| vec![open_mod_manager()],
            suppresses: &[],
        }),
        Box::new(GraphicsInitialization),
    ]
}

/// Fabric 0.19's own words when a mod lacks Fabric API (the tests' sample).
#[cfg(test)]
pub(crate) const FABRIC_LOG_FOR_TESTS: &str = r#"[14:46:57] [main/WARN]: Mod resolution failed
[14:46:57] [main/INFO]: Immediate reason: [HARD_DEP_NO_CANDIDATE needs_api 1.0 {depends fabric-api @ [*]}, ROOT_FORCELOAD_SINGLE needs_api 1.0]
[14:46:57] [main/INFO]: Reason: [HARD_DEP needs_api 1.0 {depends fabric-api @ [*]}]
[14:46:57] [main/INFO]: Fix: add [add:fabric-api 1 ([(-∞,∞)])], remove [], replace []
[14:46:57] [main/ERROR]: Incompatible mods found!
net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible with the game or each other!
A potential solution has been determined, this may resolve your problem:
	 - Install fabric-api, any version.
More details:
	 - Mod 'Needs API' (needs_api) 1.0 requires any version of fabric-api, which is missing!
	at net.fabricmc.loader.impl.FormattedException.ofLocalized(FormattedException.java:51) ~[fabric-loader-0.19.3.jar:?]"#;

#[cfg(test)]
mod tests {
    use crate::dto::{Confidence, Finding, FixKind, Severity};
    use launcher_shared::Text;

    use super::*;
    use crate::backend::{Case, diagnose};

    fn diagnosed(log: &str) -> Vec<Finding> {
        let case = Case { text: log.to_lowercase(), raw: log.to_string(), files: Vec::new(), managed: false };
        diagnose(&case, &rules(), 0).findings
    }

    fn ids(log: &str) -> Vec<String> {
        diagnosed(log).into_iter().map(|f| f.id).collect()
    }

    fn kinds(f: &Finding) -> Vec<FixKind> {
        f.actions.iter().map(|a| a.kind).collect()
    }

    #[test]
    fn missing_minecraft_is_an_error_to_repair() {
        let f = &diagnosed(
            "Mod ID: 'minecraft', Requested by: 'fabricloader', Expected range: '*', Actual version: '[MISSING]'",
        )[0];
        assert_eq!(
            (f.id.as_str(), f.severity, f.confidence, f.priority),
            ("runtime.minecraft.missing", Severity::Error, Confidence::Exact, 100)
        );
        assert_eq!(kinds(f), vec![FixKind::Repair]);
        assert_eq!(f.title, Text::key("launch_diagnostic_missing_minecraft_title"));
    }

    #[test]
    fn minecraft_missing_in_a_dependency_table_is_to_repair_not_to_install() {
        let neoforge = "Missing or unsupported mandatory dependencies:
	Mod ID: 'minecraft', Requested by: 'neoforge', Expected range: '[1.21.1]', Actual version: '[MISSING]'
[main/INFO]: Loading Minecraft 1.21.1
[main/INFO]: Minecraft folder is ready";
        let fabric = "Incompatible mods found!
	 - Mod 'Sodium' (sodium) 0.6 requires version 1.21.1 of minecraft, which is missing!
[main/INFO]: Loading Minecraft 1.21.1";
        assert_eq!(ids(neoforge), ["runtime.minecraft.missing"]);
        // Rule №1 knows NeoForge's table only; Fabric's wording at least never says "install minecraft".
        let ids = ids(fabric);
        assert!(!ids.iter().any(|id| id == "mods.missing_dependency.minecraft"), "{ids:?}");
    }

    #[test]
    fn a_locked_file_is_named() {
        let f = &diagnosed(
            "java.nio.file.FileSystemException: C:\\x\\mods\\a.jar: The process cannot access the file because it is being used by another process",
        )[0];
        assert_eq!((f.id.as_str(), f.priority, f.severity), ("files.locked", 110, Severity::Warning));
        assert!(f.actions.is_empty(), "no retry_sync outside a server build");
        assert_eq!(f.evidence.len(), 1);
        let managed =
            Case { text: "winerror 32".into(), raw: "WinError 32".into(), files: Vec::new(), managed: true };
        let f = &diagnose(&managed, &rules(), 0).findings[0];
        assert_eq!(kinds(f), vec![FixKind::RepairSync]);
    }

    #[test]
    fn a_module_conflict_names_both_modules_and_hides_their_missing_dependencies() {
        let log = "java.lang.module.ResolutionException: Modules aaa and bbb export package ccc to module ddd\nMissing or unsupported mandatory dependencies:\n\tMod ID: 'aaa', Requested by: 'x', Expected range: '[1,)', Actual version: '[MISSING]'";
        assert_eq!(ids(log), vec!["mods.module_conflict"]);
        let f = &diagnosed(log)[0];
        assert_eq!((f.priority, kinds(f)), (120, vec![FixKind::OpenModManager]));
    }

    #[test]
    fn a_broken_mixin_is_found() {
        let f = &diagnosed(
            "org.spongepowered.asm.mixin.transformer.throwables.IllegalClassLoadError: Illegal classload request for x. Mixin is missing from y",
        )[0];
        assert_eq!((f.id.as_str(), f.priority), ("mods.broken_mixin", 115));
    }

    #[test]
    fn a_player_interaction_crash_hides_the_graphics_guess() {
        let log = "java.lang.NullPointerException: Cannot invoke because \"this.minecraft.player\" is null\n\tat MultiPlayerGameMode.ensureHasSentCarriedItem\n[Render thread/ERROR]: OpenGL error";
        assert_eq!(ids(log), vec!["mods.player_interaction"]);
    }

    #[test]
    fn ftb_chunks_saving_its_map_is_found() {
        let log = "java.util.ConcurrentModificationException\n\tat dev.ftb.mods.ftbchunks.client.map.MapManager.saveAllRegions";
        let f = &diagnosed(log)[0];
        assert_eq!((f.id.as_str(), f.kind.as_str()), ("mods.ftb_chunks_local_data", "ftb_chunks_local_data"));
        assert!(f.actions.is_empty());
    }

    #[test]
    fn a_create_configuration_crash_hides_what_it_causes() {
        let log = "Cannot retrieve the client player during the configuration phase\n\tat net.createmod.ponder.X\nMismatched mod channel list";
        assert_eq!(ids(log), vec!["mods.create_configuration_payload"]);
    }

    #[test]
    fn a_create_rendering_crash_names_its_block() {
        let log = "Description: Rendering Block Entity\n\tat BakedModel.getModelData\n\tat BakedModelBuffererImpl\nBlock: Block{create:mechanical_press}";
        let f = &diagnosed(log)[0];
        assert_eq!(f.id, "mods.create_block_entity_rendering");
        assert_eq!(
            f.message,
            Text::key("launch_diagnostic_create_rendering_block").param("block", "create:mechanical_press")
        );
        let without =
            "Description: Rendering Block Entity\n\tat BakedModel.getModelData\n\tat PackagerRenderer";
        assert_eq!(diagnosed(without)[0].message, Text::key("launch_diagnostic_create_rendering"));
    }

    #[test]
    fn a_missing_fabric_api_is_named_and_hides_the_generic_finding() {
        let log = "Incompatible mods found!\n - Mod 'Needs API' (needs_api) 1.0 requires any version of fabric-api, which is missing!";
        let found = diagnosed(log);
        assert_eq!(
            found.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["mods.missing_dependency.fabric_api"]
        );
        assert_eq!(
            found[0].message,
            Text::key("launch_diagnostic_missing_mod_dependency")
                .param("mod", "needs_api")
                .param("dependency", "fabric-api")
        );
        assert_eq!(kinds(&found[0]), vec![FixKind::OpenModManager]);
    }

    #[test]
    fn neoforge_names_each_missing_dependency() {
        let log = "Missing or unsupported mandatory dependencies:\n\tMod ID: 'kotlinforforge', Requested by: 'mymod', Expected range: '[4,)', Actual version: '[MISSING]'\n\tMod ID: 'geckolib', Requested by: 'mymod', Expected range: '[4.4,)', Actual version: '[MISSING]'";
        let found = ids(log);
        assert!(found.contains(&"mods.missing_dependency.kotlinforforge".to_string()), "{found:?}");
        assert!(found.contains(&"mods.missing_dependency.geckolib".to_string()), "{found:?}");
    }

    #[test]
    fn quilt_and_plain_requirements_are_read_too() {
        assert_eq!(
            ids("Sodium Extra requires any version of sodium, which is missing!"),
            vec!["mods.missing_dependency.sodium"]
        );
        assert_eq!(
            ids("Mod needs_api requires fabric-api.\nCurrently, fabric-api is not installed."),
            vec!["mods.missing_dependency.fabric_api"]
        );
    }

    #[test]
    fn a_generic_incompatibility_is_found() {
        let f = &diagnosed("Incompatible mod set!")[0];
        assert_eq!(
            (f.id.as_str(), f.confidence, f.priority),
            ("mods.incompatible.generic", Confidence::High, 80)
        );
    }

    #[test]
    fn a_channel_mismatch_shows_its_lines() {
        let log = "Mismatched mod channel list\nclient channel create:main is missing on the server\nother";
        let f = &diagnosed(log)[0];
        assert_eq!((f.id.as_str(), f.priority), ("network.channel_mismatch", 70));
        assert_eq!(f.evidence.len(), 2);
    }

    #[test]
    fn vulkanmod_without_a_gpu_is_found() {
        let log = "java.lang.RuntimeException: Failed to find a suitable GPU\n\tat net.vulkanmod.vulkan.device.DeviceManager.pickPhysicalDevice";
        let f = &diagnosed(log)[0];
        assert_eq!((f.id.as_str(), f.priority), ("graphics.vulkan_device", 105));
    }

    #[test]
    fn a_graphics_line_is_found_but_only_under_a_thousand_characters() {
        let f = &diagnosed("[Render thread/ERROR]: OpenGL error: GL_INVALID_OPERATION")[0];
        assert_eq!((f.id.as_str(), kinds(f)), ("graphics.initialization", vec![FixKind::OpenDiagnostics]));
        let long = format!("OpenGL error {}", "x".repeat(1200));
        assert_eq!(ids(&long), vec!["launch.unknown"]);
    }

    #[test]
    fn evidence_is_at_most_four_lines_of_three_hundred_characters() {
        let lines: Vec<String> =
            (0..6).map(|n| format!("The process cannot access the file {n}{}", "y".repeat(500))).collect();
        let log = lines.join("\n");
        let f = &diagnosed(&log)[0];
        assert_eq!(f.evidence.len(), 4);
        assert!(f.evidence.iter().all(|e| e.chars().count() <= 300));
    }

    #[test]
    fn the_line_that_names_what_is_missing_shows_first() {
        let f = &diagnosed(FABRIC_LOG_FOR_TESTS)[0];
        assert_eq!(f.id, "mods.missing_dependency.fabric_api");
        assert!(f.evidence[0].contains("which is missing"), "{:?}", f.evidence);
    }

    #[test]
    fn a_line_found_in_two_logs_is_shown_once() {
        let log = format!(
            "{FABRIC_LOG_FOR_TESTS}
{FABRIC_LOG_FOR_TESTS}"
        );
        let f = &diagnosed(&log)[0];
        let mut unique = f.evidence.clone();
        unique.dedup();
        assert_eq!(f.evidence, unique);
        let lines = diagnosed(
            "The process cannot access the file a.jar
The process cannot access the file a.jar",
        );
        assert_eq!(lines[0].evidence.len(), 1);
    }
}
