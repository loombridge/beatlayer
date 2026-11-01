//! The Packages manager's data and installer (WP-9.1; the stem separation
//! spec, R15 to R21): the on-device models and runtimes every feature
//! downloads on first use.
//!
//! One manifest, built into Beatlayer as data (`BL_PACKAGES_MANIFEST` names
//! a file to use instead, which is how tests install from `file://` URLs),
//! lists the features, the role each needs, and a package for each role: ID,
//! version, name, project page, purpose, platform, size, SHA-256, URL and the
//! files kept from an archive. Features share a package by role, never by
//! file name, so a GPU build can fill a CPU build's role later.
//!
//! A download goes to a hidden file in `Packages`, is checked against its
//! size and SHA-256, and is unpacked into the package's folder, whose record
//! (`package.json`) is written last: a package is installed only once its
//! record is there and every file it lists is whole.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::stems::{Ended, Progress, Worker};

const BUILT_IN: &str = include_str!("packages/manifest.json");

/// The record of an installed package, in its folder.
const RECORD: &str = "package.json";

/// Marks a package removed while its library was loaded: its files go the
/// next time Beatlayer starts.
const REMOVE_AT_LAUNCH: &str = ".remove";

/// How a download becomes installed files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unpack {
    /// The download is the file.
    File,
    /// A gzipped tar archive.
    TarGz,
    /// A zip archive.
    Zip,
}

/// A file kept from a package's download.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Kept {
    /// What it's installed as.
    pub name: String,
    /// Where it is in an archive, at the end of the entry's path: `name`
    /// if left out.
    #[serde(default)]
    pub path: Option<String>,
}

/// A package of the manifest (R15).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Package {
    /// Its ID. Several entries may share one for different platforms.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Its project's page.
    pub project: String,
    /// What it does, for the page.
    pub purpose: String,
    /// `<os>-<architecture>` (`macos-aarch64`), or `any`.
    pub platform: String,
    /// The role it fills.
    pub role: String,
    /// What's downloaded: `https://…`, or `file://` and a path.
    pub url: String,
    /// The download's size in bytes.
    pub size: u64,
    /// The download's SHA-256, in hex.
    pub sha256: String,
    /// How it's installed.
    pub unpack: Unpack,
    /// Its folder in the packages folder.
    pub folder: String,
    /// The files kept; the first is the main file, the one a feature loads.
    pub files: Vec<Kept>,
    /// Listed but not offered: no feature asks for it and `packages.list`
    /// leaves it out, until whatever it waits on is settled.
    #[serde(default)]
    pub disabled: bool,
}

/// A feature that runs an on-device model, and the roles it needs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Feature {
    /// Its ID.
    pub id: String,
    /// Its name, for the package page.
    pub name: String,
    /// The roles it needs.
    pub roles: Vec<String>,
}

/// The manifest (R15).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Manifest {
    /// The features.
    #[serde(default)]
    pub features: Vec<Feature>,
    /// The packages, for every platform.
    #[serde(default)]
    pub packages: Vec<Package>,
}

/// What this build runs on, as a manifest names it.
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

impl Manifest {
    /// Reads a manifest.
    ///
    /// # Errors
    ///
    /// A message if it isn't one, or a package lists no files or two of
    /// this platform's share an ID.
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if let Some(p) = manifest.packages.iter().find(|p| p.files.is_empty()) {
            return Err(format!("package {} keeps no files", p.id));
        }
        let ours = manifest.for_platform();
        if let Some(p) = ours
            .iter()
            .enumerate()
            .find(|(i, p)| ours[..*i].iter().any(|q| q.id == p.id))
        {
            return Err(format!("two packages for this platform are {}", p.1.id));
        }
        Ok(manifest)
    }

    /// The packages for this platform, in the manifest's order.
    #[must_use]
    pub fn for_platform(&self) -> Vec<&Package> {
        let here = platform();
        self.packages
            .iter()
            .filter(|p| !p.disabled && (p.platform == "any" || p.platform == here))
            .collect()
    }

    /// The package filling `role` here: the first one installed in
    /// `packages`, else the first listed.
    #[must_use]
    pub fn for_role(&self, role: &str, packages: Option<&Path>) -> Option<Package> {
        let mut of_role = self.for_platform().into_iter().filter(|p| p.role == role);
        let first = of_role.next()?;
        let installed = packages.and_then(|dir| {
            std::iter::once(first)
                .chain(of_role)
                .find(|p| p.installed(dir))
        });
        Some(installed.unwrap_or(first).clone())
    }

    /// The packages `feature` needs, one for each of its roles; none if it
    /// isn't in the manifest or a role has no package for this platform.
    #[must_use]
    pub fn feature_packages(&self, feature: &str, packages: Option<&Path>) -> Vec<Package> {
        let Some(feature) = self.features.iter().find(|f| f.id == feature) else {
            return Vec::new();
        };
        feature
            .roles
            .iter()
            .map(|role| self.for_role(role, packages))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
    }

    /// The IDs of the features that need `package`'s role.
    #[must_use]
    pub fn features_needing(&self, package: &Package) -> Vec<String> {
        self.features
            .iter()
            .filter(|f| f.roles.contains(&package.role))
            .map(|f| f.id.clone())
            .collect()
    }
}

/// The manifest in the file `path` names, or the built-in one (R21). A
/// file that can't be read gives an empty manifest, and says why on
/// standard error.
#[must_use]
pub fn load(path: Option<&std::ffi::OsStr>) -> Manifest {
    let Some(path) = path else {
        return Manifest::parse(BUILT_IN).expect("the built-in manifest is valid");
    };
    std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| Manifest::parse(&text))
        .unwrap_or_else(|e| {
            eprintln!("BL_PACKAGES_MANIFEST ({}): {e}", Path::new(path).display());
            Manifest::default()
        })
}

/// The manifest this run uses: `BL_PACKAGES_MANIFEST`'s file, or the
/// built-in one.
#[must_use]
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| load(std::env::var_os("BL_PACKAGES_MANIFEST").as_deref()))
}

/// The folder the packages install into: `Packages` in Beatlayer's
/// application-data folder, or in the system's temporary folder for a
/// headless run (R14, R21).
#[must_use]
pub fn folder(config_dir: Option<&Path>) -> PathBuf {
    config_dir
        .map_or_else(|| std::env::temp_dir().join("Beatlayer"), Path::to_path_buf)
        .join("Packages")
}

/// The folder the stem separation model's compiled forms are cached in
/// (spec R34): `Cache` beside `Packages`.
#[must_use]
pub fn cache_folder(config_dir: Option<&Path>) -> PathBuf {
    folder(config_dir).with_file_name("Cache")
}

/// An installed package's record (R18).
#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    version: String,
    role: String,
    sha256: String,
    files: Vec<RecordedFile>,
}

#[derive(Serialize, Deserialize)]
struct RecordedFile {
    name: String,
    size: u64,
}

impl Package {
    /// Its folder under `packages`.
    #[must_use]
    pub fn dir(&self, packages: &Path) -> PathBuf {
        packages.join(&self.folder)
    }

    /// Where its main file is installed under `packages`.
    #[must_use]
    pub fn path(&self, packages: &Path) -> PathBuf {
        self.dir(packages).join(&self.files[0].name)
    }

    /// Whether it's installed under `packages` (R18): its record is there,
    /// for this download, and every file it lists has the size it was
    /// installed with. A folder with no record, from before the Packages
    /// manager, counts if its main file is there.
    #[must_use]
    pub fn installed(&self, packages: &Path) -> bool {
        let dir = self.dir(packages);
        if dir.join(REMOVE_AT_LAUNCH).exists() {
            return false;
        }
        let whole = |name: &str, size: Option<u64>| {
            std::fs::metadata(dir.join(name))
                .is_ok_and(|m| m.is_file() && m.len() > 0 && size.is_none_or(|s| s == m.len()))
        };
        match std::fs::read(dir.join(RECORD))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        {
            Some(record) => {
                record.sha256 == self.sha256
                    && record.files.iter().any(|f| f.name == self.files[0].name)
                    && record.files.iter().all(|f| whole(&f.name, Some(f.size)))
            }
            None => whole(&self.files[0].name, None),
        }
    }

    /// Whether its removal waits for the next launch.
    #[must_use]
    pub fn removal_pending(&self, packages: &Path) -> bool {
        self.dir(packages).join(REMOVE_AT_LAUNCH).exists()
    }

    /// Removes its files and record (R20); if `loaded` is a library in its
    /// folder, only marks it, and [`sweep`] removes it at the next launch.
    /// Returns whether it's gone now.
    ///
    /// # Errors
    ///
    /// A message if a file can't be removed or the mark can't be written.
    pub fn remove(&self, packages: &Path, loaded: Option<&Path>) -> Result<bool, String> {
        let dir = self.dir(packages);
        if loaded.is_some_and(|library| library.starts_with(&dir)) {
            std::fs::write(dir.join(REMOVE_AT_LAUNCH), b"")
                .map_err(|e| format!("couldn't mark {} for removal: {e}", self.name))?;
            return Ok(false);
        }
        clear(&dir).map_err(|e| format!("couldn't remove {}: {e}", self.name))?;
        Ok(true)
    }
}

/// Removes the files in a package's folder, record first, and the folder.
fn clear(dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let _ = std::fs::remove_file(dir.join(RECORD));
    for entry in entries {
        let path = entry?.path();
        if path.is_file() {
            std::fs::remove_file(path)?;
        }
    }
    std::fs::remove_dir(dir)
}

/// At start: removes the packages marked for removal while their library
/// was loaded.
pub fn sweep(packages: &Path) {
    let Ok(entries) = std::fs::read_dir(packages) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.join(REMOVE_AT_LAUNCH).exists() {
            let _ = clear(&dir);
        }
    }
}

/// Starts downloading and installing `package` into `packages` on a thread
/// of its own (R17).
///
/// # Errors
///
/// A message if the thread can't start.
pub fn install(packages: PathBuf, package: Package) -> Result<Worker<()>, String> {
    Worker::start("bl-shell-packages-install", move |progress| {
        progress.total.store(package.size, Ordering::Relaxed);
        match install_one(&package, &packages, progress) {
            Ok(()) => Ended::Done(()),
            Err(_) if progress.cancelled() => Ended::Cancelled,
            Err(error) => Ended::Failed(error),
        }
    })
}

/// Downloads one package, checks it, installs it and records it. A failure
/// or a cancel leaves nothing of it.
fn install_one(package: &Package, packages: &Path, progress: &Progress) -> Result<(), String> {
    std::fs::create_dir_all(packages)
        .map_err(|e| format!("couldn't make {}: {e}", packages.display()))?;
    progress.set_stage(format!("Downloading {}", package.name));
    let part = packages.join(format!(".{}.download", package.id));
    let dir = package.dir(packages);
    let result = download(package, &part, progress).and_then(|()| {
        progress.set_stage(format!("Installing {}", package.name));
        let failed = |e: &dyn std::fmt::Display| format!("couldn't install {}: {e}", package.name);
        clear(&dir).map_err(|e| failed(&e))?;
        std::fs::create_dir_all(&dir).map_err(|e| failed(&e))?;
        unpack(package, &part, &dir)?;
        let files = package
            .files
            .iter()
            .filter_map(|kept| {
                let size = std::fs::metadata(dir.join(&kept.name)).ok()?.len();
                Some(RecordedFile {
                    name: kept.name.clone(),
                    size,
                })
            })
            .collect();
        let record = Record {
            id: package.id.clone(),
            version: package.version.clone(),
            role: package.role.clone(),
            sha256: package.sha256.clone(),
            files,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(|e| failed(&e))?;
        write_atomically(&dir.join(RECORD), &mut bytes.as_slice()).map_err(|e| failed(&e))
    });
    let _ = std::fs::remove_file(&part);
    if result.is_err() {
        let _ = clear(&dir);
    }
    result
}

/// Downloads `package` to `part`, checking its size and SHA-256.
fn download(package: &Package, part: &Path, progress: &Progress) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| format!("couldn't download {}: {e}", package.name);
    // shortcut: a `file://` URL is the prefix and a plain path, for tests
    // and manifests of local files; a URL-encoded path needs a URL parser.
    let (offered, mut body): (Option<u64>, Box<dyn Read>) =
        if let Some(path) = package.url.strip_prefix("file://") {
            let file = std::fs::File::open(path).map_err(|e| failed(&e))?;
            let len = file.metadata().map_err(|e| failed(&e))?.len();
            (Some(len), Box::new(file))
        } else {
            let response = bl_assistant::provider::agent()
                .get(&package.url)
                .call()
                .map_err(|e| failed(&e))?;
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(failed(&format!("the server answered {status}")));
            }
            let len = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            (len, Box::new(response.into_body().into_reader()))
        };
    if let Some(len) = offered.filter(|&len| len != package.size) {
        return Err(failed(&format!(
            "the server offered {len} bytes, not the {} expected",
            package.size
        )));
    }
    let mut file = std::fs::File::create(part).map_err(|e| failed(&e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut received = 0u64;
    loop {
        if progress.cancelled() {
            return Err("cancelled".to_owned());
        }
        let n = body.read(&mut buffer).map_err(|e| failed(&e))?;
        if n == 0 {
            break;
        }
        if received + n as u64 > package.size {
            return Err(failed(&format!(
                "it was more than the {} bytes expected",
                package.size
            )));
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(|e| failed(&e))?;
        received += n as u64;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| failed(&e))?;
    if received != package.size {
        return Err(failed(&format!(
            "it was {received} bytes, not the {} expected",
            package.size
        )));
    }
    if hex(&hasher.finalize()) != package.sha256 {
        return Err(failed(&"its SHA-256 doesn't match the one expected"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of `bytes`, in hex, for tests that build a manifest.
#[cfg(test)]
pub(crate) fn sha256_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Installs a checked download into `dir`: the file itself, or the files
/// kept from an archive. The record is written after this, so the package
/// shows installed only once it's whole.
fn unpack(package: &Package, part: &Path, dir: &Path) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| format!("couldn't install {}: {e}", package.name);
    let main = &package.files[0].name;
    let mut found = false;
    match package.unpack {
        Unpack::File => {
            std::fs::rename(part, dir.join(main)).map_err(|e| failed(&e))?;
            found = true;
        }
        Unpack::TarGz => {
            let file = std::fs::File::open(part).map_err(|e| failed(&e))?;
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
            for entry in archive.entries().map_err(|e| failed(&e))? {
                let mut entry = entry.map_err(|e| failed(&e))?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry.path().map_err(|e| failed(&e))?.into_owned();
                let Some(name) = wanted(package, &path) else {
                    continue;
                };
                found |= name == main;
                write_atomically(&dir.join(name), &mut entry).map_err(|e| failed(&e))?;
            }
        }
        Unpack::Zip => {
            let file = std::fs::File::open(part).map_err(|e| failed(&e))?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| failed(&e))?;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|e| failed(&e))?;
                if !entry.is_file() {
                    continue;
                }
                let path = PathBuf::from(entry.name().map_err(|e| failed(&e))?.into_owned());
                let Some(name) = wanted(package, &path) else {
                    continue;
                };
                found |= name == main;
                write_atomically(&dir.join(name), &mut entry).map_err(|e| failed(&e))?;
            }
        }
    }
    found
        .then_some(())
        .ok_or_else(|| failed(&format!("the archive holds no {main}")))
}

/// The name an archive's file is installed under, if it's one the package
/// keeps: its entry's path ends with the file's `path`.
fn wanted<'a>(package: &'a Package, path: &Path) -> Option<&'a str> {
    package
        .files
        .iter()
        .find(|kept| path.ends_with(kept.path.as_deref().unwrap_or(&kept.name)))
        .map(|kept| kept.name.as_str())
}

/// Writes `from` to `to` through a temporary file beside it.
fn write_atomically(to: &Path, from: &mut dyn Read) -> std::io::Result<()> {
    let part = to.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    std::io::copy(from, &mut file)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package of `bytes` served from a file, in a folder of its own.
    fn package_of(name: &str, bytes: &[u8]) -> (Package, PathBuf) {
        let dir = std::env::temp_dir().join(format!("bl-packages-{name}-{}", std::process::id()));
        let _ = clear(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source");
        std::fs::write(&source, bytes).unwrap();
        let package = Package {
            id: "test".to_owned(),
            name: "Test".to_owned(),
            version: "1".to_owned(),
            project: String::new(),
            purpose: String::new(),
            platform: "any".to_owned(),
            role: "test-model".to_owned(),
            url: format!("file://{}", source.display()),
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
            unpack: Unpack::File,
            folder: "test-1".to_owned(),
            files: vec![Kept {
                name: "model.bin".to_owned(),
                path: None,
            }],
            disabled: false,
        };
        (package, dir.join("Packages"))
    }

    fn run(package: &Package, packages: &Path) -> Ended<()> {
        let mut worker = install(packages.to_owned(), package.clone()).unwrap();
        loop {
            if let Some(ended) = worker.poll() {
                return ended;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn the_built_in_manifest_gives_stem_separation_its_packages() {
        let manifest = Manifest::parse(BUILT_IN).unwrap();
        if cfg!(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )) {
            let packages = manifest.feature_packages("stems", None);
            let roles: Vec<_> = packages.iter().map(|p| p.role.as_str()).collect();
            assert_eq!(roles, ["onnx-runtime", "stems-model"]);
            assert!(packages.iter().all(|p| p.size > 0 && p.sha256.len() == 64));
            assert!(
                packages
                    .iter()
                    .all(|p| p.project.starts_with("https://github.com/"))
            );
        }
    }

    // Spec: audio-to-midi R18. Audio to MIDI needs the pitch model and the
    // runtime stem separation uses, the same package.
    #[test]
    fn the_built_in_manifest_gives_audio_to_midi_the_pitch_model_and_the_shared_runtime() {
        let manifest = Manifest::parse(BUILT_IN).unwrap();
        let pitch = manifest.for_role("pitch-model", None).unwrap();
        assert_eq!(pitch.id, "basic-pitch");
        assert!(pitch.size > 0 && pitch.sha256.len() == 64);
        if let Some(runtime) = manifest.for_role("onnx-runtime", None) {
            assert_eq!(
                manifest.features_needing(&runtime),
                ["stems", "audio-to-midi"]
            );
            let roles: Vec<_> = manifest
                .feature_packages("audio-to-midi", None)
                .iter()
                .map(|p| p.role.clone())
                .collect();
            assert_eq!(roles, ["onnx-runtime", "pitch-model"]);
        }
    }

    // Spec: stem-separation R21. The file `BL_PACKAGES_MANIFEST` names
    // replaces the built-in manifest.
    #[test]
    fn a_manifest_file_replaces_the_built_in_one() {
        let (package, packages) = package_of("manifest-file", b"x");
        let file = packages.with_file_name("manifest.json");
        let text = format!(
            r#"{{"features":[{{"id":"test","name":"Test","roles":["test-model"]}}],
                "packages":[{{"id":"test","name":"Test","version":"1","project":"","purpose":"",
                "platform":"any","role":"test-model","url":{},"size":1,"sha256":"{}",
                "unpack":"file","folder":"test-1","files":[{{"name":"model.bin"}}]}}]}}"#,
            serde_json::to_string(&package.url).unwrap(),
            package.sha256
        );
        std::fs::write(&file, text).unwrap();
        let manifest = load(Some(file.as_os_str()));
        assert_eq!(manifest.feature_packages("test", None).len(), 1);
        assert!(manifest.feature_packages("stems", None).is_empty());
        assert!(
            !load(None).feature_packages("stems", None).is_empty() || !cfg!(target_os = "macos")
        );
    }

    // Spec: stem-separation R38. The split model is in the manifest, in
    // the role of the whole network, and isn't offered while it's disabled.
    #[test]
    fn a_disabled_package_is_listed_but_never_offered() {
        let manifest = Manifest::parse(BUILT_IN).unwrap();
        let split = manifest
            .packages
            .iter()
            .find(|p| p.id == "htdemucs-split")
            .expect("the manifest lists the split model");
        assert_eq!((split.role.as_str(), split.disabled), ("stems-model", true));
        assert!(manifest.for_platform().iter().all(|p| p.id != split.id));
        if let Some(model) = manifest.for_role("stems-model", None) {
            assert_eq!(model.id, "htdemucs");
        }
        // Enabled, it takes the role on its own platform, ahead of the
        // whole network.
        let enabled = BUILT_IN.replace(r#""disabled": true"#, r#""disabled": false"#);
        let manifest = Manifest::parse(&enabled).unwrap();
        let model = manifest.for_role("stems-model", None).unwrap();
        let expected = if platform() == split.platform {
            "htdemucs-split"
        } else {
            "htdemucs"
        };
        assert_eq!(model.id, expected);
    }

    #[test]
    fn a_manifest_that_keeps_no_files_is_refused() {
        let text = BUILT_IN.replace(
            r#""files": [{ "name": "htdemucs.onnx" }]"#,
            r#""files": []"#,
        );
        assert!(Manifest::parse(&text).is_err());
    }

    // Spec: stem-separation R15. A role is filled by an installed package
    // before a listed one, whatever its file name.
    #[test]
    fn a_role_is_filled_by_the_installed_package_first() {
        let (first, packages) = package_of("role", b"first");
        let second = Package {
            id: "second".to_owned(),
            folder: "second-1".to_owned(),
            ..first.clone()
        };
        let manifest = Manifest {
            features: Vec::new(),
            packages: vec![first.clone(), second.clone()],
        };
        let role = |dir: &Path| manifest.for_role("test-model", Some(dir)).unwrap().id;
        assert_eq!(role(&packages), "test");
        std::fs::create_dir_all(second.dir(&packages)).unwrap();
        std::fs::write(second.path(&packages), b"x").unwrap();
        assert_eq!(role(&packages), "second");
    }

    #[test]
    fn only_the_kept_files_come_from_the_archive() {
        let kept = |name: &str, path: Option<&str>| Kept {
            name: name.to_owned(),
            path: path.map(str::to_owned),
        };
        let (mut package, _) = package_of("wanted", b"x");
        package.files = vec![
            kept(
                "libonnxruntime.dylib",
                Some("lib/libonnxruntime.1.28.3.dylib"),
            ),
            kept("LICENSE", None),
        ];
        let name = |path: &str| wanted(&package, Path::new(path));
        assert_eq!(
            name("x-1/lib/libonnxruntime.1.28.3.dylib"),
            Some("libonnxruntime.dylib")
        );
        assert_eq!(name("x-1/LICENSE"), Some("LICENSE"));
        assert_eq!(name("x-1/include/onnxruntime_c_api.h"), None);
        assert_eq!(
            name(
                "x-1/lib/libonnxruntime.1.28.3.dylib.dSYM/Contents/Resources/DWARF/libonnxruntime.1.28.3.dylib"
            ),
            None
        );
    }

    // Spec: stem-separation R17, R18. An archive installs its kept files and
    // a record; the package shows installed, and not once a file's size
    // changes.
    #[test]
    fn an_archive_installs_its_kept_files_and_a_record() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, body) in [
                ("lib-1/lib/model.so.1", &b"library"[..]),
                ("lib-1/LICENSE", b"licence"),
                ("lib-1/include/api.h", b"header"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, path, body).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar_bytes).unwrap();
        let (mut package, packages) = package_of("archive", &gz.finish().unwrap());
        package.unpack = Unpack::TarGz;
        package.files = vec![
            Kept {
                name: "model.so".to_owned(),
                path: Some("lib/model.so.1".to_owned()),
            },
            Kept {
                name: "LICENSE".to_owned(),
                path: None,
            },
        ];
        assert_eq!(run(&package, &packages), Ended::Done(()));
        let dir = package.dir(&packages);
        assert!(package.installed(&packages));
        assert!(dir.join("LICENSE").is_file() && !dir.join("api.h").exists());
        std::fs::write(dir.join("LICENSE"), b"changed later").unwrap();
        assert!(!package.installed(&packages));
    }

    #[test]
    fn a_failed_install_leaves_nothing_in_packages() {
        let (mut package, packages) = package_of("tampered", b"model");
        package.sha256 = "0".repeat(64);
        let Ended::Failed(error) = run(&package, &packages) else {
            panic!("a wrong checksum installed");
        };
        assert!(error.contains("SHA-256"), "{error}");
        assert_eq!(std::fs::read_dir(&packages).unwrap().count(), 0);
        assert!(!package.installed(&packages));
    }

    // Spec: stem-separation R20. A package whose library is loaded is
    // removed at the next launch; any other, now.
    #[test]
    fn a_loaded_package_is_removed_at_the_next_launch() {
        let (package, packages) = package_of("loaded", b"model");
        assert_eq!(run(&package, &packages), Ended::Done(()));
        let library = package.path(&packages);
        assert_eq!(package.remove(&packages, Some(&library)), Ok(false));
        assert!(package.removal_pending(&packages) && !package.installed(&packages));
        assert!(library.is_file(), "the loaded file stays until the launch");
        sweep(&packages);
        assert!(!package.dir(&packages).exists());

        assert_eq!(run(&package, &packages), Ended::Done(()));
        assert_eq!(package.remove(&packages, None), Ok(true));
        assert!(!package.dir(&packages).exists());
    }

    /// Serves one response of `head` and 1 MiB of zeros, and gives its address.
    fn serve_a_mebibyte(head: &'static str) -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&vec![0u8; 1 << 20]);
        });
        addr
    }

    /// Downloads from `addr` a package of 1024 zero bytes; gives the outcome,
    /// the bytes counted and the bytes left on disk.
    fn download_from(addr: std::net::SocketAddr, name: &str) -> (Result<(), String>, u64, u64) {
        let dir = std::env::temp_dir().join(format!("bl-shell-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join(".download");
        let (mut package, _) = package_of(name, &[0u8; 1024]);
        package.url = format!("http://{addr}/m.onnx");
        let progress = Progress::default();
        let result = download(&package, &part, &progress);
        let counted = progress.done.load(Ordering::Relaxed);
        let left = std::fs::metadata(&part).map_or(0, |m| m.len());
        let _ = std::fs::remove_file(&part);
        (result, counted, left)
    }

    #[test]
    fn an_oversized_download_stops_at_the_expected_size() {
        let addr = serve_a_mebibyte("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
        let (result, counted, left) = download_from(addr, "stems-endless");
        assert!(result.is_err());
        assert!(counted <= 1024 && left <= 1024, "{counted} {left}");
    }

    #[test]
    fn a_download_offered_at_the_wrong_length_is_refused_before_it_is_written() {
        let addr = serve_a_mebibyte(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 1048576\r\n\r\n",
        );
        let (result, counted, left) = download_from(addr, "stems-offered");
        assert!(result.is_err());
        assert_eq!((counted, left), (0, 0));
    }
}
