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

/// Whether the set has a CLAP device.
#[must_use]
pub fn has_clap(doc: &Document) -> bool {
    !clap_devices(doc).is_empty()
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

/// Whether `device` is a plug-in device of a hosted format in `doc`.
#[must_use]
pub fn is_plugin(doc: &Document, device: DeviceId) -> bool {
    is_clap(doc, device) || is_vst3(doc, device)
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

/// Spec: clap-hosting R8, au-hosting R8, which vst3-hosting's state rules
/// mirror. Takes each running plug-in's state into the set, without an undo
/// step, so saving or copying the device keeps it: CLAP plug-ins', Audio
/// Units', VST3 plug-ins', and kit plug-ins' compiled in. A device whose
/// plug-in isn't running keeps the state it had.
pub fn capture_states(
    plugins: &ClapHost,
    au: &AuHost,
    vst3: &Vst3Host,
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
    view: &DeviceView,
) -> bool {
    let mut changed = false;
    for (device_id, param, value) in changes {
        let Some(device) = editor.document().device(device_id) else {
            continue;
        };
        if device.params.get(&param) == Some(&value) {
            continue;
        }
        let name = view
            .description(&device.kind)
            .and_then(|d| d.param(param))
            .map_or_else(|| "parameter".to_owned(), |p| p.name.clone());
        let label = format!("Change \"{name}\"");
        let applied = editor.transact(label, |editor| {
            editor.apply(Command::SetDeviceParam {
                device: device_id,
                param,
                value,
            })
        });
        changed |= applied.is_ok();
    }
    changed
}

/// What `plugin.info` reports for `device`, from whichever host has it:
/// the CLAP host's report for a CLAP device, the Audio Unit host's for an
/// Audio Unit, or the VST3 host's.
#[must_use]
pub fn info(
    plugins: &ClapHost,
    au: &AuHost,
    vst3: &Vst3Host,
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
    let status = vst3.status(device)?;
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
    hidden: &BTreeSet<String>,
) -> PluginScan {
    let mut report = ScanReport::default();
    for &format in SCANNED_FORMATS {
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
            let existing = order.iter_mut().find(|(k, _)| *k == key);
            if let Some((_, plugin)) = existing {
                plugin.devices.push(device.id);
                plugin.tracks.push(track.id());
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
