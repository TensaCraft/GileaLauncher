use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use launcher_core::backups::BuildBackups;
use launcher_core::content::backups;
use launcher_core::content::service::ContentService;
use launcher_core::feedback::{FeedbackService, NullSink};
use launcher_core::launch::engine::EngineContent;
use launcher_core::launch::options::game_dir;
use launcher_core::lock::Coordinator;
use launcher_core::storage::versions::{Build, VersionStore};
use launcher_shared::{ContentItem, ContentKind, ErrorCode};

struct World {
    _tmp: tempfile::TempDir,
    versions: Arc<VersionStore>,
    instances: Arc<Coordinator>,
    service: ContentService,
    /// The backups folder.
    backups: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let (state, mc) = (tmp.path().join("state"), tmp.path().join("minecraft"));
    fs::create_dir_all(&state).unwrap();
    let versions = Arc::new(VersionStore::open(&state, &mc));
    let instances = Arc::new(Coordinator::instances());
    let backups = tmp.path().join("backups");
    let root = backups.clone();
    let service = ContentService::new(
        versions.clone(),
        instances.clone(),
        FeedbackService::new(Arc::new(NullSink)),
        tmp.path().join("cache").join("thumbs"),
        Arc::new(move || root.clone()),
    )
    .with_engine_content(vec![(ENGINE, GAME)]);
    World { _tmp: tmp, versions, instances, service, backups }
}

/// Build `key`'s mods' backups, in the backups folder.
fn shelf(w: &World, key: &str) -> PathBuf {
    BuildBackups::of(&w.backups, key).mods()
}

/// A build of `client` running `component`, and its game folder with `mods/`, `resourcepacks/`
/// and `shaderpacks/`.
fn build(w: &World, name: &str, client: &str, component: &str) -> (String, PathBuf) {
    build_on(w, name, client, component, "1.21.1")
}

fn build_on(w: &World, name: &str, client: &str, component: &str, mc: &str) -> (String, PathBuf) {
    let mut b = Build::new(name);
    b.client = Some(client.into());
    b.version = Some(mc.into());
    b.loader = Some(component.into());
    w.versions.create(&mut b).unwrap();
    let dir = game_dir(&b, w.versions.minecraft_dir());
    for kind in ContentKind::ALL {
        fs::create_dir_all(dir.join(kind.folder())).unwrap();
    }
    (b.key, dir)
}

fn jar(path: &Path, descriptor: &[u8]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("fabric.mod.json", options).unwrap();
    zip.write_all(descriptor).unwrap();
    zip.finish().unwrap();
}

fn fabric_mod(dir: &Path, file: &str, id: &str, name: &str) {
    jar(
        &dir.join("mods").join(file),
        format!(r#"{{"id":"{id}","name":"{name}","version":"1.0"}}"#).as_bytes(),
    );
}

fn files(items: &[ContentItem]) -> Vec<(&str, bool)> {
    items.iter().map(|i| (i.file.as_str(), i.enabled)).collect()
}

const FABRIC: &str = "fabric-loader-0.16.9-1.21.1";

#[test]
fn mods_switch_and_delete() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium");
    fabric_mod(&dir, "lithium.jar.disabled", "lithium", "Lithium");
    let list = w.service.list(&key, ContentKind::Mods).unwrap();
    assert!(list.supported);
    assert_eq!(files(&list.items), [("lithium.jar.disabled", false), ("sodium.jar", true)]);
    let off = w.service.toggle(&key, ContentKind::Mods, "sodium.jar", false).unwrap();
    assert_eq!(files(&off.items), [("lithium.jar.disabled", false), ("sodium.jar.disabled", false)]);
    let on = w.service.toggle(&key, ContentKind::Mods, "lithium.jar.disabled", true).unwrap();
    assert_eq!(files(&on.items), [("lithium.jar", true), ("sodium.jar.disabled", false)]);
    let left = w.service.delete(&key, ContentKind::Mods, "sodium.jar.disabled").unwrap();
    assert_eq!(files(&left.items), [("lithium.jar", true)]);
    assert!(!dir.join("mods").join("sodium.jar.disabled").exists());
}

#[test]
fn vanilla_builds_run_no_mods() {
    let w = world();
    let (key, dir) = build(&w, "Vanilla", "Minecraft", "1.21.1");
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium");
    let list = w.service.list(&key, ContentKind::Mods).unwrap();
    assert!(!list.supported && list.items.is_empty());
    assert_eq!(w.service.list("ghost", ContentKind::Mods).unwrap_err().code, ErrorCode::VersionNotFound);
    assert_eq!(
        w.service.dir(&key, ContentKind::ShaderPacks).unwrap(),
        dir.join("shaderpacks"),
        "Open folder gets the kind's folder"
    );
}

#[test]
fn a_switch_never_overwrites_a_twin() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fabric_mod(&dir, "a.jar", "a", "A new");
    fabric_mod(&dir, "a.jar.disabled", "a", "A old");
    let before =
        (fs::read(dir.join("mods/a.jar")).unwrap(), fs::read(dir.join("mods/a.jar.disabled")).unwrap());
    for (file, twin) in [("a.jar", "a.jar.disabled"), ("a.jar.disabled", "a.jar")] {
        let err = w.service.toggle(&key, ContentKind::Mods, file, twin == "a.jar").unwrap_err();
        assert_eq!((err.code, err.params["name"].as_str()), (ErrorCode::ContentConflict, twin), "{file}");
    }
    assert_eq!(
        (fs::read(dir.join("mods/a.jar")).unwrap(), fs::read(dir.join("mods/a.jar.disabled")).unwrap()),
        before
    );
}

#[test]
fn only_listed_files_can_change() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fs::create_dir_all(dir.join("mods").join("sub")).unwrap();
    fabric_mod(&dir, "sub/inner.jar", "inner", "Inner");
    fs::write(dir.join("keep.jar"), b"outside").unwrap();
    for file in ["../keep.jar", "sub/inner.jar", "sub", "missing.jar", "..", ""] {
        assert_eq!(
            w.service.toggle(&key, ContentKind::Mods, file, true).unwrap_err().code,
            ErrorCode::NotFound,
            "{file}"
        );
        assert_eq!(
            w.service.delete(&key, ContentKind::Mods, file).unwrap_err().code,
            ErrorCode::NotFound,
            "{file}"
        );
    }
    assert!(dir.join("keep.jar").exists() && dir.join("mods/sub/inner.jar").exists());
}

#[test]
fn a_running_game_does_not_stop_changes() {
    let w = world();
    let (key, dir) = build(&w, "Моя збірка", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium");
    fabric_mod(&dir, "lithium.jar", "lithium", "Lithium");
    let off = w.service.toggle(&key, ContentKind::Mods, "sodium.jar", false).unwrap();
    assert!(off.items.iter().any(|i| i.file == "sodium.jar.disabled" && !i.enabled));
    w.service.delete(&key, ContentKind::Mods, "lithium.jar").unwrap();
    assert!(!dir.join("mods/lithium.jar").exists());
}

/// A file the game holds open, as its JVM holds a loaded jar: Windows refuses to move it, and the
/// error says so instead of a bare file system error.
#[cfg(windows)]
#[test]
fn a_file_the_game_holds_is_in_use() {
    use std::os::windows::fs::OpenOptionsExt;
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium");
    // FILE_SHARE_READ | FILE_SHARE_WRITE: shared with readers and writers, not with a move.
    let held = fs::OpenOptions::new().read(true).share_mode(1 | 2).open(dir.join("mods/sodium.jar")).unwrap();
    let err = w.service.toggle(&key, ContentKind::Mods, "sodium.jar", false).unwrap_err();
    assert_eq!(err.code, ErrorCode::FileInUse);
    drop(held);
    assert!(dir.join("mods/sodium.jar").exists());
}

#[test]
fn busy_builds_refuse_changes() {
    let w = world();
    let (key, dir) = build(&w, "Моя збірка", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium");
    let lease = w.instances.try_acquire(&dir, "copy").unwrap();
    let busy = w.service.delete(&key, ContentKind::Mods, "sodium.jar").unwrap_err();
    assert_eq!((busy.code, busy.params["version"].as_str()), (ErrorCode::InstanceBusy, "Моя збірка"));
    drop(lease);
    assert!(dir.join("mods/sodium.jar").exists());
    assert!(w.service.list(&key, ContentKind::Mods).unwrap().items[0].enabled);
}

#[test]
fn resource_packs_follow_options_txt() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Minecraft", "1.21.1");
    let packs = dir.join("resourcepacks");
    fs::write(packs.join("A.zip"), b"a").unwrap();
    fs::write(packs.join("Б пак.zip"), b"b").unwrap();
    fs::write(packs.join("old.zip.disabled"), b"o").unwrap();
    fs::create_dir_all(packs.join("Folder")).unwrap();
    let options = dir.join("options.txt");
    fs::write(
        &options,
        "version:3953\nresourcePacks:[\"vanilla\",\"file/A.zip\"]\nlang:uk_ua\nincompatibleResourcePacks:[\"file/A.zip\"]\n",
    )
    .unwrap();
    let list = w.service.list(&key, ContentKind::ResourcePacks).unwrap();
    assert_eq!(
        files(&list.items),
        [("A.zip", true), ("Folder", false), ("old.zip.disabled", false), ("Б пак.zip", false)]
    );
    w.service.toggle(&key, ContentKind::ResourcePacks, "Б пак.zip", true).unwrap();
    assert_eq!(
        fs::read_to_string(&options).unwrap(),
        "version:3953\nresourcePacks:[\"vanilla\",\"file/A.zip\",\"file/Б пак.zip\"]\nlang:uk_ua\nincompatibleResourcePacks:[\"file/A.zip\"]\n"
    );
    w.service.toggle(&key, ContentKind::ResourcePacks, "A.zip", false).unwrap();
    let text = fs::read_to_string(&options).unwrap();
    assert!(
        text.contains("resourcePacks:[\"vanilla\",\"file/Б пак.zip\"]")
            && text.contains("incompatibleResourcePacks:[]")
    );
    let restored = w.service.toggle(&key, ContentKind::ResourcePacks, "old.zip.disabled", true).unwrap();
    assert!(
        files(&restored.items).contains(&("old.zip", true)),
        "a legacy .disabled pack gets its name back"
    );
    w.service.toggle(&key, ContentKind::ResourcePacks, "Folder", true).unwrap();
    let after = w.service.delete(&key, ContentKind::ResourcePacks, "Folder").unwrap();
    assert!(!packs.join("Folder").exists() && !after.items.iter().any(|i| i.file == "Folder"));
    assert!(!fs::read_to_string(&options).unwrap().contains("file/Folder"));
}

#[test]
fn shader_packs_switch_through_iris() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let shaders = dir.join("shaderpacks");
    fs::write(shaders.join("BSL.zip"), b"s").unwrap();
    let unsupported = w.service.toggle(&key, ContentKind::ShaderPacks, "BSL.zip", true).unwrap_err();
    assert_eq!(unsupported.code, ErrorCode::Unsupported);
    fabric_mod(&dir, "iris-fabric-1.8.0.jar", "iris", "Iris");
    let properties = dir.join("config").join("iris.properties");
    fs::create_dir_all(properties.parent().unwrap()).unwrap();
    fs::write(&properties, "#Iris settings\nenableShaders=false\n").unwrap();
    let on = w.service.toggle(&key, ContentKind::ShaderPacks, "BSL.zip", true).unwrap();
    assert_eq!(files(&on.items), [("BSL.zip", true)]);
    assert_eq!(
        fs::read_to_string(&properties).unwrap(),
        "#Iris settings\nenableShaders=true\nshaderPack=BSL.zip\n"
    );
    w.service.delete(&key, ContentKind::ShaderPacks, "BSL.zip").unwrap();
    assert!(!shaders.join("BSL.zip").exists());
    assert!(fs::read_to_string(&properties).unwrap().contains("enableShaders=false"));
}

#[test]
fn a_module_s_game_lists_and_takes_its_own_shader_packs() {
    let w = world();
    let mut horizon = Build::new("Horizon");
    horizon.client = Some("Horizon".into());
    horizon.version = Some("26.3".into());
    horizon.options.insert("engine".into(), "horizon".into());
    w.versions.create(&mut horizon).unwrap();
    let shaders = game_dir(&horizon, w.versions.minecraft_dir()).join("shaderpacks");
    fs::create_dir_all(&shaders).unwrap();
    fs::write(shaders.join("daybreak-0.1.1.hshader"), b"h").unwrap();
    fs::write(shaders.join("Folder.zip"), b"z").unwrap();
    fs::write(shaders.join("daybreak.txt"), b"options").unwrap();
    let radiant = outside(&w, "radiant-1.0.0.hshader", b"r");
    let list = w.service.add(&horizon.key, ContentKind::ShaderPacks, &radiant, false).unwrap();
    assert_eq!(
        files(&list.items),
        [("daybreak-0.1.1.hshader", true), ("Folder.zip", true), ("radiant-1.0.0.hshader", true)],
        "the game picks among them itself: none is off"
    );
    assert!(list.items.iter().all(|i| !i.toggle_supported));
    w.service.delete(&horizon.key, ContentKind::ShaderPacks, "daybreak-0.1.1.hshader").unwrap();
    assert!(!shaders.join("daybreak-0.1.1.hshader").exists());
    assert!(!shaders.parent().unwrap().join("config").exists(), "no Iris settings appear");

    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fs::write(dir.join("shaderpacks/daybreak-0.1.1.hshader"), b"h").unwrap();
    assert!(w.service.list(&key, ContentKind::ShaderPacks).unwrap().items.is_empty(), "Iris reads none");
    let err = w.service.add(&key, ContentKind::ShaderPacks, &radiant, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

/// How the test's own game (`ENGINE`) keeps its content: a module tells its game's.
const GAME: EngineContent = EngineContent {
    mods: "game-mods",
    mods_list: "game-mods.txt",
    mod_manifest: "game-mod.toml",
    graphics: "game-graphics.txt",
    shader_pick: "pickedShaderPack",
};
const ENGINE: &str = "game";

/// A build engine `ENGINE` runs, and its game folder.
fn engine_build(w: &World) -> (String, PathBuf) {
    let mut b = Build::new("Двигун");
    b.client = Some("Двигун".into());
    b.version = Some("26.3".into());
    b.options.insert("engine".into(), ENGINE.into());
    w.versions.create(&mut b).unwrap();
    let dir = game_dir(&b, w.versions.minecraft_dir());
    fs::create_dir_all(dir.join(GAME.mods)).unwrap();
    (b.key, dir)
}

/// A content mod of the test's game at `path`: a zip with its manifest at the root.
fn game_mod(path: &Path, manifest: &str) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file(GAME.mod_manifest, options).unwrap();
    zip.write_all(manifest.as_bytes()).unwrap();
    zip.start_file("assets/x/lang/en_us.json", options).unwrap();
    zip.write_all(b"{}").unwrap();
    zip.finish().unwrap();
}

fn manifest(id: &str, name: &str, version: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"{name}\"\nversion = \"{version}\"\ndescription = \"{name} does it\"\napi = \"0.2\"\nside = \"both\"\n"
    )
}

#[test]
fn a_module_s_game_lists_switches_and_deletes_its_own_mods() {
    let w = world();
    let (key, dir) = engine_build(&w);
    let mods = dir.join(GAME.mods);
    game_mod(&mods.join("flesh_to_leather.hmod"), &manifest("flesh_to_leather", "Flesh to Leather", "1.0.0"));
    game_mod(&mods.join("lights.hmod"), &manifest("additional_lights", "Additional Lights", "3.1.4"));
    fs::write(mods.join("broken.hmod"), b"not a zip").unwrap();
    fs::write(mods.join("sodium.jar"), b"java").unwrap();
    fs::create_dir_all(mods.join(".horizon-mod-data")).unwrap();
    let list_file = dir.join(GAME.mods_list);
    fs::write(&list_file, "# the player's\nmod.additional_lights:true\nmod.gone:true\n").unwrap();

    let list = w.service.list(&key, ContentKind::Mods).unwrap();
    assert!(list.supported, "its game's own mods, no loader");
    let view: Vec<String> = list
        .items
        .iter()
        .map(|i| format!("{} {} {:?} {:?} {:?}", i.file, i.enabled, i.name, i.version, i.mod_id))
        .collect();
    assert_eq!(
        view,
        [
            r#"lights.hmod true Some("Additional Lights") Some("3.1.4") Some("additional_lights")"#,
            r#"broken.hmod false None None Some("broken")"#,
            r#"flesh_to_leather.hmod false Some("Flesh to Leather") Some("1.0.0") Some("flesh_to_leather")"#,
        ],
        "by name; on by its line, a mod without one is off"
    );
    assert!(list.items.iter().all(|i| i.toggle_supported && !i.has_backup));
    assert_eq!(w.service.dir(&key, ContentKind::Mods).unwrap(), mods, "Open folder gets its game's");

    let on = w.service.toggle(&key, ContentKind::Mods, "flesh_to_leather.hmod", true).unwrap();
    assert!(
        on.items.iter().any(|i| i.file == "flesh_to_leather.hmod" && i.enabled),
        "the file keeps its name"
    );
    w.service.toggle(&key, ContentKind::Mods, "lights.hmod", false).unwrap();
    assert_eq!(
        fs::read_to_string(&list_file).unwrap(),
        "# the player's\nmod.additional_lights:false\nmod.gone:true\nmod.flesh_to_leather:true\n",
        "only its own line"
    );
    assert!(mods.join("lights.hmod").exists() && !mods.join("lights.hmod.disabled").exists());

    let left = w.service.delete(&key, ContentKind::Mods, "flesh_to_leather.hmod").unwrap();
    assert_eq!(files(&left.items), [("lights.hmod", false), ("broken.hmod", false)]);
    assert!(!mods.join("flesh_to_leather.hmod").exists());
    assert_eq!(
        fs::read_to_string(&list_file).unwrap(),
        "# the player's\nmod.additional_lights:false\nmod.gone:true\n",
        "its line goes with it"
    );
    assert_eq!(
        w.service.toggle(&key, ContentKind::Mods, "sodium.jar", false).unwrap_err().code,
        ErrorCode::NotFound,
        "no Java mods"
    );
}

#[test]
fn a_dropped_mod_of_a_module_s_game_is_saved_by_its_id_and_on() {
    let w = world();
    let (key, dir) = engine_build(&w);
    let dropped = w._tmp.path().join("outside");
    fs::create_dir_all(&dropped).unwrap();
    let flesh = dropped.join("flesh_to_leather-1.0.0.hmod");
    game_mod(&flesh, &manifest("flesh_to_leather", "Flesh to Leather", "1.0.0"));
    let list = w.service.add(&key, ContentKind::Mods, &flesh, false).unwrap();
    assert_eq!(files(&list.items), [("flesh_to_leather.hmod", true)], "saved as <id>.hmod and on");
    assert_eq!(fs::read_to_string(dir.join(GAME.mods_list)).unwrap(), "mod.flesh_to_leather:true\n");
    assert!(flesh.exists(), "the dropped file stays");
    let twin = w.service.add(&key, ContentKind::Mods, &flesh, false).unwrap_err();
    assert_eq!(twin.code, ErrorCode::ContentConflict, "the same mod asks first");
    w.service.toggle(&key, ContentKind::Mods, "flesh_to_leather.hmod", false).unwrap();
    let again = w.service.add(&key, ContentKind::Mods, &flesh, true).unwrap();
    assert_eq!(files(&again.items), [("flesh_to_leather.hmod", true)], "a dropped mod is on");

    let no_manifest = outside(&w, "plain.hmod", b"not a zip");
    let bad_id = dropped.join("bad.hmod");
    game_mod(&bad_id, &manifest("../evil", "Evil", "1"));
    for (file, why) in [(&no_manifest, "no manifest"), (&bad_id, "an id that is no plain name")] {
        assert_eq!(
            w.service.add(&key, ContentKind::Mods, file, false).unwrap_err().code,
            ErrorCode::InvalidInput,
            "{why}"
        );
    }
    let java = outside(&w, "sodium.jar", b"PK");
    assert_eq!(
        w.service.add(&key, ContentKind::Mods, &java, false).unwrap_err().code,
        ErrorCode::InvalidInput
    );

    let (fabric, _) = build(&w, "Aero", "Fabric", FABRIC);
    let err = w.service.add(&fabric, ContentKind::Mods, &flesh, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "a Java build takes jars alone");
}

#[test]
fn a_module_s_game_switches_its_shader_pack_and_resource_packs() {
    let w = world();
    let (key, dir) = engine_build(&w);
    let shaders = dir.join("shaderpacks");
    fs::create_dir_all(&shaders).unwrap();
    fs::write(shaders.join("daybreak-0.1.1.hshader"), b"h").unwrap();
    fs::write(shaders.join("bsl.zip"), b"z").unwrap();
    fs::write(shaders.join("daybreak-0.1.1.hshader.txt"), b"sunPath=30").unwrap();
    let graphics = dir.join(GAME.graphics);
    let list = w.service.list(&key, ContentKind::ShaderPacks).unwrap();
    assert_eq!(files(&list.items), [("bsl.zip", false), ("daybreak-0.1.1.hshader", false)], "none picked");
    assert!(list.items.iter().all(|i| i.toggle_supported), "no Iris needed");
    w.service.toggle(&key, ContentKind::ShaderPacks, "daybreak-0.1.1.hshader", true).unwrap();
    assert_eq!(
        fs::read_to_string(&graphics).unwrap(),
        "pickedShaderPack:daybreak-0.1.1.hshader
",
        "made when missing"
    );
    fs::write(
        &graphics,
        "renderDistance:12
pickedShaderPack:daybreak-0.1.1.hshader
fov:90
",
    )
    .unwrap();
    let on = w.service.toggle(&key, ContentKind::ShaderPacks, "bsl.zip", true).unwrap();
    assert_eq!(files(&on.items), [("bsl.zip", true), ("daybreak-0.1.1.hshader", false)], "one at a time");
    assert_eq!(
        fs::read_to_string(&graphics).unwrap(),
        "renderDistance:12
pickedShaderPack:bsl.zip
fov:90
"
    );
    let off = w.service.toggle(&key, ContentKind::ShaderPacks, "bsl.zip", false).unwrap();
    assert!(off.items.iter().all(|i| !i.enabled));
    assert_eq!(
        fs::read_to_string(&graphics).unwrap(),
        "renderDistance:12
pickedShaderPack:
fov:90
"
    );
    w.service.toggle(&key, ContentKind::ShaderPacks, "daybreak-0.1.1.hshader", true).unwrap();
    w.service.delete(&key, ContentKind::ShaderPacks, "daybreak-0.1.1.hshader").unwrap();
    assert_eq!(
        fs::read_to_string(&graphics).unwrap(),
        "renderDistance:12
pickedShaderPack:
fov:90
",
        "the picked pack's deletion clears the pick"
    );
    assert_eq!(
        fs::read(shaders.join("daybreak-0.1.1.hshader.txt")).unwrap(),
        b"sunPath=30",
        "its options stay"
    );
    assert!(!dir.join("config").exists(), "no Iris settings appear");

    let packs = dir.join("resourcepacks");
    fs::create_dir_all(packs.join("Folder")).unwrap();
    fs::write(packs.join("Folder").join("pack.mcmeta"), b"{}").unwrap();
    fs::write(packs.join("panorama.zip"), b"z").unwrap();
    let options = dir.join("options.txt");
    fs::write(
        &options,
        "resourcePacks:[\"vanilla\"]
lang:uk_ua
",
    )
    .unwrap();
    let on = w.service.toggle(&key, ContentKind::ResourcePacks, "panorama.zip", true).unwrap();
    assert_eq!(files(&on.items), [("Folder", false), ("panorama.zip", true)]);
    assert!(on.items.iter().all(|i| i.toggle_supported));
    assert_eq!(
        fs::read_to_string(&options).unwrap(),
        "resourcePacks:[\"vanilla\",\"file/panorama.zip\"]
lang:uk_ua
",
        "as vanilla lists them"
    );
    w.service.toggle(&key, ContentKind::ResourcePacks, "panorama.zip", false).unwrap();
    assert_eq!(
        fs::read_to_string(&options).unwrap(),
        "resourcePacks:[\"vanilla\"]
lang:uk_ua
"
    );
}

#[test]
fn broken_jars_never_break_the_list() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fs::write(dir.join("mods/not-a-zip.jar"), b"text").unwrap();
    jar(&dir.join("mods/huge.jar"), &vec![b' '; 2 * 1024 * 1024]);
    jar(&dir.join("mods/invalid.jar"), b"{not json");
    fabric_mod(&dir, "good.jar", "good", "Good");
    let list = w.service.list(&key, ContentKind::Mods).unwrap();
    let view: Vec<(&str, Option<&str>)> =
        list.items.iter().map(|i| (i.filename.as_str(), i.name.as_deref())).collect();
    assert_eq!(
        view,
        [("good.jar", Some("Good")), ("huge.jar", None), ("invalid.jar", None), ("not-a-zip.jar", None)]
    );
}

#[test]
fn a_stale_switch_keeps_what_the_user_asked_for() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fs::write(dir.join("resourcepacks/A.zip"), b"a").unwrap();
    let options = dir.join("options.txt");
    // The page showed A off; the game switched it on since.
    fs::write(&options, "resourcePacks:[\"vanilla\",\"file/A.zip\"]\n").unwrap();
    let on = w.service.toggle(&key, ContentKind::ResourcePacks, "A.zip", true).unwrap();
    assert_eq!(files(&on.items), [("A.zip", true)], "asked on, stays on");
    assert_eq!(fs::read_to_string(&options).unwrap(), "resourcePacks:[\"vanilla\",\"file/A.zip\"]\n");
    fabric_mod(&dir, "iris.jar", "iris", "Iris");
    fs::write(dir.join("shaderpacks/BSL.zip"), b"s").unwrap();
    let properties = dir.join("config").join("iris.properties");
    fs::create_dir_all(properties.parent().unwrap()).unwrap();
    fs::write(&properties, "enableShaders=true\nshaderPack=BSL.zip\n").unwrap();
    let shaders = w.service.toggle(&key, ContentKind::ShaderPacks, "BSL.zip", true).unwrap();
    assert_eq!(files(&shaders.items), [("BSL.zip", true)]);
    assert_eq!(fs::read_to_string(&properties).unwrap(), "enableShaders=true\nshaderPack=BSL.zip\n");
    let off = w.service.toggle(&key, ContentKind::ShaderPacks, "BSL.zip", false).unwrap();
    assert_eq!(files(&off.items), [("BSL.zip", false)]);
}

#[test]
fn old_minecraft_names_packs_without_prefix() {
    let w = world();
    let (key, dir) = build_on(&w, "Old", "Forge", "1.12.2-forge-14.23.5.2859", "1.12.2");
    for pack in ["Faithful.zip", "Other.zip"] {
        fs::write(dir.join("resourcepacks").join(pack), b"p").unwrap();
    }
    let options = dir.join("options.txt");
    fs::write(&options, "resourcePacks:[\"Faithful.zip\"]\n").unwrap();
    let list = w.service.list(&key, ContentKind::ResourcePacks).unwrap();
    assert_eq!(files(&list.items), [("Faithful.zip", true), ("Other.zip", false)]);
    w.service.toggle(&key, ContentKind::ResourcePacks, "Other.zip", true).unwrap();
    assert_eq!(fs::read_to_string(&options).unwrap(), "resourcePacks:[\"Faithful.zip\",\"Other.zip\"]\n");
    w.service.toggle(&key, ContentKind::ResourcePacks, "Faithful.zip", false).unwrap();
    assert_eq!(fs::read_to_string(&options).unwrap(), "resourcePacks:[\"Other.zip\"]\n");
}

#[test]
fn screenshots_are_listed_and_deleted() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let shots = dir.join("screenshots");
    fs::create_dir_all(shots.join("sub")).unwrap();
    fs::write(shots.join("2026-09-27_16.55.46.png"), b"png").unwrap();
    fs::write(shots.join("note.txt"), b"t").unwrap();
    fs::write(shots.join("sub").join("inner.png"), b"i").unwrap();
    fs::write(dir.join("keep.png"), b"outside").unwrap();
    let list = w.service.screenshots(&key).unwrap();
    assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["2026-09-27_16.55.46.png"]);
    for name in ["../keep.png", "note.txt", "sub/inner.png", "sub", "missing.png", ""] {
        assert_eq!(w.service.screenshot(&key, name).unwrap_err().code, ErrorCode::NotFound, "{name}");
        assert_eq!(w.service.delete_screenshot(&key, name).unwrap_err().code, ErrorCode::NotFound, "{name}");
    }
    assert!(w.service.delete_screenshot(&key, "2026-09-27_16.55.46.png").unwrap().is_empty());
    assert!(
        dir.join("keep.png").exists()
            && shots.join("note.txt").exists()
            && shots.join("sub/inner.png").exists()
    );
    assert_eq!(w.service.screenshots_dir(&key).unwrap(), shots);
    assert_eq!(w.service.screenshots("ghost").unwrap_err().code, ErrorCode::VersionNotFound);
}

fn picture(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(960, 540, image::Rgb([40, 120, 200])).save(path).unwrap();
}

#[test]
fn every_build_s_screenshots_are_listed_renamed_and_deleted_together() {
    let w = world();
    let (aero, aero_dir) = build(&w, "Aero", "Fabric", FABRIC);
    let (_empty, _) = build(&w, "Empty", "Fabric", FABRIC);
    let (zeta, zeta_dir) = build(&w, "Zeta", "Fabric", FABRIC);
    picture(&aero_dir.join("screenshots/a.png"));
    picture(&aero_dir.join("screenshots/b.png"));
    picture(&zeta_dir.join("screenshots/z.png"));
    let all = w.service.all_screenshots();
    let listed: Vec<(&str, usize)> = all.iter().map(|(key, shots)| (key.as_str(), shots.len())).collect();
    assert_eq!(listed, [(aero.as_str(), 2), (zeta.as_str(), 1)], "builds without screenshots are left out");

    let thumb = w.service.screenshot_thumb(&aero, "a.png").unwrap();
    assert_eq!(image::image_dimensions(&thumb).unwrap(), (480, 270));
    let renamed = w.service.rename_screenshot(&aero, "a.png", "Світанок").unwrap();
    assert_eq!(renamed.name, "Світанок.png");
    assert!(!thumb.exists(), "the old name's thumbnail goes with it");
    assert_eq!(
        w.service.rename_screenshot(&aero, "Світанок.png", "b").unwrap_err().code,
        ErrorCode::FileNameTaken
    );

    let asked = [
        (aero.clone(), "Світанок.png".to_string()),
        (zeta.clone(), "z.png".to_string()),
        (zeta.clone(), "missing.png".to_string()),
    ];
    assert_eq!(w.service.delete_screenshots(&asked), 2, "what is not there is skipped");
    let left: Vec<String> =
        w.service.all_screenshots().into_iter().flat_map(|(_, s)| s).map(|s| s.name).collect();
    assert_eq!(left, ["b.png"]);
}

fn versioned_mod(path: &Path, id: &str, version: &str) {
    jar(path, format!(r#"{{"id":"{id}","name":"{id}","version":"{version}"}}"#).as_bytes());
}

fn backed_up(items: &[ContentItem]) -> Vec<(&str, bool)> {
    items.iter().map(|i| (i.file.as_str(), i.has_backup)).collect()
}

#[test]
fn a_mod_renamed_by_its_update_comes_back_from_its_backup() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let mods = dir.join("mods");
    versioned_mod(&mods.join("sodium-0.5.jar"), "sodium", "0.5");
    backups::back_up(&shelf(&w, &key), &dir, "mods/sodium-0.5.jar", "sodium-0.5.jar").unwrap();
    fs::remove_file(mods.join("sodium-0.5.jar")).unwrap();
    versioned_mod(&mods.join("sodium-0.6.jar.disabled"), "sodium", "0.6");
    fabric_mod(&dir, "lithium.jar", "lithium", "Lithium");
    let list = w.service.list(&key, ContentKind::Mods).unwrap();
    assert_eq!(backed_up(&list.items), [("lithium.jar", false), ("sodium-0.6.jar.disabled", true)]);
    let after = w.service.restore(&key, ContentKind::Mods, "sodium-0.6.jar.disabled").unwrap();
    assert_eq!(files(&after.items), [("lithium.jar", true), ("sodium-0.5.jar.disabled", false)]);
    assert_eq!(after.items[1].version.as_deref(), Some("0.5"));
    assert!(!mods.join("sodium-0.6.jar.disabled").exists());
    assert!(shelf(&w, &key).join("sodium-0.5.jar.backup").is_file(), "the backup stays");
    assert!(!mods.join(".backups").exists(), "the build keeps no backups of its own");
    assert!(after.items.iter().all(|i| !i.has_backup), "the same bytes again would change nothing");
}

#[test]
fn a_mod_updated_under_its_own_name_is_restored_in_place() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let mods = dir.join("mods");
    versioned_mod(&mods.join("iris.jar"), "iris", "1.7");
    backups::back_up(&shelf(&w, &key), &dir, "mods/iris.jar", "iris.jar").unwrap();
    versioned_mod(&mods.join("iris.jar"), "iris", "1.8.0");
    let after = w.service.restore(&key, ContentKind::Mods, "iris.jar").unwrap();
    assert_eq!(files(&after.items), [("iris.jar", true)]);
    assert_eq!(after.items[0].version.as_deref(), Some("1.7"));
}

#[test]
fn restoring_needs_a_backup_a_quiet_build_and_a_free_name() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let mods = dir.join("mods");
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium");
    let none = w.service.restore(&key, ContentKind::Mods, "sodium.jar").unwrap_err();
    assert_eq!(none.code, ErrorCode::BackupNotFound);
    versioned_mod(&mods.join("sodium-0.5.jar"), "sodium", "0.5");
    backups::back_up(&shelf(&w, &key), &dir, "mods/sodium-0.5.jar", "sodium-0.5.jar").unwrap();
    fs::rename(mods.join("sodium-0.5.jar"), mods.join("sodium-0.5.jar.disabled")).unwrap();
    versioned_mod(&mods.join("sodium-0.5.jar.disabled"), "sodium", "0.5-edited");
    let taken = w.service.restore(&key, ContentKind::Mods, "sodium.jar").unwrap_err();
    assert_eq!(taken.code, ErrorCode::ContentConflict, "sodium-0.5.jar.disabled is another file");
    assert!(mods.join("sodium.jar").is_file() && mods.join("sodium-0.5.jar.disabled").is_file());
    let packs = w.service.restore(&key, ContentKind::ResourcePacks, "x.zip").unwrap_err();
    assert_eq!(packs.code, ErrorCode::Unsupported);
    let lost = backups::back_up(&shelf(&w, &key), &dir, "mods/missing.jar", "missing.jar").unwrap_err();
    assert_eq!(lost.code, ErrorCode::BackupFailed);
    assert!(!shelf(&w, &key).join("missing.jar.backup.part").exists(), "no half copy stays");
}

#[test]
fn a_mod_keeps_only_its_newest_backup_and_offers_it_once() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let mods = dir.join("mods");
    // 0.4 → 0.5 → 0.6, each update backing up the file it replaced.
    for (from, to) in [("0.4", "0.5"), ("0.5", "0.6")] {
        let name = format!("sodium-{from}.jar");
        if !mods.join(&name).exists() {
            versioned_mod(&mods.join(&name), "sodium", from);
        }
        backups::back_up(&shelf(&w, &key), &dir, &format!("mods/{name}"), &name).unwrap();
        fs::remove_file(mods.join(&name)).unwrap();
        versioned_mod(&mods.join(format!("sodium-{to}.jar")), "sodium", to);
    }
    assert!(!shelf(&w, &key).join("sodium-0.4.jar.backup").exists(), "one backup per mod: the newest");
    let after = w.service.restore(&key, ContentKind::Mods, "sodium-0.6.jar").unwrap();
    assert_eq!(files(&after.items), [("sodium-0.5.jar", true)]);
    assert!(!after.items[0].has_backup, "restored once, nothing older is offered");
}

#[test]
fn only_the_newest_backup_counts_even_beside_older_ones() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let (mods, shelf) = (dir.join("mods"), shelf(&w, &key));
    fs::create_dir_all(&shelf).unwrap();
    versioned_mod(&shelf.join("sodium-0.4.jar.backup"), "sodium", "0.4");
    versioned_mod(&shelf.join("sodium-0.5.jar.backup"), "sodium", "0.5");
    let older = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(shelf.join("sodium-0.4.jar.backup"))
        .unwrap()
        .set_modified(older)
        .unwrap();
    fs::copy(shelf.join("sodium-0.5.jar.backup"), mods.join("sodium-0.5.jar")).unwrap();
    let list = w.service.list(&key, ContentKind::Mods).unwrap();
    assert!(!list.items[0].has_backup, "the newest backup is this very file; an older one is not offered");
}

/// A file from outside the build (one dropped on its tab), named `name`.
fn outside(w: &World, name: &str, body: &[u8]) -> PathBuf {
    let dir = w._tmp.path().join("outside");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn files_from_outside_go_into_their_kind_s_folder() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let sodium = w._tmp.path().join("Sodium 0.6.jar");
    jar(&sodium, br#"{"id":"sodium","name":"Sodium","version":"0.6"}"#);
    let list = w.service.add(&key, ContentKind::Mods, &sodium, false).unwrap();
    assert_eq!(files(&list.items), [("Sodium 0.6.jar", true)]);
    assert!(sodium.exists(), "the dropped file stays where it was");
    for (kind, name) in
        [(ContentKind::ResourcePacks, "Faithful.zip"), (ContentKind::ShaderPacks, "Комплементарні.ZIP")]
    {
        let pack = outside(&w, name, b"PK");
        w.service.add(&key, kind, &pack, false).unwrap();
        assert_eq!(fs::read(dir.join(kind.folder()).join(name)).unwrap(), b"PK", "{name}");
    }
    let leftovers: Vec<_> =
        fs::read_dir(dir.join("mods")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(leftovers, ["Sodium 0.6.jar"], "no part file is left behind");
}

#[test]
fn only_a_kind_s_own_files_are_taken() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    for (kind, name) in [
        (ContentKind::Mods, "pack.zip"),
        (ContentKind::ResourcePacks, "sodium.jar"),
        (ContentKind::ShaderPacks, "notes.txt"),
        (ContentKind::Mods, ".jar"),
        (ContentKind::Mods, "con.jar"),
    ] {
        let file = outside(&w, name, b"x");
        let err = w.service.add(&key, kind, &file, false).unwrap_err();
        assert_eq!((err.code, err.params["name"].as_str()), (ErrorCode::InvalidInput, name), "{name}");
    }
    let (vanilla, _) = build(&w, "Vanilla", "Minecraft", "1.21.1");
    let sodium = outside(&w, "sodium.jar", b"x");
    assert_eq!(
        w.service.add(&vanilla, ContentKind::Mods, &sodium, false).unwrap_err().code,
        ErrorCode::Unsupported
    );
    assert!(fs::read_dir(dir.join("mods")).unwrap().next().is_none());
}

#[test]
fn a_file_of_the_same_name_is_replaced_only_when_asked() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar.disabled", "sodium", "Sodium old");
    let newer = w._tmp.path().join("sodium.jar");
    jar(&newer, br#"{"id":"sodium","name":"Sodium new","version":"2.0"}"#);
    let err = w.service.add(&key, ContentKind::Mods, &newer, false).unwrap_err();
    assert_eq!((err.code, err.params["name"].as_str()), (ErrorCode::ContentConflict, "sodium.jar"));
    let list = w.service.add(&key, ContentKind::Mods, &newer, true).unwrap();
    assert_eq!(files(&list.items), [("sodium.jar.disabled", false)], "a switched-off mod stays off");
    assert_eq!(fs::read(dir.join("mods/sodium.jar.disabled")).unwrap(), fs::read(&newer).unwrap());
}

#[test]
fn a_busy_build_takes_no_files() {
    let w = world();
    let (key, dir) = build(&w, "Моя збірка", "Fabric", FABRIC);
    let sodium = outside(&w, "sodium.jar", b"x");
    let lease = w.instances.try_acquire(&dir, "copy").unwrap();
    let busy = w.service.add(&key, ContentKind::Mods, &sodium, false).unwrap_err();
    assert_eq!((busy.code, busy.params["version"].as_str()), (ErrorCode::InstanceBusy, "Моя збірка"));
    drop(lease);
    assert!(!dir.join("mods/sodium.jar").exists());
}

/// The game holds its jars open as Java does: a file dropped in their place waits, whole.
#[cfg(windows)]
#[test]
fn a_jar_the_game_holds_is_in_use_and_stays_whole() {
    use std::os::windows::fs::OpenOptionsExt;
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium old");
    let before = fs::read(dir.join("mods/sodium.jar")).unwrap();
    let newer = outside(&w, "sodium.jar", b"newer");
    // FILE_SHARE_READ | FILE_SHARE_WRITE: shared with readers and writers, not with a move.
    let held = fs::OpenOptions::new().read(true).share_mode(1 | 2).open(dir.join("mods/sodium.jar")).unwrap();
    let err = w.service.add(&key, ContentKind::Mods, &newer, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::FileInUse);
    drop(held);
    assert_eq!(fs::read(dir.join("mods/sodium.jar")).unwrap(), before);
    let left: Vec<_> = fs::read_dir(dir.join("mods")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left, ["sodium.jar"], "no part file is left behind");
}

#[test]
fn an_empty_file_a_folder_or_a_missing_file_is_refused() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    let empty = outside(&w, "empty.jar", b"");
    assert_eq!(
        w.service.add(&key, ContentKind::Mods, &empty, false).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    let folder = w._tmp.path().join("outside").join("folder.jar");
    fs::create_dir_all(&folder).unwrap();
    assert_eq!(
        w.service.add(&key, ContentKind::Mods, &folder, false).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    let gone = w._tmp.path().join("outside").join("gone.jar");
    assert!(w.service.add(&key, ContentKind::Mods, &gone, false).is_err());
    assert!(fs::read_dir(dir.join("mods")).unwrap().next().is_none(), "nothing came in, no part file either");
}

#[test]
fn a_replacement_of_the_same_size_is_read_anew() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fabric_mod(&dir, "sodium.jar", "sodium", "Sodium old");
    assert_eq!(w.service.list(&key, ContentKind::Mods).unwrap().items[0].name.as_deref(), Some("Sodium old"));
    let newer = w._tmp.path().join("sodium.jar");
    jar(&newer, br#"{"id":"sodium","name":"Sodium new","version":"1.0"}"#);
    // Same size and time as the old one: only the new copy's own time tells them apart.
    let old = fs::metadata(dir.join("mods/sodium.jar")).unwrap();
    assert_eq!(fs::metadata(&newer).unwrap().len(), old.len());
    fs::File::options().write(true).open(&newer).unwrap().set_modified(old.modified().unwrap()).unwrap();
    let list = w.service.add(&key, ContentKind::Mods, &newer, true).unwrap();
    assert_eq!(list.items[0].name.as_deref(), Some("Sodium new"));
}

#[test]
fn a_resource_pack_comes_in_switched_off_and_options_stay() {
    let w = world();
    let (key, dir) = build(&w, "Aero", "Fabric", FABRIC);
    fs::write(dir.join("options.txt"), "resourcePacks:[\"vanilla\"]\nlang:uk_ua\n").unwrap();
    let before = fs::read(dir.join("options.txt")).unwrap();
    let pack = outside(&w, "Faithful.zip", b"PK");
    let list = w.service.add(&key, ContentKind::ResourcePacks, &pack, false).unwrap();
    assert_eq!(files(&list.items), [("Faithful.zip", false)]);
    assert_eq!(fs::read(dir.join("options.txt")).unwrap(), before);
}
