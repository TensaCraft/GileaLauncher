use launcher_shared::*;
use serde_json::json;

#[test]
fn error_roundtrips_with_params() {
    let err = AppError::new(ErrorCode::InvalidDirectoryPath, "bad path").with_param("path", "C:\\x");
    let v = serde_json::to_value(&err).unwrap();
    assert_eq!(
        v,
        json!({"code": "invalid_directory_path", "params": {"path": "C:\\x"}, "detail": "bad path"})
    );
    let back: AppError = serde_json::from_value(v).unwrap();
    assert_eq!(back, err);
}

#[test]
fn error_accepts_missing_optional_fields() {
    let err: AppError = serde_json::from_value(json!({"code": "busy"})).unwrap();
    assert_eq!(err.code, ErrorCode::Busy);
    assert!(err.params.is_empty());
    assert_eq!(ErrorCode::Busy.i18n_key(), "installation_already_running");
}

#[test]
fn text_is_tagged() {
    let t = Text::key("version_starting").param("version", "1.21.1");
    assert_eq!(
        serde_json::to_value(&t).unwrap(),
        json!({"kind": "key", "key": "version_starting", "params": {"version": "1.21.1"}})
    );
    assert_eq!(serde_json::to_value(Text::raw("x")).unwrap(), json!({"kind": "raw", "text": "x"}));
}

#[test]
fn setting_update_uses_key_value_shape() {
    let u = SettingUpdate::AutoUpdate(true);
    assert_eq!(serde_json::to_value(&u).unwrap(), json!({"key": "auto_update", "value": true}));
    let s: SettingUpdate =
        serde_json::from_value(json!({"key": "click_sound", "value": "plastic_bubble_click"})).unwrap();
    assert_eq!(s, SettingUpdate::ClickSound(ClickSound::PlasticBubbleClick));
}

#[test]
fn click_sound_config_strings() {
    for s in ClickSound::ALL {
        assert_eq!(ClickSound::from_config_str(s.as_config_str()), s);
    }
    assert_eq!(ClickSound::from_config_str("garbage"), ClickSound::GateLatchClick);
    assert_eq!(ClickSound::default(), ClickSound::GateLatchClick);
}

#[test]
fn branding_defaults_are_set() {
    assert!(!branding::APP_NAME.is_empty() && !branding::IDENTIFIER.is_empty());
    assert!(!branding::VERSION.is_empty());
    assert!(branding::SUPPORT_URL.starts_with("https://"));
}

#[test]
fn update_status_is_tagged_by_kind() {
    let info = UpdateInfo {
        version: "0.2.0".into(),
        channel: UpdateChannel::Beta,
        notes: "Fixes".into(),
        asset_name: "Launcher.exe".into(),
        size: 42,
        published_at: None,
    };
    let status = UpdateStatus {
        state: UpdateState::Available { info },
        ..UpdateStatus::idle(true, "0.1.0".into(), None, true, None)
    };
    let v = serde_json::to_value(&status).unwrap();
    assert_eq!(v["state"]["kind"], json!("available"));
    assert_eq!(v["state"]["info"]["channel"], json!("beta"));
    assert_eq!(v["apply_supported"], json!(true));
    let back: UpdateStatus = serde_json::from_value(v).unwrap();
    assert_eq!(back, status);
    assert_eq!(serde_json::to_value(UpdateState::UpToDate).unwrap(), json!({"kind": "up_to_date"}));
}

#[test]
fn update_error_codes_have_their_own_keys() {
    assert_eq!(ErrorCode::Network.i18n_key(), "error_network");
    assert_eq!(ErrorCode::RateLimited.i18n_key(), "update_rate_limited");
    assert_eq!(ErrorCode::IntegrityMismatch.i18n_key(), "update_hash_mismatch");
    assert_eq!(ErrorCode::NoUpdateAsset.i18n_key(), "update_no_asset");
    assert_eq!(serde_json::to_value(ErrorCode::RateLimited).unwrap(), json!("rate_limited"));
    assert_eq!(ErrorCode::ALL.len(), 53);
    assert_eq!(
        (
            serde_json::to_value(ErrorCode::ProviderFilesHeld).unwrap(),
            ErrorCode::ProviderFilesHeld.i18n_key()
        ),
        (json!("provider_files_held"), "provider_files_held")
    );
    assert_eq!(
        (serde_json::to_value(ErrorCode::FileInUse).unwrap(), ErrorCode::FileInUse.i18n_key()),
        (json!("file_in_use"), "error_file_in_use")
    );
    assert_eq!(serde_json::to_value(ErrorCode::BackupNotFound).unwrap(), json!("backup_not_found"));
    assert_eq!(
        (ErrorCode::BackupFailed.i18n_key(), ErrorCode::BackupNotFound.i18n_key()),
        ("backup_failed", "backup_not_found")
    );
    assert_eq!(serde_json::to_value(ErrorCode::NoCompatibleVersion).unwrap(), json!("no_compatible_version"));
    assert_eq!(ErrorCode::LoaderInstallFailed.i18n_key(), "loader_install_failed");
    assert_eq!(serde_json::to_value(ErrorCode::LoaderInstallFailed).unwrap(), json!("loader_install_failed"));
}

#[test]
fn default_update_api_is_github() {
    assert_eq!(branding::DEFAULT_UPDATE_API, "https://api.github.com");
    assert_eq!(branding::UPDATE_API, branding::DEFAULT_UPDATE_API);
    assert_eq!(names::UPDATE, "app://update");
}

#[test]
fn profile_types_keep_their_wire_shape() {
    let dto = ProfileDto {
        key: "Steve".into(),
        name: "Steve".into(),
        id: "5627dd98-e6be-bc21-f8a8-e92344183641".into(),
        kind: AccountKind::Offline,
        is_default: true,
        reauth_required: false,
        reauth_reason: None,
    };
    assert_eq!(
        serde_json::to_value(&dto).unwrap(),
        json!({
            "key": "Steve",
            "name": "Steve",
            "id": "5627dd98-e6be-bc21-f8a8-e92344183641",
            "kind": "offline",
            "is_default": true,
            "reauth_required": false,
            "reauth_reason": null
        })
    );
    let device = AuthState::DeviceCode {
        user_code: "ABCD-1234".into(),
        verification_uri: "https://www.microsoft.com/link".into(),
        open_url: "https://www.microsoft.com/link?otc=ABCD-1234".into(),
    };
    assert_eq!(serde_json::to_value(&device).unwrap()["kind"], json!("device_code"));
    assert_eq!(serde_json::to_value(AuthState::Idle).unwrap(), json!({ "kind": "idle" }));
    assert_eq!(names::PROFILES, "app://profiles");
    assert_eq!(names::AUTH, "app://auth");
}

#[test]
fn profiles_sort_default_first_then_by_name() {
    let p = |name: &str, is_default: bool| ProfileDto {
        key: name.into(),
        name: name.into(),
        id: String::new(),
        kind: AccountKind::Offline,
        is_default,
        reauth_required: false,
        reauth_reason: None,
    };
    let mut list = vec![p("zed", false), p("Mia", true), p("alex", false)];
    sort_profiles(&mut list);
    let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Mia", "alex", "zed"]);
    assert_eq!(ProfilesSnapshot { profiles: list }.default_profile().unwrap().name, "Mia");
}

#[test]
fn account_error_codes_use_the_original_keys() {
    let expected = [
        (ErrorCode::AuthTimeout, "microsoft_auth_timeout"),
        (ErrorCode::AuthDenied, "microsoft_auth_denied"),
        (ErrorCode::AuthFailed, "microsoft_auth_failed"),
        (ErrorCode::MinecraftServicesUnavailable, "minecraft_services_auth_unavailable"),
        (ErrorCode::XboxAccountMissing, "xbox_account_missing"),
        (ErrorCode::XboxChildAccount, "xbox_child_account"),
        (ErrorCode::XboxUnavailable, "xbox_unavailable"),
        (ErrorCode::MinecraftNotOwned, "minecraft_not_owned"),
        (ErrorCode::ReauthRequired, "profile_reauth_required"),
        (ErrorCode::CredentialStorageUnavailable, "credential_encryption_unavailable"),
        (ErrorCode::ProfileSaveFailed, "profile_save_failed"),
        (ErrorCode::ProfileExists, "profile_exists"),
        (ErrorCode::ProfileNameInvalid, "profile_name_invalid"),
        (ErrorCode::NoProfile, "no_default_profile"),
    ];
    for (code, key) in expected {
        assert_eq!(code.i18n_key(), key);
        assert!(ErrorCode::ALL.contains(&code));
    }
    assert_eq!(serde_json::to_value(ErrorCode::NoProfile).unwrap(), json!("no_profile"));
}

#[test]
fn build_error_codes_use_the_original_keys() {
    let expected = [
        (ErrorCode::InstanceBusy, "instance_operation_busy"),
        (ErrorCode::SharedBusy, "shared_minecraft_operation_busy"),
        (ErrorCode::NotEnoughSpace, "not_enough_space"),
        (ErrorCode::VersionNotFound, "version_not_found"),
        (ErrorCode::VersionExists, "version_exists"),
        (ErrorCode::VersionNameEmpty, "empty_version_name"),
        (ErrorCode::DownloadFailed, "download_error"),
        (ErrorCode::VersionFilesRemain, "version_delete_files_remain"),
    ];
    for (code, key) in expected {
        assert_eq!(code.i18n_key(), key);
        assert!(ErrorCode::ALL.contains(&code));
    }
    assert_eq!(serde_json::to_value(ErrorCode::NotEnoughSpace).unwrap(), json!("not_enough_space"));
}

#[test]
fn java_runtime_failures_have_their_own_key() {
    assert_eq!(ErrorCode::JavaRuntimeFailed.i18n_key(), "java_runtime_install_failed");
    assert!(ErrorCode::ALL.contains(&ErrorCode::JavaRuntimeFailed));
    assert_eq!(serde_json::to_value(ErrorCode::JavaRuntimeFailed).unwrap(), json!("java_runtime_failed"));
}

#[test]
fn launch_error_codes_use_the_original_keys() {
    let expected = [
        (ErrorCode::VersionRunning, "version_already_running"),
        (ErrorCode::LaunchThrottled, "version_launch_throttled"),
        (ErrorCode::LaunchFailed, "version_integrity_check_failed"),
    ];
    for (code, key) in expected {
        assert_eq!(code.i18n_key(), key);
        assert!(ErrorCode::ALL.contains(&code));
    }
}

#[test]
fn game_events_carry_a_tagged_state() {
    let event = GameEvent {
        build_key: "aero".into(),
        build_name: "Aero".into(),
        state: GameState::Crashed { code: Some(1), early: true, log: Some("G/logs/latest.log".into()) },
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({"build_key": "aero", "build_name": "Aero",
               "state": {"state": "crashed", "code": 1, "early": true, "log": "G/logs/latest.log"}})
    );
    assert_eq!(
        serde_json::to_value(GameState::Started { pid: 7 }).unwrap(),
        json!({"state": "started", "pid": 7})
    );
    assert_eq!(
        serde_json::to_value(GameState::Running { close_launcher: true }).unwrap(),
        json!({"state": "running", "close_launcher": true})
    );
    assert_eq!(names::GAME, "app://game");
}

#[test]
fn build_and_java_error_codes_use_the_original_keys() {
    let expected = [
        (ErrorCode::InvalidJavaExecutable, "custom_java_invalid"),
        (ErrorCode::BuildRunning, "version_delete_close_game_first"),
        (ErrorCode::ShortcutFailed, "desktop_shortcut_failed"),
    ];
    for (code, key) in expected {
        assert_eq!(code.i18n_key(), key);
        assert!(ErrorCode::ALL.contains(&code));
    }
    assert_eq!(names::BUILDS, "app://builds");
}

#[test]
fn build_and_java_dtos_keep_their_wire_shape() {
    let build = BuildDto {
        key: "aero".into(),
        version_id: "aero".into(),
        name: "Aero".into(),
        version: Some("1.21.1".into()),
        loader: Some("1.21.1".into()),
        client: Some("Minecraft".into()),
        loader_version: None,
        game_dir: "G/games/aero".into(),
        image: None,
        description: String::new(),
        running: true,
        profile: Some("Alex".into()),
    };
    assert_eq!(
        serde_json::to_value(BuildsSnapshot { builds: vec![build] }).unwrap(),
        json!({"builds": [{"key": "aero", "version_id": "aero", "name": "Aero", "version": "1.21.1", "loader": "1.21.1", "client": "Minecraft",
                           "loader_version": null, "game_dir": "G/games/aero", "image": null, "description": "", "running": true, "profile": "Alex"}]})
    );
    let java = JavaList {
        launcher: vec![JavaEntry {
            label: "Launcher Java 21.0.7 (java-runtime-delta)".into(),
            path: "J".into(),
        }],
        custom: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(java).unwrap(),
        json!({"launcher": [{"label": "Launcher Java 21.0.7 (java-runtime-delta)", "path": "J"}], "custom": []})
    );
    let version = CatalogVersion { id: "24w33a".into(), kind: "snapshot".into(), release_time: None };
    assert_eq!(
        serde_json::to_value(version).unwrap(),
        json!({"id": "24w33a", "kind": "snapshot", "release_time": null})
    );
    let memory = MemoryInfo { total_gb: 16, min_heap_gb: 1, max_heap_gb: 14, recommended_heap_gb: 6 };
    assert_eq!(
        serde_json::to_value(memory).unwrap(),
        json!({"total_gb": 16, "min_heap_gb": 1, "max_heap_gb": 14, "recommended_heap_gb": 6})
    );
}

#[test]
fn memory_and_gpu_settings_travel_as_key_and_value() {
    let ram = SettingUpdate::DefaultMaxRamGb(Some(6));
    assert_eq!(serde_json::to_value(&ram).unwrap(), json!({"key": "default_max_ram_gb", "value": 6}));
    let auto: SettingUpdate =
        serde_json::from_value(json!({"key": "default_max_ram_gb", "value": null})).unwrap();
    assert_eq!(auto, SettingUpdate::DefaultMaxRamGb(None));
    let gpu = SettingUpdate::GpuModeDefault("igpu".into());
    assert_eq!(serde_json::to_value(&gpu).unwrap(), json!({"key": "gpu_mode_default", "value": "igpu"}));
    let old: SettingsSnapshot = serde_json::from_value(json!({
        "lang": "uk_UA", "auto_update": true, "include_beta_updates": false, "close_launcher_on_game": false,
        "ask_profile_on_launch": false, "compact_sidebar": true, "click_sound_enabled": true,
        "click_sound": "gate_latch_click", "minecraft_dir": "M", "minecraft_dir_is_default": true
    }))
    .unwrap();
    assert_eq!((old.default_max_ram_gb, old.gpu_mode_default.as_str()), (None, ""));
    assert_eq!(
        old.on_game_start,
        launcher_shared::GameStartAction::Nothing,
        "the old switch is the core's to read"
    );
    let tray = SettingUpdate::OnGameStart(launcher_shared::GameStartAction::Tray);
    assert_eq!(serde_json::to_value(&tray).unwrap(), json!({"key": "on_game_start", "value": "tray"}));
}

#[test]
fn window_sizes_parse_and_stay_in_range() {
    assert_eq!(WindowSize::parse("fullscreen"), Some(WindowSize::Fullscreen));
    assert_eq!(WindowSize::parse(" Maximized "), Some(WindowSize::Maximized));
    assert_eq!(WindowSize::parse("1366x800"), Some(WindowSize::Size { width: 1366, height: 800 }));
    assert_eq!(WindowSize::parse("800x600"), None, "below the minimum");
    assert_eq!(WindowSize::parse("99999x800"), None, "above the maximum");
    assert_eq!(WindowSize::parse("big"), None);
    assert_eq!(WindowSize::default(), WindowSize::Size { width: 1366, height: 800 });
    assert_eq!(WindowSize::Size { width: 1600, height: 960 }.as_config_str(), "1600x960");
    assert_eq!(WindowSize::Fullscreen.as_config_str(), "fullscreen");
    assert!(WINDOW_PRESETS.iter().all(|(w, h)| WindowSize::sized(*w, *h).is_some()));
    let update = SettingUpdate::WindowSize("maximized".into());
    assert_eq!(serde_json::to_value(&update).unwrap(), json!({"key": "window_size", "value": "maximized"}));
}

#[test]
fn loader_kinds_name_their_components() {
    assert_eq!(LoaderKind::Fabric.component_id("1.21.1", "0.16.9"), "fabric-loader-0.16.9-1.21.1");
    assert_eq!(LoaderKind::Quilt.component_id("1.21.1", "0.26.4"), "quilt-loader-0.26.4-1.21.1");
    assert_eq!(LoaderKind::Forge.component_id("1.20.1", "47.3.0"), "1.20.1-forge-47.3.0");
    assert_eq!(LoaderKind::NeoForge.component_id("1.21.1", "21.1.77"), "neoforge-21.1.77");
    assert_eq!(LoaderKind::Minecraft.component_id("1.21.1", ""), "1.21.1");
    assert_eq!(LoaderKind::of_component("fabric-loader-0.16.9-1.21.1"), Some(LoaderKind::Fabric));
    assert_eq!(LoaderKind::of_component("Quilt-Loader-0.26.4-1.21.1"), Some(LoaderKind::Quilt));
    assert_eq!(LoaderKind::of_component("neoforge-21.1.77"), Some(LoaderKind::NeoForge));
    assert_eq!(LoaderKind::of_component("1.20.1-forge-47.3.0"), Some(LoaderKind::Forge));
    assert_eq!(LoaderKind::of_component("1.21.1"), None);
    assert_eq!(LoaderKind::from_client(" NeoForge "), Some(LoaderKind::NeoForge));
    assert_eq!(LoaderKind::from_client("TensaCraft"), None);
    assert_eq!(LoaderKind::NeoForge.display_name(), "NeoForge");
    assert_eq!(serde_json::to_value(LoaderKind::NeoForge).unwrap(), json!("neoforge"));
    let option = LoaderOption {
        mc: "1.21.1".into(),
        snapshot: false,
        builds: vec![LoaderBuild { version: "0.16.9".into(), stable: true }],
        default_version: "0.16.9".into(),
    };
    assert_eq!(
        serde_json::to_value(option).unwrap(),
        json!({"mc": "1.21.1", "snapshot": false, "builds": [{"version": "0.16.9", "stable": true}], "default_version": "0.16.9"})
    );
}

#[test]
fn components_keep_their_wire_shape() {
    let c = ComponentDto {
        id: "fabric-loader-0.16.9-1.21.1".into(),
        loader: Some(LoaderKind::Fabric),
        minecraft: Some("1.21.1".into()),
        loader_version: Some("0.16.9".into()),
        size: 10,
        modified_ms: Some(5),
        used_by: vec!["Aero".into()],
        base_for: Vec::new(),
    };
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(
        (&v["loader"], &v["loader_version"], &v["used_by"]),
        (&json!("fabric"), &json!("0.16.9"), &json!(["Aero"]))
    );
    assert_eq!(serde_json::from_value::<ComponentDto>(v).unwrap(), c);
    assert_eq!(serde_json::to_value(VerifyOutcome::Repaired).unwrap(), json!("repaired"));
    assert_eq!(serde_json::to_value(ComponentsSnapshot::default()).unwrap(), json!({"components": []}));
}

#[test]
fn build_settings_keep_their_wire_shape() {
    let update = BuildSettingsUpdate {
        name: "Aero".into(),
        component: Some("1.21.1".into()),
        java_path: None,
        gpu_mode: "igpu".into(),
        max_ram_gb: Some(6),
        jvm_arguments: vec!["-XX:+UseG1GC".into()],
        server_host: "play.example".into(),
        server_port: "25570".into(),
        image_path: None,
        remove_image: false,
        profile: Some("Alex".into()),
    };
    let v = serde_json::to_value(&update).unwrap();
    assert_eq!(
        (&v["max_ram_gb"], &v["server_port"], &v["remove_image"]),
        (&json!(6), &json!("25570"), &json!(false))
    );
    assert_eq!(serde_json::from_value::<BuildSettingsUpdate>(v).unwrap(), update);
    let settings = BuildSettingsDto {
        key: "aero".into(),
        name: "Aero".into(),
        image: None,
        component: Some("1.21.1".into()),
        java_path: None,
        auto_java: Some("C:/mc/runtime/java.exe".into()),
        gpu_mode: "dgpu".into(),
        max_ram_gb: None,
        jvm_arguments: Vec::new(),
        server_host: String::new(),
        server_port: None,
        profile: None,
    };
    let v = serde_json::to_value(&settings).unwrap();
    assert_eq!(serde_json::from_value::<BuildSettingsDto>(v).unwrap(), settings);
    // A build saved before builds had their own account reads as one without.
    let mut old = serde_json::to_value(&update).unwrap();
    old.as_object_mut().unwrap().remove("profile");
    assert_eq!(serde_json::from_value::<BuildSettingsUpdate>(old).unwrap().profile, None);
}

#[test]
fn content_types_keep_their_wire_shape() {
    assert_eq!(serde_json::to_value(ContentKind::ResourcePacks).unwrap(), json!("resourcepacks"));
    assert_eq!(serde_json::to_value(ContentKind::ShaderPacks).unwrap(), json!("shaderpacks"));
    assert_eq!(ContentKind::ALL.map(ContentKind::folder), ["mods", "resourcepacks", "shaderpacks"]);
    assert_eq!(
        (ContentKind::Mods.toggled_key(false), ContentKind::ShaderPacks.deleted_key()),
        ("mod_disabled", "shaderpack_deleted")
    );
    assert_eq!(ContentKind::ResourcePacks.toggled_key(true), "resourcepack_enabled");
    let list = ContentList {
        kind: ContentKind::Mods,
        supported: true,
        items: vec![ContentItem {
            file: "sodium.jar.disabled".into(),
            filename: "sodium.jar".into(),
            size: 3,
            enabled: false,
            folder: false,
            toggle_supported: true,
            name: Some("Sodium".into()),
            version: None,
            description: None,
            mod_id: Some("sodium".into()),
            has_backup: true,
        }],
    };
    let v = serde_json::to_value(&list).unwrap();
    assert_eq!((&v["kind"], &v["items"][0]["toggle_supported"]), (&json!("mods"), &json!(true)));
    assert_eq!(v["items"][0]["has_backup"], json!(true));
    let mut older = v.clone();
    older["items"][0].as_object_mut().unwrap().remove("has_backup");
    assert!(
        !serde_json::from_value::<ContentList>(older).unwrap().items[0].has_backup,
        "older lists lack it"
    );
    assert_eq!(serde_json::from_value::<ContentList>(v).unwrap(), list);
    assert_eq!(ErrorCode::GameRunning.i18n_key(), "instance_game_running");
    assert_eq!(ErrorCode::ContentConflict.i18n_key(), "content_file_exists");
}

#[test]
fn screenshots_keep_their_wire_shape() {
    let shot = ScreenshotDto {
        name: "2026-09-27_16.55.46.png".into(),
        size: 2_048,
        modified_ms: Some(1_790_000_000_000),
        src: "http://shot.localhost/aero/2026-09-27_16.55.46.png?v=1".into(),
    };
    let v = serde_json::to_value(&shot).unwrap();
    assert_eq!((&v["modified_ms"], &v["size"]), (&json!(1_790_000_000_000u64), &json!(2_048)));
    assert_eq!(serde_json::from_value::<ScreenshotDto>(v).unwrap(), shot);
}

#[test]
fn only_loader_builds_run_mods() {
    for (client, supported) in [
        (Some("Fabric"), true),
        (Some("NeoForge"), true),
        (Some("TensaCraft"), true),
        (Some("Minecraft"), false),
        (None, false),
        (Some("Custom"), false),
    ] {
        assert_eq!(mods_supported(client), supported, "{client:?}");
    }
}

#[test]
fn modrinth_install_errors_have_their_texts() {
    assert_eq!(ErrorCode::NoCompatibleVersion.i18n_key(), "no_compatible_version");
    assert_eq!(ErrorCode::NoFileFound.i18n_key(), "no_file_found");
    assert!(
        ErrorCode::ALL.contains(&ErrorCode::NoCompatibleVersion)
            && ErrorCode::ALL.contains(&ErrorCode::NoFileFound)
    );
}

#[test]
fn log_view_shape() {
    let view = LogView {
        file: "C:\\x\\app.log".into(),
        entries: vec![LogEntry {
            time: "2026-09-27 12:00:00".into(),
            level: Some(LogLevel::Warn),
            message: "a\nb".into(),
        }],
        skipped: 2,
    };
    assert_eq!(
        serde_json::to_value(&view).unwrap(),
        json!({"file": "C:\\x\\app.log", "skipped": 2,
               "entries": [{"time": "2026-09-27 12:00:00", "level": "warn", "message": "a\nb"}]})
    );
    let raw: LogEntry = serde_json::from_value(json!({"time": "", "level": null, "message": "x"})).unwrap();
    assert_eq!(raw.level, None);
}

#[test]
fn every_click_sound_has_a_file_and_a_label() {
    use launcher_shared::ClickSound;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let langs: Vec<serde_json::Value> = ["uk_UA", "en_US"]
        .iter()
        .map(|l| {
            serde_json::from_str(
                &std::fs::read_to_string(root.join(format!("assets/langs/{l}.json"))).unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(ClickSound::ALL.len(), 10, "three of the original's and seven new ones");
    for sound in ClickSound::ALL {
        assert!(root.join("assets/sounds").join(sound.file_name()).is_file(), "{sound:?}: no file");
        for lang in &langs {
            assert!(lang.get(sound.label_key()).is_some(), "{sound:?}: no label");
        }
    }
}

#[test]
fn click_sounds_round_trip_through_config() {
    use launcher_shared::ClickSound;
    for sound in ClickSound::ALL {
        assert_eq!(ClickSound::from_config_str(sound.as_config_str()), sound);
    }
    assert_eq!(ClickSound::from_config_str("no_such_sound"), ClickSound::default());
}

#[test]
fn the_title_bar_names_what_it_asks_of_the_window() {
    let names: Vec<serde_json::Value> = [
        WindowAction::Look,
        WindowAction::Minimize,
        WindowAction::Maximize,
        WindowAction::Close,
        WindowAction::Tray,
        WindowAction::Drag,
    ]
    .iter()
    .map(|a| serde_json::to_value(a).unwrap())
    .collect();
    assert_eq!(
        names,
        [json!("look"), json!("minimize"), json!("maximize"), json!("close"), json!("tray"), json!("drag")]
    );
}
