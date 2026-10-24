//! Plug-in devices in the shell (WP-2.6; the CLAP hosting spec): what the
//! `plugin.*` commands report, the plug-ins' own state taken into the set
//! before it's saved or a device is copied, and the parameter changes
//! plug-ins make themselves turned into undo steps.
//!
//! Plug-ins built on the plug-in kit bring commands of their own: a CLAP
//! plug-in through the kit's agent-commands extension, and a kit plug-in
//! compiled in as a native device (`bl_devices::kit`) directly. Each
//! device's commands join the registry as device commands scoped to it,
//! `device.d<device>.<command>` (such as `device.d17.gain.reset`), so
//! agents list, run, query, and schedule them like any other command.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bl_commands::schemars::Schema;
use bl_commands::{CommandSpec, Context, Permission};
use bl_devices::KitHost;
use bl_model::{Command, Device, DeviceId, DeviceKind, Document, Editor, ParamId, PluginFormat};
use bl_plugin_host::au::AuHost;
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::lv2::Lv2Host;
use bl_plugin_host::scan::{SCANNED_FORMATS, ScanReport, Scanner, system_paths};
use bl_plugin_host::vst3::Vst3Host;
use bl_shell_spec::plugins::{
    BlockedItem, PluginInfo, PluginScan, ScannedItem, SetPlugin, SetPluginList,
};
use serde_json::json;

use crate::devices::DeviceView;

/// The start of every device command's ID: `device.d`, then the device's
/// number.
pub const DEVICE_COMMAND_PREFIX: &str = "device.d";

/// The plug-in commands' area a host serving agents leaves out: the
/// plug-in's own agent server's settings, which are the person's.
const PERSON_ONLY: &str = "agent";

/// The ID of `device`'s command `command`.
#[must_use]
pub fn device_command_id(device: DeviceId, command: &str) -> String {
    format!("{DEVICE_COMMAND_PREFIX}{}.{command}", device.0.get())
}

/// The device and the plug-in's own command a device command's ID names.
#[must_use]
pub fn parse_device_command(id: &str) -> Option<(DeviceId, &str)> {
    let rest = id.strip_prefix(DEVICE_COMMAND_PREFIX)?;
    let (number, command) = rest.split_once('.')?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) || command.is_empty() {
        return None;
    }
    let id = bl_core::id::Id::new(number.parse().ok()?)?;
    Some((DeviceId(id), command))
}

/// A plug-in command as the registry holds it for `device` on the track
/// `track`: scoped to the device, named for it, with every part a command
/// needs (a plug-in that leaves its class out gets the most cautious one).
#[must_use]
pub fn device_command(spec: &CommandSpec, device: &Device, track: &str) -> CommandSpec {
    let title = device.title();
    let mut description = spec.description.trim().to_owned();
    if description.is_empty() {
        description = format!("Runs the plug-in's command `{}`.", spec.id);
    } else if !description.ends_with('.') {
        description.push('.');
    }
    description.push_str(&format!(
        " It acts on the device \u{201c}{title}\u{201d} ({}) on the track \u{201c}{track}\u{201d}: \
         it's that plug-in's own command `{}`, and a change it makes to the set is one undo \
         step.",
        device.id, spec.id
    ));
    let object = || Schema::try_from(json!({ "type": "object" })).unwrap_or_default();
    CommandSpec::new(
        device_command_id(device.id, &spec.id),
        format!("{title}: {}", spec.label.trim()),
    )
    .description(description)
    .permission(spec.permission.unwrap_or(Permission::External))
    .context(Context::DeviceView)
    .args_schema(spec.args.clone().unwrap_or_else(object))
    .result_schema(spec.result.clone().unwrap_or_default())
}

/// Every plug-in device's commands in `doc`, scoped to their devices, as
/// the registry holds them.
#[must_use]
pub fn device_commands(
    doc: &Document,
    plugins: &ClapHost,
    kit: &KitHost,
) -> Vec<(DeviceId, Vec<CommandSpec>)> {
    let mut found = Vec::new();
    for track in doc.tracks() {
        let label = crate::routing::track_label(doc, track.id());
        for device in track.devices_deep() {
            let own = if kit.handles(&device.kind) {
                kit.commands(device.id)
            } else if is_clap(doc, device.id) {
                plugins.commands(device.id)
            } else {
                None
            };
            let Some(own) = own else { continue };
            let scoped = own
                .iter()
                .filter(|spec| spec.area() != PERSON_ONLY)
                .map(|spec| device_command(spec, device, &label))
                .collect();
            found.push((device.id, scoped));
        }
    }
    found
}

/// Whether `device` is a CLAP plug-in device in `doc`.
#[must_use]
pub fn is_clap(doc: &Document, device: DeviceId) -> bool {
    doc.device(device).is_some_and(|d| {
        matches!(
            d.kind,
            DeviceKind::Plugin {
                format: PluginFormat::Clap,
                ..
            }
        )
    })
}

/// Whether `device` is an Audio Unit device in `doc` (WP-2.8).
#[must_use]
pub fn is_au(doc: &Document, device: DeviceId) -> bool {
    doc.device(device).is_some_and(|d| {
        matches!(
            d.kind,
            DeviceKind::Plugin {
                format: PluginFormat::AudioUnit,
                ..
            }
        )
    })
}

/// Whether the set has a plug-in device of `format`.
#[must_use]
pub fn has_format(doc: &Document, format: PluginFormat) -> bool {
    !devices(doc, format).is_empty()
}

/// Whether the set has a CLAP, VST3, Audio Unit or LV2 device: an offline
/// render of it borrows the engine's plug-ins (offline-plugin-renders R1).
#[must_use]
pub fn has_plugin(doc: &Document) -> bool {
    [
        PluginFormat::Clap,
        PluginFormat::Vst3,
        PluginFormat::AudioUnit,
        PluginFormat::Lv2,
    ]
    .into_iter()
    .any(|format| has_format(doc, format))
}

/// Whether `device` is a VST3 plug-in device in `doc`.
#[must_use]
pub fn is_vst3(doc: &Document, device: DeviceId) -> bool {
    doc.device(device).is_some_and(|d| {
        matches!(
            d.kind,
            DeviceKind::Plugin {
                format: PluginFormat::Vst3,
                ..
            }
        )
    })
}

/// Whether `device` is an LV2 plug-in device in `doc` (WP-5.39).
#[must_use]
pub fn is_lv2(doc: &Document, device: DeviceId) -> bool {
    doc.device(device).is_some_and(|d| {
        matches!(
            d.kind,
            DeviceKind::Plugin {
                format: PluginFormat::Lv2,
                ..
            }
        )
    })
}

/// Whether `device` is a plug-in device of a hosted format in `doc`.
#[must_use]
pub fn is_plugin(doc: &Document, device: DeviceId) -> bool {
    is_clap(doc, device) || is_vst3(doc, device) || is_lv2(doc, device)
}

/// Whether `path` names an LV2 bundle: a folder whose name ends in `.lv2`.
#[must_use]
pub fn is_lv2_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lv2"))
}

/// Whether `path` names a VST3 bundle or module: its own name or a folder
/// on it ends in `.vst3`.
#[must_use]
pub fn is_vst3_path(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| name.ends_with(".vst3"))
    })
}

/// Every CLAP device in the set.
fn clap_devices(doc: &Document) -> Vec<DeviceId> {
    devices(doc, PluginFormat::Clap)
}

/// Every Audio Unit device in the set.
fn au_devices(doc: &Document) -> Vec<DeviceId> {
    devices(doc, PluginFormat::AudioUnit)
}

/// Every plug-in device of `format` in the set, in track order.
fn devices(doc: &Document, format: PluginFormat) -> Vec<DeviceId> {
    doc.tracks()
        .flat_map(|track| track.devices_deep().map(|d| d.id).collect::<Vec<_>>())
        .filter(|device| {
            doc.device(*device).is_some_and(|d| {
                matches!(
                    d.kind,
                    DeviceKind::Plugin {
                        format: found,
                        ..
                    } if found == format
                )
            })
        })
        .collect()
}

/// Spec: clap-hosting R8, au-hosting R8, which vst3-hosting's and
/// lv2-hosting's state rules mirror. Takes each running plug-in's state into
/// the set, without an undo step, so saving or copying the device keeps it:
/// CLAP plug-ins', Audio Units', VST3 plug-ins', LV2 plug-ins', and kit
/// plug-ins' compiled in. A device whose plug-in isn't running keeps the
/// state it had.
pub fn capture_states(
    plugins: &ClapHost,
    au: &AuHost,
    vst3: &Vst3Host,
    lv2: &Lv2Host,
    kit: &KitHost,
    editor: &mut Editor,
) {
    for device in clap_devices(editor.document()) {
        if let Ok(state) = plugins.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
    for device in au_devices(editor.document()) {
        if let Ok(state) = au.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
    for device in devices(editor.document(), PluginFormat::Vst3) {
        if let Ok(state) = vst3.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
    for device in devices(editor.document(), PluginFormat::Lv2) {
        if let Ok(state) = lv2.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
    let kits: Vec<DeviceId> = editor
        .document()
        .tracks()
        .flat_map(|track| {
            track
                .devices_deep()
                .filter(|d| kit.handles(&d.kind))
                .map(|d| d.id)
                .collect::<Vec<_>>()
        })
        .collect();
    for device in kits {
        if let Some(state) = kit.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
}

/// Spec: clap-hosting R6. Applies the parameter changes plug-ins made
/// themselves (CLAP plug-ins' and kit plug-ins'), each one an undo step
/// named for its parameter, as a change from a device's control is.
/// Returns whether the set changed.
pub fn apply_param_changes(
    changes: impl IntoIterator<Item = (DeviceId, ParamId, f64)>,
    editor: &mut Editor,
    view: &mut DeviceView,
) -> bool {
    let mut changed = false;
    for (device_id, param, value) in changes {
        let Some(device) = editor.document().device(device_id) else {
            continue;
        };
        if device.params.get(&param) == Some(&value) {
            continue;
        }
        let kind = device.kind.clone();
        let all: Vec<ParamId> = view
            .description(&kind)
            .map(|d| d.params.iter().map(|p| p.id).collect())
            .unwrap_or_default();
        let configuring = view.configuring(device_id);
        let is_configured = device.configured(&all).contains(&param);
        // WP-5.40, manual 24.1.2. In Configure Mode the move and the
        // parameter joining the configured list are one undo step; outside
        // it an unconfigured parameter becomes the device's temporary
        // chooser entry.
        let configured: Option<Vec<ParamId>> =
            (configuring && all.contains(&param) && !is_configured).then(|| {
                let mut list = device.configured(&all).into_owned();
                list.push(param);
                list
            });
        let name = view
            .description(&kind)
            .and_then(|d| d.param(param))
            .map_or_else(|| "parameter".to_owned(), |p| p.name.clone());
        let label = format!("Change \"{name}\"");
        let applied = editor.transact(label, |editor| {
            editor
                .apply(Command::SetDeviceParam {
                    device: device_id,
                    param,
                    value,
                })
                .and_then(|_| match &configured {
                    Some(list) => editor
                        .apply(Command::SetConfiguredParams {
                            device: device_id,
                            params: list.clone(),
                        })
                        .map(|_| ()),
                    None => Ok(()),
                })
        });
        if applied.is_ok() {
            changed = true;
            let now_configured = editor
                .document()
                .device(device_id)
                .is_some_and(|d| d.configured(&all).contains(&param));
            if now_configured || !all.contains(&param) {
                view.clear_pending_param(device_id);
            } else {
                view.note_pending_param(device_id, param);
            }
        }
    }
    changed
}

/// What `plugin.info` reports for `device`, from whichever host has it:
/// the CLAP host's report for a CLAP device, the Audio Unit host's for an
/// Audio Unit, the VST3 host's, or the LV2 host's.
#[must_use]
pub fn info(
    plugins: &ClapHost,
    au: &AuHost,
    vst3: &Vst3Host,
    lv2: &Lv2Host,
    device: DeviceId,
) -> Option<PluginInfo> {
    if let Some(status) = plugins.status(device).or_else(|| au.status(device)) {
        return Some(PluginInfo {
            device,
            plugin: status.plugin_id,
            path: status.path.to_string_lossy().into_owned(),
            active: status.active,
            latency: status.latency,
            failure: status.failure,
            has_window: status.has_editor,
            window_open: status.editor_open,
            window_shown: status.editor_open && !status.editor_hidden,
            window: None,
            values: status.values,
        });
    }
    if let Some(status) = vst3.status(device) {
        return Some(PluginInfo {
            device,
            plugin: status.plugin_id,
            path: status.path.to_string_lossy().into_owned(),
            active: status.active,
            latency: status.latency,
            failure: status.failure,
            has_window: status.has_editor,
            window_open: status.editor_open,
            window_shown: status.editor_open && !status.editor_hidden,
            window: None,
            values: status.values,
        });
    }
    let status = lv2.status(device)?;
    Some(PluginInfo {
        device,
        plugin: status.plugin_id,
        path: status.path.to_string_lossy().into_owned(),
        active: status.active,
        latency: status.latency,
        failure: status.failure,
        has_window: status.has_editor,
        window_open: status.editor_open,
        window_shown: status.editor_open && !status.editor_hidden,
        window: None,
        values: status.values,
    })
}

/// Runs a scan (or a rescan) over every format the scanner describes and
/// reports it for `plugin.scan` / `plugin.rescan` (WP-2.9). With no paths,
/// each format's own system folders are scanned; with paths, all of them
/// are searched for each format's files.
#[must_use]
pub fn scan_report(
    scanner: &Scanner,
    paths: Option<&[PathBuf]>,
    fresh: bool,
    formats: &[PluginFormat],
    hidden: &BTreeSet<String>,
) -> PluginScan {
    let mut report = ScanReport::default();
    for &format in formats {
        let format_paths: Vec<PathBuf> = match paths {
            Some(paths) => paths.to_vec(),
            None => system_paths(format),
        };
        let found = if fresh {
            scanner.rescan(format, &format_paths)
        } else {
            scanner.scan(format, &format_paths)
        };
        report.scanned += found.scanned;
        report.cached += found.cached;
        report.plugins.extend(found.plugins);
        report.blocked.extend(found.blocked);
    }
    report
        .plugins
        .sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    PluginScan {
        plugins: report
            .plugins
            .into_iter()
            .map(|p| ScannedItem {
                hidden: hidden.contains(&p.key()),
                format: p.format,
                path: p.path.to_string_lossy().into_owned(),
                plugin_id: p.plugin_id,
                name: p.name,
                vendor: p.vendor,
                version: p.version,
                role: p.role,
                note_output: p.note_output,
            })
            .collect(),
        blocked: report
            .blocked
            .into_iter()
            .map(|b| BlockedItem {
                format: b.format,
                path: b.path.to_string_lossy().into_owned(),
                reason: b.reason,
            })
            .collect(),
        scanned: report.scanned,
        cached: report.cached,
    }
}

/// The formats the browser lists and a scan of the system folders covers
/// (WP-2.9b; the plug-in scanning spec, R12): CLAP always (Live has no
/// CLAP, so no switch), VST3 with Use VST3 Plug-In System Folders, Audio
/// Units with Use Audio Units v2 on macOS, and LV2 on Linux (Live has no LV2
/// either, so no switch).
#[must_use]
pub fn listed_formats(prefs: &crate::settings::Preferences) -> Vec<PluginFormat> {
    SCANNED_FORMATS
        .iter()
        .copied()
        .filter(|format| match format {
            PluginFormat::Clap => true,
            PluginFormat::Vst3 => prefs.vst3_system_folders,
            PluginFormat::AudioUnit => prefs.use_audio_units_v2,
            PluginFormat::Lv2 => true,
        })
        .collect()
}

/// Every plug-in `doc` uses, grouped by format and ID, for `plugin.list`
/// (IMP-29).
#[must_use]
pub fn set_plugin_list(doc: &Document, scanner: &Scanner) -> SetPluginList {
    let scanned = scanner.plugins();
    let blocked = scanner.blocked();
    let mut order: Vec<(String, SetPlugin)> = Vec::new();
    for track in doc.tracks() {
        for device in track.devices_deep() {
            let DeviceKind::Plugin { format, plugin_id } = &device.kind else {
                continue;
            };
            let key = format!(
                "{}/{}",
                bl_plugin_host::scan::format_name(*format),
                plugin_id
            );
            let path = device
                .plugin
                .as_ref()
                .map(|state| state.path.clone())
                .filter(|p| !p.is_empty());
            let known = scanned
                .iter()
                .find(|p| p.format == *format && p.plugin_id == *plugin_id);
            let blocked_reason = path.as_deref().and_then(|p| {
                blocked
                    .iter()
                    .find(|b| b.path.to_string_lossy() == p)
                    .map(|b| b.reason.clone())
            });
            // Spec: plugin-list R5. The preset the device holds.
            let preset = device.preset_name.clone().filter(|name| !name.is_empty());
            let existing = order.iter_mut().find(|(k, _)| *k == key);
            if let Some((_, plugin)) = existing {
                plugin.devices.push(device.id);
                plugin.tracks.push(track.id());
                if let Some(preset) = preset
                    && !plugin.preset_names.contains(&preset)
                {
                    plugin.preset_names.push(preset);
                }
                continue;
            }
            order.push((
                key,
                SetPlugin {
                    format: *format,
                    plugin_id: plugin_id.clone(),
                    name: device.title().to_owned(),
                    path: path.clone(),
                    vendor: known.and_then(|p| p.vendor.clone()),
                    version: known.and_then(|p| p.version.clone()),
                    devices: vec![device.id],
                    tracks: vec![track.id()],
                    preset_names: preset.into_iter().collect(),
                    missing: known.is_none() || blocked_reason.is_some(),
                    blocked: blocked_reason,
                },
            ));
        }
    }
    let mut plugins: Vec<SetPlugin> = order.into_iter().map(|(_, p)| p).collect();
    plugins.sort_by(|a, b| a.name.cmp(&b.name).then(a.plugin_id.cmp(&b.plugin_id)));
    SetPluginList { plugins }
}

/// Writes the set's plug-in list to `path`, as the plug-in list spec's R7
/// says: the set's name and the date, a header line, and one line per
/// plug-in with its name, format, vendor, version, preset names, tracks,
/// and `Missing` where this computer doesn't have it, separated by tabs.
/// The file is written whole or not at all.
pub fn export_list(
    doc: &Document,
    list: &SetPluginList,
    path: &Path,
    set_name: &str,
) -> Result<PathBuf, String> {
    let mut text = String::new();
    let date = bl_project::utc_stamp(std::time::SystemTime::now());
    text.push_str(&format!("{set_name}\t{}\n", &date[..10]));
    text.push_str("Name\tFormat\tVendor\tVersion\tPresets\tTracks\tMissing\n");
    for plugin in &list.plugins {
        let tracks: Vec<&str> = plugin
            .tracks
            .iter()
            .filter_map(|track| doc.track(*track))
            .map(|track| track.name())
            .collect();
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            plugin.name,
            plugin.format.short_name(),
            plugin.vendor.as_deref().unwrap_or_default(),
            plugin.version.as_deref().unwrap_or_default(),
            plugin.preset_names.join(", "),
            tracks.join(", "),
            if plugin.missing { "Missing" } else { "" },
        ));
    }
    bl_project::preset::write_whole(path, text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path.to_path_buf())
}
