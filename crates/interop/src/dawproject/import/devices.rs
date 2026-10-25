//! The devices of a channel (the DAWproject spec, R44 to R55): the chain in
//! role order, plug-ins found or kept missing with their states, Beatlayer's
//! own device and rack presets, and the four generic devices.

use bl_devices::{compressor, eq, gate, limiter};
use bl_engine::DeviceType;
use bl_model::{DeviceId, DeviceKind, OrdMap, ParamId, PluginState};
use bl_project::rack_preset::Saved;

use super::automation::Driven;
use super::*;
use crate::dawproject::generic::{self, Entry, Quantity};
use crate::dawproject::model::{BoolParameter, DeviceBody, PluginFormat as File, RealParameter};
use crate::dawproject::plugins::{self, Installed};

/// A device made, ready to go in a chain.
struct Built {
    device: bl_model::Device,
    /// Where the device sits in its chain.
    role: bl_model::DeviceRole,
    /// What the report calls it.
    label: String,
    /// Set for a plug-in this machine doesn't have, which is listed once
    /// it is kept (R46).
    missing: bool,
}

/// One value of a generic device.
enum Value<'a> {
    Real(&'a RealParameter),
    Bool(&'a BoolParameter),
}

impl Importer<'_, '_> {
    /// The device types Beatlayer ships, made once.
    pub(super) fn native_types(&mut self) -> Arc<Vec<DeviceType>> {
        Arc::clone(
            self.types
                .get_or_insert_with(|| Arc::new(bl_devices::device_types())),
        )
    }

    /// A new native device of `device_type`, with the structure its type
    /// gives it.
    fn native(&mut self, device_type: &str) -> Option<bl_model::Device> {
        let id: DeviceId = self.editor.new_id();
        let kind = DeviceKind::Native {
            device_type: device_type.to_owned(),
        };
        self.native_types()
            .iter()
            .find(|t| t.kind == kind)
            .map(|t| t.device(id))
    }

    /// The devices of a channel as a chain (R44, R45).
    pub(super) fn build_devices(&mut self, made: &Made, channel: &Channel, place: &str) {
        let mut built: Vec<Built> = Vec::new();
        for device in &channel.devices {
            if let Some(item) = self.build_device(made, device, place) {
                built.push(item);
            }
        }
        // MIDI effects first, then one instrument, then audio effects; the
        // file's order within each.
        let midi = made.kind == TrackKind::Midi;
        let mut kept: Vec<Built> = Vec::new();
        for item in built {
            if item.role != bl_model::DeviceRole::AudioEffect && !midi {
                self.report.add(
                    &format!("{} on a track that holds no MIDI", item.label),
                    place,
                    "left out",
                );
                continue;
            }
            if item.role == bl_model::DeviceRole::Instrument
                && kept
                    .iter()
                    .any(|held| held.role == bl_model::DeviceRole::Instrument)
            {
                self.report.add(
                    &format!("a second instrument, {}", item.label),
                    place,
                    "left out",
                );
                continue;
            }
            kept.push(item);
        }
        // A device moves to its role's place when an earlier one has a later
        // role than its own.
        for (index, item) in kept.iter().enumerate() {
            if kept[..index].iter().any(|earlier| earlier.role > item.role) {
                self.report.add(
                    &format!("{} out of its role's place", item.label),
                    place,
                    "moved to its role's place",
                );
            }
        }
        kept.sort_by_key(|item| item.role);
        for item in &kept {
            if item.missing {
                // A missing plug-in keeps its ID and its state (R46).
                self.report
                    .add(&item.label, place, "kept as a missing plug-in");
            }
        }
        for (index, item) in kept.into_iter().enumerate() {
            self.try_apply(
                bl_model::Command::InsertDevice {
                    track: made.id,
                    index,
                    device: item.device,
                },
                &item.label,
                place,
            );
        }
    }

    /// One device of a chain, or `None` for one left out (R46 to R54).
    fn build_device(&mut self, made: &Made, device: &model::Device, place: &str) -> Option<Built> {
        let mut built = match &device.body {
            DeviceBody::Plugin(File::Vst2) => {
                self.report.add("a VST2 plug-in", place, "left out");
                return None;
            }
            DeviceBody::Plugin(format) => self.plugin_device(made, device, *format, place)?,
            _ => {
                // A Beatlayer preset in the state makes the device it was
                // saved from, and its generic values aren't read (R50).
                if let Some(built) = self.preset_device(device, place) {
                    built
                } else if matches!(device.body, DeviceBody::Builtin) {
                    let what = format!(
                        "a device `{}` by `{}`",
                        device.device_name.as_deref().unwrap_or("unnamed"),
                        device.device_vendor.as_deref().unwrap_or("unknown"),
                    );
                    self.report.add(&what, place, "left out");
                    return None;
                } else {
                    self.generic_device(made, device, place)?
                }
            }
        };
        if let Some(name) = device.info.name.clone().filter(|name| !name.is_empty())
            && name != built.device.name
        {
            if built.device.own_name.is_empty() {
                built.device.own_name = built.device.name.clone();
            }
            built.device.name = name;
        }
        if device.enabled.as_ref().and_then(|e| e.value) == Some(false) {
            built.device.enabled = false;
        }
        if device.loaded == Some(false) {
            built.device.enabled = false;
            self.report
                .add("a device that isn't loaded", place, "switched off");
        }
        if let Some(parameter) = device.enabled.as_ref().and_then(|e| e.id.clone()) {
            self.targets
                .insert(parameter, Driven::Unsupported("a device's Enabled"));
        }
        Some(built)
    }

    /// A plug-in, found among the installed ones or kept missing (R46 to
    /// R49).
    fn plugin_device(
        &mut self,
        made: &Made,
        device: &model::Device,
        format: File,
        place: &str,
    ) -> Option<Built> {
        let (format, word) = match format {
            File::Clap => (bl_model::PluginFormat::Clap, "a CLAP plug-in"),
            File::Vst3 => (bl_model::PluginFormat::Vst3, "a VST3 plug-in"),
            File::Au => (bl_model::PluginFormat::AudioUnit, "an Audio Unit plug-in"),
            File::Vst2 => return None,
        };
        let Some(id) = device
            .device_id
            .as_deref()
            .and_then(|id| plugins::plugin_id(format, id))
        else {
            self.report
                .add(&format!("{word} whose ID doesn't read"), place, "left out");
            return None;
        };
        let installed: Option<Installed> = self
            .context
            .plugins
            .installed()
            .iter()
            .find(|plugin| plugin.format == format && plugin.plugin_id == id)
            .cloned();
        let state = self.plugin_state(device, format, &id, word, place);
        let file_role = device.role.map(role_of);
        let name = device
            .info
            .name
            .clone()
            .or_else(|| device.device_name.clone())
            .or_else(|| installed.as_ref().map(|plugin| plugin.name.clone()))
            .unwrap_or_else(|| id.clone());
        let role = file_role
            .or_else(|| installed.as_ref().map(|plugin| plugin.role))
            .unwrap_or(bl_model::DeviceRole::AudioEffect);
        let new_id: DeviceId = self.editor.new_id();
        let mut built = bl_model::Device::new(
            new_id,
            installed
                .as_ref()
                .map_or(name.clone(), |plugin| plugin.name.clone()),
            DeviceKind::Plugin {
                format,
                plugin_id: id,
            },
        )
        .with_role(role);
        built.plugin = Some(PluginState {
            path: installed
                .as_ref()
                .map(|plugin| plugin.path.clone())
                .unwrap_or_default(),
            data: state.unwrap_or_default(),
        });
        for parameter in &device.parameters {
            self.register_plugin_parameter(made.id, new_id, installed.as_ref(), parameter);
        }
        Some(Built {
            device: built,
            role,
            label: word.to_owned(),
            missing: installed.is_none(),
        })
    }

    /// The bytes a plug-in's state file holds, as its host loads them (R47
    /// to R49).
    fn plugin_state(
        &mut self,
        device: &model::Device,
        format: bl_model::PluginFormat,
        id: &str,
        word: &str,
        place: &str,
    ) -> Option<Vec<u8>> {
        let reference = device.state.as_ref()?;
        let path = reference.path.replace('\\', "/");
        let bytes = if reference.external {
            self.report.add(
                &format!("the state of {word}"),
                place,
                "left out; it isn't embedded",
            );
            return None;
        } else {
            match self.container.files.get(&path) {
                Some(bytes) => bytes.clone(),
                None => {
                    self.report.add(
                        &format!("the state of {word}"),
                        place,
                        "left out; the container doesn't hold it",
                    );
                    return None;
                }
            }
        };
        match format {
            bl_model::PluginFormat::Vst3 => match bl_model::vst3_state::VstPreset::read(&bytes) {
                Ok(preset) if bl_model::vst3_state::same_class_id(&preset.class_id, id) => {
                    Some(preset.host_state())
                }
                _ => {
                    self.report.add(
                        &format!("the state of {word}"),
                        place,
                        "left out; the plug-in starts at its defaults",
                    );
                    None
                }
            },
            bl_model::PluginFormat::AudioUnit => {
                let text = bytes.starts_with(b"bplist") || {
                    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
                    head.contains("<plist")
                };
                if text {
                    Some(bytes)
                } else {
                    self.report.add(
                        &format!("the state of {word}"),
                        place,
                        "left out; it isn't a property list",
                    );
                    None
                }
            }
            _ => Some(bytes),
        }
    }

    /// Records the parameters a plug-in's automation can name (R57).
    fn register_plugin_parameter(
        &mut self,
        track: TrackId,
        device: DeviceId,
        installed: Option<&Installed>,
        parameter: &model::Parameter,
    ) {
        use model::Parameter::{Bool, Enumerated, Integer, Real};
        let (id, native) = match parameter {
            Real(p) => (&p.id, p.parameter_id),
            Bool(p) => (&p.id, p.parameter_id),
            Integer {
                id, parameter_id, ..
            }
            | Enumerated {
                id, parameter_id, ..
            } => (id, *parameter_id),
            model::Parameter::Signature(_) => return,
        };
        let (Some(id), Some(native)) = (id, native.and_then(|n| u32::try_from(n).ok())) else {
            return;
        };
        let unit = match parameter {
            Real(p) => p.unit,
            _ => None,
        };
        self.targets.insert(
            id.clone(),
            Driven::Plugin {
                track,
                device,
                native: ParamId(native),
                installed: installed.cloned(),
                unit,
            },
        );
    }

    /// The device a Beatlayer preset in the state makes (R50), or `None`
    /// when the state isn't one for this device.
    fn preset_device(&mut self, device: &model::Device, place: &str) -> Option<Built> {
        let reference = device.state.as_ref().filter(|state| !state.external)?;
        let bytes = self
            .container
            .files
            .get(&reference.path.replace('\\', "/"))?
            .clone();
        let saved = Saved::decode(&bytes).ok()?;
        let wanted = device.device_id.as_deref()?;
        match saved {
            Saved::Device(preset) => {
                let DeviceKind::Native { device_type } = &preset.kind else {
                    return None;
                };
                if device_type != wanted {
                    return None;
                }
                let mut made = self.native(device_type).unwrap_or_else(|| {
                    let id: DeviceId = self.editor.new_id();
                    bl_model::Device::new(id, preset.device_name.clone(), preset.kind.clone())
                        .with_role(preset.role)
                });
                let types = self.native_types();
                let params: OrdMap<ParamId, f64> = preset
                    .params
                    .into_iter()
                    .filter(|(param, _)| {
                        generic::range_of(&types, device_type, *param).is_some()
                            || !types.iter().any(|t| t.kind == preset.kind)
                    })
                    .collect();
                made.set_both(params);
                made.shape = preset.shape;
                if let Some(articulations) = preset.articulations {
                    made.set_articulations(articulations);
                }
                if let Some(mpe) = preset.mpe {
                    made.mpe = mpe;
                }
                let role = made.role;
                Some(Built {
                    device: made,
                    role,
                    label: format!("a {} preset", preset.device_name),
                    missing: false,
                })
            }
            Saved::Rack(preset) => {
                let mut rack = preset.rack;
                if rack.rack_kind().map(|kind| kind.name()) != Some(wanted) {
                    return None;
                }
                let mut next = || self.editor.new_id::<DeviceId>().0;
                rack.reassign_ids(&mut next);
                rack.clear_mappings();
                for (hash, sample) in preset.samples {
                    self.add_sample(hash, &sample.name, sample.data, place);
                }
                let role = rack.role;
                Some(Built {
                    label: format!("a {}", rack.title()),
                    device: rack,
                    role,
                    missing: false,
                })
            }
            Saved::Chain(_) => None,
        }
    }

    /// A sample a rack preset carries, written with the set's files (R50).
    fn add_sample(&mut self, hash: bl_model::ContentHash, name: &str, data: Vec<u8>, place: &str) {
        if hash_of(&data) != hash {
            self.report
                .add("a sample of a preset that is damaged", place, "left out");
            return;
        }
        if self.files.iter().any(|file| file.hash == hash) {
            return;
        }
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '.' | '-' | ' ') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = format!("presets/{}-{safe}", &hash.to_string()[..16]);
        let size = data.len() as u64;
        if let Ok(relative) = MediaPath::new(path.clone()) {
            self.media.insert(MediaRef {
                hash,
                path: relative,
                size: 0,
                reversed_from: None,
            });
        }
        self.files.push(Extracted {
            path,
            bytes: Some(data),
            on_disk: None,
            hash,
            size,
        });
    }

    /// One of DAWproject's four generic devices (R51 to R55).
    fn generic_device(
        &mut self,
        made: &Made,
        device: &model::Device,
        place: &str,
    ) -> Option<Built> {
        let (device_type, entries, label): (&'static str, &[Entry], &str) = match &device.body {
            DeviceBody::Compressor { .. } => (
                compressor::COMPRESSOR,
                generic::COMPRESSOR,
                "a generic compressor",
            ),
            DeviceBody::NoiseGate { .. } => (gate::GATE, generic::GATE, "a generic gate"),
            DeviceBody::Limiter { .. } => (limiter::LIMITER, generic::LIMITER, "a generic limiter"),
            DeviceBody::Equalizer { .. } => (eq::EQ, &[], "a generic equaliser"),
            _ => return None,
        };
        let built = self.native(device_type)?;
        let mut set = Setting {
            track: made.id,
            device: built.id,
            device_type,
            types: self.native_types(),
            place: place.to_owned(),
            params: OrdMap::new(),
        };
        for entry in entries {
            if let Some(value) = value_of(&device.body, entry.element) {
                self.set_generic(&mut set, entry, &value);
            }
        }
        match &device.body {
            DeviceBody::Compressor { input_gain, .. } => {
                self.expect_zero_db(
                    input_gain.as_ref(),
                    "a generic compressor's input gain",
                    place,
                );
            }
            DeviceBody::NoiseGate { ratio, .. } => {
                if ratio
                    .as_ref()
                    .and_then(|ratio| ratio.value)
                    .is_some_and(|value| ratio_percent(ratio.as_ref(), value) < 100.0)
                {
                    self.report
                        .add("a generic gate's ratio under 100 %", place, "left out");
                }
            }
            DeviceBody::Limiter { attack, .. } => {
                if attack.is_some() {
                    self.report
                        .add("a generic limiter's attack", place, "left out");
                }
                self.limiter_values(&mut set, device);
            }
            DeviceBody::Equalizer { .. } => self.equaliser_values(&mut set, device),
            _ => {}
        }
        let mut built = built;
        built.set_both(set.params);
        Some(Built {
            device: built,
            role: bl_model::DeviceRole::AudioEffect,
            label: label.to_owned(),
            missing: false,
        })
    }

    /// Sets one value of a generic device, and records the parameter its
    /// automation names (R55).
    fn set_generic(&mut self, set: &mut Setting, entry: &Entry, value: &Value<'_>) {
        let (id, unit, min, max, raw) = match value {
            Value::Real(p) => (&p.id, p.unit, p.min, p.max, p.value),
            Value::Bool(p) => (
                &p.id,
                None,
                None,
                None,
                p.value.map(|v| f64::from(u8::from(v))),
            ),
        };
        if let Some(id) = id {
            self.targets.insert(
                id.clone(),
                Driven::Device {
                    track: set.track,
                    device: set.device,
                    param: entry.param,
                    quantity: entry.quantity,
                    device_type: set.device_type,
                    unit,
                    min,
                    max,
                    offset_db: 0.0,
                },
            );
        }
        let Some(raw) = raw else { return };
        match generic::convert(entry.quantity, unit, raw, min, max, set.range(entry.param)) {
            Some(converted) => {
                set.params.insert(entry.param, converted);
            }
            None => self.report.add(
                &format!(
                    "a value in a unit Beatlayer can't convert ({})",
                    entry.element
                ),
                &set.place,
                "left at its default",
            ),
        }
    }

    /// Lists an input gain the device cannot hold (R51, R54).
    fn expect_zero_db(&mut self, gain: Option<&RealParameter>, what: &str, place: &str) {
        if let Some(gain) = gain
            && let Some(db) = db_of(gain)
            && db.abs() > 1e-9
        {
            self.report.add(what, place, "left out");
        }
    }

    /// The Limiter's Gain and Ceiling, which take the output gain too (R53).
    fn limiter_values(&mut self, set: &mut Setting, device: &model::Device) {
        let DeviceBody::Limiter {
            input_gain,
            output_gain,
            threshold,
            ..
        } = &device.body
        else {
            return;
        };
        let output = output_gain.as_ref().and_then(db_of);
        let input = input_gain.as_ref().and_then(db_of);
        let ceiling = threshold.as_ref().and_then(db_of);
        let offset = output.unwrap_or(0.0);
        if let Some(ceiling) = ceiling.or(output.map(|_| 0.0)) {
            let range = set.range(limiter::CEILING);
            set.params
                .insert(limiter::CEILING, (ceiling + offset).clamp(range.0, range.1));
        }
        if input.is_some() || output.is_some() {
            let range = set.range(limiter::GAIN);
            set.params.insert(
                limiter::GAIN,
                bl_model::param::db_to_gain(input.unwrap_or(0.0) + offset).clamp(range.0, range.1),
            );
        }
        // The automation of the threshold and the input gain moves the same
        // parameters, offset by the output gain; the output gain itself
        // moves both and isn't automated.
        for (parameter, entry) in [
            (threshold, &generic::LIMITER_CEILING),
            (input_gain, &generic::LIMITER_GAIN),
        ] {
            if let Some(parameter) = parameter
                && let Some(id) = &parameter.id
            {
                self.targets.insert(
                    id.clone(),
                    Driven::Device {
                        track: set.track,
                        device: set.device,
                        param: entry.param,
                        quantity: entry.quantity,
                        device_type: limiter::LIMITER,
                        unit: parameter.unit,
                        min: parameter.min,
                        max: parameter.max,
                        offset_db: offset,
                    },
                );
            }
        }
        if let Some(id) = output_gain.as_ref().and_then(|gain| gain.id.clone()) {
            self.targets
                .insert(id, Driven::Unsupported("a generic limiter's output gain"));
        }
    }

    /// The EQ's bands and Gain (R54).
    fn equaliser_values(&mut self, set: &mut Setting, device: &model::Device) {
        let DeviceBody::Equalizer {
            bands,
            input_gain,
            output_gain,
        } = &device.body
        else {
            return;
        };
        let place = set.place.clone();
        self.expect_zero_db(
            input_gain.as_ref(),
            "a generic equaliser's input gain",
            &place,
        );
        if let Some(gain) = output_gain {
            self.set_generic(set, &generic::EQ_GAIN, &Value::Real(gain));
        }
        let mut ordered: Vec<(usize, &model::EqBand)> = bands.iter().enumerate().collect();
        ordered.sort_by_key(|(index, band)| (band.order.unwrap_or(*index as i64), *index));
        if ordered.len() > generic::EQ_BANDS {
            self.report.add_count(
                "a generic equaliser's band beyond the eighth",
                &place,
                "left out",
                ordered.len() - generic::EQ_BANDS,
            );
        }
        for (slot, (_, band)) in ordered.into_iter().take(generic::EQ_BANDS).enumerate() {
            let Some(kind) = band.band_type.as_deref().and_then(generic::eq_band_type) else {
                self.report.add(
                    &format!(
                        "a band of type `{}`",
                        band.band_type.as_deref().unwrap_or("unknown")
                    ),
                    &place,
                    "switched off",
                );
                continue;
            };
            let on = band.enabled.as_ref().and_then(|e| e.value).unwrap_or(true);
            set.params.insert(
                generic::eq_param(slot, generic::EQ_ON),
                f64::from(u8::from(on)),
            );
            set.params
                .insert(generic::eq_param(slot, generic::EQ_TYPE), kind as f64);
            let cells = [
                (&band.frequency, generic::EQ_FREQ, Quantity::Hertz),
                (&band.gain, generic::EQ_GAIN_SLOT, Quantity::Decibels),
                (&band.q, generic::EQ_Q, Quantity::Plain),
            ];
            for (parameter, cell, quantity) in cells {
                let entry = Entry {
                    element: "",
                    param: generic::eq_param(slot, cell),
                    quantity,
                };
                if let Some(parameter) = parameter {
                    self.set_generic(set, &entry, &Value::Real(parameter));
                }
            }
        }
    }
}

/// Where a generic device's values go while it is made: its track and ID,
/// the device types that give each parameter its range, and the values set.
struct Setting {
    track: TrackId,
    device: DeviceId,
    device_type: &'static str,
    types: Arc<Vec<DeviceType>>,
    place: String,
    params: OrdMap<ParamId, f64>,
}

impl Setting {
    /// The range of values a parameter of the device holds.
    fn range(&self, param: ParamId) -> (f64, f64) {
        generic::range_of(&self.types, self.device_type, param).unwrap_or((f64::MIN, f64::MAX))
    }
}

/// A DAWproject device role as a chain role (R44).
fn role_of(role: model::DeviceRole) -> bl_model::DeviceRole {
    match role {
        model::DeviceRole::NoteFx => bl_model::DeviceRole::MidiEffect,
        model::DeviceRole::Instrument => bl_model::DeviceRole::Instrument,
        model::DeviceRole::AudioFx | model::DeviceRole::Analyzer => {
            bl_model::DeviceRole::AudioEffect
        }
    }
}

/// A gain parameter's value in decibels, or `None` where it has none or is
/// in a unit that isn't a level.
fn db_of(parameter: &RealParameter) -> Option<f64> {
    let value = parameter.value?;
    match parameter.unit.unwrap_or(model::Unit::Decibel) {
        model::Unit::Decibel => Some(value),
        model::Unit::Linear => Some(bl_model::param::gain_to_db(value)),
        _ => None,
    }
}

/// A ratio's value as a percent.
fn ratio_percent(parameter: Option<&RealParameter>, value: f64) -> f64 {
    match parameter.and_then(|p| p.unit) {
        Some(model::Unit::Percent) => value,
        _ => 100.0,
    }
}

/// The value of a generic device's element (R51 to R53).
fn value_of<'a>(body: &'a DeviceBody, element: &str) -> Option<Value<'a>> {
    match body {
        DeviceBody::Compressor {
            attack,
            auto_makeup,
            output_gain,
            ratio,
            release,
            threshold,
            ..
        } => match element {
            "Threshold" => threshold.as_ref().map(Value::Real),
            "Ratio" => ratio.as_ref().map(Value::Real),
            "Attack" => attack.as_ref().map(Value::Real),
            "Release" => release.as_ref().map(Value::Real),
            "OutputGain" => output_gain.as_ref().map(Value::Real),
            "AutoMakeup" => auto_makeup.as_ref().map(Value::Bool),
            _ => None,
        },
        DeviceBody::NoiseGate {
            attack,
            range,
            release,
            threshold,
            ..
        } => match element {
            "Threshold" => threshold.as_ref().map(Value::Real),
            "Attack" => attack.as_ref().map(Value::Real),
            "Release" => release.as_ref().map(Value::Real),
            "Range" => range.as_ref().map(Value::Real),
            _ => None,
        },
        DeviceBody::Limiter { release, .. } => match element {
            "Release" => release.as_ref().map(Value::Real),
            _ => None,
        },
        _ => None,
    }
}

/// A point of a generic device's automation as the parameter holds it: the
/// conversion of a static value (R55), with the decibel offset the limiter's
/// output gain adds (R53).
pub(super) fn native_value(
    quantity: Quantity,
    unit: Option<model::Unit>,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    range: (f64, f64),
    offset_db: f64,
) -> Option<f64> {
    if offset_db == 0.0 {
        return generic::convert(quantity, unit, value, min, max, range);
    }
    let db = match unit.unwrap_or(model::Unit::Decibel) {
        model::Unit::Decibel => value,
        model::Unit::Linear => bl_model::param::gain_to_db(value),
        _ => return None,
    } + offset_db;
    let converted = match quantity {
        Quantity::Gain => bl_model::param::db_to_gain(db),
        _ => db,
    };
    Some(converted.clamp(range.0, range.1))
}
