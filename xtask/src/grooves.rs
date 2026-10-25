//! Beatlayer's bundled grooves (the grooves spec, R36 and R37): this script
//! writes their files into `crates/app/resources/Grooves`, which the packaged
//! app carries, and a test checks that the files in the repository are what it
//! writes. The grooves themselves are `bl_project::groove_file::bundled`'s,
//! which the factory default set takes its own from.

use std::path::{Path, PathBuf};

use bl_project::groove_file::{GROOVE_EXTENSION, bundled};

/// Where the files go, from the repository's root.
pub const DIR: &str = "crates/app/resources/Grooves";

/// The files the script writes: each one's name and text. They name no
/// writer, so they stay the same from one version to the next.
fn files() -> Vec<(String, String)> {
    bundled()
        .into_iter()
        .map(|groove| {
            let text = String::from_utf8(groove.encode_by("")).unwrap_or_default();
            (format!("{}.{GROOVE_EXTENSION}", groove.name), text)
        })
        .collect()
}

fn dir(root: &Path) -> PathBuf {
    root.join(DIR)
}

/// Writes the bundled grooves' files, and takes away any other file in the
/// folder.
pub fn generate(root: &Path) -> Result<(), String> {
    let dir = dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make {DIR}: {e}"))?;
    let files = files();
    for (name, text) in &files {
        std::fs::write(dir.join(name), text)
            .map_err(|e| format!("couldn't write {DIR}/{name}: {e}"))?;
    }
    for name in on_disk(&dir)? {
        if !files.iter().any(|(file, _)| *file == name) {
            std::fs::remove_file(dir.join(&name))
                .map_err(|e| format!("couldn't remove {DIR}/{name}: {e}"))?;
        }
    }
    eprintln!("wrote {} grooves into {DIR}", files.len());
    Ok(())
}

/// Fails if the files in the repository aren't the script's output.
pub fn check(root: &Path) -> Result<(), String> {
    shipped(&dir(root))
}

/// Fails if the files in `dir`, the repository's folder or the one a package
/// carries, aren't the script's output (the grooves spec R37).
pub fn shipped(dir: &Path) -> Result<(), String> {
    let stale = || {
        format!(
            "{} isn't the bundled grooves the script makes (`cargo xtask grooves` writes {DIR})",
            dir.display()
        )
    };
    let mut found = on_disk(dir)?;
    found.sort();
    let files = files();
    let mut expected: Vec<&String> = files.iter().map(|(name, _)| name).collect();
    expected.sort();
    if found.iter().ne(expected) {
        return Err(stale());
    }
    for (name, text) in &files {
        let actual = std::fs::read_to_string(dir.join(name))
            .map_err(|e| format!("couldn't read {}/{name}: {e}", dir.display()))?;
        if actual.replace("\r\n", "\n") != *text {
            return Err(stale());
        }
    }
    Ok(())
}

fn on_disk(dir: &Path) -> Result<Vec<String>, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    entries
        .map(|entry| {
            entry
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .map_err(|e| format!("couldn't read {DIR}: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: grooves R37.
    #[test]
    fn the_shipped_grooves_are_the_scripts_output() {
        check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..")).unwrap();
    }
}
