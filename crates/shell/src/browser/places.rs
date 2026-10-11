//! The User Library: where it lives and what it starts with (spec: browser
//! R3).

use std::io;
use std::path::{Path, PathBuf};

/// The folders a new User Library starts with, as Live's does.
pub const USER_LIBRARY_FOLDERS: [&str; 5] = ["Clips", "Defaults", "Grooves", "Presets", "Samples"];

/// The User Library's name.
pub const USER_LIBRARY: &str = "User Library";

/// Where the User Library goes on this platform, given the home folder:
/// `Music/Beatlayer/User Library` on macOS and Linux, and
/// `Documents\Beatlayer\User Library` on Windows, where Live keeps its own
/// (Music on macOS, Documents on Windows).
#[must_use]
pub fn user_library_in(home: &Path, windows: bool) -> PathBuf {
    let base = if windows { "Documents" } else { "Music" };
    home.join(base).join("Beatlayer").join(USER_LIBRARY)
}

/// The User Library's default place for the person running Beatlayer, or
/// `None` if their home folder isn't known.
#[must_use]
pub fn default_user_library() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(variable).filter(|value| !value.is_empty())?;
    Some(user_library_in(Path::new(&home), cfg!(windows)))
}

/// Makes the User Library at `path` with its starting folders, if it isn't
/// there yet. What's already there is left alone. Returns whether it made
/// the library.
///
/// # Errors
///
/// If a folder can't be made.
pub fn create_user_library(path: &Path) -> io::Result<bool> {
    if path.is_dir() {
        return Ok(false);
    }
    std::fs::create_dir_all(path)?;
    for folder in USER_LIBRARY_FOLDERS {
        std::fs::create_dir_all(path.join(folder))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: browser R3.
    #[test]
    fn the_user_library_lives_where_live_keeps_it() {
        let home = Path::new("/Users/someone");
        assert_eq!(
            user_library_in(home, false),
            Path::new("/Users/someone/Music/Beatlayer/User Library")
        );
        assert!(user_library_in(home, true).ends_with("Documents/Beatlayer/User Library"));
    }
}
