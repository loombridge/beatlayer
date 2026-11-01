//! The plug-in scanner (WP-2.9; the plug-in scanning spec): finds the plug-ins
//! installed on this machine, describes each one in a child process so a
//! crashing or hanging plug-in can't take Beatlayer down, puts the ones that
//! fail on a blocklist, and caches what it found by path and modification
//! time so unchanged files aren't scanned again.
//!
//! Scheduling is `Scanner`'s: [`Scanner::scan`] looks at each file, answers
//! from the cache when the file's size and modification time are unchanged,
//! and otherwise runs the `beatlayer-scan` child process with a timeout.
//! [`Scanner::rescan`] ignores the cache. The child process is
//! [`beatlayer-scan`](../bin/beatlayer-scan.rs): it loads the file and prints
//! what it found as JSON, and only it ever loads the plug-in's code.
//!
//! CLAP and VST3 are scanned everywhere, Audio Units on macOS, and LV2 on
//! Linux. An LV2 bundle folder counts as one file holding several plug-ins;
//! its child process reads the bundle's metadata with lilv, which runs no
//! plug-in code, and then instantiates each plug-in once, which does, so a
//! plug-in that crashes or hangs is blocklisted (the plug-in scanning spec,
//! R10).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use bl_engine::DeviceType;
use bl_model::{DeviceKind, DeviceRole, PluginFormat};
use serde::{Deserialize, Serialize};

use crate::clap::ClapType;
use crate::lv2::{self, Lv2Type};
use crate::vst3::Vst3Type;

/// The formats the scanner can describe: CLAP and VST3 everywhere, and
/// Audio Units on macOS (WP-2.8).
#[cfg(target_os = "macos")]
pub const SCANNED_FORMATS: &[PluginFormat] = &[
    PluginFormat::Clap,
    PluginFormat::Vst3,
    PluginFormat::AudioUnit,
];

/// The formats the scanner can describe: CLAP, VST3 and LV2 (WP-5.39).
#[cfg(target_os = "linux")]
pub const SCANNED_FORMATS: &[PluginFormat] =
    &[PluginFormat::Clap, PluginFormat::Vst3, PluginFormat::Lv2];

/// The formats the scanner can describe: CLAP and VST3 (Audio Units are
/// macOS only, and LV2 Linux only).
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub const SCANNED_FORMATS: &[PluginFormat] = &[PluginFormat::Clap, PluginFormat::Vst3];

/// How long a plug-in may take to answer a scan before it's killed and
/// blocklisted.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// What a cached scan records: 1 added the plug-ins' categories.
const CACHE_VERSION: u32 = 1;

/// The child process's name, beside the app's own executable.
pub const SCANNER_BINARY: &str = "beatlayer-scan";

/// The environment variable that points at the scanner binary, for a test or
/// a packaged app that keeps it elsewhere.
pub const SCANNER_PROGRAM_VAR: &str = "BEATLAYER_SCAN";

/// A plug-in the scanner found: enough for the browser and the set's list of
/// plug-ins (IMP-07, IMP-29).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScannedPlugin {
    /// Its format.
    pub format: PluginFormat,
    /// The file or bundle it's in.
    pub path: PathBuf,
    /// Its identifier within its format (its CLAP ID).
    pub plugin_id: String,
    /// Its name.
    pub name: String,
    /// Its vendor, if it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Its version, if it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// What it is in a chain, from its features.
    pub role: DeviceRole,
    /// Whether it sends notes on from a note output (IMP-24): such a
    /// plug-in can sit in the MIDI-effect position of a MIDI track's chain.
    #[serde(default)]
    pub note_output: bool,
    /// The categories its format declares it in: a CLAP plug-in's
    /// features, a VST3 class's subcategories, an Audio Unit's type and
    /// subtype, an LV2 plug-in's classes (spec: browser-tags R49). The
    /// browser's automatic tags come from them ([`crate::categories`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

impl ScannedPlugin {
    /// The key a hidden plug-in is remembered by: its format and its ID, so
    /// hiding it survives a move of its file.
    #[must_use]
    pub fn key(&self) -> String {
        hide_key(self.format, &self.plugin_id)
    }
}

/// The key a hidden plug-in is remembered by.
#[must_use]
pub fn hide_key(format: PluginFormat, plugin_id: &str) -> String {
    format!("{}/{plugin_id}", format_name(format))
}

/// The short name of a format, for a hidden key and for report text.
#[must_use]
pub fn format_name(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
        PluginFormat::AudioUnit => "au",
        PluginFormat::Lv2 => "lv2",
    }
}

/// The file-name extension of a format's files.
#[must_use]
pub fn format_extension(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
        PluginFormat::AudioUnit => "component",
        PluginFormat::Lv2 => "lv2",
    }
}

/// A plug-in that failed a scan, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedPlugin {
    /// Its format.
    pub format: PluginFormat,
    /// The file or bundle it's in.
    pub path: PathBuf,
    /// What went wrong: it crashed, hung, or refused to load.
    pub reason: String,
}

/// What a scan found and what it refused to load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanReport {
    /// Every plug-in the scan found, from the cache or freshly.
    pub plugins: Vec<ScannedPlugin>,
    /// Every plug-in that failed, with why.
    pub blocked: Vec<BlockedPlugin>,
    /// How many files the scanner loaded.
    pub scanned: usize,
    /// How many files the cache answered.
    pub cached: usize,
}

/// The child process's reply, as JSON on its standard output.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChildReply {
    /// Whether it described the file.
    pub ok: bool,
    /// What it found, when `ok`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<ScannedPlugin>,
    /// Why it failed, when not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ChildReply {
    /// What the scanner found in `path`, as `beatlayer-scan` prints it.
    ///
    /// # Errors
    ///
    /// Why it couldn't: an unsupported format, or the plug-in refused to
    /// load.
    pub fn describe(format: PluginFormat, path: &Path) -> Result<Vec<ScannedPlugin>, String> {
        match format {
            PluginFormat::Clap => clap_plugins(path),
            PluginFormat::Vst3 => vst3_plugins(path),
            PluginFormat::AudioUnit => au_plugins(path),
            PluginFormat::Lv2 => lv2_plugins(path),
        }
    }
}

/// Describes the CLAP plug-ins in `path` by loading and probing them, as the
/// scanner child process does.
fn clap_plugins(path: &Path) -> Result<Vec<ScannedPlugin>, String> {
    let host = crate::clap::ClapHost::new();
    let found = host.load(path).map_err(|e| e.to_string())?;
    Ok(found.into_iter().map(ScannedPlugin::from_clap).collect())
}

/// Describes the VST3 plug-ins in `path` by loading and probing them, as
/// the scanner child process does.
fn vst3_plugins(path: &Path) -> Result<Vec<ScannedPlugin>, String> {
    let host = crate::vst3::Vst3Host::new();
    let found = host.load(path).map_err(|e| e.to_string())?;
    Ok(found.into_iter().map(ScannedPlugin::from_vst3).collect())
}

/// Describes the Audio Units a `.component` bundle declares, by loading each
/// one in its own right, as the scanner child process does (WP-2.8's second
/// part). On Windows and Linux this answers that Audio Units are macOS only.
fn au_plugins(path: &Path) -> Result<Vec<ScannedPlugin>, String> {
    crate::au::scan_file(path).map_err(|e| e.to_string())
}

/// The environment variable that names a lilv library for the scanner's
/// child to load instead of the system's, for a test off Linux.
const LILV_LIBRARY_VAR: &str = "BL_TEST_LILV";

/// Describes the LV2 plug-ins in the bundle folder `path`: lilv reads its
/// metadata (no plug-in code runs), then each plug-in is instantiated once,
/// here in the child process, so one that crashes, hangs or won't load is
/// the scan's failure. A plug-in that won't instantiate is left out when its
/// bundle holds others that do.
fn lv2_plugins(path: &Path) -> Result<Vec<ScannedPlugin>, String> {
    let world = match std::env::var_os(LILV_LIBRARY_VAR) {
        Some(library) => lv2::World::with_library(Path::new(&library)),
        None => lv2::World::new(),
    }
    .map_err(|e| e.to_string())?;
    world.load_bundle(path).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    let mut refused = Vec::new();
    for info in lv2::bundle_plugins(&world, path) {
        match world.instantiate(&info.uri, 48_000.0, 512) {
            Ok(_) => found.push(ScannedPlugin::from_lv2(Lv2Type::new(&info, path))),
            Err(e) => refused.push(format!("{}: {e}", info.name)),
        }
    }
    if found.is_empty() {
        return Err(if refused.is_empty() {
            "it offers no plug-ins".to_owned()
        } else {
            refused.join("; ")
        });
    }
    Ok(found)
}

impl ScannedPlugin {
    /// A scanned plug-in from a loaded LV2 type.
    #[must_use]
    pub fn from_lv2(found: Lv2Type) -> Self {
        Self::from_type(
            PluginFormat::Lv2,
            found.path,
            found.device_type,
            found.vendor,
            None,
            found.categories,
        )
    }

    /// A scanned plug-in from a loaded CLAP type.
    #[must_use]
    pub fn from_clap(found: ClapType) -> Self {
        Self::from_type(
            PluginFormat::Clap,
            found.path,
            found.device_type,
            found.vendor,
            found.version,
            found.categories,
        )
    }

    /// A scanned plug-in from a loaded VST3 type.
    #[must_use]
    pub fn from_vst3(found: Vst3Type) -> Self {
        Self::from_type(
            PluginFormat::Vst3,
            found.path,
            found.device_type,
            found.vendor,
            found.version,
            found.categories,
        )
    }

    /// A scanned plug-in from a host's loaded type: its format, file, ID,
    /// name, vendor, version, role and categories.
    fn from_type(
        format: PluginFormat,
        path: PathBuf,
        device_type: DeviceType,
        vendor: Option<String>,
        version: Option<String>,
        categories: Vec<String>,
    ) -> Self {
        let plugin_id = match device_type.kind {
            DeviceKind::Plugin { plugin_id, .. } => plugin_id,
            DeviceKind::Native { device_type } => device_type,
            // A host loads plug-ins; a rack is the document's own.
            DeviceKind::Rack { kind } => kind.name().to_owned(),
        };
        Self {
            format,
            path,
            plugin_id,
            name: device_type.name,
            vendor,
            version,
            role: device_type.role,
            note_output: device_type.note_output,
            categories,
        }
    }
}

/// One file the cache knows.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedFile {
    /// Its size when it was scanned.
    size: u64,
    /// Its modification time since the Unix epoch, in nanoseconds, when it
    /// was scanned.
    modified: u128,
    /// What it held then.
    plugins: Vec<ScannedPlugin>,
    /// The version of what a scan records, [`CACHE_VERSION`] when it was
    /// scanned: an entry of an earlier version is scanned again.
    #[serde(default)]
    version: u32,
}

/// What the scanner keeps between runs: its cache and its blocklist.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    cache: BTreeMap<String, CachedFile>,
    #[serde(default)]
    blocked: BTreeMap<String, BlockedPlugin>,
    /// The size and modification time (see [`fingerprint`]) of each blocklisted
    /// file the plug-in itself is to blame for, so a plain scan skips it until
    /// it changes (the plug-in scanning spec, R4).
    #[serde(default)]
    blocked_at: BTreeMap<String, (u64, u128)>,
}

/// Why a file couldn't be described.
struct Failure {
    reason: String,
    /// Whether the plug-in is to blame, rather than the scanner's own set-up.
    plugin_fault: bool,
}

impl Failure {
    fn plugin(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            plugin_fault: true,
        }
    }

    fn scanner(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            plugin_fault: false,
        }
    }
}

/// The plug-in scanner.
#[derive(Debug)]
pub struct Scanner {
    program: Option<PathBuf>,
    timeout: Duration,
    env: Vec<(String, String)>,
    store_path: Option<PathBuf>,
    /// Where the cache and blocklist live, and the last time the store was
    /// read.
    store: Mutex<StoreFile>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    /// A scanner that runs [`scan_program`] with [`DEFAULT_TIMEOUT`] and
    /// keeps its cache in memory.
    #[must_use]
    pub fn new() -> Self {
        Self {
            program: scan_program(),
            timeout: DEFAULT_TIMEOUT,
            env: Vec::new(),
            store_path: None,
            store: Mutex::new(StoreFile::default()),
        }
    }

    /// Points the scanner at `program`, for a test.
    #[must_use]
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = Some(program.into());
        self
    }

    /// Sets an environment variable on the scanner's child process, for a
    /// test.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Keeps the cache and blocklist in `path`, reading what's there.
    #[must_use]
    pub fn with_store(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(store) = serde_json::from_str(&text)
        {
            self.store = Mutex::new(store);
        }
        self.store_path = Some(path);
        self
    }

    /// How long a plug-in may take to answer.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The scanner binary, if one was found.
    #[must_use]
    pub fn program(&self) -> Option<&Path> {
        self.program.as_deref()
    }

    /// Scans `paths` (files or folders), answering from the cache when a
    /// file's size and modification time are unchanged.
    #[must_use]
    pub fn scan(&self, format: PluginFormat, paths: &[PathBuf]) -> ScanReport {
        self.run(format, paths, false)
    }

    /// Scans `paths` afresh, ignoring the cache and clearing the blocklist
    /// for them, as the Rescan command does.
    #[must_use]
    pub fn rescan(&self, format: PluginFormat, paths: &[PathBuf]) -> ScanReport {
        self.run(format, paths, true)
    }

    fn run(&self, format: PluginFormat, paths: &[PathBuf], fresh: bool) -> ScanReport {
        let mut report = ScanReport::default();
        // The store is locked only to read or record one file at a time, never
        // while a child process runs, so the browser and `plugin.list` read
        // it while a scan runs in the background (WP-2.9b).
        for path in plugin_files(format, paths) {
            let key = path.to_string_lossy().into_owned();
            let (size, modified) = match fingerprint(format, &path) {
                Ok(fingerprint) => fingerprint,
                Err(e) => {
                    let blocked = BlockedPlugin {
                        format,
                        path: path.clone(),
                        reason: format!("couldn't read the file: {e}"),
                    };
                    let mut guard = self.lock();
                    guard.blocked_at.remove(&key);
                    guard.blocked.insert(key, blocked.clone());
                    report.blocked.push(blocked);
                    continue;
                }
            };
            if !fresh {
                let guard = self.lock();
                if let Some(cached) = guard.cache.get(&key)
                    && cached.size == size
                    && cached.modified == modified
                    && cached.version == CACHE_VERSION
                {
                    report.cached += 1;
                    report.plugins.extend(cached.plugins.iter().cloned());
                    continue;
                }
                // A blocklisted file that hasn't changed isn't loaded again:
                // only Rescan tries it.
                if let Some(blocked) = guard.blocked.get(&key)
                    && guard.blocked_at.get(&key) == Some(&(size, modified))
                {
                    report.blocked.push(blocked.clone());
                    continue;
                }
            }
            report.scanned += 1;
            let described = self.describe(format, &path);
            let mut guard = self.lock();
            guard.cache.remove(&key);
            guard.blocked.remove(&key);
            guard.blocked_at.remove(&key);
            match described {
                Ok(plugins) => {
                    guard.cache.insert(
                        key,
                        CachedFile {
                            size,
                            modified,
                            plugins: plugins.clone(),
                            version: CACHE_VERSION,
                        },
                    );
                    report.plugins.extend(plugins);
                }
                Err(failure) => {
                    let blocked = BlockedPlugin {
                        format,
                        path: path.clone(),
                        reason: failure.reason,
                    };
                    if failure.plugin_fault {
                        guard.blocked_at.insert(key.clone(), (size, modified));
                    }
                    guard.blocked.insert(key, blocked.clone());
                    report.blocked.push(blocked);
                }
            }
        }
        report
            .plugins
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        let mut guard = self.lock();
        // A plug-in removed from a folder this scan searched is forgotten,
        // so it leaves the browser.
        guard.cache.retain(|key, _| {
            let file = Path::new(key);
            file.exists() || !paths.iter().any(|root| file.starts_with(root))
        });
        self.persist(&guard);
        report
    }

    /// Describes one file in the child process, with a timeout.
    fn describe(&self, format: PluginFormat, path: &Path) -> Result<Vec<ScannedPlugin>, Failure> {
        let Some(program) = &self.program else {
            return Err(Failure::scanner(format!(
                "the scanner program `{SCANNER_BINARY}` wasn't found beside the app"
            )));
        };
        let mut child = Command::new(program)
            .arg(format_name(format))
            .arg(path)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Failure::scanner(format!("couldn't start the scanner: {e}")))?;
        // Read the reply while the child runs: a reply bigger than the pipe's
        // buffer would otherwise block the child's write and look like a hang.
        // The reader keeps what it has read where this thread can take it,
        // because a helper process the plug-in started can hold the pipe open
        // after the child has exited.
        let mut out = child.stdout.take();
        let output = Arc::new(Mutex::new(Vec::new()));
        let (done, reader_done) = channel();
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 64 * 1024];
            while let Some(out) = &mut out {
                match out.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => sink
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = done.send(());
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Failure::plugin(format!(
                        "it didn't answer within {} seconds",
                        self.timeout.as_secs()
                    )));
                }
                Err(e) => {
                    return Err(Failure::scanner(format!(
                        "couldn't wait for the scanner: {e}"
                    )));
                }
            }
        };
        // The child has exited, so its whole reply is in the pipe. Wait a
        // bounded time for the reader to reach the end, then use what it has.
        let wait = deadline
            .max(Instant::now() + Duration::from_secs(1))
            .saturating_duration_since(Instant::now());
        let _ = reader_done.recv_timeout(wait);
        let bytes = std::mem::take(&mut *output.lock().unwrap_or_else(|e| e.into_inner()));
        // The reply is the last line that reads as one: a plug-in may print
        // its own text to standard output, before or after it. It counts
        // whatever the exit status, which `beatlayer-scan` sets to 1 when it
        // refuses a file, so the reason it gave isn't lost.
        let text = String::from_utf8_lossy(&bytes);
        let reply: Option<ChildReply> = text
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line.trim()).ok());
        match (reply, status.success()) {
            (Some(reply), _) if reply.ok => Ok(reply.plugins),
            (Some(reply), _) => Err(Failure::plugin(
                reply
                    .error
                    .unwrap_or_else(|| "it refused to be scanned".to_owned()),
            )),
            (None, true) => Err(Failure::plugin("it crashed or wrote nothing readable")),
            (None, false) => Err(Failure::plugin(format!(
                "it crashed or exited while being scanned ({status})"
            ))),
        }
    }

    /// Every plug-in the scanner has found, from the cache.
    #[must_use]
    pub fn plugins(&self) -> Vec<ScannedPlugin> {
        let guard = self.lock();
        let mut plugins: Vec<ScannedPlugin> = guard
            .cache
            .values()
            .flat_map(|f| f.plugins.clone())
            .collect();
        plugins.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        plugins
    }

    /// Every plug-in the scanner blocklisted, with why.
    #[must_use]
    pub fn blocked(&self) -> Vec<BlockedPlugin> {
        self.lock().blocked.values().cloned().collect()
    }

    /// Whether `path` is on the blocklist.
    #[must_use]
    pub fn is_blocked(&self, path: &Path) -> bool {
        self.lock()
            .blocked
            .contains_key(&path.to_string_lossy().into_owned())
    }

    /// The plug-ins the browser shows: the ones not hidden, or every one
    /// when `show_hidden`, with the format preference applied when a
    /// plug-in exists in several formats (IMP-07).
    #[must_use]
    pub fn visible(
        &self,
        show_hidden: bool,
        hidden: &BTreeSet<String>,
        preference: Option<PluginFormat>,
    ) -> Vec<ScannedPlugin> {
        let mut plugins: Vec<ScannedPlugin> = self
            .plugins()
            .into_iter()
            .filter(|p| show_hidden || !hidden.contains(&p.key()))
            .collect();
        apply_format_preference(&mut plugins, preference);
        plugins.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        plugins
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoreFile> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn persist(&self, store: &StoreFile) {
        let Some(path) = &self.store_path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(store) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Keeps the preferred format when a plug-in exists in several: a plug-in
/// whose name also exists in `preference`'s format, in another format, is
/// dropped (IMP-07).
fn apply_format_preference(plugins: &mut Vec<ScannedPlugin>, preference: Option<PluginFormat>) {
    let Some(wanted) = preference else {
        return;
    };
    let preferred: BTreeSet<String> = plugins
        .iter()
        .filter(|p| p.format == wanted)
        .map(|p| p.name.to_ascii_lowercase())
        .collect();
    plugins.retain(|p| p.format == wanted || !preferred.contains(&p.name.to_ascii_lowercase()));
}

/// The scanner binary: `BEATLAYER_SCAN` if it names one, otherwise
/// `beatlayer-scan` beside the running executable or one folder up (beside a
/// test binary in `target/debug/deps`).
#[must_use]
pub fn scan_program() -> Option<PathBuf> {
    if let Some(program) = std::env::var_os(SCANNER_PROGRAM_VAR).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(program));
    }
    let name = format!("{SCANNER_BINARY}{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors().skip(1).take(3) {
        let candidate = ancestor.join(&name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The size and modification time (nanoseconds since the Unix epoch) that say
/// whether a plug-in changed: the file's own; for an LV2 bundle folder its
/// manifest; for any other bundle folder, the total size of the files in it
/// and the newest modification time of anything in it. A folder's own size
/// and time change only when an entry is added or removed, so an update that
/// rewrites a file inside it would be missed.
fn fingerprint(format: PluginFormat, path: &Path) -> std::io::Result<(u64, u128)> {
    let manifest = path.join("manifest.ttl");
    let file = if format == PluginFormat::Lv2 && manifest.is_file() {
        manifest
    } else {
        path.to_owned()
    };
    let metadata = std::fs::metadata(&file)?;
    let mut modified = nanos(metadata.modified().ok());
    if !metadata.is_dir() {
        return Ok((metadata.len(), modified));
    }
    let mut size = 0;
    let mut folders = vec![file];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            // Not following links, so a bundle's links can't loop.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            modified = modified.max(nanos(metadata.modified().ok()));
            if metadata.is_dir() {
                folders.push(entry.path());
            } else {
                size += metadata.len();
            }
        }
    }
    Ok((size, modified))
}

/// The folders an `LV2_PATH` value names, `~` standing for `home`.
#[must_use]
pub fn lv2_path_folders(entries: &std::ffi::OsStr, home: Option<&Path>) -> Vec<PathBuf> {
    std::env::split_paths(entries)
        .map(|path| match (path.strip_prefix("~"), home) {
            (Ok(rest), Some(home)) => home.join(rest),
            _ => path,
        })
        .collect()
}

/// The system's plug-in folders for `format`, as the format's conventions
/// put them (the VST3 file format's own locations for VST3), plus the
/// `CLAP_PATH` entries (the CLAP specification requires them).
#[must_use]
pub fn system_paths(format: PluginFormat) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // lilv's rule, which the LV2 host follows (the LV2 hosting spec, R10):
    // `LV2_PATH` names the folders when it is set, and the standard ones
    // are searched when it isn't.
    if format == PluginFormat::Lv2
        && let Some(entries) = std::env::var_os("LV2_PATH").filter(|e| !e.is_empty())
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        return lv2_path_folders(&entries, home.as_deref());
    }
    if format == PluginFormat::Clap
        && let Some(entries) = std::env::var_os("CLAP_PATH")
    {
        paths.extend(std::env::split_paths(&entries));
    }
    if let Some(program_files) = std::env::var_os("CommonProgramFiles") {
        paths.push(PathBuf::from(program_files).join(format_folder(format)));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        match format {
            PluginFormat::Clap => {
                paths.push(home.join("Library/Audio/Plug-Ins/CLAP"));
                paths.push(home.join(".clap"));
            }
            PluginFormat::Vst3 => {
                paths.push(home.join("Library/Audio/Plug-Ins/VST3"));
                paths.push(home.join(".vst3"));
            }
            PluginFormat::AudioUnit => {
                paths.push(home.join("Library/Audio/Plug-Ins/Components"));
            }
            PluginFormat::Lv2 => {
                paths.push(home.join(".lv2"));
                paths.push(home.join("Library/Audio/Plug-Ins/LV2"));
            }
        }
    }
    if format == PluginFormat::AudioUnit {
        // Apple's own units live here, not in the user's or the system's
        // plug-in folders; the scan is expected to find them (WP-2.8).
        paths.push(PathBuf::from("/System/Library/Components"));
    }
    if format == PluginFormat::Lv2 {
        // The standard folders lilv searches on Linux.
        for folder in [
            "/usr/lib/lv2",
            "/usr/lib64/lv2",
            "/usr/local/lib/lv2",
            "/usr/local/lib64/lv2",
        ] {
            paths.push(PathBuf::from(folder));
        }
    }
    if format == PluginFormat::Vst3 {
        // The rest of the VST3 format's plug-in locations (macOS' user and
        // global folders and Windows' global folder are above): macOS'
        // network folder, Windows' user folder and Linux' global folders.
        // Folders this machine doesn't have are skipped when the paths are
        // searched.
        paths.push(PathBuf::from("/Network/Library/Audio/Plug-Ins/VST3"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(local).join("Programs/Common/VST3"));
        }
        paths.push(PathBuf::from("/usr/lib64/vst3"));
        paths.push(PathBuf::from("/usr/lib/vst3"));
        paths.push(PathBuf::from("/usr/local/lib64/vst3"));
        paths.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    paths.push(PathBuf::from("/Library/Audio/Plug-Ins").join(format_folder(format)));
    paths
}

/// The folder a format's plug-ins go in on macOS.
fn format_folder(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Clap => "CLAP",
        PluginFormat::Vst3 => "VST3",
        PluginFormat::AudioUnit => "Components",
        PluginFormat::Lv2 => "LV2",
    }
}

/// Every plug-in file in `paths` (files are taken as they are; folders are
/// searched, and a bundle directory named for the format — `.clap` or
/// `.vst3` — counts as one file). Missing paths are left out.
#[must_use]
pub fn plugin_files(format: PluginFormat, paths: &[PathBuf]) -> Vec<PathBuf> {
    let extension = format_extension(format);
    let mut files = Vec::new();
    for path in paths {
        if path.is_file() {
            if has_extension(path, extension) {
                files.push(path.clone());
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        // A macOS CLAP bundle is a folder that ends in `.clap`.
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        {
            files.push(path.clone());
            continue;
        }
        walk(path, extension, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn walk(folder: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
            {
                out.push(path);
            } else {
                walk(&path, extension, out);
            }
        } else if has_extension(&path, extension) {
            out.push(path);
        }
    }
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

fn nanos(time: Option<SystemTime>) -> u128 {
    time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_key_names_the_format_and_the_id() {
        assert_eq!(
            hide_key(PluginFormat::Clap, "org.example.synth"),
            "clap/org.example.synth"
        );
    }

    #[test]
    fn files_are_taken_by_extension_and_bundles_by_name() {
        let dir = std::env::temp_dir().join(format!("bl-scan-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.clap"), b"").unwrap();
        std::fs::write(dir.join("b.txt"), b"").unwrap();
        let bundle = dir.join("Synth.clap");
        std::fs::create_dir_all(&bundle).unwrap();
        let files = plugin_files(PluginFormat::Clap, std::slice::from_ref(&dir));
        assert_eq!(files, vec![bundle, dir.join("a.clap")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_format_preference_keeps_the_preferred_copy() {
        let plugin = |format, id: &str, name: &str| ScannedPlugin {
            format,
            path: PathBuf::from(format!("/{id}")),
            plugin_id: id.to_owned(),
            name: name.to_owned(),
            vendor: None,
            version: None,
            role: DeviceRole::AudioEffect,
            note_output: false,
            categories: Vec::new(),
        };
        let mut plugins = vec![
            plugin(PluginFormat::Clap, "org.synth.clap", "Synth"),
            plugin(PluginFormat::Vst3, "org.synth.vst3", "Synth"),
            plugin(PluginFormat::Clap, "org.other.clap", "Other"),
        ];
        apply_format_preference(&mut plugins, Some(PluginFormat::Vst3));
        let names: Vec<&str> = plugins.iter().map(|p| p.plugin_id.as_str()).collect();
        assert_eq!(names, ["org.synth.vst3", "org.other.clap"]);
    }
}
