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

use bl_commands::schemars::Schema;
use bl_commands::{CommandSpec, Context, Permission};
use bl_devices::KitHost;
use bl_model::{Command, Device, DeviceId, DeviceKind, Document, Editor, ParamId, PluginFormat};
use bl_plugin_host::clap::ClapHost;
use bl_shell_spec::plugins::PluginInfo;
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
        for device in track.devices() {
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

/// Every CLAP device in the set.
fn clap_devices(doc: &Document) -> Vec<DeviceId> {
    doc.tracks()
        .flat_map(|track| track.devices().map(|d| d.id).collect::<Vec<_>>())
        .filter(|device| is_clap(doc, *device))
        .collect()
}

/// Spec: clap-hosting R8. Takes each running plug-in's state into the set,
/// without an undo step, so saving or copying the device keeps it: CLAP
/// plug-ins' and kit plug-ins' compiled in. A device whose plug-in isn't
/// running keeps the state it had.
pub fn capture_states(plugins: &ClapHost, kit: &KitHost, editor: &mut Editor) {
    for device in clap_devices(editor.document()) {
        if let Ok(state) = plugins.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
    let kits: Vec<DeviceId> = editor
        .document()
        .tracks()
        .flat_map(|track| {
            track
                .devices()
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

/// What `plugin.info` reports for `device`.
#[must_use]
pub fn info(plugins: &ClapHost, device: DeviceId) -> Option<PluginInfo> {
    let status = plugins.status(device)?;
    Some(PluginInfo {
        device,
        plugin: status.plugin_id,
        path: status.path.to_string_lossy().into_owned(),
        active: status.active,
        latency: status.latency,
        failure: status.failure,
        has_window: status.has_editor,
        window_open: status.editor_open,
        values: status.values,
    })
}
