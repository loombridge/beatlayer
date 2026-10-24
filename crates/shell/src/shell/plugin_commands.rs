//! The plug-in commands and loading a plug-in from the Browser.

use super::{Effect, MpeDialogTarget, Shell, ShellError, to_value};
use crate::devices;
use crate::devices::DeviceView;
use bl_model::AutomationTarget;
use bl_model::TrackId;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::plugins as plugin_ids;
use serde_json::Value;
use serde_json::json;

impl Shell {
    /// Runs a plug-in command (WP-2.6).
    pub(super) fn run_plugins(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let target = |shell: &Self, device: Option<bl_model::DeviceId>| {
            let device = device
                .or(shell.devices.selected())
                .ok_or_else(|| unavailable("select a plug-in device first".to_owned()))?;
            if crate::plugins::is_plugin(shell.editor.document(), device)
                || crate::plugins::is_au(shell.editor.document(), device)
            {
                Ok(device)
            } else {
                Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("{device} isn't a plug-in device"),
                })
            }
        };
        let info = |shell: &Self, device| {
            crate::plugins::info(&shell.plugins, &shell.au, &shell.vst3, device)
                .map(|mut info| {
                    // WP-2.10: where the window sits, from the frame store
                    // the set keeps.
                    info.window = shell.devices.window_frame(device);
                    // Spec: mpe R14. Its MPE Mode and settings are the
                    // device's.
                    info.mpe = shell
                        .editor
                        .document()
                        .device(device)
                        .map(|d| d.mpe)
                        .unwrap_or_default();
                    to_value(&info)
                })
                .ok_or_else(|| unavailable(format!("{device}'s plug-in hasn't started yet")))
        };
        match id {
            plugin_ids::LOAD => {
                let plugin_ids::LoadPluginArgs {
                    path,
                    plugin,
                    track,
                    index,
                } = Self::args(id, args)?;
                let path = std::path::PathBuf::from(path);
                let failed = |e: String| ShellError::Failed {
                    command: id.to_owned(),
                    message: e,
                };
                let found: Vec<(bl_engine::DeviceType, std::path::PathBuf)> =
                    if crate::plugins::is_vst3_path(&path) {
                        self.vst3
                            .load(&path)
                            .map_err(|e| failed(e.to_string()))?
                            .into_iter()
                            .map(|t| (t.device_type, t.path))
                            .collect()
                    } else {
                        self.plugins
                            .load(&path)
                            .map_err(|e| failed(e.to_string()))?
                            .into_iter()
                            .map(|t| (t.device_type, t.path))
                            .collect()
                    };
                let (device_type, loaded_path) = match &plugin {
                    Some(wanted) => found.iter().find(|(t, _)| {
                        matches!(&t.kind,
                            bl_model::DeviceKind::Plugin { plugin_id, .. } if plugin_id == wanted)
                    }),
                    None => found.first(),
                }
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!(
                        "{} has no plug-in {}",
                        path.display(),
                        plugin.unwrap_or_default()
                    ),
                })?
                .clone();
                self.refresh_device_types();
                let added: device_ids::DeviceAdded = serde_json::from_value(self.run_devices(
                    device_ids::ADD,
                    to_value(&device_ids::AddDeviceArgs {
                        device: device_ids::NewDevice {
                            kind: device_type.kind.clone(),
                            name: device_type.name.clone(),
                            role: device_type.role,
                            sidechain: false,
                            midi_sidechain: false,
                        },
                        track,
                        chain: None,
                        index,
                    }),
                )?)
                .map_err(|e| ShellError::Failed {
                    command: id.to_owned(),
                    message: e.to_string(),
                })?;
                // Spec: clap-hosting R8. The set knows where the plug-in
                // came from from the start.
                let _ = self.editor.amend_plugin_state(
                    added.device,
                    Some(bl_model::PluginState {
                        path: loaded_path.to_string_lossy().into_owned(),
                        data: Vec::new(),
                    }),
                );
                let plugin_id = match &device_type.kind {
                    bl_model::DeviceKind::Plugin { plugin_id, .. } => plugin_id.clone(),
                    bl_model::DeviceKind::Native { device_type } => device_type.clone(),
                    bl_model::DeviceKind::Rack { kind } => kind.name().to_owned(),
                };
                Ok(to_value(&plugin_ids::PluginLoaded {
                    device: added.device,
                    track: added.track,
                    new_track: added.new_track,
                    plugin: plugin_id,
                    name: device_type.name,
                }))
            }
            plugin_ids::LOAD_AU => {
                let plugin_ids::LoadAudioUnitArgs {
                    component_type,
                    subtype,
                    manufacturer,
                    track,
                    index,
                } = Self::args(id, args)?;
                let found = self
                    .au
                    .load(&component_type, &subtype, &manufacturer)
                    .map_err(|error| match error {
                        bl_plugin_host::au::AuError::Unavailable => unavailable(error.to_string()),
                        error => ShellError::Failed {
                            command: id.to_owned(),
                            message: error.to_string(),
                        },
                    })?;
                self.refresh_device_types();
                let device_type = found.device_type;
                let preset = found.preset;
                let added: device_ids::DeviceAdded = serde_json::from_value(self.run_devices(
                    device_ids::ADD,
                    to_value(&device_ids::AddDeviceArgs {
                        device: device_ids::NewDevice {
                            kind: device_type.kind.clone(),
                            name: device_type.name.clone(),
                            role: device_type.role,
                            sidechain: false,
                            midi_sidechain: false,
                        },
                        track,
                        chain: None,
                        index,
                    }),
                )?)
                .map_err(|e| ShellError::Failed {
                    command: id.to_owned(),
                    message: e.to_string(),
                })?;
                // Spec: au-hosting R8. An Audio Unit has no file: the set
                // keeps its own state from the start.
                let _ = self.editor.amend_plugin_state(
                    added.device,
                    Some(bl_model::PluginState {
                        path: String::new(),
                        data: Vec::new(),
                    }),
                );
                // Spec: plugin-list R5. The name of the preset the unit
                // holds, as it reports it.
                let _ = self.editor.amend_device_preset_name(added.device, preset);
                let plugin_id = match &device_type.kind {
                    bl_model::DeviceKind::Plugin { plugin_id, .. } => plugin_id.clone(),
                    bl_model::DeviceKind::Native { device_type } => device_type.clone(),
                    bl_model::DeviceKind::Rack { kind } => kind.name().to_owned(),
                };
                Ok(to_value(&plugin_ids::PluginLoaded {
                    device: added.device,
                    track: added.track,
                    new_track: added.new_track,
                    plugin: plugin_id,
                    name: device_type.name,
                }))
            }
            plugin_ids::SHOW_WINDOW => {
                let plugin_ids::ShowWindowArgs { device, shown } = Self::args(id, args)?;
                let device = target(self, device)?;
                if crate::plugins::is_vst3(self.editor.document(), device) {
                    return Err(unavailable(
                        "a VST3 plug-in's editor is a later part of VST3 hosting".to_owned(),
                    ));
                }
                let is_au = crate::plugins::is_au(self.editor.document(), device);
                self.plugins.idle();
                self.au.idle();
                let open = crate::plugins::info(&self.plugins, &self.au, &self.vst3, device)
                    .is_some_and(|info| info.window_open);
                if shown.unwrap_or(!open) {
                    // Spec: clap-hosting R13 and au-hosting's second part.
                    // Titled "<device>/<track>", as Live titles a plug-in's
                    // window.
                    let doc = self.editor.document();
                    let title = doc
                        .device_track(device)
                        .map(|t| crate::routing::track_label(doc, t))
                        .unwrap_or_default();
                    let name = doc
                        .device(device)
                        .map(|d| d.title().to_owned())
                        .unwrap_or_default();
                    let title = format!("{name}/{title}");
                    if is_au {
                        self.au
                            .open_editor(device, &title)
                            .map_err(|e| ShellError::Failed {
                                command: id.to_owned(),
                                message: e.to_string(),
                            })?;
                    } else {
                        self.plugins.open_editor(device, &title).map_err(|e| {
                            ShellError::Failed {
                                command: id.to_owned(),
                                message: e.to_string(),
                            }
                        })?;
                    }
                    // Its window follows View ▸ Plug-In Windows and
                    // Auto-Hide Plug-In Windows (R31, R33).
                    self.apply_plug_in_windows();
                } else if is_au {
                    self.au
                        .close_editor(device)
                        .map_err(|e| ShellError::Failed {
                            command: id.to_owned(),
                            message: e.to_string(),
                        })?;
                } else {
                    self.plugins
                        .close_editor(device)
                        .map_err(|e| ShellError::Failed {
                            command: id.to_owned(),
                            message: e.to_string(),
                        })?;
                }
                self.touch();
                info(self, device)
            }
            plugin_ids::SET_MPE | plugin_ids::SET_MPE_SETTINGS => {
                // Spec: mpe R14, R18.
                let (device, change): (_, Box<dyn Fn(bl_model::PluginMpe) -> bl_model::PluginMpe>) =
                    if id == plugin_ids::SET_MPE {
                        let plugin_ids::SetMpeArgs { device, on } = Self::args(id, args)?;
                        (
                            device,
                            Box::new(move |mpe| bl_model::PluginMpe {
                                on: on.unwrap_or(!mpe.on),
                                ..mpe
                            }),
                        )
                    } else {
                        let plugin_ids::SetMpeSettingsArgs { device, settings } =
                            Self::args(id, args)?;
                        (
                            device,
                            Box::new(move |mpe| bl_model::PluginMpe { settings, ..mpe }),
                        )
                    };
                let device = target(self, device)?;
                let current = self
                    .editor
                    .document()
                    .device(device)
                    .map(|d| d.mpe)
                    .ok_or_else(|| unavailable(format!("no {device}")))?;
                let mpe = change(current);
                let label = if id == plugin_ids::SET_MPE {
                    "Change MPE Mode"
                } else {
                    "Change MPE Settings"
                };
                self.editor
                    .transact(label, |e| {
                        e.apply(bl_model::Command::SetPluginMpe { device, mpe })
                    })
                    .map_err(|error| ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                Ok(to_value(&plugin_ids::PluginMpeState { device, mpe }))
            }
            plugin_ids::SHOW_MPE_SETTINGS => {
                let plugin_ids::ShowMpeSettingsArgs {
                    device,
                    track,
                    shown,
                } = Self::args(id, args)?;
                let wanted = match (device, track) {
                    (Some(_), Some(_)) => {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: "give a device or a track, not both".to_owned(),
                        });
                    }
                    (None, Some(track)) => Some(MpeDialogTarget::Track(track)),
                    (device, None) => device
                        .or(self.devices.selected())
                        .map(MpeDialogTarget::Device),
                };
                if shown == Some(false) {
                    self.mpe_dialog = None;
                } else {
                    let target = wanted
                        .ok_or_else(|| unavailable("select a plug-in device first".to_owned()))?;
                    let previous = self.mpe_dialog.replace(target);
                    if self.mpe_settings_dialog().settings.is_none() {
                        self.mpe_dialog = previous;
                        return Err(unavailable(
                            "it has no MPE/Multi-channel Settings: a plug-in device, an \
                             External Instrument or a MIDI track set to MPE has them"
                                .to_owned(),
                        ));
                    }
                }
                self.touch();
                Ok(to_value(&self.mpe_settings_dialog()))
            }
            plugin_ids::INFO => {
                let plugin_ids::PluginInfoArgs { device } = Self::args(id, args)?;
                let device = target(self, device)?;
                self.plugins.idle();
                self.au.idle();
                self.vst3.idle();
                info(self, device)
            }
            plugin_ids::SET_WINDOW => {
                let plugin_ids::SetWindowArgs { device, frame } = Self::args(id, args)?;
                let device = target(self, device)?;
                if !frame.is_valid() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message:
                            "a window's position and size must be finite, and its size above 0"
                                .to_owned(),
                    });
                }
                // WP-2.10: the frame store the set saves; no undo step.
                self.devices.set_editor_frame(device, frame);
                self.plugins.idle();
                info(self, device)
            }
            plugin_ids::SET_PANEL_PARAM => {
                let plugin_ids::SetPanelParamArgs {
                    device,
                    index,
                    param,
                } = Self::args(id, args)?;
                let device = target(self, device)?;
                if index >= plugin_ids::PANEL_KNOBS {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!(
                            "a plug-in panel has {} knobs, so index {index} is out of range",
                            plugin_ids::PANEL_KNOBS
                        ),
                    });
                }
                let kind = self
                    .editor
                    .document()
                    .device(device)
                    .map(|d| d.kind.clone())
                    .ok_or_else(|| unavailable(format!("no {device}")))?;
                let Some(key) = DeviceView::plugin_key(&kind) else {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{device} isn't a plug-in device"),
                    });
                };
                let mapping = self.prefs.plugin_panels.entry(key).or_default();
                match param {
                    Some(param) => {
                        mapping.insert(index, param);
                    }
                    None => {
                        mapping.remove(&index);
                    }
                }
                self.devices
                    .set_plugin_panels(self.prefs.plugin_panels.clone());
                self.save_preferences();
                self.touch();
                Ok(to_value(&self.device_view_state()))
            }
            plugin_ids::CONFIGURE => {
                let plugin_ids::ConfigureArgs { device, on } = Self::args(id, args)?;
                let device = target(self, device)?;
                let kind = self
                    .editor
                    .document()
                    .device(device)
                    .map(|d| d.kind.clone())
                    .ok_or_else(|| unavailable(format!("no {device}")))?;
                if DeviceView::plugin_key(&kind).is_none() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{device} isn't a plug-in device"),
                    });
                }
                let on = on.unwrap_or(!self.devices.configuring(device));
                self.devices.set_configuring(device, on);
                self.touch();
                Ok(to_value(&self.device_view_state()))
            }
            plugin_ids::CONFIGURE_PARAM => {
                let plugin_ids::ConfigureParamArgs {
                    device,
                    param,
                    index,
                } = Self::args(id, args)?;
                let device = target(self, device)?;
                let Some((all, mut configured)) = self.configured_lists(device) else {
                    return Err(unavailable(format!(
                        "{device} isn't a described plug-in device"
                    )));
                };
                if !all.contains(&param) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{} has no parameter {}", device, param.0),
                    });
                }
                configured.retain(|p| *p != param);
                let at = index.unwrap_or(configured.len()).min(configured.len());
                configured.insert(at, param);
                self.editor
                    .transact("Configure Plug-In", |e| {
                        e.apply(bl_model::Command::SetConfiguredParams {
                            device,
                            params: configured.clone(),
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.devices.clear_pending_param(device);
                self.dirty = true;
                self.document_changed();
                self.touch();
                self.configured_params_result(id, device)
            }
            plugin_ids::UNCONFIGURE_PARAM => {
                let plugin_ids::UnconfigureParamArgs {
                    device,
                    param,
                    force,
                } = Self::args(id, args)?;
                let device = target(self, device)?;
                let Some((all, mut configured)) = self.configured_lists(device) else {
                    return Err(unavailable(format!(
                        "{device} isn't a described plug-in device"
                    )));
                };
                if !all.contains(&param) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("{} has no parameter {}", device, param.0),
                    });
                }
                if !force {
                    let uses = self.param_uses(device, param);
                    if !uses.is_empty() {
                        // Manual 24.1.2: Live warns before deleting a
                        // parameter with automation, clip envelopes, or
                        // mappings.
                        return Err(ShellError::Failed {
                            command: id.to_owned(),
                            message: format!(
                                "this parameter has {}, so removing it from the panel changes \
                                 what plays; run again with force to remove it",
                                uses.join(", ")
                            ),
                        });
                    }
                }
                configured.retain(|p| *p != param);
                self.editor
                    .transact("Unconfigure Plug-In Parameter", |e| {
                        e.apply(bl_model::Command::SetConfiguredParams {
                            device,
                            params: configured.clone(),
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.devices.clear_pending_param(device);
                self.dirty = true;
                self.document_changed();
                self.touch();
                self.configured_params_result(id, device)
            }
            plugin_ids::MOVE_CONFIGURED_PARAM => {
                let plugin_ids::MoveConfiguredParamArgs { device, from, to } =
                    Self::args(id, args)?;
                let device = target(self, device)?;
                let Some((_all, mut configured)) = self.configured_lists(device) else {
                    return Err(unavailable(format!(
                        "{device} isn't a described plug-in device"
                    )));
                };
                if from >= configured.len() || to >= configured.len() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!(
                            "the configured list has {} entries, so {from} to {to} is out of \
                             range",
                            configured.len()
                        ),
                    });
                }
                let param = configured.remove(from);
                configured.insert(to, param);
                self.editor
                    .transact("Move Plug-In Parameter", |e| {
                        e.apply(bl_model::Command::SetConfiguredParams {
                            device,
                            params: configured.clone(),
                        })
                    })
                    .map_err(|error| ShellError::Failed {
                        command: id.to_owned(),
                        message: error.to_string(),
                    })?;
                self.dirty = true;
                self.document_changed();
                self.touch();
                self.configured_params_result(id, device)
            }
            plugin_ids::SCAN | plugin_ids::RESCAN => {
                let plugin_ids::ScanArgs { paths, background } = Self::args(id, args)?;
                if background {
                    // The Settings window's Rescan button: the scan runs
                    // while the app keeps drawing, and the browser lists
                    // what it found when it's done.
                    if paths.is_some() {
                        return Err(ShellError::InvalidArgs {
                            command: id.to_owned(),
                            message: "a background scan covers the system folders; leave \
                                      `paths` out"
                                .to_owned(),
                        });
                    }
                    self.spawn_plugin_scan(id == plugin_ids::RESCAN);
                    return Ok(to_value(&plugin_ids::PluginScan::default()));
                }
                let paths: Option<Vec<std::path::PathBuf>> =
                    paths.map(|paths| paths.into_iter().map(std::path::PathBuf::from).collect());
                // Named paths are searched for every hosted format; the
                // system folders only for the formats switched on.
                let formats = match &paths {
                    Some(_) => bl_plugin_host::scan::SCANNED_FORMATS.to_vec(),
                    None => crate::plugins::listed_formats(&self.prefs),
                };
                let report = crate::plugins::scan_report(
                    &self.scanner,
                    paths.as_deref(),
                    id == plugin_ids::RESCAN,
                    &formats,
                    &self.prefs.hidden_plugins,
                );
                self.publish_plugins();
                Ok(to_value(&report))
            }
            plugin_ids::LIST => {
                let _: plugin_ids::ListArgs = Self::args(id, args)?;
                let list = self.set_plugin_list();
                Ok(to_value(&list))
            }
            plugin_ids::EXPORT_LIST => {
                // Spec: plugin-list R7, R10.
                let plugin_ids::ExportListArgs { path } = Self::args(id, args)?;
                let Some(path) = path else {
                    // The list's Export… button: the app asks where to
                    // save it, and runs this again with the path.
                    self.effects.push(Effect::PickPluginListPath {
                        default_name: self.plugin_list_file_name(),
                    });
                    return Ok(Value::Null);
                };
                let path = std::path::PathBuf::from(path);
                let written = crate::plugins::export_list(
                    self.editor.document(),
                    &self.set_plugin_list(),
                    &path,
                    &self.set_name(),
                )
                .map_err(|message| ShellError::Failed {
                    command: id.to_owned(),
                    message,
                })?;
                Ok(to_value(&plugin_ids::ExportedList {
                    path: written.to_string_lossy().into_owned(),
                }))
            }
            plugin_ids::SETTINGS => {
                let plugin_ids::PluginSettingsArgs {
                    hidden,
                    show_hidden,
                    format_preference,
                    use_audio_units_v2,
                    use_audio_units_v3,
                    vst3_system_folders,
                } = Self::args(id, args)?;
                let mut changed = false;
                // Spec: plugin-scanning R12. Turning a format's switch on
                // scans it, as Live does.
                let mut scan = false;
                if let Some(on) = use_audio_units_v2 {
                    scan |= on && !self.prefs.use_audio_units_v2;
                    self.prefs.use_audio_units_v2 = on;
                    changed = true;
                }
                if let Some(on) = use_audio_units_v3 {
                    self.prefs.use_audio_units_v3 = on;
                    changed = true;
                }
                if let Some(on) = vst3_system_folders {
                    scan |= on && !self.prefs.vst3_system_folders;
                    self.prefs.vst3_system_folders = on;
                    changed = true;
                }
                if scan && self.auto_scan {
                    self.spawn_plugin_scan(false);
                }
                if let Some(hidden) = hidden {
                    self.prefs.hidden_plugins = hidden.into_iter().collect();
                    changed = true;
                }
                if let Some(show_hidden) = show_hidden {
                    self.prefs.show_hidden_plugins = show_hidden;
                    changed = true;
                }
                if let Some(format) = format_preference {
                    self.prefs.plugin_format_preference = Some(format);
                    changed = true;
                }
                if changed {
                    self.save_preferences();
                    self.publish_plugins();
                }
                Ok(to_value(&self.plugin_settings()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// A plug-in device's modifiable parameters and its configured list, in
    /// order (WP-5.40, clap-hosting R5); `None` for a device that isn't a
    /// plug-in, or whose type isn't described.
    fn configured_lists(
        &self,
        device: bl_model::DeviceId,
    ) -> Option<(Vec<bl_model::ParamId>, Vec<bl_model::ParamId>)> {
        let doc = self.editor.document();
        let kind = doc.device(device)?.kind.clone();
        if !matches!(kind, bl_model::DeviceKind::Plugin { .. }) {
            return None;
        }
        let description = self.devices.description(&kind)?;
        let all: Vec<bl_model::ParamId> = description.params.iter().map(|p| p.id).collect();
        let configured: Vec<bl_model::ParamId> = doc.device(device)?.configured(&all).into_owned();
        Some((all, configured))
    }

    /// What uses a plug-in parameter, which Live warns about before the panel
    /// drops it (manual 24.1.2): its automation, a clip envelope, or a
    /// mapping.
    fn param_uses(
        &self,
        device: bl_model::DeviceId,
        param: bl_model::ParamId,
    ) -> Vec<&'static str> {
        use bl_model::modulator::ModTarget;
        let doc = self.editor.document();
        let target = AutomationTarget::Device { device, param };
        let mut uses = Vec::new();
        if let Some(track) = doc.device_track(device).and_then(|t| doc.track(t)) {
            if track.automation_for(&target).is_some() {
                uses.push("automation");
            }
            if track.clips().any(|clip| clip.envelope(&target).is_some()) {
                uses.push("a clip envelope");
            }
        }
        let mapped = doc.tracks().any(|track| {
            track.devices_deep().any(|d| {
                d.mappings
                    .iter()
                    .any(|m| m.target == ModTarget::Device { device, param })
            })
        });
        if mapped {
            uses.push("a mapping");
        }
        uses
    }

    /// Adds a plug-in parameter to its device's configured list, as one undo
    /// step, when the device hasn't configured it (WP-5.40, manual 24.1.2):
    /// using a parameter in the automation or clip-envelope choosers makes its
    /// temporary entry permanent.
    pub(super) fn configure_on_use(
        &mut self,
        device: bl_model::DeviceId,
        param: bl_model::ParamId,
    ) {
        let Some((all, mut configured)) = self.configured_lists(device) else {
            return;
        };
        if !all.contains(&param) || configured.contains(&param) {
            return;
        }
        configured.push(param);
        let command = bl_model::Command::SetConfiguredParams {
            device,
            params: configured,
        };
        if self
            .editor
            .transact("Configure Plug-In", |e| e.apply(command))
            .is_ok()
        {
            self.devices.clear_pending_param(device);
            self.dirty = true;
            self.document_changed();
        }
    }

    /// `device.parameters` after a configure edit, as its command returns it.
    fn configured_params_result(
        &self,
        id: &str,
        device: bl_model::DeviceId,
    ) -> Result<Value, ShellError> {
        self.devices
            .parameters_of(self.editor.document(), device)
            .map(|params| to_value(&params))
            .ok_or_else(|| ShellError::Failed {
                command: id.to_owned(),
                message: format!("no {device}"),
            })
    }

    /// Loads a plug-in the browser lists (WP-2.9b): onto the selected track
    /// as `plugin.load` (or `plugin.load_au`) places it, or, while a device
    /// is being hot-swapped, in its place, as one undo step.
    pub(super) fn load_browser_plugin(
        &mut self,
        id: &str,
        plugin: &bl_plugin_host::scan::ScannedPlugin,
    ) -> Result<Value, ShellError> {
        let swapped = self.devices.hot_swap().and_then(|device| {
            let doc = self.editor.document();
            let track = doc.device_track(device)?;
            let index = doc.track(track)?.device_position(device)?;
            Some((device, track, index))
        });
        // An instrument replaces the selected track's instrument, as Live's
        // browser does (the plug-in scanning spec, R11).
        let swapped = swapped.or_else(|| {
            if plugin.role != bl_model::DeviceRole::Instrument {
                return None;
            }
            let doc = self.editor.document();
            let track = devices::shown_track(&self.arrangement, doc)?;
            let chain = doc.track(track)?;
            let device = chain
                .devices()
                .find(|d| d.role == bl_model::DeviceRole::Instrument)?
                .id;
            Some((device, track, chain.device_position(device)?))
        });
        let (mut track, index) = swapped.map_or((None, None), |(_, t, i)| (Some(t), Some(i)));
        self.editor.begin("Load Plug-In");
        let loaded = (|| {
            // Live always has a track selected; a set with none gets a new
            // track for the plug-in: MIDI for an instrument or a MIDI
            // effect, audio for an audio effect.
            if track.is_none()
                && devices::shown_track(&self.arrangement, self.editor.document()).is_none()
            {
                let kind = match plugin.role {
                    bl_model::DeviceRole::AudioEffect => "audio",
                    _ => "midi",
                };
                let before: std::collections::BTreeSet<TrackId> = self
                    .editor
                    .document()
                    .tracks()
                    .map(bl_model::Track::id)
                    .collect();
                self.invoke_inner("track.create", json!({ "kind": kind }))?;
                track = self
                    .editor
                    .document()
                    .tracks()
                    .map(bl_model::Track::id)
                    .find(|t| !before.contains(t));
            }
            // The device it replaces goes first, so the chain never holds
            // two instruments.
            if let Some((old, _, _)) = swapped {
                self.editor
                    .apply(bl_model::Command::RemoveDevice { device: old })
                    .map_err(|e| ShellError::Failed {
                        command: id.to_owned(),
                        message: e.to_string(),
                    })?;
            }
            let loaded = match plugin.format {
                bl_model::PluginFormat::AudioUnit => {
                    let (kind, subtype, manufacturer) = bl_plugin_host::au::parse_description_id(
                        &plugin.plugin_id,
                    )
                    .ok_or_else(|| ShellError::Failed {
                        command: id.to_owned(),
                        message: format!("{} isn't an Audio Unit's identifier", plugin.plugin_id),
                    })?;
                    self.run_plugins(
                        plugin_ids::LOAD_AU,
                        to_value(&plugin_ids::LoadAudioUnitArgs {
                            component_type: kind.to_owned(),
                            subtype: subtype.to_owned(),
                            manufacturer: manufacturer.to_owned(),
                            track,
                            index,
                        }),
                    )?
                }
                _ => self.run_plugins(
                    plugin_ids::LOAD,
                    to_value(&plugin_ids::LoadPluginArgs {
                        path: plugin.path.to_string_lossy().into_owned(),
                        plugin: Some(plugin.plugin_id.clone()),
                        track,
                        index,
                    }),
                )?,
            };
            Ok(loaded)
        })();
        match loaded {
            Ok(loaded) => {
                if self.editor.commit().unwrap_or(false) {
                    self.dirty = true;
                }
                self.document_changed();
                Ok(loaded)
            }
            Err(error) => {
                self.editor.cancel().ok();
                self.document_changed();
                Err(error)
            }
        }
    }
}

impl Shell {
    /// The MPE/Multi-channel Settings dialog (the MPE spec, R18): whether
    /// it's open, what it edits, and the settings it shows. It closes by
    /// itself when what it edits has gone or no longer sends MPE.
    #[must_use]
    pub fn mpe_settings_dialog(&self) -> plugin_ids::MpeSettingsDialog {
        let document = self.editor.document();
        let (device, track, settings) = match self.mpe_dialog {
            None => return plugin_ids::MpeSettingsDialog::default(),
            Some(MpeDialogTarget::Device(id)) => {
                let settings = document.device(id).and_then(|device| {
                    if matches!(device.kind, bl_model::DeviceKind::Plugin { .. }) {
                        Some(device.mpe.settings)
                    } else {
                        device.external.as_ref().and_then(|external| external.mpe)
                    }
                });
                (Some(id), None, settings)
            }
            Some(MpeDialogTarget::Track(id)) => {
                let settings =
                    document
                        .track(id)
                        .and_then(|track| match &track.routing().midi_output {
                            Some(bl_model::MidiTo::Port { mpe, .. }) => *mpe,
                            _ => None,
                        });
                (None, Some(id), settings)
            }
        };
        plugin_ids::MpeSettingsDialog {
            open: settings.is_some(),
            device,
            track,
            settings,
        }
    }
}
