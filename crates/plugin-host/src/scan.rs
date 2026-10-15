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
//! CLAP and VST3 are scanned; Audio Unit and LV2 are the hooks
//! [`PluginFormat`] and [`format_extension`] leave for WP-2.8.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use bl_engine::DeviceType;
use bl_model::{DeviceKind, DeviceRole, PluginFormat};
use serde::{Deserialize, Serialize};

use crate::clap::ClapType;
use crate::vst3::Vst3Type;

/// The formats the scanner can describe: CLAP and VST3 so far. Audio Unit
/// and LV2 wait for their hosting (WP-2.8).
pub const SCANNED_FORMATS: &[PluginFormat] = &[PluginFormat::Clap, PluginFormat::Vst3];

/// How long a plug-in may take to answer a scan before it's killed and
/// blocklisted.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

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
            other => Err(format!(
                "{} plug-ins aren't scanned yet",
                format_name(other)
            )),
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

impl ScannedPlugin {
    /// A scanned plug-in from a loaded CLAP type.
    #[must_use]
    pub fn from_clap(found: ClapType) -> Self {
        Self::from_type(
            PluginFormat::Clap,
            found.path,
            found.device_type,
            found.vendor,
            found.version,
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
        )
    }

    /// A scanned plug-in from a host's loaded type: its format, file, ID,
    /// name, vendor, version and role.
    fn from_type(
        format: PluginFormat,
        path: PathBuf,
        device_type: DeviceType,
        vendor: Option<String>,
        version: Option<String>,
    ) -> Self {
        let plugin_id = match device_type.kind {
            DeviceKind::Plugin { plugin_id, .. } => plugin_id,
            DeviceKind::Native { device_type } => device_type,
        };
        Self {
            format,
            path,
            plugin_id,
            name: device_type.name,
            vendor,
            version,
            role: device_type.role,
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
}

/// What the scanner keeps between runs: its cache and its blocklist.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    cache: BTreeMap<String, CachedFile>,
    #[serde(default)]
    blocked: BTreeMap<String, BlockedPlugin>,
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
        let mut guard = self.lock();
        for path in plugin_files(format, paths) {
            let key = path.to_string_lossy().into_owned();
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(e) => {
                    let blocked = BlockedPlugin {
                        format,
                        path: path.clone(),
                        reason: format!("couldn't read the file: {e}"),
                    };
                    guard.blocked.insert(key, blocked.clone());
                    report.blocked.push(blocked);
                    continue;
                }
            };
            let size = metadata.len();
            let modified = nanos(metadata.modified().ok());
            if !fresh
                && let Some(cached) = guard.cache.get(&key)
                && cached.size == size
                && cached.modified == modified
            {
                report.cached += 1;
                report.plugins.extend(cached.plugins.iter().cloned());
                continue;
            }
            guard.cache.remove(&key);
            guard.blocked.remove(&key);
            report.scanned += 1;
            match self.describe(format, &path) {
                Ok(plugins) => {
                    guard.cache.insert(
                        key,
                        CachedFile {
                            size,
                            modified,
                            plugins: plugins.clone(),
                        },
                    );
                    report.plugins.extend(plugins);
                }
                Err(reason) => {
                    let blocked = BlockedPlugin {
                        format,
                        path: path.clone(),
                        reason,
                    };
                    guard.blocked.insert(key, blocked.clone());
                    report.blocked.push(blocked);
                }
            }
        }
        report
            .plugins
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        self.persist(&guard);
        report
    }

    /// Describes one file in the child process, with a timeout.
    fn describe(&self, format: PluginFormat, path: &Path) -> Result<Vec<ScannedPlugin>, String> {
        let Some(program) = &self.program else {
            return Err(format!(
                "the scanner program `{SCANNER_BINARY}` wasn't found beside the app"
            ));
        };
        let mut child = Command::new(program)
            .arg(format_name(format))
            .arg(path)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start the scanner: {e}"))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => {
                    return Err(format!(
                        "it crashed or exited while being scanned ({status})"
                    ));
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "it didn't answer within {} seconds",
                        self.timeout.as_secs()
                    ));
                }
                Err(e) => return Err(format!("couldn't wait for the scanner: {e}")),
            }
        }
        let mut text = String::new();
        if let Some(mut out) = child.stdout.take() {
            out.read_to_string(&mut text)
                .map_err(|e| format!("couldn't read the scanner's answer: {e}"))?;
        }
        let reply: ChildReply = serde_json::from_str(&text)
            .map_err(|_| "it crashed or wrote nothing readable".to_owned())?;
        if reply.ok {
            Ok(reply.plugins)
        } else {
            Err(reply
                .error
                .unwrap_or_else(|| "it refused to be scanned".to_owned()))
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

/// The system's plug-in folders for `format`, as the format's conventions
/// put them (the VST3 file format's own locations for VST3), plus the
/// `CLAP_PATH` entries (the CLAP specification requires them).
#[must_use]
pub fn system_paths(format: PluginFormat) -> Vec<PathBuf> {
    let mut paths = Vec::new();
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
