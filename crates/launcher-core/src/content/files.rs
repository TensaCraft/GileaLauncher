//! Where a provider puts a file in a build, and its part of a transaction.

use std::fs;
use std::path::Path;

use launcher_shared::{AppError, AppResult, ContentKind, ErrorCode};

use super::packs::rename_listed;
use crate::storage::journal::normalized_path;
use crate::storage::transaction::FileTransaction;

/// File names Windows keeps for devices, whatever the extension.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `<folder>/<filename>` when the name is one plain file name Windows keeps as it is.
pub fn destination(kind: ContentKind, filename: &str) -> AppResult<String> {
    let unsafe_name = || {
        AppError::new(ErrorCode::InvalidInput, format!("unsafe file name {filename:?}"))
            .with_param("name", filename)
    };
    let wanted = format!("{}/{filename}", kind.folder());
    let stem = filename.split('.').next().unwrap_or_default().trim_end().to_ascii_lowercase();
    if filename.contains(['/', '\\']) || RESERVED.contains(&stem.as_str()) {
        return Err(unsafe_name());
    }
    match normalized_path(&wanted) {
        Ok(relative) if relative == wanted => Ok(relative),
        _ => Err(unsafe_name()),
    }
}

/// Stages the game's list of packs (`file`) with the renamed packs under their new names.
pub fn stage_listing(
    tx: &FileTransaction,
    game: &Path,
    file: &str,
    kind: ContentKind,
    legacy: bool,
    renames: &[(String, String)],
) -> AppResult<()> {
    let staged = tx.stage_path(file)?;
    let failed = |e: std::io::Error| AppError::new(ErrorCode::Io, format!("{}: {e}", staged.display()));
    if let Some(parent) = staged.parent() {
        fs::create_dir_all(parent).map_err(failed)?;
    }
    fs::copy(game.join(file), &staged).map_err(failed)?;
    rename_listed(&staged, kind, legacy, renames).map_err(failed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_are_plain_file_names() {
        assert_eq!(destination(ContentKind::Mods, "sodium-0.6.jar").unwrap(), "mods/sodium-0.6.jar");
        assert_eq!(destination(ContentKind::ShaderPacks, "BSL v8.zip").unwrap(), "shaderpacks/BSL v8.zip");
        for bad in [
            "../evil.jar",
            "sub/evil.jar",
            "sub\\evil.jar",
            "evil.jar.",
            "evil.jar ",
            "a:b.jar",
            "NUL.jar",
            "con",
            "Com1.tar.gz",
            ".",
            "..",
        ] {
            let err = destination(ContentKind::Mods, bad).unwrap_err();
            assert_eq!(
                (err.code, err.params.get("name").map(String::as_str)),
                (ErrorCode::InvalidInput, Some(bad)),
                "{bad}"
            );
        }
    }
}
