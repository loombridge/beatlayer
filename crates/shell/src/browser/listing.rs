//! Folders as the browser lists them: which files show, what each is, and
//! their order (spec: browser R6).

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

use bl_shell_spec::browser::ItemKind;

/// The extensions of the audio files Beatlayer opens (audio-import R1),
/// lower-case.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "wave", "bwf", "rf64", "bw64", "aif", "aiff", "aifc", "flac", "ogg", "oga", "mp3",
    "m4a", "aac",
];

/// What a file is, by its name: an audio file, a Beatlayer set, a device
/// preset, or none of these
/// (`None`), which the browser leaves out.
#[must_use]
pub fn file_kind(name: &str) -> Option<ItemKind> {
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        Some(ItemKind::Audio)
    } else if extension == bl_project::SET_EXTENSION {
        Some(ItemKind::Set)
    } else if extension == bl_project::preset::PRESET_EXTENSION {
        Some(ItemKind::Preset)
    } else {
        None
    }
}

/// Whether a name is hidden: dot files, which the browser leaves out.
#[must_use]
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// One item in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its path.
    pub path: PathBuf,
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: ItemKind,
}

/// Compares names as the browser orders them: ignoring case, with runs of
/// digits compared by their value, so "Kick 2" comes before "Kick 10".
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();
    loop {
        match (a_chars.peek().copied(), b_chars.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        chars.next();
                    }
                    digits
                };
                let (x, y) = (take(&mut a_chars), take(&mut b_chars));
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a_chars.next();
                b_chars.next();
            }
        }
    }
}

/// Orders items as the content pane lists them: folders first, then files,
/// each by [`natural_cmp`].
#[must_use]
pub fn browser_order(a: &Entry, b: &Entry) -> Ordering {
    let folder = |e: &Entry| e.kind != ItemKind::Folder;
    folder(a)
        .cmp(&folder(b))
        .then_with(|| natural_cmp(&a.name, &b.name))
}

/// The folders and the files Beatlayer opens in `folder`, in the content
/// pane's order. Hidden items and other files are left out; a folder that's
/// a link to another is listed and opens like any folder.
///
/// # Errors
///
/// If the folder can't be read.
pub fn list(folder: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_hidden(&name) {
            continue;
        }
        let path = entry.path();
        let is_dir = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => path.is_dir(),
            Ok(kind) => kind.is_dir(),
            Err(_) => continue,
        };
        let kind = if is_dir {
            Some(ItemKind::Folder)
        } else {
            file_kind(&name)
        };
        if let Some(kind) = kind {
            entries.push(Entry { path, name, kind });
        }
    }
    entries.sort_by(browser_order);
    Ok(entries)
}

/// A file's name without its extension: a clip's or a new track's name.
#[must_use]
pub fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: browser R6.
    #[test]
    fn names_sort_with_numbers_in_order() {
        let mut names = vec!["kick 10", "Kick 2", "kick 1", "Bass", "bass 02", "bass 1"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            ["Bass", "bass 1", "bass 02", "kick 1", "Kick 2", "kick 10"]
        );
        assert_eq!(natural_cmp("a", "A").reverse(), natural_cmp("A", "a"));
        assert_ne!(natural_cmp("a", "A"), Ordering::Equal, "a stable order");
    }

    #[test]
    fn files_are_known_by_their_extension() {
        assert_eq!(file_kind("Kick.WAV"), Some(ItemKind::Audio));
        assert_eq!(file_kind("pad.aif"), Some(ItemKind::Audio));
        assert_eq!(file_kind("Song.blset"), Some(ItemKind::Set));
        assert_eq!(file_kind("Warm.blpreset"), Some(ItemKind::Preset));
        assert_eq!(file_kind("notes.txt"), None);
        assert_eq!(file_kind("README"), None);
        assert!(is_hidden(".DS_Store"));
        assert_eq!(stem(Path::new("/a/Kick 01.wav")), "Kick 01");
    }
}
