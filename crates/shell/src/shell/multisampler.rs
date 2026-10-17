//! The multisampler's commands that need the shell (WP-5.13; the
//! multisampler spec, R3, R7, R10, R11, R13, R20): adding layers from
//! files, changing layer settings with Snap, replacing a layer's sample,
//! normalising, and the views and selection.
//!
//! Each edit goes through the document's own layer commands, inside one
//! transaction where it takes several, so it's one undo step.

use std::collections::BTreeSet;

use bl_media::{SampleData, read_sampler_info};
use bl_model::{
    Command, ContentHash, Device, DeviceId, Layer, LayerId, LayerSettings, ZoneKind,
    snap_to_zero_crossing,
};
use bl_shell_spec::multisampler::{
    self as ids, AddLayersArgs, LayersAdded, LayersChanged, LoopKind, MultisamplerState,
    Normalize, NormalizeArgs, SelectLayersArgs, LayerSelectMode, SetLayerArgs, SetLayerSampleArgs,
    SetViewArgs, SnapMarkersArgs,
};

use super::*;

impl Shell {
    /// Runs one of the multisampler's shell commands.
    pub(super) fn run_multisampler(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::ADD_LAYERS => {
                let args: AddLayersArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                self.add_layers(id, device, &args.paths, args.index)
            }
            ids::SET_LAYER => {
                let args: SetLayerArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layers = self.layers_or_selection(id, device, args.layers)?;
                let snap = self.layers_of(device).snap;
                let changed = self.edit_layers(id, "Change Sample Settings", device, &layers, |_, settings| {
                    settings.clone()
                }, &args.settings, snap)?;
                Ok(to_value(&changed))
            }
            ids::SNAP_LAYER_MARKERS => {
                let args: SnapMarkersArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layers = self.layers_or_selection(id, device, args.layers)?;
                let which = args.which;
                let changed = self.edit_layers(
                    id,
                    "Snap Markers",
                    device,
                    &layers,
                    move |layer, _| match which {
                        LoopKind::Sustain => LayerSettings::markers(
                            None,
                            None,
                            Some(layer.loop_start),
                            Some(layer.loop_end),
                            None,
                        ),
                        LoopKind::Release => {
                            LayerSettings::markers(None, None, None, None, Some(layer.release_loop))
                        }
                    },
                    &LayerSettings::default(),
                    true,
                )?;
                Ok(to_value(&changed))
            }
            ids::SET_LAYER_SAMPLE => {
                let args: SetLayerSampleArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layer = match args.layer {
                    Some(layer) => layer,
                    None => *self
                        .layers_or_selection(id, device, None)?
                        .first()
                        .ok_or_else(|| self.failed(id, "select a layer first"))?,
                };
                let (media, length, rate, channels, _) = self.import_layer_file(id, &args.path)?;
                self.editor
                    .apply(Command::SetLayerMedia {
                        device,
                        layer,
                        media,
                        length,
                        sample_rate: rate,
                        channels,
                    })
                    .map_err(|error| self.failed(id, error))?;
                self.changed();
                Ok(to_value(&LayersChanged {
                    device,
                    layers: vec![layer],
                }))
            }
            ids::NORMALIZE_LAYERS => {
                let args: NormalizeArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let layers = self.layers_or_selection(id, device, args.layers)?;
                self.normalize(id, device, &layers, args.what)
            }
            ids::SET_VIEW => {
                let args: SetViewArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                let mut view = self.devices.multisampler_view(device);
                view.tab = args.tab.unwrap_or(view.tab);
                view.zone_editor = args.zone_editor.unwrap_or(view.zone_editor);
                view.editor = args.editor.unwrap_or(view.editor);
                view.size = args.size.unwrap_or(view.size);
                view.auto_select = args.auto_select.unwrap_or(view.auto_select);
                self.devices.set_multisampler_view(device, view);
                self.touch();
                Ok(to_value(&self.multisampler_state(device)))
            }
            ids::SELECT_LAYERS => {
                let args: SelectLayersArgs = Self::args(id, args)?;
                let device = self.multisampler_target(id, args.device)?;
                self.select(id, device, &args)?;
                self.touch();
                Ok(to_value(&self.multisampler_state(device)))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    fn failed(&self, id: &str, message: impl ToString) -> ShellError {
        ShellError::Failed {
            command: id.to_owned(),
            message: message.to_string(),
        }
    }

    /// Records that the set changed.
    fn changed(&mut self) {
        self.dirty = true;
        self.document_changed();
        self.touch();
    }

    /// The multisampler a command acts on: `device`, or the selected one.
    fn multisampler_target(
        &self,
        id: &str,
        device: Option<DeviceId>,
    ) -> Result<DeviceId, ShellError> {
        let device = device
            .or_else(|| self.devices.selected())
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "select a device first".to_owned(),
            })?;
        let found = self.editor.document().device(device).ok_or_else(|| {
            ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("no {device}"),
            }
        })?;
        if !DeviceView::is_multisampler(found) {
            return Err(self.failed(id, format!("{device} isn't a multisampler")));
        }
        Ok(device)
    }

    fn layers_of(&self, device: DeviceId) -> bl_model::Layers {
        self.editor
            .document()
            .device(device)
            .map(|d| d.layers.clone())
            .unwrap_or_default()
    }

    /// `layers`, or the selected ones when none are given; they must all be
    /// there, and some must be.
    fn layers_or_selection(
        &self,
        id: &str,
        device: DeviceId,
        layers: Option<Vec<LayerId>>,
    ) -> Result<Vec<LayerId>, ShellError> {
        let layers = layers.unwrap_or_else(|| self.devices.layer_selection(self.editor.document(), device));
        if layers.is_empty() {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "select a layer first".to_owned(),
            });
        }
        Ok(layers)
    }

    /// A multisampler's views and selection.
    fn multisampler_state(&self, device: DeviceId) -> MultisamplerState {
        MultisamplerState {
            view: self.devices.multisampler_view(device),
            selection: self.devices.layer_selection(self.editor.document(), device),
            layers: self.layers_of(device).list.len(),
        }
    }

    /// Imports a file into the set's media and reads what a layer needs of
    /// it: the media, its length, rate and channels, and the audio.
    fn import_layer_file(
        &mut self,
        id: &str,
        path: &str,
    ) -> Result<(ContentHash, u64, u32, u8, std::sync::Arc<SampleData>), ShellError> {
        let path = PathBuf::from(path);
        let hash = self
            .media
            .import(&path)
            .map_err(|error| self.failed(id, format!("{}: {error}", path.display())))?;
        let entry = self
            .media
            .entry(hash)
            .ok_or_else(|| self.failed(id, "the media store lost the file"))?;
        let data = self
            .media
            .load(hash)
            .map_err(|error| self.failed(id, format!("{}: {error}", path.display())))?;
        let media = ContentHash(*hash.as_bytes());
        self.store.add_media(media, &entry.path, entry.size);
        let channels = u8::try_from(data.channel_count()).unwrap_or(u8::MAX);
        Ok((media, data.frames() as u64, data.sample_rate(), channels, data))
    }

    /// Adds files as layers (R3).
    fn add_layers(
        &mut self,
        id: &str,
        device: DeviceId,
        paths: &[String],
        index: Option<usize>,
    ) -> Result<Value, ShellError> {
        let mut layers = Vec::new();
        let mut skipped = Vec::new();
        for path in paths {
            let file = Path::new(path);
            match self.import_layer_file(id, path) {
                Ok((media, length, rate, channels, _)) if length > 0 => {
                    let name = file
                        .file_stem()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    let mut layer = Layer::new(LayerId(0), name, media, length, rate, channels);
                    let info = read_sampler_info(file);
                    layer.root_key = info.root_key.unwrap_or(bl_model::DEFAULT_ROOT_KEY);
                    if let Some((start, end)) = info.loop_points
                        && end <= length
                    {
                        layer.loop_start = start;
                        layer.loop_end = end;
                        layer.release_loop = end;
                    }
                    layers.push(layer);
                }
                Ok(_) => skipped.push(format!("{path}: it has no audio")),
                Err(error) => skipped.push(match error {
                    ShellError::Failed { message, .. } => message,
                    other => other.to_string(),
                }),
            }
        }
        if layers.is_empty() {
            return Err(self.failed(
                id,
                if skipped.is_empty() {
                    "no files were given".to_owned()
                } else {
                    skipped.join("; ")
                },
            ));
        }
        let held = self.layers_of(device);
        let at = index.unwrap_or_else(|| {
            let selected = self.devices.layer_selection(self.editor.document(), device);
            held.list
                .iter()
                .rposition(|l| selected.contains(&l.id))
                .map_or(held.list.len(), |i| i + 1)
        });
        let count = layers.len();
        self.editor
            .apply(Command::InsertLayers {
                device,
                index: Some(at),
                layers,
            })
            .map_err(|error| self.failed(id, error))?;
        let added: Vec<LayerId> = self.layers_of(device).list[at..at + count]
            .iter()
            .map(|l| l.id)
            .collect();
        self.devices.select_layers(device, added.clone());
        self.changed();
        Ok(to_value(&LayersAdded {
            device,
            layers: added,
            skipped,
        }))
    }

    /// The left channel of a layer's sample, at its own rate.
    fn left_channel(&self, layer: &Layer) -> Option<std::sync::Arc<SampleData>> {
        self.media
            .load(bl_media::ContentHash::from_bytes(layer.media.0))
            .ok()
    }

    /// Changes `layers` one at a time inside one transaction: each gets the
    /// settings `settings_of` gives (the layer as it is, and `base`), its
    /// markers snapped to zero crossings if `snap` says.
    #[expect(clippy::too_many_arguments, reason = "one edit's parts")]
    fn edit_layers(
        &mut self,
        id: &str,
        label: &'static str,
        device: DeviceId,
        layers: &[LayerId],
        settings_of: impl Fn(&Layer, &LayerSettings) -> LayerSettings,
        base: &LayerSettings,
        snap: bool,
    ) -> Result<LayersChanged, ShellError> {
        let held = self.layers_of(device);
        let mut edits = Vec::new();
        for &layer_id in layers {
            let layer = held
                .get(layer_id)
                .ok_or_else(|| self.failed(id, format!("no layer {}", layer_id.0)))?;
            let mut settings = settings_of(layer, base);
            if snap && let Some(sample) = self.left_channel(layer) {
                let left = sample.channel(0).unwrap_or(&[]);
                let snapped = |marker: Option<u64>| {
                    marker.map(|m| snap_to_zero_crossing(left, m).min(layer.length))
                };
                settings.start = snapped(settings.start);
                settings.end = snapped(settings.end);
                settings.loop_start = snapped(settings.loop_start);
                settings.loop_end = snapped(settings.loop_end);
                settings.release_loop = snapped(settings.release_loop);
            }
            edits.push((layer_id, settings));
        }
        let result = self.editor.transact(label, |editor| {
            for (layer, settings) in edits {
                editor.apply(Command::UpdateLayers {
                    device,
                    layers: vec![layer],
                    settings,
                })?;
            }
            Ok::<_, bl_model::EditError>(())
        });
        result.map_err(|error| self.failed(id, error))?;
        self.changed();
        Ok(LayersChanged {
            device,
            layers: layers.to_vec(),
        })
    }

    /// Normalize Volume and Normalize Pan (R13).
    fn normalize(
        &mut self,
        id: &str,
        device: DeviceId,
        layers: &[LayerId],
        what: Normalize,
    ) -> Result<Value, ShellError> {
        let held = self.layers_of(device);
        let mut edits = Vec::new();
        for &layer_id in layers {
            let layer = held
                .get(layer_id)
                .ok_or_else(|| self.failed(id, format!("no layer {}", layer_id.0)))?;
            let Some(sample) = self.left_channel(layer) else {
                continue;
            };
            let (start, end) = (layer.start as usize, (layer.end as usize).min(sample.frames()));
            let channel = |c: usize| sample.channel(c).map_or(&[][..], |s| &s[start.min(end)..end]);
            let settings = match what {
                Normalize::Volume => {
                    let peak = (0..sample.channel_count())
                        .flat_map(|c| channel(c).iter())
                        .fold(0.0f32, |peak, s| peak.max(s.abs()));
                    (peak > 0.0).then(|| LayerSettings {
                        volume: Some((1.0 / f64::from(peak)).min(bl_model::LAYER_GAIN_MAX)),
                        ..LayerSettings::default()
                    })
                }
                Normalize::Pan if sample.channel_count() >= 2 => {
                    let rms = |c: usize| {
                        let data = channel(c);
                        (data.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                            / data.len().max(1) as f64)
                            .sqrt()
                    };
                    let (left, right) = (rms(0), rms(1));
                    (left > 0.0 && right > 0.0).then(|| {
                        // Pan takes one side down by (1 - b) while the
                        // other stays: the louder channel comes down to
                        // the quieter one.
                        let balance = if left > right {
                            1.0 - right / left
                        } else {
                            left / right - 1.0
                        };
                        LayerSettings {
                            pan: Some(balance * 50.0),
                            ..LayerSettings::default()
                        }
                    })
                }
                Normalize::Pan => None,
            };
            if let Some(settings) = settings {
                edits.push((layer_id, settings));
            }
        }
        let label = match what {
            Normalize::Volume => "Normalize Volume",
            Normalize::Pan => "Normalize Pan",
        };
        self.editor
            .transact(label, |editor| {
                for (layer, settings) in edits {
                    editor.apply(Command::UpdateLayers {
                        device,
                        layers: vec![layer],
                        settings,
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|error| self.failed(id, error))?;
        self.changed();
        Ok(to_value(&LayersChanged {
            device,
            layers: layers.to_vec(),
        }))
    }

    /// Changes the layer selection (R11).
    fn select(&mut self, id: &str, device: DeviceId, args: &SelectLayersArgs) -> Result<(), ShellError> {
        let held = self.layers_of(device);
        if let Some(missing) = args.layers.iter().find(|l| held.get(**l).is_none()) {
            return Err(self.failed(id, format!("no layer {}", missing.0)));
        }
        let now: BTreeSet<LayerId> = self
            .devices
            .layer_selection(self.editor.document(), device)
            .into_iter()
            .collect();
        let picked: BTreeSet<LayerId> = args.layers.iter().copied().collect();
        let next: BTreeSet<LayerId> = match args.mode {
            LayerSelectMode::Replace if args.layers.is_empty() => {
                held.list.iter().map(|l| l.id).collect()
            }
            LayerSelectMode::Replace => picked,
            LayerSelectMode::Add => &now | &picked,
            LayerSelectMode::Toggle => &now ^ &picked,
            LayerSelectMode::SameRange => {
                let kind = self.devices.multisampler_view(device).editor;
                let zone = |l: &Layer| match kind {
                    ZoneKind::Key => l.key,
                    ZoneKind::Velocity => l.velocity,
                    ZoneKind::Select => l.select,
                };
                let first = args
                    .layers
                    .first()
                    .and_then(|l| held.get(*l))
                    .ok_or_else(|| self.failed(id, "name a layer first"))?;
                let wanted = (zone(first).low, zone(first).high);
                held.list
                    .iter()
                    .filter(|l| (zone(l).low, zone(l).high) == wanted)
                    .map(|l| l.id)
                    .collect()
            }
        };
        let ordered: Vec<LayerId> = held
            .list
            .iter()
            .map(|l| l.id)
            .filter(|l| next.contains(l))
            .collect();
        self.devices.select_layers(device, ordered);
        Ok(())
    }

    /// A key of the computer keyboard went down or up: with Auto Select on,
    /// the multisamplers of the track the Device View shows select the layers
    /// the note plays while it's held (R7).
    pub(super) fn follow_note(&mut self, key: u8, velocity: u8, down: bool) {
        let Some(track) = devices::shown_track(&self.arrangement, self.editor.document()) else {
            return;
        };
        self.devices
            .note(self.editor.document(), track, key, velocity, down);
        self.touch();
    }

    /// The device a command found, for tests and the UI.
    #[must_use]
    pub fn multisampler_device(&self, device: DeviceId) -> Option<&Device> {
        self.editor
            .document()
            .device(device)
            .filter(|d| DeviceView::is_multisampler(d))
    }
}
