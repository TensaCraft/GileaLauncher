//! What Horizon's programs are told and what they answer (its Launch API, `docs/api/launch.md`):
//! `horizon-prepare`'s progress lines, `horizon-client --launch-api`'s options, the client's
//! command line and the session it reads on its standard input.

use std::path::Path;

use launcher_shared::recent::Join;
use launcher_shared::{AppError, AppResult, ErrorCode};
use serde_json::{Value, json};

/// The Launch API line this launcher speaks: a build serving another refuses our command line.
pub const LAUNCH_API_LINE: &str = "0.1";

/// One line of `horizon-prepare --progress json`.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// How far the work is, 0 to 1.
    Fraction(f64),
    /// A line for the player.
    Message(String),
    /// Why it failed (the last line).
    Error(String),
    /// A line with no key we know: ignored.
    Other,
}

pub fn progress(line: &str) -> Progress {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line.trim()) else { return Progress::Other };
    if let Some(error) = map.get("error").and_then(Value::as_str) {
        return Progress::Error(error.to_string());
    }
    if let Some(fraction) = map.get("fraction").and_then(Value::as_f64) {
        return Progress::Fraction(fraction.clamp(0.0, 1.0));
    }
    map.get("message").and_then(Value::as_str).map_or(Progress::Other, |m| Progress::Message(m.to_string()))
}

/// `horizon-prepare`'s arguments that turn the player's Minecraft (`minecraft`, the official
/// launcher's layout) into the resources in `output`.
pub fn prepare_args(
    output: &Path,
    minecraft: &Path,
    server_jar: &Path,
    java_home: Option<&Path>,
) -> Vec<String> {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut args = vec![
        "--prepare".into(),
        "--output".into(),
        path(output),
        "--minecraft-dir".into(),
        path(minecraft),
        "--server-jar".into(),
        path(server_jar),
    ];
    if let Some(home) = java_home {
        args.extend(["--java-home".into(), path(home)]);
    }
    args.extend(["--complete".into(), "--progress".into(), "json".into()]);
    args
}

/// The options `horizon-client --launch-api`'s reply lists; a build of another Launch API line is
/// refused.
pub fn offered(reply: &str) -> AppResult<Vec<String>> {
    let refused = |why: String| AppError::new(ErrorCode::LaunchFailed, why);
    let reply: Value = serde_json::from_str(reply).map_err(|e| refused(format!("--launch-api: {e}")))?;
    let line = reply["launch_api"].as_str().unwrap_or_default();
    if line != LAUNCH_API_LINE && !line.starts_with(&format!("{LAUNCH_API_LINE}.")) {
        return Err(refused(format!(
            "Horizon's Launch API {line:?}; this launcher speaks {LAUNCH_API_LINE}"
        )));
    }
    Ok(reply["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o["name"].as_str().map(str::to_string))
        .collect())
}

/// What a start of the client is asked for.
pub struct Start<'a> {
    pub resources: &'a Path,
    /// The build's folder: its saves, packs and settings, and its logs and crash reports.
    pub game_dir: &'a Path,
    /// The window's size, when the build sets one.
    pub size: Option<(u32, u32)>,
    pub fullscreen: bool,
    pub join: Option<&'a Join>,
}

/// The client's arguments for `start`: only options the build `offered`; the session always comes
/// on standard input (refused when the build cannot take it).
pub fn client_args(offered: &[String], start: &Start) -> AppResult<Vec<String>> {
    let has = |option: &str| offered.iter().any(|o| o == option);
    if !has("--session-stdin") {
        return Err(AppError::new(
            ErrorCode::LaunchFailed,
            "this Horizon build takes no session on its input",
        ));
    }
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut args = Vec::new();
    let mut give = |option: &str, value: Option<String>| {
        if has(option) {
            args.push(option.to_string());
            args.extend(value);
        }
    };
    give("--resources", Some(path(start.resources)));
    give("--game-dir", Some(path(start.game_dir)));
    give("--home", Some(path(start.game_dir)));
    if let Some((width, height)) = start.size {
        give("--width", Some(width.to_string()));
        give("--height", Some(height.to_string()));
    }
    if start.fullscreen {
        give("--fullscreen", None);
    }
    match start.join {
        Some(Join::World { folder }) => give("--quickPlaySingleplayer", Some(folder.clone())),
        Some(Join::Server { host, port }) if !host.trim().is_empty() => {
            give("--quickPlayMultiplayer", Some(format!("{}:{port}", host.trim())));
        }
        _ => {}
    }
    give("--session-stdin", None);
    Ok(args)
}

/// The session JSON the client reads on its standard input: a Microsoft account's.
pub fn session(name: &str, uuid: &str, access_token: &str, xuid: Option<&str>) -> Vec<u8> {
    let mut session = json!({"name": name, "uuid": uuid, "access_token": access_token, "user_type": "msa"});
    if let Some(xuid) = xuid {
        session["xuid"] = json!(xuid);
    }
    session.to_string().into_bytes()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// `horizon-client --launch-api` of Launch API 0.1.0 (their `docs/api/launch-reference.md`).
    const REPLY: &str = r#"{"crash_report_line":"Horizon: crash report saved to ","exit_codes":[0,1,2,101],
        "horizon":"0.2.36","launch_api":"0.1.0","minecraft":"26.3","options":[
        {"name":"--resources","value":"DIR"},{"name":"--game-dir","value":"DIR"},{"name":"--home","value":"DIR"},
        {"name":"--player-dir","value":"DIR"},{"name":"--width","value":"N"},{"name":"--height","value":"N"},
        {"name":"--fullscreen","value":null},{"name":"--session-file","value":"PATH"},
        {"name":"--session-stdin","value":null},{"name":"--quickPlaySingleplayer","value":"WORLD"},
        {"name":"--quickPlayMultiplayer","value":"HOST[:PORT]"},{"name":"--launch-api","value":null}]}"#;

    #[test]
    fn preparing_reports_its_fraction_its_words_and_its_error() {
        assert_eq!(progress(r#"{"fraction": 0.25}"#), Progress::Fraction(0.25));
        assert_eq!(progress(r#"{"fraction": 7}"#), Progress::Fraction(1.0));
        assert_eq!(progress(r#"{"message": "Copying sounds"}"#), Progress::Message("Copying sounds".into()));
        assert_eq!(
            progress(r#"{"error": "no client jar", "fraction": 0.5}"#),
            Progress::Error("no client jar".into())
        );
        assert_eq!(progress(r#"{"stage": 3}"#), Progress::Other);
        assert_eq!(progress("Preparing Horizon"), Progress::Other);
    }

    #[test]
    fn preparing_names_the_player_s_files_and_ends_complete() {
        let (out, mc, jar, java) =
            (PathBuf::from("out"), PathBuf::from("mc"), PathBuf::from("server.jar"), PathBuf::from("jre"));
        assert_eq!(
            prepare_args(&out, &mc, &jar, Some(&java)),
            [
                "--prepare",
                "--output",
                "out",
                "--minecraft-dir",
                "mc",
                "--server-jar",
                "server.jar",
                "--java-home",
                "jre",
                "--complete",
                "--progress",
                "json"
            ]
        );
        assert!(!prepare_args(&out, &mc, &jar, None).contains(&"--java-home".to_string()));
    }

    #[test]
    fn only_a_build_of_our_launch_api_line_is_asked_for_its_options() {
        let options = offered(REPLY).unwrap();
        assert!(options.contains(&"--session-stdin".to_string()) && options.len() == 12);
        let newer = REPLY.replace("\"0.1.0\"", "\"0.2.0\"");
        assert_eq!(offered(&newer).unwrap_err().code, ErrorCode::LaunchFailed);
        assert_eq!(offered("not json").unwrap_err().code, ErrorCode::LaunchFailed);
    }

    #[test]
    fn the_client_gets_only_what_its_build_offers() {
        let (resources, game) = (PathBuf::from("res"), PathBuf::from("game"));
        let join = Join::Server { host: " play.example.net ".into(), port: 25565 };
        let start = Start {
            resources: &resources,
            game_dir: &game,
            size: Some((1280, 720)),
            fullscreen: true,
            join: Some(&join),
        };
        assert_eq!(
            client_args(&offered(REPLY).unwrap(), &start).unwrap(),
            [
                "--resources",
                "res",
                "--game-dir",
                "game",
                "--home",
                "game",
                "--width",
                "1280",
                "--height",
                "720",
                "--fullscreen",
                "--quickPlayMultiplayer",
                "play.example.net:25565",
                "--session-stdin"
            ]
        );
        let old = ["--resources".to_string(), "--session-stdin".to_string()];
        assert_eq!(client_args(&old, &start).unwrap(), ["--resources", "res", "--session-stdin"]);
        let no_stdin = ["--resources".to_string(), "--session-file".to_string()];
        assert_eq!(client_args(&no_stdin, &start).unwrap_err().code, ErrorCode::LaunchFailed);
        let world = Join::World { folder: "Мій світ".into() };
        let quiet = Start { size: None, fullscreen: false, join: Some(&world), ..start };
        let args = client_args(&offered(REPLY).unwrap(), &quiet).unwrap();
        assert!(args.windows(2).any(|w| w == ["--quickPlaySingleplayer", "Мій світ"]));
        assert!(!args.iter().any(|a| a == "--width" || a == "--fullscreen"));
    }

    #[test]
    fn the_session_is_a_microsoft_account_s() {
        let session: Value =
            serde_json::from_slice(&session("Alex", "069a79f4", "token", Some("2535"))).unwrap();
        assert_eq!(
            session,
            json!({"name": "Alex", "uuid": "069a79f4", "access_token": "token", "user_type": "msa", "xuid": "2535"})
        );
        let bare: Value = serde_json::from_slice(&super::session("Alex", "u", "t", None)).unwrap();
        assert!(bare.get("xuid").is_none());
    }
}
