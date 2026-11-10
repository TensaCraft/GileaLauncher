//! Horizon's registry in the browser preview: its content mods, shader packs and a resource pack,
//! as a Horizon build's source; an installed one lands in the build's list (the preview's
//! content), a mod on, a pack off.

use std::cell::RefCell;
use std::rc::Rc;

use launcher_shared::provider::{
    Action, FileNote, InstallAnswer, InstallArgs, InstallOutcome, Overview, OverviewArgs, PlanDto, PlanItem,
    ProjectHit, SearchArgs, SearchPage, installed_key,
};
use launcher_shared::{AppError, AppResult, ContentItem, ContentKind, ErrorCode, Level, Text};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::mock_builds::{to_value, toast};
use crate::mock_content::MockContent;

/// Id, kind, name, author, summary, downloads, version, file, size: as the registry lists them.
type Entry = (
    &'static str,
    ContentKind,
    &'static str,
    &'static str,
    &'static str,
    u64,
    &'static str,
    &'static str,
    u64,
);

const REGISTRY: [Entry; 12] = [
    (
        "additional_lights",
        ContentKind::Mods,
        "Additional Lights",
        "mgen256, AdLf0208",
        "80 decorative lamps, floor torches and wall torches with the mod's own light levels and recipes.",
        204,
        "3.1.4",
        "additional_lights.hmod",
        262_863,
    ),
    (
        "factory_blocks",
        ContentKind::Mods,
        "Factory Blocks",
        "slaincow, AdLf0208",
        "49 industrial decorative blocks: panels, grates, vents, pipes and hazard stripes for Horizon.",
        159,
        "1.4.2",
        "factory_blocks.hmod",
        776_638,
    ),
    (
        "mcwtrpdoors",
        ContentKind::Mods,
        "Macaw's Trapdoors",
        "Sketch Macaw, Peachy Macaw, AdLf0208",
        "All 179 of Macaw's Trapdoors: vanilla-matched woods plus glass, metal and themed sets.",
        127,
        "1.1.5",
        "mcwtrpdoors.hmod",
        605_483,
    ),
    (
        "silentgems",
        ContentKind::Mods,
        "Silent's Gems",
        "SilentChaos512, AdLf0208",
        "21 gems with storage blocks and 84 ores, plus overworld, nether and end worldgen.",
        137,
        "5.2.0",
        "silentgems.hmod",
        302_704,
    ),
    (
        "flesh_to_leather",
        ContentKind::Mods,
        "Flesh to Leather",
        "CGRLabs",
        "Smelt rotten flesh into leather in a furnace.",
        115,
        "1.0.0",
        "flesh_to_leather.hmod",
        812,
    ),
    (
        "netherstarrecipe",
        ContentKind::Mods,
        "Nether Star Crafting",
        "Mya",
        "Adds a Netherstar Shard and a crafting recipe for the Nether Star.",
        656,
        "1.0.0",
        "netherstarrecipe.hmod",
        2_726,
    ),
    (
        "daybreak",
        ContentKind::ShaderPacks,
        "Daybreak",
        "hildlabs",
        "Sun shadows, reflective water, waving plants, coloured light, sun rays and bloom, built to cost as little GPU time as possible.",
        6107,
        "0.1.1",
        "daybreak-0.1.1.hshader",
        20_268,
    ),
    (
        "montarc",
        ContentKind::ShaderPacks,
        "Montarc",
        "Muurrcc",
        "Bright, colorful shader pack: soft shadows, colored light, living water, wind, and own look for the Nether and the End.",
        4714,
        "1.1.1",
        "montarc-1.1.1.hshader",
        23_426,
    ),
    (
        "radiant",
        ContentKind::ShaderPacks,
        "Radiant",
        "Daniel",
        "Cinematic but playable: warm sun, soft shadows, light shafts, golden haze, sunset clouds, wet rainy ground and a filmic grade.",
        4730,
        "1.0.0",
        "radiant-1.0.0.hshader",
        42_445,
    ),
    (
        "wisdom_horizon",
        ContentKind::ShaderPacks,
        "Wisdom Shaders V3 (Horizon port)",
        "bobcao3, tomasmendes01",
        "bobcao3's Wisdom Shaders V3 ported to Horizon's native shader API.",
        21_400,
        "1.3.7",
        "wisdom_horizon-1.3.7.hshader",
        39_196,
    ),
    (
        "wisdom_horizon_lite",
        ContentKind::ShaderPacks,
        "Wisdom Shaders V3 Lite (Horizon port)",
        "bobcao3, tomasmendes01",
        "A lighter Wisdom Shaders V3 port for Horizon, for more FPS.",
        17_814,
        "1.3.7",
        "wisdom_horizon_lite-1.3.7.hshader",
        37_136,
    ),
    (
        "menu_panorama",
        ContentKind::ResourcePacks,
        "Menu Panorama",
        "form1t",
        "Put your own wallpaper behind the main menu in Horizon — Minecraft 26.3, rewritten in Rust.",
        2184,
        "1.0.0",
        "menu_panorama-1.0.0.zip",
        467_828,
    ),
];

fn parse<T: DeserializeOwned>(value: &Value) -> AppResult<T> {
    serde_json::from_value(value.clone()).map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))
}

pub struct MockHorizon {
    content: Rc<RefCell<MockContent>>,
}

impl MockHorizon {
    pub fn new(content: Rc<RefCell<MockContent>>) -> MockHorizon {
        MockHorizon { content }
    }

    /// Whether build `key` has `file` among its `kind`.
    fn has(&self, key: &str, kind: ContentKind, file: &str) -> bool {
        self.content.borrow_mut().list(key, kind).items.iter().any(|i| i.file == file)
    }

    fn plan(&self, args: &InstallArgs) -> AppResult<(Entry, PlanDto)> {
        let entry = *REGISTRY
            .iter()
            .find(|e| e.0 == args.project_id && e.1 == args.kind)
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "mock").with_param("name", &args.project_id))?;
        let (id, _, name, .., version, file, _) = entry;
        let there = self.has(&args.key, args.kind, file);
        let main = PlanItem {
            project_id: id.into(),
            version_id: format!("{id}-sha256"),
            title: name.into(),
            version_number: version.into(),
            filename: file.into(),
            action: if there { Action::Satisfied } else { Action::Install },
            current: None,
            url: None,
            unrecognized: false,
        };
        let plan = PlanDto {
            main: Some(main),
            install: Vec::new(),
            replace: Vec::new(),
            satisfied: Vec::new(),
            optional: Vec::new(),
            optional_issues: Vec::new(),
            embedded: Vec::new(),
            blocking: Vec::new(),
        };
        Ok((entry, plan))
    }

    /// Answers provider command `cmd` (`provider_*`) for provider `horizon`.
    pub fn handle(&mut self, cmd: &str, call: &Value) -> AppResult<Value> {
        let args = &call["args"];
        match cmd {
            "provider_search" => {
                let args: SearchArgs = parse(args)?;
                let query = args.query.trim().to_lowercase();
                let found: Vec<ProjectHit> = REGISTRY
                    .iter()
                    .filter(|e| e.1 == args.kind)
                    .filter(|e| {
                        query.is_empty()
                            || e.2.to_lowercase().contains(&query)
                            || e.4.to_lowercase().contains(&query)
                    })
                    .map(|&(id, _, name, author, summary, downloads, ..)| ProjectHit {
                        project_id: id.into(),
                        slug: id.into(),
                        title: name.into(),
                        author: author.into(),
                        description: summary.into(),
                        downloads,
                        icon_url: None,
                        url: Some(format!("https://github.com/horizon-rust/horizon-mods/{id}")),
                    })
                    .collect();
                let total = found.len() as u32;
                let hits =
                    found.into_iter().skip(args.offset as usize).take(args.page_size() as usize).collect();
                to_value(SearchPage { hits, total, offset: args.offset, limit: args.page_size() })
            }
            "provider_plan" => to_value(self.plan(&parse(args)?)?.1),
            "provider_install" => {
                let args: InstallArgs = parse(args)?;
                let ((id, kind, name, _, summary, _, version, file, size), plan) = self.plan(&args)?;
                let main = plan.main.clone().unwrap_or_else(|| unreachable!("the plan has its pack"));
                if !plan.changes(&[]).iter().all(|c| args.approved.contains(c)) {
                    return to_value(InstallAnswer::Replanned(Box::new(plan)));
                }
                // A mod is on once installed (its manifest names it); a pack is switched in the list.
                let content_mod = kind == ContentKind::Mods;
                let item = ContentItem {
                    file: file.into(),
                    filename: file.into(),
                    size,
                    enabled: content_mod,
                    folder: false,
                    toggle_supported: true,
                    name: content_mod.then(|| name.into()),
                    version: content_mod.then(|| version.into()),
                    description: content_mod.then(|| summary.into()),
                    mod_id: content_mod.then(|| id.into()),
                    has_backup: false,
                };
                self.content.borrow_mut().put(&args.key, kind, item);
                toast(Level::Success, Text::key(installed_key(kind)).param("name", name));
                to_value(InstallAnswer::Installed(InstallOutcome {
                    project_id: main.project_id,
                    filename: main.filename,
                    version_number: main.version_number,
                }))
            }
            "provider_overview" => {
                let args: OverviewArgs = parse(args)?;
                let notes: Vec<FileNote> = REGISTRY
                    .iter()
                    .filter(|e| e.1 == args.kind && self.has(&args.key, args.kind, e.7))
                    .map(|&(id, _, name, .., version, file, _)| FileNote {
                        file: file.into(),
                        project_id: id.into(),
                        slug: id.into(),
                        title: name.into(),
                        version_number: version.into(),
                        update: None,
                        url: Some(format!("https://github.com/horizon-rust/horizon-mods/{id}")),
                        icon_url: None,
                        installed: true,
                    })
                    .collect();
                to_value(Overview { notes, updates: None })
            }
            other => Err(AppError::new(ErrorCode::InvalidInput, format!("mock horizon: no {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(args: Value) -> Value {
        json!({"provider": module_horizon::ID, "args": args})
    }

    #[test]
    fn the_preview_installs_a_registry_shader_into_the_build_s_list() {
        let content = Rc::new(RefCell::new(MockContent::new()));
        content.borrow_mut().set_engine("horizon", "Horizon");
        let mut horizon = MockHorizon::new(content.clone());
        let search = json!({"key": "horizon", "kind": "shaderpacks", "query": "", "offset": 0});
        let page: SearchPage = parse(&horizon.handle("provider_search", &call(search)).unwrap()).unwrap();
        assert_eq!(page.hits.len(), 5, "its shader packs alone");
        assert!(content.borrow_mut().list("horizon", ContentKind::ShaderPacks).items.is_empty());
        let args = InstallArgs::new("horizon", ContentKind::ShaderPacks, "daybreak", "daybreak", "Daybreak");
        let plan: PlanDto = parse(&horizon.handle("provider_plan", &call(json!(args))).unwrap()).unwrap();
        let approved = InstallArgs { approved: plan.changes(&[]), ..args };
        horizon.handle("provider_install", &call(json!(approved))).unwrap();
        let listed = content.borrow_mut().list("horizon", ContentKind::ShaderPacks);
        assert_eq!(
            listed.items.iter().map(|i| i.file.as_str()).collect::<Vec<_>>(),
            ["daybreak-0.1.1.hshader"]
        );
        assert!(listed.items.iter().all(|i| i.toggle_supported && !i.enabled), "switched in the list, off");
        let overview = json!({"key": "horizon", "kind": "shaderpacks", "check_updates": false});
        let overview: Overview =
            parse(&horizon.handle("provider_overview", &call(overview)).unwrap()).unwrap();
        assert_eq!(overview.notes[0].project_id, "daybreak");
    }

    #[test]
    fn the_preview_installs_a_content_mod_on() {
        let content = Rc::new(RefCell::new(MockContent::new()));
        content.borrow_mut().set_engine("horizon", "Horizon");
        let mut horizon = MockHorizon::new(content.clone());
        let search = json!({"key": "horizon", "kind": "mods", "query": "", "offset": 0});
        let page: SearchPage = parse(&horizon.handle("provider_search", &call(search)).unwrap()).unwrap();
        let ids: Vec<&str> = page.hits.iter().map(|h| h.project_id.as_str()).collect();
        assert!(ids.contains(&"flesh_to_leather") && !ids.contains(&"daybreak"), "{ids:?}");
        let args =
            InstallArgs::new("horizon", ContentKind::Mods, "flesh_to_leather", "flesh_to_leather", "Flesh");
        let plan: PlanDto = parse(&horizon.handle("provider_plan", &call(json!(args))).unwrap()).unwrap();
        let approved = InstallArgs { approved: plan.changes(&[]), ..args };
        horizon.handle("provider_install", &call(json!(approved))).unwrap();
        let mods = content.borrow_mut().list("horizon", ContentKind::Mods);
        let view: Vec<(&str, bool, Option<&str>)> =
            mods.items.iter().map(|i| (i.file.as_str(), i.enabled, i.name.as_deref())).collect();
        assert_eq!(view, [("flesh_to_leather.hmod", true, Some("Flesh to Leather"))]);
    }
}
