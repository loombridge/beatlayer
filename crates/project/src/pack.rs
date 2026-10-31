//! Packs: a whole project folder in one file (the set exchange spec, R16 to
//! R23).
//!
//! A pack is a ZIP archive with ZIP64 records. Its first entry is the
//! manifest ([`Manifest`]); the rest are the project folder's folders and
//! files under the folder's own name, each file byte for byte, compressed
//! with Deflate when that makes it smaller. ZIP keeps a CRC-32 for each file,
//! which unpacking checks.
//!
//! [`create`] and [`install`] run on the caller's thread (the shell runs them
//! on a worker): they check `cancel` between chunks, report how far they have
//! got through `progress`, and leave nothing behind when they stop. The
//! reader is fuzzed through [`verify`].

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::folder::ProjectFolder;

/// The file extension of a pack.
pub const EXTENSION: &str = "blpack";

/// The format name the manifest carries.
pub const FORMAT: &str = "beatlayer-pack";

/// The manifest's schema version this build writes and reads.
pub const SCHEMA: u32 = 1;

/// The manifest's name in the archive: the first entry.
const MANIFEST: &str = "beatlayer-pack.json";

/// The most a manifest may unpack to.
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

/// The most entries a pack may hold.
const MAX_ENTRIES: usize = 1_000_000;

/// The size of the chunks files are copied in; `cancel` and `progress` are
/// served between them. The first chunk of a file is also the sample that
/// decides whether Deflate helps.
const CHUNK: usize = 256 << 10;

/// What a pack says about itself, in its first entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always [`FORMAT`].
    pub format: String,
    /// The schema version of this manifest.
    pub schema: u32,
    /// The version of Beatlayer that wrote the pack.
    pub written_by: String,
    /// The project folder's name, which every other entry is under.
    pub project: String,
    /// The sets in the project folder, by file name.
    pub sets: Vec<String>,
}

/// Why packing or unpacking stopped.
#[derive(Debug)]
pub enum Error {
    /// It was cancelled. Nothing was left behind.
    Cancelled,
    /// The pack was refused (or can't be made), and why.
    Refused(String),
    /// A file couldn't be read or written.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Refused(reason) => f.write_str(reason),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Refused(format!("the pack can't be read: {error}"))
    }
}

/// What [`create`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packed {
    /// The pack's path.
    pub path: PathBuf,
    /// How many files it holds.
    pub files: usize,
    /// How many bytes those files hold, before compression.
    pub bytes: u64,
}

/// What [`install`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpacked {
    /// The new project folder.
    pub folder: PathBuf,
    /// The sets in it.
    pub sets: Vec<PathBuf>,
    /// How many files were written.
    pub files: usize,
    /// How many bytes they hold.
    pub bytes: u64,
}

/// One entry of a pack that [`plan`] accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    index: usize,
    path: PathBuf,
    dir: bool,
    size: u64,
}

impl Entry {
    /// Where the entry goes, inside the project folder: only normal parts.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// What a pack holds, once every name in it is checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The pack's manifest.
    pub manifest: Manifest,
    /// Its folders and files, with paths inside the project folder.
    pub entries: Vec<Entry>,
    /// How many bytes its files unpack to, as the pack declares.
    pub bytes: u64,
}

/// A file or folder of the project to pack.
struct Item {
    path: PathBuf,
    /// The size of a file; `None` for a folder.
    size: Option<u64>,
}

/// Packs the project folder `project` into the file `pack`.
///
/// The files go in byte for byte, under the folder's name; hidden files
/// (among them sets' journals and saves under way) and `Backup` are left out
/// (R17). The pack is written to a hidden file beside `pack` and renamed into
/// place, so a pack stopped at any point leaves no pack behind (R19).
///
/// # Errors
///
/// [`Error::Cancelled`] when `cancel` is set; otherwise a file couldn't be
/// read or written, or the folder or one of its files has no usable name.
pub fn create(
    project: &Path,
    pack: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Packed, Error> {
    let name = project
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Refused(format!("{} has no usable name", project.display())))?;
    let sets = ProjectFolder::new(project)
        .sets()?
        .iter()
        .filter_map(|set| set.file_name()?.to_str().map(str::to_owned))
        .collect();
    let mut items = Vec::new();
    collect(project, Path::new(""), &mut items)?;
    let manifest = Manifest {
        format: FORMAT.to_owned(),
        schema: SCHEMA,
        written_by: env!("CARGO_PKG_VERSION").to_owned(),
        project: name.to_owned(),
        sets,
    };
    let total: u64 = items.iter().filter_map(|item| item.size).sum();
    let temp = hidden_beside(pack);
    let written = write_pack(&temp, project, &manifest, &items, total, cancel, progress);
    match written.and_then(|()| fs::rename(&temp, pack).map_err(Error::from)) {
        Ok(()) => Ok(Packed {
            path: pack.to_owned(),
            files: items.iter().filter(|item| item.size.is_some()).count(),
            bytes: total,
        }),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// The folders and files under `root/rel`, folders before what is in them.
fn collect(root: &Path, rel: &Path, items: &mut Vec<Item>) -> io::Result<()> {
    let mut entries = fs::read_dir(root.join(rel))?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let file_name = entry.file_name();
        let hidden = file_name.to_string_lossy().starts_with('.');
        if hidden || (rel.as_os_str().is_empty() && file_name == "Backup") {
            continue;
        }
        let path = rel.join(&file_name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            items.push(Item {
                path: path.clone(),
                size: None,
            });
            collect(root, &path, items)?;
        } else if kind.is_file() {
            items.push(Item {
                path,
                size: Some(entry.metadata()?.len()),
            });
        }
        // shortcut: links and other special files are left out, when a pack
        // should hold the files they lead to.
    }
    Ok(())
}

fn write_pack(
    temp: &Path,
    project: &Path,
    manifest: &Manifest,
    items: &[Item],
    total: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), Error> {
    let mut zip = ZipWriter::new(File::create(temp)?);
    let options = SimpleFileOptions::default().large_file(true);
    zip.start_file(
        MANIFEST,
        options.compression_method(CompressionMethod::Deflated),
    )?;
    zip.write_all(&serde_json::to_vec_pretty(manifest).expect("a manifest is always valid JSON"))?;
    let mut done = 0u64;
    let mut chunk = vec![0u8; CHUNK];
    for item in items {
        let name = entry_name(&manifest.project, &item.path)?;
        if item.size.is_none() {
            zip.add_directory(name, options)?;
            continue;
        }
        let mut file = File::open(project.join(&item.path))?;
        let first = read_up_to(&mut file, &mut chunk)?;
        let method = if deflate_helps(&chunk[..first]) {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        };
        zip.start_file(name, options.compression_method(method))?;
        let mut filled = first;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            zip.write_all(&chunk[..filled])?;
            done += filled as u64;
            progress(done as f64 / total.max(1) as f64);
            filled = read_up_to(&mut file, &mut chunk)?;
            if filled == 0 {
                break;
            }
        }
    }
    zip.finish()?.sync_all()?;
    Ok(())
}

/// Reads until `buffer` is full or the file ends, and returns how much.
fn read_up_to(file: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Whether Deflate makes `sample` at least 5% smaller.
// shortcut: the file's first chunk stands for the whole file, when a pack
// should try each file whole.
fn deflate_helps(sample: &[u8]) -> bool {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    let packed = encoder.write_all(sample).and_then(|()| encoder.finish());
    packed.is_ok_and(|packed| packed.len() < sample.len() / 20 * 19)
}

/// An entry's name in the archive: the project's name and the path inside
/// it, joined with `/`, and a trailing `/` for a folder.
fn entry_name(project: &str, path: &Path) -> Result<String, Error> {
    let mut name = project.to_owned();
    for part in path.components() {
        let part = part
            .as_os_str()
            .to_str()
            .ok_or_else(|| Error::Refused(format!("{} has no usable name", path.display())))?;
        name.push('/');
        name.push_str(part);
    }
    Ok(name)
}

/// A hidden name beside `target` that no other pack, in this process or
/// another, is using.
fn hidden_beside(target: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(".{}-{n}.packing", std::process::id()));
    target.with_file_name(name)
}

/// Reads a pack's manifest and checks every name in it (R21): the first
/// entry is the manifest, from this version of Beatlayer or an earlier one,
/// and every other entry is a folder or a file inside the project folder,
/// with a name that is a file name on every platform.
///
/// # Errors
///
/// [`Error::Refused`] for a pack that fails a check, and why.
pub fn plan<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Plan, Error> {
    if archive.is_empty() || archive.len() > MAX_ENTRIES {
        return Err(Error::Refused(
            "this isn't a Beatlayer pack: it has no manifest or too many entries".to_owned(),
        ));
    }
    let manifest = read_manifest(archive)?;
    if let Err(why) = check_component(&manifest.project) {
        return Err(Error::Refused(format!(
            "the pack's project folder name {:?} {why}",
            manifest.project
        )));
    }
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut folded = std::collections::HashSet::new();
    let mut bytes = 0u64;
    for index in 1..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let name = entry.name()?.into_owned();
        let refuse =
            |why: &str| Error::Refused(format!("the pack has an entry {name:?} that {why}"));
        if entry.is_symlink() {
            return Err(refuse("is a link"));
        }
        let dir = entry.is_dir();
        let path =
            inside(&manifest.project, name.trim_end_matches('/')).map_err(|why| refuse(&why))?;
        if !seen.insert(path.clone()) {
            return Err(refuse("is there twice"));
        }
        // shortcut: lower-casing only, so NFC and NFD spellings still pass; upgrade with a normalisation crate.
        if !folded.insert(path.to_string_lossy().to_lowercase()) {
            return Err(refuse("is there twice, differing only in case"));
        }
        if !dir && path.as_os_str().is_empty() {
            return Err(refuse("is a file named like the project folder"));
        }
        let size = if dir { 0 } else { entry.size() };
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| refuse("makes the pack too big to count"))?;
        entries.push(Entry {
            index,
            path,
            dir,
            size,
        });
    }
    Ok(Plan {
        manifest,
        entries,
        bytes,
    })
}

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Manifest, Error> {
    let entry = archive.by_index(0)?;
    if entry.name()? != MANIFEST {
        return Err(Error::Refused(
            "this isn't a Beatlayer pack: its first entry isn't the manifest".to_owned(),
        ));
    }
    let mut text = Vec::new();
    entry
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut text)
        .map_err(|e| Error::Refused(format!("the pack's manifest can't be read: {e}")))?;
    let manifest: Manifest = serde_json::from_slice(&text)
        .ok()
        .filter(|manifest: &Manifest| {
            manifest.format == FORMAT && text.len() as u64 <= MAX_MANIFEST_BYTES
        })
        .ok_or_else(|| {
            Error::Refused("this isn't a Beatlayer pack: its manifest is wrong".to_owned())
        })?;
    if manifest.schema > SCHEMA {
        return Err(Error::Refused(format!(
            "this pack was made by Beatlayer {}, a later version than this one; update Beatlayer to install it",
            manifest.written_by
        )));
    }
    Ok(manifest)
}

/// The path of an entry named `name` inside the project folder `project`:
/// empty for the folder itself.
fn inside(project: &str, name: &str) -> Result<PathBuf, String> {
    let mut parts = name.split('/');
    if parts.next() != Some(project) {
        return Err(if name.starts_with('/') {
            "has an absolute name".to_owned()
        } else {
            "isn't inside the project folder".to_owned()
        });
    }
    let mut path = PathBuf::new();
    for part in parts {
        check_component(part).map_err(str::to_owned)?;
        path.push(part);
    }
    Ok(path)
}

/// Why `part` can't be a file or folder name on every platform, if it can't.
fn check_component(part: &str) -> Result<(), &'static str> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = part.split('.').next().unwrap_or_default();
    if part.is_empty() || part == "." {
        Err("has an empty part in its name")
    } else if part == ".." {
        Err("climbs out of the project folder")
    } else if part.len() > 255
        || part.ends_with(['.', ' '])
        || part.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*'])
        || part.chars().any(char::is_control)
        || RESERVED.iter().any(|name| stem.eq_ignore_ascii_case(name))
    {
        Err("can't be a file name")
    } else {
        Ok(())
    }
}

/// Unpacks the pack at `pack` into a new project folder inside `parent`.
///
/// The whole pack is checked first ([`plan`]), and so is the space `parent`
/// has. The files are written into a hidden folder beside the destination,
/// which is renamed into place when all of them are there with the checksums
/// and sizes the pack declares, so an unpack stopped or refused at any point
/// leaves nothing behind (R21). The folder is the pack's own name, with " 2",
/// " 3" and so on when `parent` holds one of that name.
///
/// # Errors
///
/// [`Error::Cancelled`] when `cancel` is set; [`Error::Refused`] for a pack
/// that fails a check; otherwise a file couldn't be read or written.
pub fn install(
    pack: &Path,
    parent: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Unpacked, Error> {
    let mut archive = ZipArchive::new(File::open(pack)?)?;
    let plan = plan(&mut archive)?;
    if let Some(free) = free_space(parent)
        && plan.bytes > free
    {
        return Err(Error::Refused(format!(
            "the pack needs {} MB and the folder has {} MB free",
            plan.bytes.div_ceil(1 << 20),
            free >> 20
        )));
    }
    let temp = hidden_beside(&parent.join(&plan.manifest.project));
    fs::create_dir(&temp)?;
    let unpacked = extract(&mut archive, &plan, cancel, progress, |entry| {
        let path = temp.join(&entry.path);
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder)?;
        }
        // The folder starts empty, so an existing file is a name collision.
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(Box::new(io::BufWriter::new(file)))
    })
    .and_then(|()| {
        for entry in plan.entries.iter().filter(|entry| entry.dir) {
            fs::create_dir_all(temp.join(&entry.path))?;
        }
        place(&temp, parent, &plan.manifest.project)
    });
    match unpacked {
        Ok(folder) => Ok(Unpacked {
            sets: ProjectFolder::new(&folder).sets()?,
            files: plan.entries.iter().filter(|entry| !entry.dir).count(),
            bytes: plan.bytes,
            folder,
        }),
        Err(error) => {
            // The folder is ours, hidden, and made a moment ago.
            let _ = fs::remove_dir_all(&temp);
            Err(error)
        }
    }
}

/// Renames the hidden folder `temp` to the first of `name`, `name 2`, `name
/// 3` and so on that nothing in `parent` is called.
fn place(temp: &Path, parent: &Path, name: &str) -> Result<PathBuf, Error> {
    for n in 1u32.. {
        let target = if n == 1 {
            parent.join(name)
        } else {
            parent.join(format!("{name} {n}"))
        };
        // A link counts as taken even if it leads nowhere.
        match fs::symlink_metadata(&target) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::rename(temp, &target)?;
                return Ok(target);
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("a u32 of names is more than a folder holds")
}

/// Checks every file of the pack against the pack's own sizes and checksums
/// without writing any, for the fuzzer and for a pack to be installed later.
/// A pack that declares more than `max_bytes` is refused.
///
/// # Errors
///
/// [`Error::Refused`] for a pack that fails a check, and why.
pub fn verify<R: Read + Seek>(reader: R, max_bytes: u64) -> Result<Plan, Error> {
    let mut archive = ZipArchive::new(reader)?;
    let plan = plan(&mut archive)?;
    if plan.bytes > max_bytes {
        return Err(Error::Refused(
            "the pack holds more than was asked for".to_owned(),
        ));
    }
    extract(
        &mut archive,
        &plan,
        &AtomicBool::new(false),
        &mut |_| {},
        |_| Ok(Box::new(io::sink())),
    )?;
    Ok(plan)
}

/// Copies each file of the plan to the writer `open` gives for it, in the
/// pack's order, and checks that it is as large as the pack says and has
/// the checksum the pack holds.
fn extract<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    plan: &Plan,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
    mut open: impl FnMut(&Entry) -> io::Result<Box<dyn Write>>,
) -> Result<(), Error> {
    let mut done = 0u64;
    let mut chunk = vec![0u8; CHUNK];
    for entry in plan.entries.iter().filter(|entry| !entry.dir) {
        let mut source = archive.by_index(entry.index)?.take(entry.size + 1);
        let mut out = open(entry).map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => Error::Refused(format!(
                "{} is there twice, differing only in case",
                entry.path.display()
            )),
            _ => error.into(),
        })?;
        let mut written = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = source.read(&mut chunk).map_err(|error| {
                // The reader fails with InvalidData for a checksum that
                // doesn't match and for a stream that isn't valid.
                if error.kind() == io::ErrorKind::InvalidData {
                    Error::Refused(format!("{} is damaged: {error}", entry.path.display()))
                } else {
                    Error::Io(error)
                }
            })?;
            if n == 0 {
                break;
            }
            out.write_all(&chunk[..n])?;
            written += n as u64;
            done += n as u64;
            progress(done as f64 / plan.bytes.max(1) as f64);
        }
        if written != entry.size {
            return Err(Error::Refused(format!(
                "{} unpacks to a different size than the pack says",
                entry.path.display()
            )));
        }
        out.flush()?;
    }
    Ok(())
}

/// The bytes free for a new file in the folder `dir`, if the system says.
#[cfg(unix)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    fn wide(n: impl Into<u64>) -> u64 {
        n.into()
    }
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero `statvfs` is a valid value to be filled in.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stat` is a valid out pointer.
    let failed = unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0;
    (!failed).then(|| wide(stat.f_bavail).saturating_mul(wide(stat.f_frsize)))
}

/// The bytes free for a new file in the folder `dir`, if the system says.
#[cfg(windows)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `path` is NUL-terminated and outlives the call, `free` is a
    // valid out pointer, and the other two may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(any(unix, windows)))]
fn free_space(_dir: &Path) -> Option<u64> {
    None
}
