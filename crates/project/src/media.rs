//! Where the project's media files are.
//!
//! The document names audio by content hash and never by path, so moving a
//! file never changes the song. The set file keeps a media table beside the
//! document: for each hash, the file's path relative to the project folder
//! and its size. When a file isn't at its path, a missing-files search can
//! find it again by size and hash.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use bl_model::{ClipData, ContentHash, Document};
use serde::de::{Error as _, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A media file's path relative to the project folder, such as
/// `Samples/Imported/Kick.wav` or `../Shared/Pad.wav`.
///
/// It's always written with `/` between components, whatever the platform, so
/// a project moves between macOS, Windows, and Linux unchanged. It's
/// normalised: any `..` components come first, followed by at least one name,
/// and there are no empty or `.` components. A name can't contain `/`, `\`,
/// or NUL.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MediaPath(String);

/// Why a string isn't a [`MediaPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaPathError {
    path: String,
}

impl fmt::Display for MediaPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} isn't a relative media path: components are separated by `/`, any `..` come first, and none is empty, `.`, or holds `\\` or NUL",
            self.path
        )
    }
}

impl std::error::Error for MediaPathError {}

impl MediaPath {
    /// Checks and wraps a relative path.
    ///
    /// # Errors
    ///
    /// If the path isn't in the normalised form described on [`MediaPath`].
    pub fn new(path: impl Into<String>) -> Result<Self, MediaPathError> {
        let path = path.into();
        let mut names = 0;
        for component in path.split('/') {
            let valid = if component == ".." {
                names == 0
            } else {
                names += 1;
                !component.is_empty() && component != "." && !component.contains(['\\', '\0'])
            };
            if !valid {
                return Err(MediaPathError { path });
            }
        }
        if names == 0 {
            return Err(MediaPathError { path });
        }
        Ok(Self(path))
    }

    /// The path from `folder` to `file`, worked out from the paths alone,
    /// without looking at the disk.
    ///
    /// `None` if there's no such path: if one is absolute and the other
    /// isn't, they're on different drives, a name isn't valid Unicode, or
    /// `file` isn't below some folder.
    #[must_use]
    pub fn between(folder: &Path, file: &Path) -> Option<Self> {
        let folder: Vec<Component<'_>> = folder
            .components()
            .filter(|c| *c != Component::CurDir)
            .collect();
        let file: Vec<Component<'_>> = file
            .components()
            .filter(|c| *c != Component::CurDir)
            .collect();
        let shared = folder.iter().zip(&file).take_while(|(a, b)| a == b).count();
        let rooted = |c: &Component<'_>| matches!(c, Component::Prefix(_) | Component::RootDir);
        // A different root or drive can't be reached by `..`.
        if folder[shared..].iter().any(rooted) || file[shared..].iter().any(rooted) {
            return None;
        }
        let mut parts = Vec::new();
        for component in &folder[shared..] {
            match component {
                Component::Normal(_) => parts.push(".."),
                // `..` in the folder can't be undone without the disk.
                _ => return None,
            }
        }
        for component in &file[shared..] {
            match component {
                Component::Normal(name) => parts.push(name.to_str()?),
                _ => return None,
            }
        }
        Self::new(parts.join("/")).ok()
    }

    /// The path as written in the set file.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the file is inside the project folder, that is, the path
    /// doesn't start with `..`.
    #[must_use]
    pub fn is_inside(&self) -> bool {
        !self.0.starts_with("../")
    }

    /// The file's path on this platform, below `folder`.
    ///
    /// `None` if a name means something else on this platform, such as a
    /// drive (`C:`) on Windows.
    #[must_use]
    pub fn resolve(&self, folder: &Path) -> Option<PathBuf> {
        let mut path = folder.to_path_buf();
        for name in self.0.split('/') {
            let mut parts = Path::new(name).components();
            match (parts.next(), parts.next()) {
                (Some(Component::ParentDir | Component::Normal(_)), None) => path.push(name),
                _ => return None,
            }
        }
        Some(path)
    }
}

impl fmt::Debug for MediaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaPath({:?})", self.0)
    }
}

impl fmt::Display for MediaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for MediaPath {
    type Error = MediaPathError;

    fn try_from(path: String) -> Result<Self, MediaPathError> {
        Self::new(path)
    }
}

impl From<MediaPath> for String {
    fn from(path: MediaPath) -> Self {
        path.0
    }
}

/// One media file: its content hash, where it was last seen, and its size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaRef {
    /// The hash the document's clips name it by.
    pub hash: ContentHash,
    /// Where it is, relative to the project folder.
    pub path: MediaPath,
    /// Its size in bytes, a quick first test when searching for it by
    /// content.
    pub size: u64,
    /// The file this one is a reversed copy of, when Beatlayer made it (the
    /// audio clip editing spec, R10): reversing the clip again gives it
    /// back that file, writing nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversed_from: Option<ContentHash>,
}

/// The project's media table: at most one [`MediaRef`] per content hash.
///
/// It can name files no clip uses, and a clip can use a hash the table
/// doesn't list (its file is then missing until a search finds it).
/// [`Media::retain_used`] and [`Media::missing`] compare the two.
///
/// It's saved as a list ordered by hash.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Media {
    files: BTreeMap<ContentHash, MediaRef>,
}

impl Media {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file, replacing and returning any entry with the same hash.
    pub fn insert(&mut self, file: MediaRef) -> Option<MediaRef> {
        self.files.insert(file.hash, file)
    }

    /// The entry for a hash.
    #[must_use]
    pub fn get(&self, hash: &ContentHash) -> Option<&MediaRef> {
        self.files.get(hash)
    }

    /// The file a reversed copy was made from, if the table says so (the
    /// audio clip editing spec, R10).
    #[must_use]
    pub fn reversed_from(&self, hash: &ContentHash) -> Option<ContentHash> {
        self.files.get(hash).and_then(|file| file.reversed_from)
    }

    /// Removes and returns the entry for a hash.
    pub fn remove(&mut self, hash: &ContentHash) -> Option<MediaRef> {
        self.files.remove(hash)
    }

    /// Every entry, ordered by hash.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &MediaRef> + DoubleEndedIterator {
        self.files.values()
    }

    /// How many files the table lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Drops every entry that no clip in `document` uses.
    pub fn retain_used(&mut self, document: &Document) {
        let used = used_hashes(document);
        self.files.retain(|hash, _| used.contains(hash));
    }

    /// The hashes that clips in `document` use but the table doesn't list,
    /// ordered by hash.
    #[must_use]
    pub fn missing(&self, document: &Document) -> Vec<ContentHash> {
        used_hashes(document)
            .into_iter()
            .filter(|hash| !self.files.contains_key(hash))
            .collect()
    }
}

fn used_hashes(document: &Document) -> BTreeSet<ContentHash> {
    document
        .tracks()
        .flat_map(|track| track.clips())
        .filter_map(|clip| match &clip.data {
            ClipData::Audio(audio) => Some(audio.media),
            ClipData::Midi(_) => None,
        })
        .collect()
}

impl FromIterator<MediaRef> for Media {
    fn from_iter<I: IntoIterator<Item = MediaRef>>(files: I) -> Self {
        Self {
            files: files.into_iter().map(|file| (file.hash, file)).collect(),
        }
    }
}

impl<'a> IntoIterator for &'a Media {
    type Item = &'a MediaRef;
    type IntoIter = std::collections::btree_map::Values<'a, ContentHash, MediaRef>;

    fn into_iter(self) -> Self::IntoIter {
        self.files.values()
    }
}

impl Serialize for Media {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.files.len()))?;
        for file in self.files.values() {
            seq.serialize_element(file)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Media {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(Files)
    }
}

struct Files;

impl<'de> Visitor<'de> for Files {
    type Value = Media;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list of media files")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Media, A::Error> {
        let mut media = Media::new();
        while let Some(file) = seq.next_element::<MediaRef>()? {
            let hash = file.hash;
            if media.insert(file).is_some() {
                return Err(A::Error::custom(format_args!(
                    "media file {hash} is listed twice"
                )));
            }
        }
        Ok(media)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_paths_are_normalised_and_relative() {
        for good in [
            "Kick.wav",
            "Samples/Imported/Kick.wav",
            "../Kick.wav",
            "../../Library/Pads/Warm pad.aif",
            "Samples/Recorded/1-Audio 0001 [2026-10-10 120000].wav",
            "Ünïcödé/ドラム.wav",
            "C:/x.wav",
            "...",
        ] {
            let path = MediaPath::new(good).unwrap();
            assert_eq!(path.as_str(), good);
            assert_eq!(path.is_inside(), !good.starts_with("../"), "{good}");
        }
        for bad in [
            "",
            "/abs.wav",
            "a//b.wav",
            "a/",
            "./a.wav",
            "a/./b.wav",
            "a/../b.wav",
            "..",
            "../..",
            "a\\b.wav",
            "a\0b.wav",
        ] {
            assert!(MediaPath::new(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn media_paths_serialise_as_strings_and_are_checked_on_load() {
        let path = MediaPath::new("Samples/Imported/Kick.wav").unwrap();
        let json = serde_json::to_string(&path).unwrap();
        assert_eq!(json, r#""Samples/Imported/Kick.wav""#);
        assert_eq!(serde_json::from_str::<MediaPath>(&json).unwrap(), path);
        let error = serde_json::from_str::<MediaPath>(r#""/etc/passwd""#).unwrap_err();
        assert!(error.to_string().contains("isn't a relative media path"));
    }

    #[test]
    fn paths_between_folders() {
        let between = |folder: &str, file: &str| {
            MediaPath::between(Path::new(folder), Path::new(file)).map(String::from)
        };
        assert_eq!(
            between(
                "/music/Song Project",
                "/music/Song Project/Samples/Imported/a.wav"
            )
            .as_deref(),
            Some("Samples/Imported/a.wav")
        );
        assert_eq!(
            between("/music/Song Project", "/music/Library/b.wav").as_deref(),
            Some("../Library/b.wav")
        );
        assert_eq!(
            between("/music/Song Project/", "/c.wav").as_deref(),
            Some("../../c.wav")
        );
        assert_eq!(
            between("/music/./Song Project", "/music/Song Project/./d.wav").as_deref(),
            Some("d.wav")
        );
        // Nothing to name: the folder itself, or one of its parents.
        assert_eq!(between("/music/Song Project", "/music/Song Project"), None);
        assert_eq!(between("/music/Song Project", "/music"), None);
        // One relative and one absolute.
        assert_eq!(between("Song Project", "/music/a.wav"), None);
        assert_eq!(between("/music/x/../Song Project", "/music/a.wav"), None);
    }

    #[test]
    fn resolving_joins_below_the_folder() {
        let folder = Path::new("project");
        let path = MediaPath::new("../Library/b.wav").unwrap();
        let expected: PathBuf = ["project", "..", "Library", "b.wav"].iter().collect();
        assert_eq!(path.resolve(folder), Some(expected));
        let drive = MediaPath::new("C:/x.wav").unwrap();
        if cfg!(windows) {
            assert_eq!(drive.resolve(folder), None);
        } else {
            assert_eq!(drive.resolve(folder), Some(folder.join("C:").join("x.wav")));
        }
    }

    #[test]
    fn media_tables_save_as_ordered_lists_without_duplicates() {
        let file = |byte: u8, path: &str| MediaRef {
            hash: ContentHash([byte; 32]),
            path: MediaPath::new(path).unwrap(),
            size: u64::from(byte) * 1000,
            reversed_from: None,
        };
        let media: Media = [file(9, "b.wav"), file(1, "a.wav")].into_iter().collect();
        let json = serde_json::to_value(&media).unwrap();
        assert_eq!(json[0]["path"], "a.wav");
        assert_eq!(json[1]["size"], 9000);
        assert_eq!(
            serde_json::from_value::<Media>(json.clone()).unwrap(),
            media
        );

        let mut twice = json.clone();
        twice[1]["hash"] = twice[0]["hash"].clone();
        let error = serde_json::from_value::<Media>(twice).unwrap_err();
        assert!(error.to_string().contains("listed twice"), "{error}");

        let mut extra = json;
        extra[0]["volume"] = 1.into();
        assert!(serde_json::from_value::<Media>(extra).is_err());
    }
}
