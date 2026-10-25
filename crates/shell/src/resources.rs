//! The files the packaged app carries beside its program, read-only (the
//! grooves spec R37): where they are, found once.
//!
//! The package puts the contents of `crates/app/resources` in the app's
//! resources folder, `Grooves` among them; a build run from the repository
//! reads that folder itself. Nothing here writes: the bundled files are the
//! app's, and a person's own go in the User Library.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder of the bundled grooves, in the resources folder.
const GROOVES: &str = "Grooves";

/// The resources folder: the one `BL_RESOURCES` names, or the first of the
/// places a package puts it, or the repository's own, that holds the bundled
/// grooves.
#[must_use]
pub fn folder() -> Option<&'static Path> {
    static FOLDER: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOLDER.get_or_init(find).as_deref()
}

/// The bundled grooves' folder, if the app has one.
#[must_use]
pub fn grooves() -> Option<PathBuf> {
    folder().map(|folder| folder.join(GROOVES))
}

fn find() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("BL_RESOURCES")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Ok(exe) = std::env::current_exe()
        && let Some(beside) = exe.parent()
    {
        candidates.extend([
            // A macOS bundle keeps them in Contents/Resources, the program in
            // Contents/MacOS; the others keep them beside the program.
            beside.join("../Resources"),
            beside.join("resources"),
            beside.to_path_buf(),
            // An AppImage and a Debian package keep them in a folder of the
            // program's name under lib.
            beside.join("../lib/Beatlayer"),
            beside.join("../lib/beatlayer"),
        ]);
    }
    // A build run from the repository.
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/resources"));
    candidates
        .into_iter()
        .find(|folder| folder.join(GROOVES).is_dir())
}
