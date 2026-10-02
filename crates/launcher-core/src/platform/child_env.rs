//! What a program the launcher starts must not inherit. An AppImage's runtime points GTK, GIO,
//! GSettings, `PATH` and the data folders into the mounted image, for the launcher's own
//! libraries; a file manager, a browser or Java started with them can fail without a word.

use std::process::Command;

/// The runtime's own variables, about the image itself.
const IMAGE_VARIABLES: [&str; 4] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// The changes a child of an AppImage needs, from the launcher's environment `vars`: a variable to
/// remove (`None`) or to set again without the image's entries. None outside an AppImage.
pub fn appimage_cleanup(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, Option<String>)> {
    let vars: Vec<(String, String)> = vars.into_iter().collect();
    let get = |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let (Some(appdir), Some(_)) = (get("APPDIR").filter(|d| !d.is_empty()), get("APPIMAGE")) else {
        return Vec::new();
    };
    let appdir = appdir.trim_end_matches('/');
    let in_image = |entry: &str| entry == appdir || entry.starts_with(&format!("{appdir}/"));
    let mut changes = Vec::new();
    for (key, value) in &vars {
        if IMAGE_VARIABLES.contains(&key.as_str()) {
            changes.push((key.clone(), None));
        } else if value.split(':').any(in_image) {
            let kept: Vec<&str> = value.split(':').filter(|e| !e.is_empty() && !in_image(e)).collect();
            changes.push((key.clone(), (!kept.is_empty()).then(|| kept.join(":"))));
        }
    }
    changes
}

/// Starts `command` without the AppImage's environment (see `appimage_cleanup`).
pub fn clean(command: &mut Command) {
    let vars = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
    for (key, value) in appimage_cleanup(vars) {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn change(key: &str, value: Option<&str>) -> (String, Option<String>) {
        (key.to_string(), value.map(str::to_string))
    }

    #[test]
    fn outside_an_appimage_a_child_inherits_everything() {
        let env = vars(&[("PATH", "/usr/bin"), ("GTK_PATH", "/usr/lib/gtk-3.0"), ("APPDIR", "/opt/x")]);
        assert!(appimage_cleanup(env).is_empty(), "APPDIR alone is no AppImage");
    }

    #[test]
    fn a_child_of_an_appimage_gets_the_image_s_paths_taken_out() {
        let env = vars(&[
            ("APPIMAGE", "/home/u/Gilea.AppImage"),
            ("APPDIR", "/tmp/.mount_GileaX"),
            ("ARGV0", "./Gilea.AppImage"),
            ("OWD", "/home/u"),
            ("GSETTINGS_SCHEMA_DIR", "/tmp/.mount_GileaX/usr/share/glib-2.0/schemas"),
            ("XDG_DATA_DIRS", "/tmp/.mount_GileaX/usr/share:/usr/local/share:/usr/share"),
            ("PATH", "/tmp/.mount_GileaX/usr/bin:/usr/bin:/bin"),
            ("GTK_EXE_PREFIX", "/tmp/.mount_GileaX/usr"),
            ("HOME", "/home/u"),
            ("GDK_BACKEND", "x11"),
            ("NEIGHBOUR", "/tmp/.mount_GileaXY/usr/share"),
        ]);
        let mut changes = appimage_cleanup(env);
        changes.sort();
        assert_eq!(
            changes,
            [
                change("APPDIR", None),
                change("APPIMAGE", None),
                change("ARGV0", None),
                change("GSETTINGS_SCHEMA_DIR", None),
                change("GTK_EXE_PREFIX", None),
                change("OWD", None),
                change("PATH", Some("/usr/bin:/bin")),
                change("XDG_DATA_DIRS", Some("/usr/local/share:/usr/share")),
            ],
            "the user's own paths stay, and another folder that only starts alike is not the image's"
        );
    }
}
