//! Desktop shortcuts that start one build: `.lnk` on Windows, `.desktop` on
//! Linux, an `.app` bundle on macOS. The command is the launcher (or its AppImage) with
//! `--launch-version=<id>`; the icon is the build's picture, or the grass block when it has none.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use launcher_shared::{AppError, AppResult, ErrorCode};
use sha2::{Digest, Sha256};

use crate::builds::ids::WINDOWS_RESERVED;
use crate::builds::settings::ICON_MAX_BYTES;
use crate::net::meta::MetaClient;
use crate::paths::Os;
use crate::storage::atomic::atomic_write;

pub const GRASS_BLOCK_PNG: &[u8] = include_bytes!("../../../../assets/img/grass_block.png");
const MAX_NAME_CHARS: usize = 64;
const MAX_COPIES: u32 = 100;
const ICON_DIR: &str = "shortcut-icons";

fn failed(detail: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ShortcutFailed, detail)
}

/// A file name for the shortcut: forbidden characters become `_`, no leading or trailing spaces
/// and dots, at most 64 characters, `instance` when nothing is left, `_` before Windows' reserved
/// names.
pub fn shortcut_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let short: String = cleaned.trim_matches([' ', '.']).chars().take(MAX_NAME_CHARS).collect();
    let mut short = short.trim_matches([' ', '.']).to_string();
    if short.is_empty() {
        return "instance".to_string();
    }
    let stem = short.split('.').next().unwrap_or_default();
    if WINDOWS_RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        short.insert(0, '_');
    }
    short
}

/// A short stable id of the build for icon files and the macOS bundle id.
pub fn identity(version_id: &str) -> String {
    hex::encode(Sha256::digest(version_id.as_bytes()))[..12].to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// `<AppImage or launcher> --launch-version=<id>`, run from the program's folder. Control
/// characters are refused, and quotes in the id (it becomes a bare Windows argument).
pub fn launch_command(exe: &Path, appimage: Option<&Path>, version_id: &str) -> AppResult<LaunchCommand> {
    let program = appimage.unwrap_or(exe).to_path_buf();
    let bad_id = version_id.trim().is_empty() || version_id.chars().any(|c| c.is_control() || c == '"');
    if bad_id || program.to_string_lossy().chars().any(char::is_control) {
        return Err(failed(format!("unusable shortcut command for {version_id:?}")));
    }
    let cwd = program.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(LaunchCommand { program, args: vec![format!("--launch-version={version_id}")], cwd })
}

/// A desktop-entry string value: backslashes doubled.
fn entry_value(text: &str) -> String {
    text.replace('\\', "\\\\")
}

/// One quoted word of `Exec`: `"`, `` ` ``, `$` and `\` escaped, `%` doubled.
fn exec_word(word: &str) -> String {
    let mut quoted = String::from("\"");
    for c in word.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The `Exec` value of a desktop entry.
pub fn desktop_exec(command: &LaunchCommand) -> String {
    let words: Vec<String> = std::iter::once(command.program.to_string_lossy().into_owned())
        .chain(command.args.iter().cloned())
        .map(|w| exec_word(&w))
        .collect();
    entry_value(&words.join(" "))
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(png[16..20].try_into().ok()?), u32::from_be_bytes(png[20..24].try_into().ok()?)))
}

/// A one-image ICO holding `png` as is (Windows reads PNG images inside ICO files).
pub fn ico_from_png(png: &[u8]) -> Vec<u8> {
    let (width, height) = png_size(png).unwrap_or((256, 256));
    let side = |v: u32| if v >= 256 { 0 } else { v as u8 };
    let mut ico = Vec::with_capacity(22 + png.len());
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    ico.extend_from_slice(&[side(width), side(height), 0, 0]);
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&32u16.to_le_bytes());
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    ico
}

/// An ICNS holding `png` under the type code of its size.
pub fn icns_from_png(png: &[u8]) -> Vec<u8> {
    let code: &[u8; 4] = match png_size(png).map(|s| s.0) {
        Some(16) => b"icp4",
        Some(32) => b"icp5",
        Some(64) => b"icp6",
        Some(256) => b"ic08",
        Some(512) => b"ic09",
        Some(1024) => b"ic10",
        _ => b"ic07",
    };
    let entry = 8 + png.len() as u32;
    let mut icns = Vec::with_capacity(16 + png.len());
    icns.extend_from_slice(b"icns");
    icns.extend_from_slice(&(8 + entry).to_be_bytes());
    icns.extend_from_slice(code);
    icns.extend_from_slice(&entry.to_be_bytes());
    icns.extend_from_slice(png);
    icns
}

/// The side of a shortcut's square icon.
const ICON_SIDE: u32 = 256;

/// A shortcut's icon as a PNG: build picture `picture` (PNG, JPEG, GIF or WebP) fitted into a
/// square, or the grass block when there is none or it cannot be read.
pub fn icon_png(picture: Option<&[u8]>) -> Vec<u8> {
    picture.and_then(square_png).unwrap_or_else(|| GRASS_BLOCK_PNG.to_vec())
}

/// `bytes` fitted into an `ICON_SIDE` square PNG, centred on transparency. A small picture (pixel
/// art, as build icons usually are) is enlarged without blurring.
fn square_png(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::imageops::{self, FilterType};
    let picture = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = picture.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let scale = f64::from(ICON_SIDE) / f64::from(w.max(h));
    let fit = |side: u32| ((f64::from(side) * scale).round() as u32).clamp(1, ICON_SIDE);
    let (fw, fh) = (fit(w), fit(h));
    let filter = if scale > 1.0 { FilterType::Nearest } else { FilterType::Lanczos3 };
    let fitted = imageops::resize(&picture, fw, fh, filter);
    let mut square = image::RgbaImage::new(ICON_SIDE, ICON_SIDE);
    imageops::overlay(&mut square, &fitted, i64::from((ICON_SIDE - fw) / 2), i64::from((ICON_SIDE - fh) / 2));
    let mut png = std::io::Cursor::new(Vec::new());
    square.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// A build picture's bytes from its record: raw base64 (the original's format) or a `data:` URL.
/// An address gives `None`: the caller fetches it.
pub fn picture_bytes(raw: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let raw = raw.trim();
    let data = match raw.strip_prefix("data:") {
        Some(url) => url.split_once(";base64,")?.1,
        None if raw.contains("://") => return None,
        None => raw,
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// The picture of a build whose record says `raw`: decoded from the record, or fetched when it
/// is an address (as the network rules allow, at most `ICON_MAX_BYTES`). `None` when there is
/// none or it cannot be had: the shortcut then shows the grass block.
pub async fn build_picture(meta: &MetaClient, raw: Option<&str>) -> Option<Vec<u8>> {
    let raw = raw.map(str::trim).filter(|r| !r.is_empty())?;
    if raw.contains("://") && !raw.starts_with("data:") {
        let bytes = meta.get_bytes(raw).await.ok()?;
        return (bytes.len() as u64 <= ICON_MAX_BYTES).then_some(bytes);
    }
    picture_bytes(raw)
}

fn icon_bytes(os: Os, png: &[u8]) -> (Vec<u8>, &'static str) {
    match os {
        Os::Windows => (ico_from_png(png), "ico"),
        Os::MacOs => (icns_from_png(png), "icns"),
        _ => (png.to_vec(), "png"),
    }
}

/// The icon file for `os` in `<state>/shortcut-icons/<identity>-<sha256[..16]>.<ext>`, written
/// once (atomically) and reused while its content is the same.
fn cached_icon(state_dir: &Path, identity: &str, os: Os, png: &[u8]) -> AppResult<PathBuf> {
    let (bytes, ext) = icon_bytes(os, png);
    let digest = hex::encode(Sha256::digest(&bytes));
    let path = state_dir.join(ICON_DIR).join(format!("{identity}-{}.{ext}", &digest[..16]));
    if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64) {
        return Ok(path);
    }
    fs::create_dir_all(state_dir.join(ICON_DIR))
        .and_then(|()| atomic_write(&path, &bytes))
        .map_err(|e| failed(format!("unable to write {}: {e}", path.display())))?;
    Ok(path)
}

/// `<desktop>/<name><ext>`, else `<name> (2)<ext>` … `(100)`; created exclusively so nothing is
/// ever overwritten. Files come back open; bundle folders come back created.
fn reserve(desktop: &Path, name: &str, ext: &str, folder: bool) -> AppResult<(PathBuf, Option<File>)> {
    for n in 1..=MAX_COPIES {
        let file_name = if n == 1 { format!("{name}{ext}") } else { format!("{name} ({n}){ext}") };
        let path = desktop.join(file_name);
        let created = if folder {
            fs::create_dir(&path).map(|()| None)
        } else {
            OpenOptions::new().write(true).create_new(true).open(&path).map(Some)
        };
        match created {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(failed(format!("unable to create {}: {e}", path.display()))),
        }
    }
    Err(failed(format!("{MAX_COPIES} shortcuts named {name} already exist")))
}

/// `0700`, as the original writes desktop entries (nothing to change on Windows).
fn owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_desktop_entry(
    desktop: &Path,
    name: &str,
    command: &LaunchCommand,
    icon: &Path,
) -> AppResult<PathBuf> {
    let (path, file) = reserve(desktop, name, ".desktop", false)?;
    let text = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nExec={}\nPath={}\nIcon={}\nTerminal=false\nCategories=Game;\n",
        entry_value(name),
        desktop_exec(command),
        entry_value(&command.cwd.to_string_lossy()),
        entry_value(&icon.to_string_lossy()),
    );
    let written = file.expect("a reserved file").write_all(text.as_bytes()).and_then(|()| owner_only(&path));
    if let Err(e) = written {
        let _ = fs::remove_file(&path);
        return Err(failed(format!("unable to write {}: {e}", path.display())));
    }
    Ok(path)
}

fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn fill_app_bundle(contents: &Path, script: &str, plist: &str, png: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;
    let launch = contents.join("MacOS").join("launch");
    fs::write(&launch, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&launch, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(contents.join("Resources").join("instance.icns"), icon_bytes(Os::MacOs, png).0)?;
    fs::write(contents.join("Info.plist"), plist)
}

fn write_app_bundle(
    desktop: &Path,
    name: &str,
    identity: &str,
    command: &LaunchCommand,
    png: &[u8],
) -> AppResult<PathBuf> {
    let (app, _) = reserve(desktop, name, ".app", true)?;
    let contents = app.join("Contents");
    let words: Vec<String> = std::iter::once(command.program.to_string_lossy().into_owned())
        .chain(command.args.iter().cloned())
        .map(|w| sh_quote(&w))
        .collect();
    let script = format!(
        "#!/bin/sh\ncd {} || exit 1\nexec {}\n",
        sh_quote(&command.cwd.to_string_lossy()),
        words.join(" ")
    );
    let plist = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\">\n<dict>\n",
            "  <key>CFBundleName</key><string>{name}</string>\n",
            "  <key>CFBundleIdentifier</key><string>{IDENTIFIER}.instance.{identity}</string>\n",
            "  <key>CFBundlePackageType</key><string>APPL</string>\n",
            "  <key>CFBundleExecutable</key><string>launch</string>\n",
            "  <key>CFBundleIconFile</key><string>instance</string>\n",
            "  <key>LSUIElement</key><true/>\n",
            "</dict>\n</plist>\n"
        ),
        name = xml_escape(name),
        identity = identity,
        IDENTIFIER = launcher_shared::branding::IDENTIFIER,
    );
    if let Err(e) = fill_app_bundle(&contents, &script, &plist, png) {
        let _ = fs::remove_dir_all(&app);
        return Err(failed(format!("unable to write {}: {e}", app.display())));
    }
    Ok(app)
}

#[cfg(windows)]
fn write_shell_link(link: &Path, command: &LaunchCommand, icon: &Path) -> Result<(), String> {
    // mslnk panics on some unusual paths; a panic becomes a failed shortcut, not a crash.
    std::panic::catch_unwind(|| -> Result<(), String> {
        let mut shell = mslnk::ShellLink::new(&command.program).map_err(|e| e.to_string())?;
        shell.set_arguments(Some(command.args.join(" ")));
        shell.set_working_dir(Some(command.cwd.to_string_lossy().into_owned()));
        shell.set_icon_location(Some(icon.to_string_lossy().into_owned()));
        shell.create_lnk(link).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|_| Err("the shell link writer panicked".to_string()))
}

#[cfg(not(windows))]
fn write_shell_link(_link: &Path, _command: &LaunchCommand, _icon: &Path) -> Result<(), String> {
    Err("Windows shortcuts are written on Windows only".to_string())
}

fn write_windows_link(
    desktop: &Path,
    name: &str,
    command: &LaunchCommand,
    icon: &Path,
) -> AppResult<PathBuf> {
    let (path, file) = reserve(desktop, name, ".lnk", false)?;
    drop(file);
    if let Err(e) = write_shell_link(&path, command, icon) {
        let _ = fs::remove_file(&path);
        return Err(failed(format!("unable to write {}: {e}", path.display())));
    }
    Ok(path)
}

pub struct ShortcutService {
    /// Icons are cached in `<state>/shortcut-icons`.
    pub state_dir: PathBuf,
    pub exe: PathBuf,
    /// `$APPIMAGE` on Linux: shortcuts start the AppImage, not its mounted copy.
    pub appimage: Option<PathBuf>,
    pub os: Os,
    pub desktop: Option<PathBuf>,
}

impl ShortcutService {
    pub fn from_system(state_dir: &Path) -> ShortcutService {
        let os = Os::current();
        let appimage =
            std::env::var_os("APPIMAGE").filter(|v| os == Os::Linux && !v.is_empty()).map(PathBuf::from);
        ShortcutService {
            state_dir: state_dir.to_path_buf(),
            exe: std::env::current_exe().unwrap_or_default(),
            appimage,
            os,
            desktop: dirs::desktop_dir().or_else(|| dirs::home_dir().map(|h| h.join("Desktop"))),
        }
    }

    /// Creates a desktop shortcut that starts build `version_id`, with the build's picture
    /// `picture` as its icon, and returns its path. Blocking.
    pub fn create(&self, version_id: &str, name: &str, picture: Option<&[u8]>) -> AppResult<PathBuf> {
        let png = icon_png(picture);
        let command = launch_command(&self.exe, self.appimage.as_deref(), version_id)?;
        let desktop =
            self.desktop.as_deref().filter(|d| d.is_dir()).ok_or_else(|| failed("no desktop folder"))?;
        let identity = identity(version_id);
        let name = shortcut_name(name);
        match self.os {
            Os::Windows => write_windows_link(
                desktop,
                &name,
                &command,
                &cached_icon(&self.state_dir, &identity, self.os, &png)?,
            ),
            Os::MacOs => write_app_bundle(desktop, &name, &identity, &command, &png),
            Os::Linux | Os::Other => write_desktop_entry(
                desktop,
                &name,
                &command,
                &cached_icon(&self.state_dir, &identity, self.os, &png)?,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Desk {
        dir: tempfile::TempDir,
        service: ShortcutService,
    }

    fn desk(os: Os, exe: &str) -> Desk {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Desktop")).unwrap();
        let service = ShortcutService {
            state_dir: dir.path().join("state"),
            exe: PathBuf::from(exe),
            appimage: None,
            os,
            desktop: Some(dir.path().join("Desktop")),
        };
        Desk { dir, service }
    }

    fn file_name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn shortcut_names_are_safe_file_names() {
        assert_eq!(shortcut_name("a/b:c*?"), "a_b_c__");
        assert_eq!(shortcut_name("  .Aero. "), "Aero");
        assert_eq!(shortcut_name("tab\there"), "tab_here");
        assert_eq!(shortcut_name("CON"), "_CON");
        assert_eq!(shortcut_name("con.txt"), "_con.txt");
        assert_eq!(shortcut_name("Console"), "Console");
        assert_eq!(shortcut_name("..."), "instance");
        assert_eq!(shortcut_name(""), "instance");
        assert_eq!(shortcut_name(&"ї".repeat(100)).chars().count(), 64);
        assert_eq!(identity("aero"), "101a07e5f182");
        for bad in ["", "a\nb", "a\"b"] {
            assert_eq!(
                launch_command(Path::new("/opt/app"), None, bad).unwrap_err().code,
                ErrorCode::ShortcutFailed,
                "{bad:?}"
            );
        }
        let command =
            launch_command(Path::new("/opt/app/Launcher"), Some(Path::new("/home/p/App.AppImage")), "aero")
                .unwrap();
        assert_eq!(command.program, PathBuf::from("/home/p/App.AppImage"));
        assert_eq!(command.args, ["--launch-version=aero"]);
        assert_eq!(command.cwd, PathBuf::from("/home/p"));
    }

    #[test]
    fn desktop_entries_escape_the_command() {
        let d = desk(Os::Linux, "/opt/App $x/\"q\"/Launcher 100%");
        let path = d.service.create("aero", "Aero", None).unwrap();
        assert_eq!(path, d.dir.path().join("Desktop").join("Aero.desktop"));
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[Desktop Entry]");
        assert!(
            lines.contains(&r#"Exec="/opt/App \\$x/\\"q\\"/Launcher 100%%" "--launch-version=aero""#),
            "{text}"
        );
        assert!(lines.contains(&r#"Path=/opt/App $x/"q""#), "{text}");
        assert!(
            lines.contains(&"Name=Aero")
                && lines.contains(&"Terminal=false")
                && lines.contains(&"Categories=Game;")
        );
        let icon = lines.iter().find_map(|l| l.strip_prefix("Icon=")).unwrap().replace(r"\\", r"\");
        let icon_name = file_name(Path::new(&icon));
        assert!(icon_name.starts_with("101a07e5f182-") && icon_name.ends_with(".png"), "{icon_name}");
        assert_eq!(fs::read(&icon).unwrap(), GRASS_BLOCK_PNG);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn shortcuts_never_overwrite() {
        let d = desk(Os::Linux, "/opt/app/Launcher");
        let desktop = d.dir.path().join("Desktop");
        assert_eq!(file_name(&d.service.create("aero", "Aero", None).unwrap()), "Aero.desktop");
        assert_eq!(file_name(&d.service.create("aero", "Aero", None).unwrap()), "Aero (2).desktop");
        fs::write(desktop.join("Aero (3).desktop"), b"mine").unwrap();
        assert_eq!(file_name(&d.service.create("aero", "Aero", None).unwrap()), "Aero (4).desktop");
        assert_eq!(fs::read(desktop.join("Aero (3).desktop")).unwrap(), b"mine");
        for n in 5..=100 {
            fs::write(desktop.join(format!("Aero ({n}).desktop")), b"x").unwrap();
        }
        assert_eq!(d.service.create("aero", "Aero", None).unwrap_err().code, ErrorCode::ShortcutFailed);
        let mut no_desktop = d.service;
        no_desktop.desktop = Some(d.dir.path().join("missing"));
        assert_eq!(no_desktop.create("aero", "Aero", None).unwrap_err().code, ErrorCode::ShortcutFailed);
    }

    #[test]
    fn mac_bundles_launch_the_build() {
        let d = desk(Os::MacOs, "/Applications/App's/Launcher");
        let app = d.service.create("aero", "Aero & Co", None).unwrap();
        assert_eq!(file_name(&app), "Aero & Co.app");
        let launch = fs::read_to_string(app.join("Contents").join("MacOS").join("launch")).unwrap();
        assert!(launch.starts_with("#!/bin/sh\n"), "{launch}");
        assert!(launch.contains(r"cd '/Applications/App'\''s' || exit 1"), "{launch}");
        assert!(
            launch.contains(r"exec '/Applications/App'\''s/Launcher' '--launch-version=aero'"),
            "{launch}"
        );
        let plist = fs::read_to_string(app.join("Contents").join("Info.plist")).unwrap();
        let id = format!("<string>{}.instance.101a07e5f182</string>", launcher_shared::branding::IDENTIFIER);
        assert!(plist.contains(&id), "{plist}");
        assert!(plist.contains("<string>Aero &amp; Co</string>"), "{plist}");
        let icns = fs::read(app.join("Contents").join("Resources").join("instance.icns")).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(&icns[8..12], b"ic07");
        assert_eq!(file_name(&d.service.create("aero", "Aero & Co", None).unwrap()), "Aero & Co (2).app");
    }

    /// A `w`×`h` picture of one colour, encoded as `format`.
    fn picture(w: u32, h: u32, colour: [u8; 4], format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba(colour)));
        let img = if format == image::ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(img.to_rgb8())
        } else {
            img
        };
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn pixels(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap().to_rgba8()
    }

    #[test]
    fn a_shortcut_shows_the_build_picture() {
        let red = [200, 30, 30, 255];
        let icon = pixels(&icon_png(Some(&picture(64, 64, red, image::ImageFormat::Png))));
        assert_eq!(icon.dimensions(), (ICON_SIDE, ICON_SIDE));
        assert_eq!(icon.get_pixel(0, 0).0, red, "a small picture fills the icon");
        assert_eq!(icon.get_pixel(128, 128).0, red);
        let wide = pixels(&icon_png(Some(&picture(512, 256, red, image::ImageFormat::Png))));
        assert_eq!(wide.dimensions(), (ICON_SIDE, ICON_SIDE));
        assert_eq!(wide.get_pixel(128, 10).0[3], 0, "a wide picture keeps its shape, centred");
        assert_eq!(wide.get_pixel(128, 128).0, red);
        let jpeg = pixels(&icon_png(Some(&picture(100, 100, [20, 180, 60, 255], image::ImageFormat::Jpeg))));
        let [r, g, b, a] = jpeg.get_pixel(128, 128).0;
        assert!(r < 60 && g > 140 && b < 100 && a == 255, "a JPEG picture too: {r} {g} {b} {a}");
        assert_eq!(icon_png(None), GRASS_BLOCK_PNG, "no picture: the grass block");
        assert_eq!(icon_png(Some(b"not a picture")), GRASS_BLOCK_PNG);
    }

    #[test]
    fn build_pictures_come_from_the_record() {
        use base64::Engine;
        let png = picture(8, 8, [1, 2, 3, 255], image::ImageFormat::Png);
        let raw = base64::engine::general_purpose::STANDARD.encode(&png);
        assert_eq!(picture_bytes(&raw), Some(png.clone()), "the original's raw base64");
        assert_eq!(picture_bytes(&format!("data:image/png;base64,{raw}")), Some(png));
        assert_eq!(picture_bytes("https://cdn.example/icon.png"), None, "an address is fetched elsewhere");
        assert_eq!(picture_bytes("  "), None);
        assert_eq!(picture_bytes("%%% not base64"), None);
    }

    #[test]
    fn the_shortcut_icon_is_the_build_picture() {
        let red = [200, 30, 30, 255];
        let pic = picture(32, 32, red, image::ImageFormat::Png);
        let d = desk(Os::Linux, "/opt/app/Launcher");
        let text = fs::read_to_string(d.service.create("aero", "Aero", Some(&pic)).unwrap()).unwrap();
        let icon = text.lines().find_map(|l| l.strip_prefix("Icon=")).unwrap().replace(r"\\", r"\");
        assert_eq!(pixels(&fs::read(&icon).unwrap()).get_pixel(128, 128).0, red);
        let mac = desk(Os::MacOs, "/Applications/Launcher");
        let app = mac.service.create("aero", "Aero", Some(&pic)).unwrap();
        let icns = fs::read(app.join("Contents").join("Resources").join("instance.icns")).unwrap();
        assert_eq!(&icns[8..12], b"ic08", "a 256 px icon");
        assert_eq!(pixels(&icns[16..]).get_pixel(128, 128).0, red);
    }

    #[test]
    fn icons_are_content_addressed() {
        let ico = ico_from_png(GRASS_BLOCK_PNG);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!((ico[6], ico[7]), (128, 128));
        assert_eq!(u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize, GRASS_BLOCK_PNG.len());
        assert_eq!(&ico[22..], GRASS_BLOCK_PNG);
        let dir = tempfile::tempdir().unwrap();
        let first = cached_icon(dir.path(), "101a07e5f182", Os::Windows, GRASS_BLOCK_PNG).unwrap();
        let second = cached_icon(dir.path(), "101a07e5f182", Os::Windows, GRASS_BLOCK_PNG).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.parent().unwrap(), dir.path().join("shortcut-icons"));
        let name = file_name(&first);
        assert!(
            name.starts_with("101a07e5f182-") && name.ends_with(".ico") && name.len() == 13 + 16 + 4,
            "{name}"
        );
        assert_eq!(fs::read(&first).unwrap(), ico);
    }

    #[cfg(windows)]
    #[test]
    fn windows_links_point_at_the_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Launcher.exe");
        fs::write(&exe, b"MZ").unwrap();
        let mut d = desk(Os::Windows, &exe.to_string_lossy());
        d.service.desktop = Some(dir.path().to_path_buf());
        let link = d.service.create("aero", "Aero", None).unwrap();
        assert_eq!(link, dir.path().join("Aero.lnk"));
        let bytes = fs::read(&link).unwrap();
        assert_eq!(&bytes[..4], &[0x4C, 0, 0, 0], "a shell link header");
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let contains = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        assert!(contains(&utf16("--launch-version=aero")));
        assert!(contains(&utf16(".ico")), "the icon is set");
        assert_eq!(file_name(&d.service.create("aero", "Aero", None).unwrap()), "Aero (2).lnk");
    }
}
