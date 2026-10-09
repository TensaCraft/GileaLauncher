mod support;

use launcher_core::feedback::OperationSpec;
use launcher_core::launch::options::game_dir;
use launcher_shared::{ErrorCode, Text};
use module_tensa::backend::identity::CLIENT;
use module_tensa::backend::install::install;
use serde_json::{Value, json};
use support::World;

fn aero(w: &World) -> Value {
    json!({
        "id": "aero", "name": "Aero", "minecraft_version": "26.3", "loader_id": "fabric", "loader_version": "0.19.5",
        "server_host": "play.example", "server_port": 25565, "jvm_arguments": ["-XX:+UseG1GC"],
        "gpu_preference": "discrete", "description": "The server's build", "image": "logo.png",
        "files_endpoint": w.server.url("/api/mods/aero/files")
    })
}

/// The build folders under `games` (the launcher's own `.`-folders, such as its locks, aside).
fn build_folders(w: &World) -> Vec<String> {
    std::fs::read_dir(w.deps.versions.games_dir())
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default()
}

fn files(w: &World, entries: Value) {
    w.server.json("/api/mods/aero/files", json!({"files": entries}));
}

#[tokio::test]
async fn a_server_build_is_installed_as_a_new_build() {
    let w = World::start().await;
    w.catalog(aero(&w));
    let (a, b) = (w.file("a.jar", "jar-a"), w.file("b.json", "{}"));
    files(
        &w,
        json!([
            {"relative_path": "mods/a.jar", "download_url": a, "size": 5},
            {"relative_path": "config/b.json", "download_url": b},
            {"relative_path": "../evil.jar", "download_url": a},
            {"relative_path": "mods/no-address.jar"}
        ]),
    );
    let build = install(&w.deps, "aero", "Aero").await.unwrap();
    assert_eq!(w.components.asked.lock().unwrap().as_slice(), ["fabric-loader-0.19.5-26.3"]);
    assert_eq!(build.name, "Aero");
    assert_eq!((build.client.as_deref(), build.remote_pack_id.clone()), (Some(CLIENT), Some(json!("aero"))));
    assert_eq!(
        (build.version.as_deref(), build.loader.as_deref()),
        (Some("26.3"), Some("fabric-loader-0.19.5-26.3"))
    );
    assert_eq!(build.loader_version.as_deref(), Some("0.19.5"));
    assert_eq!(build.options.get("managedByApi"), Some(&json!(true)));
    assert_eq!(build.options.get("server"), Some(&json!({"host": "play.example", "port": 25565})));
    assert_eq!(build.options.get("jvmArguments"), Some(&json!(["-XX:+UseG1GC"])));
    assert_eq!(build.options.get("gpuMode"), Some(&json!("dgpu")));
    assert_eq!(build.options.get("executablePath"), Some(&json!("java-for-26.3")));
    assert_eq!(
        (build.description.as_str(), build.image.as_deref()),
        ("The server's build", Some("logo.png"))
    );
    let saved = w.deps.versions.get(&build.key).expect("the build is kept");
    assert_eq!(saved.remote_pack_id, Some(json!("aero")));
    let game = game_dir(&saved, w.deps.versions.minecraft_dir());
    assert_eq!(std::fs::read_to_string(game.join("mods/a.jar")).unwrap(), "jar-a");
    assert_eq!(std::fs::read_to_string(game.join("config/b.json")).unwrap(), "{}");
    assert!(!game.join("mods/no-address.jar").exists());
    assert!(
        !w.deps.versions.games_dir().join("evil.jar").exists()
            && !game.parent().unwrap().join("evil.jar").exists()
    );
}

#[tokio::test]
async fn an_installed_build_s_icon_carries_its_picture_s_version() {
    let w = World::start().await;
    let icon = w.server.url("/icons/aero.png");
    w.server.reply("/icons/aero.png", 200, "png");
    w.server.header("/icons/aero.png", "etag", "\"aaa-1\"");
    let mut client = aero(&w);
    client["image"] = json!(icon);
    w.catalog(client);
    files(&w, json!([]));
    let build = install(&w.deps, "aero", "Aero").await.unwrap();
    assert_eq!(build.image, Some(format!("{icon}?iv=aaa-1")));
}

#[tokio::test]
async fn a_failed_install_leaves_no_build_and_no_folder() {
    let w = World::start().await;
    w.catalog(aero(&w));
    files(&w, json!([{"relative_path": "mods/a.jar", "download_url": w.server.url("/files/missing.jar")}]));
    assert!(install(&w.deps, "aero", "Aero").await.is_err());
    assert!(w.deps.versions.list().is_empty());
    assert!(build_folders(&w).is_empty(), "no folder is left: {:?}", build_folders(&w));

    w.components.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(install(&w.deps, "aero", "Aero").await.unwrap_err().code, ErrorCode::LoaderInstallFailed);
    assert!(w.deps.versions.list().is_empty());
}

#[tokio::test]
async fn an_unknown_pack_or_loader_is_refused() {
    let w = World::start().await;
    w.catalog(aero(&w));
    assert_eq!(install(&w.deps, "nope", "Nope").await.unwrap_err().code, ErrorCode::NotFound);
    let other = World::start().await;
    other.catalog(
        json!({"id": "odd", "minecraft_version": "1.12.2", "loader_id": "liteloader", "loader_version": "1"}),
    );
    assert_eq!(install(&other.deps, "odd", "Odd").await.unwrap_err().code, ErrorCode::InvalidInput);
    assert!(other.deps.versions.list().is_empty() && w.deps.versions.list().is_empty());
}

#[tokio::test]
async fn a_busy_launcher_refuses_to_install() {
    let w = World::start().await;
    w.catalog(aero(&w));
    let _running = w.deps.feedback.begin(OperationSpec::new(Text::key("installation_started"), "install"));
    assert_eq!(install(&w.deps, "aero", "Aero").await.unwrap_err().code, ErrorCode::Busy);
    assert!(build_folders(&w).is_empty());
}
