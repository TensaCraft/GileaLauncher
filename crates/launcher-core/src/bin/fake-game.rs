//! A stand-in for `java` in launch tests (never shipped). It records how it was started in
//! `fake-game.json` in its working directory, prints a line, then exits with the code of
//! `-Dfake.exit=N` after `-Dfake.after=MS` milliseconds (default: code 0 at once); `-Dfake.say=TEXT`
//! prints TEXT as a line of its output first; `-Dfake.log` keeps `logs/latest.log` open to write
//! while it runs, as a real game does.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number =
        |prefix: &str| args.iter().find_map(|a| a.strip_prefix(prefix)).and_then(|v| v.parse::<u64>().ok());
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let record = serde_json::json!({
        "args": args,
        "cwd": cwd,
        "java_tool_options": std::env::var("JAVA_TOOL_OPTIONS").ok(),
        "dri_prime": std::env::var("DRI_PRIME").ok(),
    });
    let _ = std::fs::write("fake-game.json", record.to_string());
    let _log = args.iter().any(|a| a == "-Dfake.log").then(|| {
        let _ = std::fs::create_dir_all("logs");
        std::fs::File::create("logs/latest.log")
    });
    println!("fake game started");
    if let Some(said) = args.iter().find_map(|a| a.strip_prefix("-Dfake.say=")) {
        println!("{said}");
    }
    if let Some(ms) = number("-Dfake.after=") {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    std::process::exit(number("-Dfake.exit=").unwrap_or(0) as i32);
}
