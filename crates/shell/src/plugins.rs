//! Plug-in devices in the shell (WP-2.6; the CLAP hosting spec): what the
//! `plugin.*` commands report, the plug-ins' own state taken into the set
//! before it's saved or a device is copied, and the parameter changes
//! plug-ins make themselves turned into undo steps.

use bl_model::{Command, DeviceId, DeviceKind, Document, Editor, PluginFormat};
use bl_plugin_host::clap::ClapHost;
use bl_shell_spec::plugins::PluginInfo;

use crate::devices::DeviceView;

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
/// without an undo step, so saving or copying the device keeps it. A
/// device whose plug-in isn't running keeps the state it had.
pub fn capture_states(plugins: &ClapHost, editor: &mut Editor) {
    for device in clap_devices(editor.document()) {
        if let Ok(state) = plugins.save_state(device) {
            let _ = editor.amend_plugin_state(device, Some(state));
        }
    }
}

/// Spec: clap-hosting R6. Applies the parameter changes plug-ins made
/// themselves, each one an undo step named for its parameter, as a change
/// from a device's control is. Returns whether the set changed.
pub fn apply_param_changes(plugins: &ClapHost, editor: &mut Editor, view: &DeviceView) -> bool {
    let mut changed = false;
    for change in plugins.take_param_changes() {
        let Some(device) = editor.document().device(change.device) else {
            continue;
        };
        if device.params.get(&change.param) == Some(&change.value) {
            continue;
        }
        let name = view
            .description(&device.kind)
            .and_then(|d| d.param(change.param))
            .map_or_else(|| "parameter".to_owned(), |p| p.name.clone());
        let label = format!("Change \"{name}\"");
        let applied = editor.transact(label, |editor| {
            editor.apply(Command::SetDeviceParam {
                device: change.device,
                param: change.param,
                value: change.value,
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
