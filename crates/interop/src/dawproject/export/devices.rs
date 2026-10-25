//! The devices of an export (the DAWproject spec, R77, R78): plug-ins with
//! their states in each format, the four generic devices, and Beatlayer's
//! own devices and racks with their presets.

use bl_devices::{compressor, eq, gate, limiter};
use bl_model::{Device, DeviceKind, DeviceRole, ParamId, RackKind};
use bl_project::preset::Preset;
use bl_project::rack_preset::{RackPreset, SampleFile, Samples};

use super::automation::Conversion;
use super::*;
use crate::dawproject::generic::{self, Entry as Value, Quantity};
use crate::dawproject::model::{
    Device as DawDevice, DeviceBody, DeviceRole as FileRole, EqBand, FileReference, Parameter,
    PluginFormat as File,
};
use crate::dawproject::plugins::{self, Installed};

/// The vendor of Beatlayer's own devices (R78).
const VENDOR: &str = "Beatlayer";

impl Exporter<'_> {
    /// A channel's devices in their chain's order (R77, R78).
    pub(super) fn devices(
        &mut self,
        track: &Track,
        wanted: &BTreeSet<AutomationTarget>,
    ) -> Vec<DawDevice> {
        let mut out = Vec::new();
        for device in track.devices() {
            if let Some(written) = self.device(track, device, wanted) {
                out.push(written);
            }
        }
        out
    }

    fn device(
        &mut self,
        track: &Track,
        device: &Device,
        wanted: &BTreeSet<AutomationTarget>,
    ) -> Option<DawDevice> {
        let place = format!(
            "the device \"{}\" on {}",
            device.title(),
            Self::place(track)
        );
        for inner in device.walk() {
            let heard = inner.sidechain.as_ref().is_some_and(|sidechain| {
                sidechain.enabled || !matches!(sidechain.input, bl_model::Input::None)
            }) || inner
                .midi_sidechain
                .as_ref()
                .is_some_and(|sidechain| sidechain.source.is_some());
            if heard {
                self.report.add("a sidechain", &place, "not written");
            }
        }
        match &device.kind {
            DeviceKind::Plugin { format, plugin_id } => {
                self.plugin(track, device, *format, plugin_id, wanted, &place)
            }
            DeviceKind::Native { device_type } => {
                self.native(track, device, device_type, wanted, &place)
            }
            DeviceKind::Rack { kind } => self.rack(track, device, *kind, wanted, &place),
        }
    }

    /// The parts of a device every kind shares.
    fn base(device: &Device, id: String, body: DeviceBody) -> DawDevice {
        DawDevice {
            info: TimelineInfo {
                name: Some(device.name.clone()),
                ..TimelineInfo::default()
            },
            device_id: Some(id),
            device_name: Some(device.own_name().to_owned()),
            device_vendor: None,
            role: Some(match device.role {
                DeviceRole::MidiEffect => FileRole::NoteFx,
                DeviceRole::Instrument => FileRole::Instrument,
                DeviceRole::AudioEffect => FileRole::AudioFx,
            }),
            loaded: None,
            plugin_version: None,
            enabled: Some(BoolParameter {
                value: Some(device.enabled),
                ..BoolParameter::default()
            }),
            state: None,
            parameters: Vec::new(),
            body,
        }
    }

    /// Adds a state file to the container (R77).
    fn state(&mut self, extension: &str, bytes: Vec<u8>) -> FileReference {
        let name = format!("plugins/{}.{extension}", self.states.len() + 1);
        self.taken.insert(name.clone());
        self.states.push(Entry {
            name: name.clone(),
            data: Data::Bytes(bytes),
            deflate: false,
        });
        FileReference {
            path: name,
            external: false,
        }
    }

    /// A plug-in's parameters, loading it the first time (R76).
    fn described(&mut self, installed: &Installed) -> Described {
        let key = (installed.format, installed.plugin_id.clone());
        if let Some(found) = self.described.get(&key) {
            return found.clone();
        }
        let found = self.context.plugins.describe(installed).ok().map(Arc::new);
        self.described.insert(key, found.clone());
        found
    }

    /// A plug-in with its state and the parameters it automates (R77).
    fn plugin(
        &mut self,
        track: &Track,
        device: &Device,
        format: PluginFormat,
        plugin_id: &str,
        wanted: &BTreeSet<AutomationTarget>,
        place: &str,
    ) -> Option<DawDevice> {
        let (file_format, extension) = match format {
            PluginFormat::Clap => (File::Clap, "clap-preset"),
            PluginFormat::Vst3 => (File::Vst3, "vstpreset"),
            PluginFormat::AudioUnit => (File::Au, "aupreset"),
            PluginFormat::Lv2 => {
                self.report.add("an LV2 plug-in", place, "not written");
                return None;
            }
        };
        let Some(id) = plugins::plugin_id(format, plugin_id) else {
            self.report
                .add("a plug-in whose ID doesn't read", place, "not written");
            return None;
        };
        let installed = self
            .context
            .plugins
            .installed()
            .iter()
            .find(|plugin| plugin.format == format && plugin.plugin_id == plugin_id)
            .cloned();
        let data = device
            .plugin
            .as_ref()
            .map_or(&[][..], |state| state.data.as_slice());
        let state = if data.is_empty() {
            None
        } else {
            self.plugin_state(format, &id, data, place)
        };
        let mut written = Self::base(device, id, DeviceBody::Plugin(file_format));
        written.device_vendor = installed.as_ref().and_then(|plugin| plugin.vendor.clone());
        written.plugin_version = installed.as_ref().and_then(|plugin| plugin.version.clone());
        written.state = state.map(|bytes| self.state(extension, bytes));
        written.parameters =
            self.plugin_parameters(track, device, installed.as_ref(), wanted, place);
        Some(written)
    }

    /// A plug-in's state as its format's file holds it (R77).
    fn plugin_state(
        &mut self,
        format: PluginFormat,
        id: &str,
        data: &[u8],
        place: &str,
    ) -> Option<Vec<u8>> {
        match format {
            PluginFormat::Vst3 => {
                let Some((component, controller)) = bl_model::vst3_state::split_state(data) else {
                    self.report.add(
                        "a VST3 plug-in's state that isn't in the host's form",
                        place,
                        "not written",
                    );
                    return None;
                };
                Some(
                    bl_model::vst3_state::VstPreset {
                        class_id: id.replace('-', ""),
                        component: component.to_vec(),
                        controller: controller.to_vec(),
                    }
                    .write(),
                )
            }
            PluginFormat::AudioUnit => {
                let head = String::from_utf8_lossy(&data[..data.len().min(512)]);
                if data.starts_with(b"bplist") || head.contains("<plist") {
                    Some(data.to_vec())
                } else {
                    self.report.add(
                        "an Audio Unit's state that isn't a property list",
                        place,
                        "not written",
                    );
                    None
                }
            }
            _ => Some(data.to_vec()),
        }
    }

    /// The parameters of a plug-in its automation names, by the plug-in's
    /// own numbers (R76, R77).
    fn plugin_parameters(
        &mut self,
        track: &Track,
        device: &Device,
        installed: Option<&Installed>,
        wanted: &BTreeSet<AutomationTarget>,
        place: &str,
    ) -> Vec<Parameter> {
        let targets: Vec<(AutomationTarget, ParamId)> = wanted
            .iter()
            .filter_map(|target| match target {
                AutomationTarget::Device { device: id, param } if *id == device.id => {
                    Some((*target, *param))
                }
                _ => None,
            })
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }
        let described = installed.and_then(|plugin| self.described(plugin));
        let mut out = Vec::new();
        for (target, param) in targets {
            let key = (track.id(), target);
            let Some(descriptor) = described
                .as_ref()
                .and_then(|list| list.iter().find(|held| held.id == param))
            else {
                self.dropped.insert(key);
                self.report.add(
                    "an envelope of a plug-in parameter the machine can't describe",
                    place,
                    "not written",
                );
                continue;
            };
            let toggle = matches!(descriptor.kind, bl_model::ParamKind::Toggle);
            let id = self.parameter(
                track.id(),
                target,
                wanted,
                Conversion::Plugin(descriptor.kind.clone()),
            );
            let name = Some(descriptor.name.clone());
            let parameter_id = Some(i64::from(param.0));
            out.push(if toggle {
                Parameter::Bool(BoolParameter {
                    id,
                    name,
                    parameter_id,
                    value: None,
                })
            } else {
                Parameter::Real(RealParameter {
                    id,
                    name,
                    parameter_id,
                    unit: Some(Unit::Normalized),
                    min: Some(0.0),
                    max: Some(1.0),
                    value: None,
                })
            });
        }
        out
    }

    /// One of Beatlayer's own devices: the generic device it has, or a
    /// built-in device, with its preset as the state (R78).
    fn native(
        &mut self,
        track: &Track,
        device: &Device,
        device_type: &str,
        wanted: &BTreeSet<AutomationTarget>,
        place: &str,
    ) -> Option<DawDevice> {
        let generic =
            [compressor::COMPRESSOR, gate::GATE, limiter::LIMITER, eq::EQ].contains(&device_type);
        let body = if generic {
            self.generic_body(track, device, device_type, wanted, place)
        } else {
            self.report.add(
                &format!("the device {}", device.own_name()),
                place,
                "written with its Beatlayer preset; other applications leave it out",
            );
            DeviceBody::Builtin
        };
        let mut written = Self::base(device, device_type.to_owned(), body);
        written.device_vendor = Some(VENDOR.to_owned());
        written.state = Some(self.state("blpreset", Preset::of(device).encode()));
        if !generic {
            written.parameters = self.native_parameters(track, device, Some(device_type), wanted);
        }
        Some(written)
    }

    /// A rack with everything in it and the samples it plays, as a preset
    /// (R78).
    fn rack(
        &mut self,
        track: &Track,
        device: &Device,
        kind: RackKind,
        wanted: &BTreeSet<AutomationTarget>,
        place: &str,
    ) -> Option<DawDevice> {
        let mut samples = Samples::new();
        for hash in device.media_used() {
            let loaded = self
                .context
                .media
                .path(hash)
                .and_then(|path| Some((std::fs::read(&path).ok()?, path)));
            let Some((data, path)) = loaded else {
                self.report
                    .add("a rack whose sample is missing", place, "left out");
                return None;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            samples.insert(hash, SampleFile { name, data });
        }
        let preset = RackPreset::new(device.clone(), samples).ok()?;
        self.report.add(
            &format!("the {}", device.own_name()),
            place,
            "written with its Beatlayer preset; other applications leave it out",
        );
        let mut written = Self::base(device, kind.name().to_owned(), DeviceBody::Builtin);
        written.device_vendor = Some(VENDOR.to_owned());
        written.state = Some(self.state("blpreset", preset.encode()));
        written.parameters = self.native_parameters(track, device, None, wanted);
        Some(written)
    }

    /// The parameters of a built-in device its automation names, in its own
    /// units (R76).
    fn native_parameters(
        &mut self,
        track: &Track,
        device: &Device,
        device_type: Option<&str>,
        wanted: &BTreeSet<AutomationTarget>,
    ) -> Vec<Parameter> {
        let mut out = Vec::new();
        for target in wanted {
            let AutomationTarget::Device { device: id, param } = target else {
                continue;
            };
            if *id != device.id {
                continue;
            }
            let descriptor = device_type.and_then(|name| {
                self.types
                    .iter()
                    .find(|t| matches!(&t.kind, DeviceKind::Native { device_type: held } if held == name))
                    .and_then(|t| t.description.param(*param))
                    .cloned()
            });
            let toggle = descriptor
                .as_ref()
                .is_some_and(|d| matches!(d.kind, bl_model::ParamKind::Toggle));
            let range = device_type.and_then(|name| generic::range_of(&self.types, name, *param));
            let quantity = if toggle {
                Quantity::Toggle
            } else {
                Quantity::Plain
            };
            let id = self.parameter(
                track.id(),
                *target,
                wanted,
                Conversion::Native { quantity, range },
            );
            let name = descriptor.map(|d| d.name);
            let parameter_id = Some(i64::from(param.0));
            out.push(if toggle {
                Parameter::Bool(BoolParameter {
                    id,
                    name,
                    parameter_id,
                    value: None,
                })
            } else {
                Parameter::Real(RealParameter {
                    id,
                    name,
                    parameter_id,
                    unit: Some(Unit::Linear),
                    ..RealParameter::default()
                })
            });
        }
        out
    }

    /// A parameter's value, set or default.
    fn value_of(&self, device: &Device, device_type: &str, param: ParamId) -> f64 {
        self.types
            .iter()
            .find(|t| matches!(&t.kind, DeviceKind::Native { device_type: held } if held == device_type))
            .and_then(|t| t.description.value(&device.params, param))
            .unwrap_or(0.0)
    }

    /// One value of a generic device, in the unit the file gives it (R78).
    fn generic_real(
        &mut self,
        track: &Track,
        device: &Device,
        device_type: &str,
        value: &Value,
        wanted: &BTreeSet<AutomationTarget>,
    ) -> RealParameter {
        let range = generic::range_of(&self.types, device_type, value.param)
            .unwrap_or((f64::MIN, f64::MAX));
        let native = self.value_of(device, device_type, value.param);
        let (written, unit) = generic::to_file(value.quantity, native, range);
        let id = self.parameter(
            track.id(),
            AutomationTarget::Device {
                device: device.id,
                param: value.param,
            },
            wanted,
            Conversion::Native {
                quantity: value.quantity,
                range: Some(range),
            },
        );
        RealParameter {
            id,
            unit: Some(unit),
            value: Some(written),
            ..RealParameter::default()
        }
    }

    /// A switch of a generic device.
    fn generic_bool(
        &mut self,
        track: &Track,
        device: &Device,
        device_type: &str,
        param: ParamId,
        wanted: &BTreeSet<AutomationTarget>,
    ) -> BoolParameter {
        let id = self.parameter(
            track.id(),
            AutomationTarget::Device {
                device: device.id,
                param,
            },
            wanted,
            Conversion::Native {
                quantity: Quantity::Toggle,
                range: Some((0.0, 1.0)),
            },
        );
        BoolParameter {
            id,
            value: Some(self.value_of(device, device_type, param) > 0.5),
            ..BoolParameter::default()
        }
    }

    /// The generic device a Compressor, Gate, Limiter or EQ writes as, with
    /// its values mapped as R51 to R54 read them (R78).
    fn generic_body(
        &mut self,
        track: &Track,
        device: &Device,
        device_type: &str,
        wanted: &BTreeSet<AutomationTarget>,
        place: &str,
    ) -> DeviceBody {
        let real = |this: &mut Self, value: &Value| {
            Some(this.generic_real(track, device, device_type, value, wanted))
        };
        let find = |entries: &'static [Value], element: &str| {
            entries.iter().find(|entry| entry.element == element)
        };
        match device_type {
            compressor::COMPRESSOR => {
                let entry = |element| find(generic::COMPRESSOR, element);
                DeviceBody::Compressor {
                    attack: entry("Attack").and_then(|e| real(self, e)),
                    auto_makeup: Some(self.generic_bool(
                        track,
                        device,
                        device_type,
                        compressor::MAKEUP,
                        wanted,
                    )),
                    input_gain: None,
                    output_gain: entry("OutputGain").and_then(|e| real(self, e)),
                    ratio: entry("Ratio").and_then(|e| real(self, e)),
                    release: entry("Release").and_then(|e| real(self, e)),
                    threshold: entry("Threshold").and_then(|e| real(self, e)),
                }
            }
            gate::GATE => {
                let entry = |element| find(generic::GATE, element);
                DeviceBody::NoiseGate {
                    attack: entry("Attack").and_then(|e| real(self, e)),
                    range: entry("Range").and_then(|e| real(self, e)),
                    ratio: None,
                    release: entry("Release").and_then(|e| real(self, e)),
                    threshold: entry("Threshold").and_then(|e| real(self, e)),
                }
            }
            limiter::LIMITER => DeviceBody::Limiter {
                attack: None,
                input_gain: real(self, &generic::LIMITER_GAIN),
                // The Limiter has no output gain apart from its Gain and
                // Ceiling, which the input gain and threshold carry whole.
                output_gain: Some(RealParameter {
                    unit: Some(Unit::Decibel),
                    value: Some(0.0),
                    ..RealParameter::default()
                }),
                release: find(generic::LIMITER, "Release").and_then(|e| real(self, e)),
                threshold: real(self, &generic::LIMITER_CEILING),
            },
            _ => self.equaliser_body(track, device, wanted, place),
        }
    }

    /// The EQ's first curve as a generic equaliser's bands (R78).
    fn equaliser_body(
        &mut self,
        track: &Track,
        device: &Device,
        wanted: &BTreeSet<AutomationTarget>,
        place: &str,
    ) -> DeviceBody {
        let output = self.generic_real(track, device, eq::EQ, &generic::EQ_GAIN, wanted);
        let mut bands = Vec::new();
        for band in 0..generic::EQ_BANDS {
            let on = self.value_of(device, eq::EQ, generic::eq_param(band, generic::EQ_ON)) > 0.5;
            let kind = self.value_of(device, eq::EQ, generic::eq_param(band, generic::EQ_TYPE));
            let (name, steeper) =
                generic::eq_band_name(kind.round() as usize).unwrap_or(("bell", false));
            if on && steeper {
                self.report.add(
                    "a Low Cut or High Cut at 48 dB per octave",
                    place,
                    "written as a cut at 12 dB per octave",
                );
            }
            let cell = |this: &mut Self, slot: usize, quantity| {
                let value = Value {
                    element: "",
                    param: generic::eq_param(band, slot),
                    quantity,
                };
                this.generic_real(track, device, eq::EQ, &value, wanted)
            };
            let frequency = cell(self, generic::EQ_FREQ, Quantity::Hertz);
            let gain = cell(self, generic::EQ_GAIN_SLOT, Quantity::Decibels);
            let q = cell(self, generic::EQ_Q, Quantity::Plain);
            let enabled = self.generic_bool(
                track,
                device,
                eq::EQ,
                generic::eq_param(band, generic::EQ_ON),
                wanted,
            );
            debug_assert_eq!(enabled.value, Some(on));
            bands.push(EqBand {
                band_type: Some(name.to_owned()),
                order: Some(band as i64),
                frequency: Some(frequency),
                gain: Some(gain),
                q: Some(q),
                enabled: Some(enabled),
            });
        }
        DeviceBody::Equalizer {
            bands,
            input_gain: None,
            output_gain: Some(output),
        }
    }
}
