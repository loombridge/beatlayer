//! The device commands: loading, hot-swapping and the Device View.

use super::{DevicesLoad, Effect, Shell, ShellError, device_error, file_stem, to_value};
use crate::browser;
use crate::devices;
use crate::devices::DeviceView;
use bl_devices::KitHost;
use bl_model::TrackId;
use bl_model::registry as model;
use bl_shell_spec::browser as browser_ids;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::pads as pad_ids;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;

impl Shell {
    /// Brings the registry's device commands in step with the set: each
    /// plug-in device whose plug-in lists commands has them, scoped to it,
    /// and a device that's gone has none. `force` remakes them even if no
    /// device changed, as when plug-ins may have started.
    pub(super) fn refresh_device_commands(&mut self, force: bool) {
        let doc = self.editor.document();
        let key: Vec<(bl_model::DeviceId, String, String, bool)> = doc
            .tracks()
            .flat_map(|track| {
                let label = crate::routing::track_label(doc, track.id());
                track
                    .devices_deep()
                    .filter(|d| self.kit.handles(&d.kind) || crate::plugins::is_clap(doc, d.id))
                    .map(|d| {
                        let listed = if self.kit.handles(&d.kind) {
                            self.kit.commands(d.id).is_some()
                        } else {
                            self.plugins.commands(d.id).is_some()
                        };
                        (d.id, d.title().to_owned(), label.clone(), listed)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        if !force && key == self.device_commands_key {
            return;
        }
        let found = crate::plugins::device_commands(doc, &self.plugins, &self.kit);
        for ids in std::mem::take(&mut self.device_commands).into_values() {
            for id in ids {
                self.registry.remove(&id);
            }
        }
        for (device, specs) in found {
            let mut ids = Vec::with_capacity(specs.len());
            for spec in specs {
                let id = spec.id.to_string();
                if self.registry.register(spec).is_ok() {
                    ids.push(id);
                }
            }
            self.device_commands.insert(device, ids);
        }
        self.device_commands_key = key;
    }

    /// The device commands of `device`: its plug-in's own commands, scoped
    /// to it (`device.d<device>.<command>`), as the registry holds them.
    #[must_use]
    pub fn device_command_ids(&self, device: bl_model::DeviceId) -> &[String] {
        self.device_commands.get(&device).map_or(&[], Vec::as_slice)
    }

    /// The kit plug-ins compiled in as native devices: their instances, on
    /// this thread.
    #[must_use]
    pub fn kit(&self) -> &KitHost {
        &self.kit
    }

    /// Runs `device`'s plug-in's own command `command`, which the registry
    /// holds as the device command `id`, and records the parameters it
    /// changed as one undo step named for it (or inside the step already
    /// open, as an agent's labelled one).
    pub(super) fn run_device_command(
        &mut self,
        id: &str,
        device: bl_model::DeviceId,
        command: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let kind = self
            .editor
            .document()
            .device(device)
            .map(|d| d.kind.clone())
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: format!("{device} isn't in the set"),
            })?;
        let (result, changes) = if self.kit.handles(&kind) {
            self.kit.sync(self.editor.document());
            let run = self
                .kit
                .run_command(device, command, &args)
                .map_err(failed)?;
            (run.result, run.changes)
        } else {
            let run =
                self.plugins
                    .run_command(device, command, &args)
                    .map_err(|error| match error {
                        bl_plugin_host::clap::ClapError::Refused(why) => failed(why),
                        other => failed(other.to_string()),
                    })?;
            (run.result, run.changes)
        };
        let label = self
            .registry
            .get(id)
            .map_or_else(|| command.to_owned(), |spec| spec.label.to_string());
        let before = self.editor.document().clone();
        let applied = self.editor.transact(label, |editor| {
            for (param, value) in &changes {
                let same = editor
                    .document()
                    .device(device)
                    .and_then(|d| d.params.get(param))
                    == Some(value);
                if !same {
                    editor.apply(bl_model::Command::SetDeviceParam {
                        device,
                        param: *param,
                        value: *value,
                    })?;
                }
            }
            Ok::<(), bl_model::EditError>(())
        });
        applied.map_err(|error| failed(error.to_string()))?;
        if *self.editor.document() != before {
            self.dirty = true;
            self.document_changed();
        }
        self.touch();
        Ok(result)
    }

    /// The Device View's selection, folded devices, clipboard, and
    /// hot-swapping (WP-2.4).
    #[must_use]
    pub fn devices(&self) -> &DeviceView {
        &self.devices
    }

    /// Sets the device types the host's engine offers, with their
    /// parameters' descriptions, which the Device View's generic UI draws,
    /// `device.parameters` reports, and loading a preset checks values
    /// against (WP-2.5).
    pub fn set_device_types(&mut self, types: Vec<bl_engine::DeviceType>) {
        self.native_types = types;
        self.refresh_device_types();
    }

    /// Gives the Device View the native types, the kit plug-ins compiled
    /// in, and the plug-ins loaded.
    pub(super) fn refresh_device_types(&mut self) {
        let mut types = self.native_types.clone();
        types.extend(self.kit.device_types());
        types.extend(self.plugins.types().into_iter().map(|t| t.device_type));
        types.extend(self.au.types().into_iter().map(|t| t.device_type));
        types.extend(self.vst3.types().into_iter().map(|t| t.device_type));
        types.extend(self.lv2.types().into_iter().map(|t| t.device_type));
        self.devices.set_device_types(types);
    }

    /// Whether `id` is a device command the registry holds: a plug-in's
    /// own command scoped to its device.
    pub(super) fn is_device_command(&self, id: &str) -> bool {
        id.starts_with(crate::plugins::DEVICE_COMMAND_PREFIX) && self.registry.contains(id)
    }

    /// What the Device View shows: the selected track's chain (WP-2.4).
    #[must_use]
    pub fn device_view_state(&self) -> device_ids::DeviceViewState {
        let doc = self.editor.document();
        self.devices
            .state(doc, devices::shown_track(&self.arrangement, doc))
    }

    /// What `device_view.state` returns: [`Shell::device_view_state`] with
    /// the meters between the devices of the chain, and of the chains of its
    /// racks, as the engine last published them (the MIDI meters spec, R6).
    fn device_view_report(&self) -> device_ids::DeviceViewState {
        let doc = self.editor.document();
        let mut state = self.device_view_state();
        state.meters = state
            .track
            .and_then(|id| doc.track(id))
            .map(|track| self.chain_meter_values(track))
            .unwrap_or_default();
        state
    }

    /// The meters in front of each device of `track`'s chain and after the
    /// last, then those of the chains of its racks, in signal order.
    fn chain_meter_values(&self, track: &bl_model::Track) -> Vec<device_ids::ChainMeterValue> {
        fn chain<'a>(
            shell: &Shell,
            owner: bl_engine::MeterOwner,
            devices: impl Iterator<Item = &'a bl_model::Device>,
            out: &mut Vec<device_ids::ChainMeterValue>,
        ) {
            let mut racks = Vec::new();
            let point = |before: Option<bl_model::DeviceId>, out: &mut Vec<_>| {
                let at = before.map_or(bl_engine::MeterSite::End, bl_engine::MeterSite::Before);
                if let Some(value) = shell.chain_meter(owner, at) {
                    out.push(device_ids::ChainMeterValue {
                        track: match owner {
                            bl_engine::MeterOwner::Track(id) => Some(id),
                            bl_engine::MeterOwner::Chain(_) => None,
                        },
                        chain: match owner {
                            bl_engine::MeterOwner::Chain(id) => Some(id),
                            bl_engine::MeterOwner::Track(_) => None,
                        },
                        before,
                        value,
                    });
                }
            };
            for device in devices {
                point(Some(device.id), out);
                racks.push(device);
            }
            point(None, out);
            for rack in racks {
                for rack_chain in &rack.chains {
                    chain(
                        shell,
                        bl_engine::MeterOwner::Chain(rack_chain.id),
                        rack_chain.devices.iter(),
                        out,
                    );
                }
            }
        }
        let mut out = Vec::new();
        chain(
            self,
            bl_engine::MeterOwner::Track(track.id()),
            track.devices(),
            &mut out,
        );
        out
    }

    /// The modulator slot whose Map button is lit, if one is (WP-5.10): a
    /// click on any parameter's control maps it.
    #[must_use]
    pub fn armed_map(&self) -> Option<device_ids::ArmedMap> {
        self.devices
            .armed()
            .map(|(device, slot)| device_ids::ArmedMap { device, slot })
    }

    /// Sets the display data a device's display draws, as
    /// [`Shell::poll_engine`] does with the engine's (for a shell without an
    /// engine, as in tests; while one runs, the next poll takes its data
    /// instead).
    pub fn set_device_display_data(&mut self, device: bl_model::DeviceId, values: &[f32]) {
        let data = self.devices.displays_mut().entry(device).or_default();
        data.published += 1;
        data.values.clear();
        data.values.extend_from_slice(values);
    }

    /// What `device`'s display shows, as the engine last published it
    /// (the device framework spec, R36): the cell draws it, a screen reader
    /// says it. `None` if the set has no such device.
    #[must_use]
    pub fn device_display(&self, device: bl_model::DeviceId) -> Option<bl_model::DeviceDisplay> {
        let device = self.editor.document().device(device)?;
        Some(self.devices.display(device))
    }

    /// Runs a Device View command and carries out what it asks of the rest
    /// of the shell.
    pub(super) fn run_devices(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        if id == device_ids::LOAD_SAMPLE {
            return self.load_sample(id, args);
        }
        if id == pad_ids::PAD_DROP {
            return self.pad_drop(args);
        }
        if id == pad_ids::PAD_HOT_SWAP_SAMPLE {
            return self.pad_hot_swap_sample(args);
        }
        if matches!(
            id,
            device_ids::CUT
                | device_ids::COPY
                | device_ids::DUPLICATE
                | device_ids::SAVE_PRESET
                | device_ids::SAVE_DEFAULT_PRESET
                | device_ids::CHAIN_SAVE_PRESET
        ) {
            // Spec: clap-hosting R8. A copy of a plug-in device starts
            // from its plug-in's state now.
            crate::plugins::capture_states(
                &self.plugins,
                &self.au,
                &self.vst3,
                &self.lv2,
                &self.kit,
                &mut self.editor,
            );
        }
        if id == device_ids::DISPLAY && self.audio.engine_running() {
            // Spec: device-framework R37. The newest the nodes published,
            // for an agent that never polls the engine.
            self.audio.displays_into(self.devices.displays_mut());
        }
        // Spec: mixer-view R28. Selecting a device that floats brings its
        // window to the front.
        let selected = (id == device_ids::SELECT)
            .then(|| serde_json::from_value::<device_ids::SelectDeviceArgs>(args.clone()).ok())
            .flatten()
            .and_then(|a| a.device);
        let user_library = self.browser_view.user_library().map(Path::to_path_buf);
        let zoom = self.zoom();
        let auto_hide = self.prefs.auto_hide_plug_in_windows.0;
        let swapping = self.devices.hot_swap_targets();
        let result = self.devices.run(
            id,
            args,
            devices::Context {
                editor: &mut self.editor,
                arrangement: &mut self.arrangement,
                user_library: user_library.as_deref(),
                media: &self.media,
                zoom,
                auto_hide,
                windows_shown: self.plug_in_windows,
                look: &self.prefs.look,
            },
        );
        let mut outcome = result.map_err(|error| device_error(id, error))?;
        if id == device_ids::STATE {
            outcome.value = to_value(&self.device_view_report());
        }
        // Spec: browser-tags R31. Starting hot-swap clears the search words
        // and the selected tags as a new step.
        let now_swapping = self.devices.hot_swap_targets();
        if now_swapping != swapping && now_swapping != Default::default() {
            self.browser_view.hot_swap_started();
        }
        // Spec: drum-racks R5. A held Preview button plays its chain.
        for &(chain, on) in &outcome.previews {
            self.audio.send_param(
                bl_engine::NodeKey::ChainNotes(chain),
                bl_engine::drum::param::PREVIEW,
                f64::from(u8::from(on)),
            );
        }
        // Spec: racks R42. Samples a rack or chain preset brought in.
        for file in &outcome.media {
            self.store.add_media(file.hash, &file.path, file.size);
        }
        if outcome.document_changed {
            self.dirty = true;
            self.document_changed();
        } else if outcome.view_changed {
            self.dirty = true;
        }
        if let Some(message) = outcome.message {
            self.set_message(message, false);
        }
        if let Some(device) = selected
            && self
                .floating_windows()
                .windows
                .iter()
                .any(|w| w.device == device && w.shown)
        {
            self.effects.push(Effect::RaiseDeviceWindow(device));
        }
        if let Some(route) = outcome.browse {
            // Spec: device-chains R19. Hot-swapping shows the device's
            // presets in the browser.
            self.browser = true;
            self.run_browser(browser_ids::OPEN_PLACE, json!({ "label": "user_library" }))
                .ok();
            for folder in route {
                self.run_browser(
                    browser_ids::OPEN_FOLDER,
                    json!({ "path": folder, "open": true, "keep_others": true }),
                )
                .ok();
            }
        }
        self.touch();
        Ok(outcome.value)
    }

    /// Drops samples, a device or a preset on a pad of a drum rack
    /// (`pad.drop`, the drum racks spec, R14 to R16): the media store takes
    /// the files, the set records where they are, and the pad's chains play
    /// them. One undo step.
    pub(super) fn pad_drop(&mut self, args: Value) -> Result<Value, ShellError> {
        let id = pad_ids::PAD_DROP;
        let args: pad_ids::PadDropArgs = Self::args(id, args)?;
        let mut samples = Vec::with_capacity(args.samples.len());
        for path in &args.samples {
            let path = PathBuf::from(path);
            let hash = self
                .media
                .import(&path)
                .map_err(|error| ShellError::Failed {
                    command: id.to_owned(),
                    message: format!("{}: {error}", path.display()),
                })?;
            let entry = self.media.entry(hash).ok_or_else(|| ShellError::Failed {
                command: id.to_owned(),
                message: "the media store lost the file".to_owned(),
            })?;
            let media = bl_model::ContentHash(*hash.as_bytes());
            self.store.add_media(media, &entry.path, entry.size);
            samples.push(devices::PadSample {
                media,
                name: file_stem(&entry.path),
            });
        }
        let user_library = self.browser_view.user_library().map(Path::to_path_buf);
        let dropped = self
            .devices
            .pad_drop(&mut self.editor, user_library.as_deref(), args, &samples)
            .map_err(|error| device_error(id, error))?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        Ok(to_value(&dropped))
    }

    /// A pad's Hot-Swap Sample button (`pad.hot_swap_sample`, the drum racks
    /// spec, R19): the pad becomes the hot-swap target and the browser opens
    /// the folder of the sample it plays.
    fn pad_hot_swap_sample(&mut self, args: Value) -> Result<Value, ShellError> {
        let id = pad_ids::PAD_HOT_SWAP_SAMPLE;
        let args: pad_ids::PadHotSwapArgs = Self::args(id, args)?;
        let media = self
            .devices
            .pad_sample_of(self.editor.document(), args.rack, args.note)
            .map_err(|error| device_error(id, error))?
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the pad's chain plays no sample".to_owned(),
            })?;
        let state = self.run_devices(pad_ids::PAD_HOT_SWAP, to_value(&args))?;
        let folder = self
            .media
            .entry(bl_media::ContentHash::from_bytes(media.0))
            .and_then(|entry| entry.path.parent().map(Path::to_path_buf));
        if let Some(folder) = folder {
            self.browser = true;
            self.run_browser(
                browser_ids::OPEN_FOLDER,
                json!({ "path": folder, "open": true, "keep_others": true }),
            )
            .ok();
        }
        Ok(state)
    }

    /// Loads a file into a device that plays one sample
    /// (`device.load_sample`, WP-2.16): the media store takes the file, the
    /// set records where it is and the device names it by content hash, and
    /// the engine plays it. One undo step.
    pub(super) fn load_sample(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let device_ids::LoadSampleArgs { device, path, slot } = Self::args(id, args)?;
        let device =
            device
                .or_else(|| self.devices.selected())
                .ok_or_else(|| ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "select a device first".to_owned(),
                })?;
        let target = self
            .editor
            .document()
            .device(device)
            .cloned()
            .ok_or_else(|| ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("no {device}"),
            })?;
        // Only the sampler and the drum sampler play samples (the sampler
        // spec, R11).
        let plays = matches!(
            &target.kind,
            bl_model::DeviceKind::Native { device_type } if bl_devices::plays_samples(device_type)
        );
        if !plays {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: format!("{device} doesn't play a sample"),
            });
        }
        // A device with several slots loads into the one selected unless
        // told which.
        let slot = slot.unwrap_or_else(|| self.devices.slot_of(&target) as u8);
        let slots = self.devices.slot_count(&target.kind).max(1);
        if u32::from(slot) >= slots {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!(
                    "{device} has {slots} sample slots, counted from 0; there is no slot {slot}"
                ),
            });
        }
        let path = PathBuf::from(path);
        let hash = self
            .media
            .import(&path)
            .map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: format!("{}: {error}", path.display()),
            })?;
        let entry = self.media.entry(hash).ok_or_else(|| ShellError::Failed {
            command: id.to_owned(),
            message: "the media store lost the file".to_owned(),
        })?;
        // The set records where the file is, so it reopens playing it
        // (WP-2.25's media table).
        let media = bl_model::ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        self.editor
            .apply(bl_model::Command::SetDeviceSample {
                device,
                media: Some(media),
                slot,
            })
            .map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: error.to_string(),
            })?;
        self.dirty = true;
        self.document_changed();
        self.touch();
        let name = entry
            .path
            .file_name()
            .map_or(String::new(), |name| name.to_string_lossy().into_owned());
        Ok(to_value(&device_ids::LoadSampleResult {
            device,
            media,
            slot,
            name,
        }))
    }

    /// Whether every top-level device on `tracks` is switched off, with at
    /// least one device among them (IMP-25, the device chains spec R39).
    pub(super) fn all_devices_off(&self, tracks: &[TrackId]) -> bool {
        let doc = self.editor.document();
        let mut any = false;
        for &track in tracks {
            let Some(track) = doc.track(track) else {
                continue;
            };
            for device in track.devices() {
                any = true;
                if device.enabled {
                    return false;
                }
            }
        }
        any
    }

    /// What the browser's selection loads into the Device View: one preset
    /// file, or a device listed under a label (device chains R19, racks R43
    /// and R45).
    pub(super) fn devices_load(&self) -> Option<DevicesLoad> {
        match self.browser_view.selected() {
            [one] if bl_project::preset::is_preset(one) => Some(DevicesLoad::Preset(one.clone())),
            _ if self.browser_view.selected_audio_is_empty() => {
                self.browser_view.selected_device().map(DevicesLoad::Device)
            }
            _ => None,
        }
    }

    /// Loads what the browser has selected: added where `device.add` puts
    /// a device, or, while a device, rack or chain is hot-swapped, in place
    /// of it.
    pub(super) fn load_into_devices(&mut self, load: DevicesLoad) -> Result<Value, ShellError> {
        let armed_chain = self.devices.hot_swap_chain();
        let swapping = self.devices.hot_swap().is_some() || armed_chain.is_some();
        match load {
            DevicesLoad::Preset(path) => {
                let path = path.to_string_lossy().into_owned();
                if devices::is_chain_preset(Path::new(&path)) {
                    self.run_devices(device_ids::CHAIN_LOAD_PRESET, json!({ "path": path }))
                } else if swapping {
                    self.run_devices(
                        device_ids::REPLACE,
                        json!({ "preset": path, "chain": armed_chain }),
                    )
                } else {
                    self.run_devices(device_ids::ADD_PRESET, json!({ "path": path }))
                }
            }
            DevicesLoad::Device(device) => {
                let new = device_ids::NewDevice {
                    kind: device.kind,
                    name: device.name,
                    role: device.role,
                    sidechain: device.sidechain,
                    midi_sidechain: device.midi_sidechain,
                };
                if swapping {
                    self.run_devices(
                        device_ids::REPLACE,
                        json!({ "new_device": new, "chain": armed_chain }),
                    )
                } else {
                    self.run_devices(
                        device_ids::ADD,
                        to_value(&device_ids::AddDeviceArgs {
                            device: new,
                            track: None,
                            chain: None,
                            index: None,
                        }),
                    )
                }
            }
        }
    }

    /// The browser's selection, if it's one audio file and a device that
    /// plays samples is being hot-swapped: what Enter or a double-click
    /// loads into the device's selected slot (WP-5.17).
    pub(super) fn pad_hot_swap_drop(
        &self,
        rack: bl_model::DeviceId,
        note: u8,
    ) -> Option<pad_ids::PadDropArgs> {
        let selected = self.browser_view.selected();
        let samples: Vec<String> = selected
            .iter()
            .filter(|p| browser::arrange::is_audio(&p.to_string_lossy()))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let preset = match selected {
            [one]
                if samples.is_empty()
                    && one
                        .extension()
                        .is_some_and(|e| e == bl_project::preset::PRESET_EXTENSION) =>
            {
                Some(one.to_string_lossy().into_owned())
            }
            _ => None,
        };
        (!samples.is_empty() || preset.is_some()).then(|| pad_ids::PadDropArgs {
            rack: Some(rack),
            note,
            samples,
            preset,
            ..Default::default()
        })
    }

    pub(super) fn hot_swap_sample(&self) -> Option<(bl_model::DeviceId, PathBuf)> {
        let device = self.devices.hot_swap()?;
        let takes = self.editor.document().device(device).is_some_and(|d| {
            matches!(
                &d.kind,
                bl_model::DeviceKind::Native { device_type }
                    if bl_devices::plays_samples(device_type)
            )
        });
        match self.browser_view.selected() {
            [one] if takes && browser::arrange::is_audio(&one.to_string_lossy()) => {
                Some((device, one.clone()))
            }
            _ => None,
        }
    }

    /// Spec: device-chains R2, R3, and IMP-24. A device may take the
    /// MIDI-effect position only if its type sends notes on from a note
    /// output; otherwise its role is its type's role. The document can't
    /// check this: only the host knows whether a plug-in has a note output.
    pub(super) fn prepare_device_role(
        &self,
        id: &str,
        mut args: Value,
    ) -> Result<Value, ShellError> {
        let Ok(bl_model::Command::SetDeviceRole { device, role }) =
            model::command_from_args(id, args.clone())
        else {
            return Ok(args);
        };
        // An unknown device is the document's to refuse.
        let Some(current) = self.editor.document().device(device) else {
            return Ok(args);
        };
        // A type the host hasn't met can't be checked; the chain's order
        // still is.
        let Some(kind) = self.devices.device_type(&current.kind) else {
            return Ok(args);
        };
        // Left out, the role is the other position: into the MIDI-effect
        // position when the type sends notes on and isn't there already,
        // and back where the type goes otherwise. A type that sends no
        // notes on can't take the MIDI-effect position at all, so the
        // menu's item says why rather than doing nothing.
        let role = match role {
            Some(role) => role,
            None if current.role == bl_model::DeviceRole::MidiEffect => kind.role,
            None if kind.note_output => bl_model::DeviceRole::MidiEffect,
            None => {
                return Err(ShellError::Failed {
                    command: id.to_owned(),
                    message: format!(
                        "{} has no note output, so it can't go in the MIDI-effect position",
                        current.title()
                    ),
                });
            }
        };
        if kind.role != role && !(role == bl_model::DeviceRole::MidiEffect && kind.note_output) {
            let message = if role == bl_model::DeviceRole::MidiEffect {
                format!(
                    "{} has no note output, so it can't go in the MIDI-effect position",
                    current.title()
                )
            } else {
                format!(
                    "{} goes in the chain as {}",
                    current.title(),
                    kind.role.label()
                )
            };
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message,
            });
        }
        if let Some(object) = args.as_object_mut() {
            object.insert(
                "role".to_owned(),
                serde_json::to_value(role).unwrap_or_default(),
            );
        }
        Ok(args)
    }
}
