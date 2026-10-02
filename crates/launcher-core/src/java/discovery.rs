//! Finding Java on this computer: the launcher's runtimes, `JAVA_HOME` and
//! friends, `PATH`, and the usual install folders — including `MinecraftJava.exe` and the Linux and
//! macOS locations.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

pub const JAVA_EXECUTABLES: [&str; 4] = ["java.exe", "javaw.exe", "MinecraftJava.exe", "java"];
const MAX_DEPTH: usize = 5;
const MAX_DIRS: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryRoots {
    pub minecraft_dir: PathBuf,
    pub app_state_dir: PathBuf,
    /// `JAVA_HOME`, `JDK_HOME`, `JRE_HOME`.
    pub java_homes: Vec<PathBuf>,
    /// The folders of `PATH`.
    pub path_dirs: Vec<PathBuf>,
    /// Searched up to five folders deep.
    pub common_roots: Vec<PathBuf>,
}

impl DiscoveryRoots {
    pub fn from_system(minecraft_dir: &Path, app_state_dir: &Path) -> DiscoveryRoots {
        let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let java_homes = ["JAVA_HOME", "JDK_HOME", "JRE_HOME"].iter().filter_map(|n| env(n)).collect();
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).filter(|d| !d.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        let mut common_roots = Vec::new();
        for base in ["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(|n| env(n)) {
            for vendor in ["Java", "Eclipse Adoptium", "Microsoft", "BellSoft", "Azul Systems", "Zulu"] {
                common_roots.push(base.join(vendor));
            }
        }
        if let Some(local) = env("LOCALAPPDATA") {
            common_roots.push(local.join("Programs").join("Java"));
            common_roots.push(local.join("Programs").join("Eclipse Adoptium"));
        }
        if let Some(roaming) = env("APPDATA") {
            common_roots.push(roaming.join(".minecraft").join("runtime"));
            common_roots.push(
                roaming.join(".tlauncher").join("legacy").join("Minecraft").join("game").join("runtime"),
            );
        }
        if let Some(home) = dirs::home_dir() {
            common_roots.push(home.join(".jdks"));
            common_roots.push(home.join(".sdkman").join("candidates").join("java"));
            common_roots.push(home.join("Library").join("Java").join("JavaVirtualMachines"));
        }
        for fixed in ["/usr/lib/jvm", "/usr/java", "/opt", "/Library/Java/JavaVirtualMachines"] {
            common_roots.push(PathBuf::from(fixed));
        }
        DiscoveryRoots {
            minecraft_dir: minecraft_dir.to_path_buf(),
            app_state_dir: app_state_dir.to_path_buf(),
            java_homes,
            path_dirs,
            common_roots,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundJava {
    pub label: String,
    pub path: PathBuf,
}

pub fn is_java_name(name: &str) -> bool {
    JAVA_EXECUTABLES.iter().any(|j| j.eq_ignore_ascii_case(name))
}

/// `path` without links, absolute, and without Windows' `\\?\` prefix on plain drive paths.
pub fn resolve_path(path: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(path).ok()?;
    #[cfg(windows)]
    {
        let text = real.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return Some(PathBuf::from(rest.to_string()));
        }
    }
    Some(real)
}

/// The Java home: the folder above `bin/`, else the executable's folder.
pub fn java_home_of(executable: &Path) -> PathBuf {
    let parent = executable.parent().unwrap_or(Path::new(""));
    if parent.file_name().is_some_and(|n| n.eq_ignore_ascii_case("bin")) {
        parent.parent().unwrap_or(parent).to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

/// `KEY="value"` lines of the home's `release` file.
pub fn release_metadata(executable: &Path) -> BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(java_home_of(executable).join("release")) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().trim_matches('"').to_string()))
        .collect()
}

/// The launcher runtime folder (`java-runtime-delta`, `jre-legacy`…) holding `executable`.
fn launcher_runtime_id(executable: &Path, roots: &DiscoveryRoots) -> Option<String> {
    [roots.minecraft_dir.join("runtime"), roots.app_state_dir.join("runtime")]
        .iter()
        .filter_map(|root| resolve_path(root))
        .find_map(|root| executable.strip_prefix(&root).ok().and_then(|rel| rel.components().next()))
        .map(|first| first.as_os_str().to_string_lossy().into_owned())
}

/// The original's label: launcher runtimes by id and version, a given name, `release` data, or
/// the Java home's folder name.
pub fn label_for(executable: &Path, name: Option<&str>, roots: &DiscoveryRoots) -> String {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    let meta = release_metadata(executable);
    let version = meta.get("JAVA_VERSION").map(|v| v.trim()).filter(|v| !v.is_empty());
    let runtime = name
        .filter(|n| n.starts_with("java-runtime-"))
        .map(str::to_string)
        .or_else(|| launcher_runtime_id(executable, roots));
    if let Some(runtime) = runtime {
        return match version {
            Some(v) => format!("Launcher Java {v} ({runtime})"),
            None => format!("Launcher Java ({runtime})"),
        };
    }
    if let Some(name) = name {
        return name.to_string();
    }
    let implementor = meta.get("IMPLEMENTOR").map(|v| v.trim()).filter(|v| !v.is_empty());
    match (version, implementor) {
        (Some(v), Some(i)) => format!("{i} Java {v}"),
        (Some(v), None) => format!("Java {v}"),
        _ => java_home_of(executable)
            .file_name()
            .or(executable.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// Java executables under `base`: folders up to `max_depth` below it, at most `MAX_DIRS` folders,
/// links not followed; with `first_only` the first one found.
fn find_executables(base: &Path, max_depth: usize, first_only: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(base.to_path_buf(), 0usize)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_DIRS {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        let (mut files, mut dirs) = (Vec::new(), Vec::new());
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
                Ok(_) => files.push(entry.path()),
                Err(_) => {}
            }
        }
        for candidate in JAVA_EXECUTABLES {
            let hit = files
                .iter()
                .find(|f| f.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(candidate)));
            if let Some(file) = hit {
                found.push(file.clone());
                if first_only {
                    return found;
                }
            }
        }
        if depth < max_depth {
            dirs.sort();
            stack.extend(dirs.into_iter().rev().map(|d| (d, depth + 1)));
        }
    }
    found
}

/// Every Java found, one per real path, sorted by label.
pub fn discover(roots: &DiscoveryRoots) -> Vec<FoundJava> {
    let mut found: HashMap<String, FoundJava> = HashMap::new();
    let mut add = |name: Option<&str>, candidate: &Path| {
        let Some(path) = resolve_path(candidate) else { return };
        if !path.is_file() || !path.file_name().is_some_and(|n| is_java_name(&n.to_string_lossy())) {
            return;
        }
        let key = path.to_string_lossy().to_lowercase();
        found
            .entry(key)
            .or_insert_with(|| FoundJava { label: label_for(&path, name, roots).trim().to_string(), path });
    };
    for runtime_root in [roots.minecraft_dir.join("runtime"), roots.app_state_dir.join("runtime")] {
        let Ok(entries) = fs::read_dir(&runtime_root) else { continue };
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .collect();
        children.sort();
        for child in children {
            let name = child.file_name().map(|n| n.to_string_lossy().into_owned());
            if let Some(java) = find_executables(&child, usize::MAX, true).first() {
                add(name.as_deref(), java);
            }
        }
    }
    for home in &roots.java_homes {
        if let Some(java) = find_executables(home, usize::MAX, true).first() {
            add(None, java);
        }
    }
    for dir in &roots.path_dirs {
        for exe in JAVA_EXECUTABLES {
            add(None, &dir.join(exe));
        }
    }
    for root in &roots.common_roots {
        for exe in find_executables(root, MAX_DEPTH, false) {
            add(None, &exe);
        }
    }
    let mut list: Vec<FoundJava> = found.into_values().collect();
    list.sort_by(|a, b| {
        a.label.to_lowercase().cmp(&b.label.to_lowercase()).then_with(|| a.path.cmp(&b.path))
    });
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jdk(root: &Path, folder: &str, release: Option<&str>) -> PathBuf {
        let bin = root.join(folder).join("bin");
        fs::create_dir_all(&bin).unwrap();
        let java = bin.join("java");
        fs::write(&java, b"java").unwrap();
        if let Some(release) = release {
            fs::write(root.join(folder).join("release"), release).unwrap();
        }
        resolve_path(&java).unwrap()
    }

    fn roots(dir: &Path) -> DiscoveryRoots {
        DiscoveryRoots {
            minecraft_dir: dir.join("mc"),
            app_state_dir: dir.join("state"),
            ..DiscoveryRoots::default()
        }
    }

    #[test]
    fn labels_come_from_release_files_and_runtime_folders() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = roots(dir.path());
        let runtime = dir.path().join("mc").join("runtime").join("java-runtime-delta").join("windows-x64");
        let managed = jdk(&runtime, "java-runtime-delta", Some("JAVA_VERSION=\"21.0.7\"\n"));
        let generation = dir.path().join("mc").join("runtime").join(".generations").join("x");
        jdk(&generation, "java-runtime-delta", Some("JAVA_VERSION=\"21.0.7\"\n"));
        let common = dir.path().join("Program Files").join("Java");
        let temurin =
            jdk(&common, "jdk-21", Some("IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"21.0.3\"\n"));
        let bare = jdk(&dir.path().join("homes"), "corretto-17", None);
        r.common_roots = vec![common];
        r.java_homes =
            vec![dir.path().join("homes").join("corretto-17"), dir.path().join("homes").join("corretto-17")];
        let found = discover(&r);
        let pairs: Vec<(&str, &Path)> = found.iter().map(|f| (f.label.as_str(), f.path.as_path())).collect();
        assert_eq!(
            pairs,
            [
                ("corretto-17", bare.as_path()),
                ("Eclipse Adoptium Java 21.0.3", temurin.as_path()),
                ("Launcher Java 21.0.7 (java-runtime-delta)", managed.as_path()),
            ],
            "sorted by label, no duplicates, hidden folders skipped"
        );
    }

    #[test]
    fn path_folders_and_executable_names_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = roots(dir.path());
        let java = jdk(&dir.path().join("sdk"), "zulu-8", Some("JAVA_VERSION=\"1.8.0_402\"\n"));
        let other = dir.path().join("tools");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("notjava.exe"), b"x").unwrap();
        r.path_dirs = vec![java.parent().unwrap().to_path_buf(), other];
        let found = discover(&r);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].label.as_str(), found[0].path.clone()), ("Java 1.8.0_402", java));
        assert!(is_java_name("MinecraftJava.exe") && is_java_name("JAVAW.EXE") && !is_java_name("javac"));
    }

    #[test]
    fn discovery_stays_within_its_limits() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = roots(dir.path());
        let deep = dir.path().join("deep");
        let mut folder = deep.clone();
        for level in 0..6 {
            folder = folder.join(format!("l{level}"));
        }
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("java"), b"java").unwrap();
        let shallow = deep.join("a").join("bin");
        fs::create_dir_all(&shallow).unwrap();
        fs::write(shallow.join("java"), b"java").unwrap();
        r.common_roots = vec![deep, dir.path().join("missing")];
        let found = discover(&r);
        assert_eq!(found.len(), 1, "six levels down is out of reach: {found:?}");
        assert!(found[0].path.ends_with(Path::new("a").join("bin").join("java")));
    }
}
