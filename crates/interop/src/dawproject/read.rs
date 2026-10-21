//! Reading a DAWproject file's XML (the DAWproject spec, R2, R4, R6, R7 and
//! R8): the text of `project.xml` into the elements the schema has, with
//! what Beatlayer doesn't know counted and what it can't read left out.
//!
//! Nothing here panics on a damaged or foreign file: the reader takes what
//! it can and says what it left out.

use quick_xml::Reader;
use quick_xml::events::Event;

use super::model::{
    Application, BoolParameter, Channel, Clip, ClipSlot, Clips, Content, ContentKind, Device,
    DeviceRole, EqBand, Expression, FileReference, Interpolation, Lanes, Marker, Markers,
    MediaFile, MixerRole, Note, Parameter, PluginFormat, Points, Project, RealParameter, Send,
    Structure, Target, TimeSignatureParameter, TimeUnit, Timeline, TimelineInfo, Track, Transport,
    Unit, Warp, Warps,
};

/// How deep elements may nest before the reader leaves the deeper ones out
/// (R6).
pub const MAX_DEPTH: usize = 256;

/// An element of the file, as it was read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// The element's name, as the file spells it.
    pub name: String,
    /// Its attributes, in the file's order.
    pub attributes: Vec<(String, String)>,
    /// Its child elements, in the file's order.
    pub children: Vec<Node>,
    /// Its text.
    pub text: String,
}

impl Node {
    /// The node's attribute, if it has one.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The node's first child with the given name.
    #[must_use]
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The node's children with the given name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |child| child.name == name)
    }
}

/// What the reader found it couldn't use (R4, R6, R7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    /// Elements Beatlayer doesn't know, with how many of each.
    pub unknown_elements: Vec<(String, usize)>,
    /// Attributes Beatlayer doesn't know, with how many of each.
    pub unknown_attributes: Vec<(String, usize)>,
    /// Entities the file defines that Beatlayer doesn't expand (R6).
    pub unknown_entities: Vec<(String, usize)>,
    /// Elements left out for a missing attribute, with what they held.
    pub left_out: Vec<String>,
    /// Elements whose `id` was already used (R7).
    pub duplicate_ids: Vec<String>,
    /// The `id`s read so far, to find a second use of one (R7).
    ids: std::collections::HashSet<String>,
    /// Elements nested deeper than the limit (R6).
    pub too_deep: Vec<String>,
}

impl ReadReport {
    /// Counts one more of a name.
    fn count(list: &mut Vec<(String, usize)>, name: &str) {
        if let Some(entry) = list.iter_mut().find(|(known, _)| known == name) {
            entry.1 += 1;
        } else {
            list.push((name.to_owned(), 1));
        }
    }
}

/// The attributes every timeline carries.
const TIMELINE: &[&str] = &["id", "name", "color", "comment", "timeUnit", "track"];
/// The attributes an element may have, or `None` when Beatlayer doesn't know
/// the element at all (R4).
#[must_use]
pub fn allowed_attributes(element: &str) -> Option<&'static [&'static str]> {
    Some(match element {
        "Project" => &["version"],
        "Application" => &["name", "version"],
        "Transport" | "Structure" | "Scenes" | "Devices" | "Parameters" | "Sends" => &[],
        "Track" => &[
            "id",
            "name",
            "color",
            "comment",
            "contentType",
            "loaded",
            "timeUnit",
            "track",
        ],
        "Channel" => &[
            "id",
            "name",
            "color",
            "comment",
            "role",
            "destination",
            "audioChannels",
            "solo",
            "timeUnit",
            "track",
        ],
        "Device" | "BuiltinDevice" => &[
            "id",
            "name",
            "color",
            "comment",
            "deviceID",
            "deviceName",
            "deviceVendor",
            "deviceRole",
            "loaded",
        ],
        "Vst2Plugin" | "Vst3Plugin" | "ClapPlugin" | "AuPlugin" => &[
            "id",
            "name",
            "color",
            "comment",
            "deviceID",
            "deviceName",
            "deviceVendor",
            "deviceRole",
            "loaded",
            "pluginVersion",
        ],
        "Equalizer" | "Compressor" | "NoiseGate" | "Limiter" => &[
            "id",
            "name",
            "color",
            "comment",
            "deviceID",
            "deviceName",
            "deviceVendor",
            "deviceRole",
            "loaded",
        ],
        "RealParameter" | "Tempo" | "Pan" | "Volume" | "InputGain" | "OutputGain" | "Attack"
        | "Ratio" | "Release" | "Threshold" | "Range" | "Freq" | "Gain" | "Q" => &[
            "id",
            "name",
            "color",
            "comment",
            "parameterID",
            "unit",
            "value",
            "min",
            "max",
        ],
        "BoolParameter" | "Mute" | "Enabled" | "AutoMakeup" | "Enable" => {
            &["id", "name", "color", "comment", "parameterID", "value"]
        }
        "IntegerParameter" => &[
            "id",
            "name",
            "color",
            "comment",
            "parameterID",
            "value",
            "min",
            "max",
        ],
        "EnumParameter" => &[
            "id",
            "name",
            "color",
            "comment",
            "parameterID",
            "value",
            "count",
            "labels",
        ],
        "TimeSignatureParameter" | "TimeSignature" => &[
            "id",
            "name",
            "color",
            "comment",
            "parameterID",
            "numerator",
            "denominator",
        ],
        "State" | "File" => &["path", "external"],
        "Send" => &["id", "name", "color", "comment", "destination", "type"],
        "Arrangement" => TIMELINE,
        "Lanes" => TIMELINE,
        "Markers" => TIMELINE,
        "Marker" => &["id", "name", "color", "comment", "time"],
        "Clips" => TIMELINE,
        "Clip" => &[
            "id",
            "name",
            "color",
            "comment",
            "time",
            "duration",
            "contentTimeUnit",
            "playStart",
            "playStop",
            "loopStart",
            "loopEnd",
            "fadeTimeUnit",
            "fadeInTime",
            "fadeOutTime",
            "enable",
            "reference",
        ],
        "Notes" => TIMELINE,
        "Note" => &[
            "id", "name", "color", "comment", "time", "duration", "channel", "key", "vel", "rel",
        ],
        "ClipSlot" => &[
            "id", "name", "color", "comment", "timeUnit", "track", "hasStop",
        ],
        "Warps" => &[
            "id",
            "name",
            "color",
            "comment",
            "timeUnit",
            "track",
            "contentTimeUnit",
        ],
        "Warp" => &["time", "contentTime"],
        "Audio" | "Video" => &[
            "id",
            "name",
            "color",
            "comment",
            "timeUnit",
            "track",
            "duration",
            "channels",
            "sampleRate",
            "algorithm",
        ],
        "Points" | "TempoAutomation" | "TimeSignatureAutomation" => &[
            "id", "name", "color", "comment", "timeUnit", "track", "unit",
        ],
        "Target" => &["parameter", "expression", "channel", "key", "controller"],
        "RealPoint" => &["time", "value", "interpolation"],
        "Point" | "BoolPoint" | "IntegerPoint" | "EnumPoint" => &["time", "value"],
        "TimeSignaturePoint" => &["time", "numerator", "denominator"],
        "Scene" => TIMELINE,
        "Band" => &["type", "order"],
        _ => return None,
    })
}

/// Reads the text of a DAWproject XML part into its elements.
///
/// The text is read as far as it can be: a file that breaks off leaves what
/// came before it (R4).
#[must_use]
pub fn read_xml(text: &str, report: &mut ReadReport) -> Option<Node> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_text(false);

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    // How many elements deep the reader is inside a subtree it leaves out.
    let mut skipped = 0usize;

    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            // A file that breaks off leaves what came before it.
            Err(_) => break,
        };
        match event {
            Event::Start(element) => {
                let node = node_of(&element, report);
                if skipped > 0 {
                    skipped += 1;
                } else if stack.len() >= MAX_DEPTH {
                    report.too_deep.push(node.name.clone());
                    skipped = 1;
                } else {
                    stack.push(node);
                }
            }
            Event::Empty(element) => {
                let node = node_of(&element, report);
                if skipped > 0 {
                    continue;
                }
                if stack.len() >= MAX_DEPTH {
                    report.too_deep.push(node.name.clone());
                    continue;
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Event::End(_) => {
                if skipped > 0 {
                    skipped -= 1;
                    continue;
                }
                let Some(node) = stack.pop() else {
                    continue;
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Event::Text(text) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&String::from_utf8_lossy(text.as_ref()));
                }
            }
            Event::CData(text) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&String::from_utf8_lossy(text.as_ref()));
                }
            }
            Event::GeneralRef(reference) => {
                let name = reference.decode().unwrap_or_default().into_owned();
                let resolved = resolve_entity(&name);
                if let Some(text) = resolved {
                    if let Some(node) = stack.last_mut() {
                        node.text.push_str(&text);
                    }
                } else {
                    ReadReport::count(&mut report.unknown_entities, &name);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root
}

/// The element's node, with its attributes.
fn node_of(element: &quick_xml::events::BytesStart<'_>, report: &mut ReadReport) -> Node {
    let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
    let allowed = allowed_attributes(&name);
    if allowed.is_none() {
        // An element Beatlayer doesn't know is skipped and counted (R4).
        ReadReport::count(&mut report.unknown_elements, &name);
    }
    let mut node = Node {
        name: name.clone(),
        ..Node::default()
    };
    for attribute in element.attributes().with_checks(false).flatten() {
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let value = unescape(&String::from_utf8_lossy(attribute.value.as_ref()));
        match allowed {
            Some(allowed) if !allowed.contains(&key.as_str()) => {
                ReadReport::count(&mut report.unknown_attributes, &key);
            }
            None => ReadReport::count(&mut report.unknown_attributes, &key),
            Some(_) => {}
        }
        if allowed.is_some() {
            // Every `id` names one element; a second use is read without it
            // (R7).
            if key == "id" && !report.ids.insert(value.clone()) {
                report.duplicate_ids.push(value);
                continue;
            }
            node.attributes.push((key, value));
        }
    }
    node
}

/// The text an attribute holds, with the five entities XML defines and the
/// character references expanded, and any other entity left as it was
/// written (R6).
fn unescape(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let Some(end) = rest.find(';') else {
            out.push('&');
            break;
        };
        let name = &rest[..end];
        match resolve_entity(name) {
            Some(text) => out.push_str(&text),
            None => {
                out.push('&');
                out.push_str(name);
                out.push(';');
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The text an entity reference stands for, or `None` for one Beatlayer
/// doesn't expand (R6).
fn resolve_entity(name: &str) -> Option<String> {
    match name {
        "amp" => Some("&".to_owned()),
        "lt" => Some("<".to_owned()),
        "gt" => Some(">".to_owned()),
        "apos" => Some("'".to_owned()),
        "quot" => Some("\"".to_owned()),
        other => {
            let digits = other.strip_prefix('#')?;
            let code = if let Some(hex) = digits.strip_prefix('x').or(digits.strip_prefix('X')) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                digits.parse::<u32>().ok()?
            };
            char::from_u32(code).map(String::from)
        }
    }
}

/// A number the file gives, or `None` when it doesn't read (R8).
fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    match text {
        "inf" | "+inf" | "Infinity" => return Some(f64::INFINITY),
        "-inf" | "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let value = text.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// A real value, which may be infinite (R8).
fn real(text: &str) -> Option<f64> {
    let text = text.trim();
    match text {
        "inf" | "+inf" | "Infinity" => return Some(f64::INFINITY),
        "-inf" | "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    text.parse::<f64>().ok()
}

/// A boolean the file gives: `true`, `false`, `1` or `0` (R8).
fn boolean(text: &str) -> Option<bool> {
    match text.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// A whole number the file gives.
fn integer(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok()
}

/// The timeline's own information.
fn timeline_info(node: &Node, report: &mut ReadReport) -> TimelineInfo {
    TimelineInfo {
        id: node.attribute("id").map(str::to_owned),
        name: node.attribute("name").map(str::to_owned),
        color: node.attribute("color").map(str::to_owned),
        comment: node.attribute("comment").map(str::to_owned),
        time_unit: node.attribute("timeUnit").and_then(|unit| {
            let parsed = TimeUnit::parse(unit);
            if parsed.is_none() {
                ReadReport::count(&mut report.unknown_attributes, "timeUnit");
            }
            parsed
        }),
        track: node.attribute("track").map(str::to_owned),
    }
}

/// A real parameter.
fn real_parameter(node: &Node) -> RealParameter {
    RealParameter {
        id: node.attribute("id").map(str::to_owned),
        name: node.attribute("name").map(str::to_owned),
        unit: node.attribute("unit").and_then(Unit::parse),
        value: node.attribute("value").and_then(real),
        min: node.attribute("min").and_then(real),
        max: node.attribute("max").and_then(real),
    }
}

/// A boolean parameter.
fn bool_parameter(node: &Node) -> BoolParameter {
    BoolParameter {
        id: node.attribute("id").map(str::to_owned),
        name: node.attribute("name").map(str::to_owned),
        value: node.attribute("value").and_then(boolean),
    }
}

/// A time signature parameter.
fn signature_parameter(node: &Node) -> TimeSignatureParameter {
    TimeSignatureParameter {
        id: node.attribute("id").map(str::to_owned),
        name: node.attribute("name").map(str::to_owned),
        numerator: node.attribute("numerator").and_then(|n| n.parse().ok()),
        denominator: node.attribute("denominator").and_then(|n| n.parse().ok()),
    }
}

/// A file the project holds.
fn file_reference(node: &Node) -> FileReference {
    FileReference {
        path: node.attribute("path").unwrap_or_default().to_owned(),
        external: node
            .attribute("external")
            .and_then(boolean)
            .unwrap_or(false),
    }
}

/// The parameters a device holds.
fn parameters(node: &Node) -> Vec<Parameter> {
    node.child("Parameters")
        .map(|parameters| {
            parameters
                .children
                .iter()
                .filter_map(|child| match child.name.as_str() {
                    "RealParameter" => Some(Parameter::Real(real_parameter(child))),
                    "BoolParameter" => Some(Parameter::Bool(bool_parameter(child))),
                    "IntegerParameter" => Some(Parameter::Integer {
                        id: child.attribute("id").map(str::to_owned),
                        name: child.attribute("name").map(str::to_owned),
                        value: child.attribute("value").and_then(integer),
                        min: child.attribute("min").and_then(integer),
                        max: child.attribute("max").and_then(integer),
                    }),
                    "EnumParameter" => Some(Parameter::Enumerated {
                        id: child.attribute("id").map(str::to_owned),
                        name: child.attribute("name").map(str::to_owned),
                        value: child.attribute("value").and_then(integer),
                        count: child.attribute("count").and_then(integer),
                        labels: child
                            .attribute("labels")
                            .map(|labels| labels.split(',').map(str::to_owned).collect())
                            .unwrap_or_default(),
                    }),
                    "TimeSignatureParameter" => {
                        Some(Parameter::Signature(signature_parameter(child)))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A device in a channel's chain.
fn device(node: &Node, report: &mut ReadReport) -> Option<Device> {
    let info = timeline_info(node, report);
    let device_id = node.attribute("deviceID").map(str::to_owned);
    let device_name = node.attribute("deviceName").map(str::to_owned);
    let device_vendor = node.attribute("deviceVendor").map(str::to_owned);
    let role = node.attribute("deviceRole").and_then(DeviceRole::parse);
    let loaded = node.attribute("loaded").and_then(boolean);
    let state = node.child("State").map(file_reference);
    let format = match node.name.as_str() {
        "Vst2Plugin" => Some(PluginFormat::Vst2),
        "Vst3Plugin" => Some(PluginFormat::Vst3),
        "ClapPlugin" => Some(PluginFormat::Clap),
        "AuPlugin" => Some(PluginFormat::Au),
        _ => None,
    };
    if let Some(format) = format {
        return Some(Device::Plugin {
            info,
            format,
            device_id,
            device_name,
            device_vendor,
            role,
            loaded,
            state,
            parameters: parameters(node),
        });
    }
    match node.name.as_str() {
        "Device" | "BuiltinDevice" => Some(Device::Builtin {
            info,
            device_id,
            device_name,
            device_vendor,
            role,
            loaded,
            state,
            parameters: parameters(node),
        }),
        "Equalizer" => Some(Device::Equalizer {
            info,
            bands: node
                .children_named("Band")
                .map(|band| EqBand {
                    band_type: band.attribute("type").map(str::to_owned),
                    order: band.attribute("order").and_then(integer),
                    frequency: band.child("Freq").map(real_parameter),
                    gain: band.child("Gain").map(real_parameter),
                    q: band.child("Q").map(real_parameter),
                    enabled: band.child("Enabled").map(bool_parameter),
                })
                .collect(),
            input_gain: node.child("InputGain").map(real_parameter),
            output_gain: node.child("OutputGain").map(real_parameter),
        }),
        "Compressor" => Some(Device::Compressor {
            info,
            attack: node.child("Attack").map(real_parameter),
            auto_makeup: node.child("AutoMakeup").map(bool_parameter),
            input_gain: node.child("InputGain").map(real_parameter),
            output_gain: node.child("OutputGain").map(real_parameter),
            ratio: node.child("Ratio").map(real_parameter),
            release: node.child("Release").map(real_parameter),
            threshold: node.child("Threshold").map(real_parameter),
        }),
        "NoiseGate" => Some(Device::NoiseGate {
            info,
            attack: node.child("Attack").map(real_parameter),
            range: node.child("Range").map(real_parameter),
            ratio: node.child("Ratio").map(real_parameter),
            release: node.child("Release").map(real_parameter),
            threshold: node.child("Threshold").map(real_parameter),
        }),
        "Limiter" => Some(Device::Limiter {
            info,
            attack: node.child("Attack").map(real_parameter),
            input_gain: node.child("InputGain").map(real_parameter),
            output_gain: node.child("OutputGain").map(real_parameter),
            release: node.child("Release").map(real_parameter),
            threshold: node.child("Threshold").map(real_parameter),
        }),
        _ => None,
    }
}

/// A channel.
fn channel(node: &Node, report: &mut ReadReport) -> Channel {
    Channel {
        info: timeline_info(node, report),
        role: node.attribute("role").and_then(MixerRole::parse),
        destination: node.attribute("destination").map(str::to_owned),
        audio_channels: node.attribute("audioChannels").and_then(integer),
        solo: node.attribute("solo").and_then(boolean),
        mute: node.child("Mute").map(bool_parameter),
        pan: node.child("Pan").map(real_parameter),
        volume: node.child("Volume").map(real_parameter),
        sends: node
            .child("Sends")
            .map(|sends| {
                sends
                    .children_named("Send")
                    .map(|send| Send {
                        id: send.attribute("id").map(str::to_owned),
                        destination: send.attribute("destination").map(str::to_owned),
                        send_type: send.attribute("type").map(str::to_owned),
                        enable: send.child("Enable").map(bool_parameter),
                        pan: send.child("Pan").map(real_parameter),
                        volume: send.child("Volume").map(real_parameter),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        devices: node
            .child("Devices")
            .map(|devices| {
                devices
                    .children
                    .iter()
                    .filter_map(|child| device(child, report))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A track.
fn track(node: &Node, report: &mut ReadReport) -> Track {
    Track {
        info: timeline_info(node, report),
        content_types: node
            .attribute("contentType")
            .map(|types| {
                types
                    .split_whitespace()
                    .filter_map(|kind| {
                        ContentKind::parse(kind).or_else(|| {
                            ReadReport::count(&mut report.unknown_attributes, "contentType");
                            None
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        loaded: node.attribute("loaded").and_then(boolean),
        channel: node.child("Channel").map(|child| channel(child, report)),
        children: node
            .children_named("Track")
            .map(|child| track(child, report))
            .collect(),
    }
}

/// A clip.
fn clip(node: &Node, report: &mut ReadReport) -> Clip {
    let content = node
        .children
        .first()
        .and_then(|child| content_of(child, report))
        .map(Box::new);
    Clip {
        info: timeline_info(node, report),
        time: node.attribute("time").and_then(number),
        duration: node.attribute("duration").and_then(number),
        content_time_unit: node.attribute("contentTimeUnit").and_then(TimeUnit::parse),
        play_start: node.attribute("playStart").and_then(number),
        play_stop: node.attribute("playStop").and_then(number),
        loop_start: node.attribute("loopStart").and_then(number),
        loop_end: node.attribute("loopEnd").and_then(number),
        fade_time_unit: node.attribute("fadeTimeUnit").and_then(TimeUnit::parse),
        fade_in: node.attribute("fadeInTime").and_then(number),
        fade_out: node.attribute("fadeOutTime").and_then(number),
        enable: node.attribute("enable").and_then(boolean),
        reference: node.attribute("reference").map(str::to_owned),
        content,
    }
}

/// What a clip, a note or a scene holds.
fn content_of(node: &Node, report: &mut ReadReport) -> Option<Content> {
    timeline(node, report).map(|timeline| Content::Timeline(Box::new(timeline)))
}

/// A timeline: one of the schema's timeline elements.
fn timeline(node: &Node, report: &mut ReadReport) -> Option<Timeline> {
    let info = timeline_info(node, report);
    Some(match node.name.as_str() {
        "Clips" => Timeline::Clips(Clips {
            info,
            clips: node
                .children_named("Clip")
                .map(|child| clip(child, report))
                .collect(),
        }),
        "Notes" => Timeline::Notes(super::model::Notes {
            info,
            notes: node
                .children_named("Note")
                .map(|child| note(child, report))
                .collect(),
        }),
        "ClipSlot" => Timeline::ClipSlot(ClipSlot {
            info,
            has_stop: node.attribute("hasStop").and_then(boolean),
            clip: node
                .child("Clip")
                .map(|child| clip(child, report))
                .map(Box::new),
        }),
        "Warps" => Timeline::Warps(Warps {
            info,
            content_time_unit: node.attribute("contentTimeUnit").and_then(TimeUnit::parse),
            audio: node.child("Audio").map(|child| media_file(child, report)),
            warps: node
                .children_named("Warp")
                .map(|child| Warp {
                    time: child.attribute("time").and_then(number),
                    content_time: child.attribute("contentTime").and_then(number),
                })
                .collect(),
        }),
        "Audio" => Timeline::Audio(media_file(node, report)),
        "Video" => Timeline::Video(media_file(node, report)),
        "Markers" => Timeline::Markers(Markers {
            info,
            markers: node
                .children_named("Marker")
                .map(|child| Marker {
                    info: timeline_info(child, report),
                    time: child.attribute("time").and_then(number),
                })
                .collect(),
        }),
        "Points" => Timeline::Points(points(node, report)),
        "Lanes" => Timeline::Lanes(Lanes {
            info,
            children: node
                .children
                .iter()
                .filter_map(|child| timeline(child, report))
                .collect(),
        }),
        _ => return None,
    })
}

/// A note.
fn note(node: &Node, report: &mut ReadReport) -> Note {
    Note {
        time: node.attribute("time").and_then(number),
        duration: node.attribute("duration").and_then(number),
        channel: node.attribute("channel").and_then(integer),
        key: node.attribute("key").and_then(integer),
        vel: node.attribute("vel").and_then(real),
        rel: node.attribute("rel").and_then(real),
        content: node
            .children
            .first()
            .and_then(|child| content_of(child, report))
            .map(Box::new),
    }
}

/// An audio or video file.
fn media_file(node: &Node, report: &mut ReadReport) -> MediaFile {
    MediaFile {
        info: timeline_info(node, report),
        duration: node.attribute("duration").and_then(number),
        channels: node.attribute("channels").and_then(integer),
        sample_rate: node.attribute("sampleRate").and_then(integer),
        algorithm: node.attribute("algorithm").map(str::to_owned),
        file: node.child("File").map(file_reference),
    }
}

/// A set of automation points.
fn points(node: &Node, report: &mut ReadReport) -> Points {
    let target_node = node.child("Target");
    let target = Target {
        parameter: target_node
            .and_then(|t| t.attribute("parameter"))
            .map(str::to_owned),
        expression: target_node
            .and_then(|t| t.attribute("expression"))
            .and_then(Expression::parse),
        channel: target_node
            .and_then(|t| t.attribute("channel"))
            .and_then(integer),
        key: target_node
            .and_then(|t| t.attribute("key"))
            .and_then(integer),
        controller: target_node
            .and_then(|t| t.attribute("controller"))
            .and_then(integer),
    };
    Points {
        info: timeline_info(node, report),
        unit: node.attribute("unit").and_then(Unit::parse),
        target,
        points: node
            .children
            .iter()
            .filter_map(|child| point(child))
            .collect(),
    }
}

/// One automation point.
fn point(node: &Node) -> Option<super::model::PointValue> {
    let time = node.attribute("time").and_then(number);
    Some(match node.name.as_str() {
        "RealPoint" => super::model::PointValue::Real {
            time,
            value: node.attribute("value").and_then(real),
            interpolation: node
                .attribute("interpolation")
                .and_then(Interpolation::parse),
        },
        "BoolPoint" => super::model::PointValue::Bool {
            time,
            value: node.attribute("value").and_then(boolean),
        },
        "IntegerPoint" => super::model::PointValue::Integer {
            time,
            value: node.attribute("value").and_then(integer),
        },
        "EnumPoint" => super::model::PointValue::Enumerated {
            time,
            value: node.attribute("value").and_then(integer),
        },
        "TimeSignaturePoint" => super::model::PointValue::Signature {
            time,
            numerator: node.attribute("numerator").and_then(integer),
            denominator: node.attribute("denominator").and_then(integer),
        },
        _ => return None,
    })
}

/// The project the file holds.
#[must_use]
pub fn project(node: &Node, report: &mut ReadReport) -> Option<Project> {
    if node.name != "Project" {
        return None;
    }
    let structure = node
        .child("Structure")
        .map(|structure| {
            structure
                .children
                .iter()
                .filter_map(|child| match child.name.as_str() {
                    "Track" => Some(Structure::Track(track(child, report))),
                    "Channel" => Some(Structure::Channel(channel(child, report))),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let arrangement = node
        .child("Arrangement")
        .map(|arrangement| super::model::Arrangement {
            info: timeline_info(arrangement, report),
            lanes: arrangement
                .child("Lanes")
                .map(|lanes| {
                    lanes
                        .children
                        .iter()
                        .filter_map(|child| timeline(child, report))
                        .collect()
                })
                .unwrap_or_default(),
            tempo_automation: arrangement
                .child("TempoAutomation")
                .map(|points| points_of(points, report)),
            signature_automation: arrangement
                .child("TimeSignatureAutomation")
                .map(|points| points_of(points, report)),
        });
    let scenes = node
        .child("Scenes")
        .map(|scenes| {
            scenes
                .children_named("Scene")
                .map(|scene| super::model::Scene {
                    info: timeline_info(scene, report),
                    // A scene holds several slots, so they all go in one set
                    // of lanes.
                    content: Some(Box::new(Content::Timeline(Box::new(Timeline::Lanes(
                        Lanes {
                            info: timeline_info(scene, report),
                            children: scene
                                .children
                                .iter()
                                .filter_map(|child| timeline(child, report))
                                .collect(),
                        },
                    ))))),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Project {
        version: node.attribute("version").map(str::to_owned),
        application: node.child("Application").map(|application| Application {
            name: application.attribute("name").map(str::to_owned),
            version: application.attribute("version").map(str::to_owned),
        }),
        transport: node.child("Transport").map(|transport| Transport {
            tempo: transport.child("Tempo").map(real_parameter),
            time_signature: transport.child("TimeSignature").map(signature_parameter),
        }),
        structure,
        arrangement,
        scenes,
    })
}

/// A set of points under a name the schema gives them.
fn points_of(node: &Node, report: &mut ReadReport) -> Points {
    points(node, report)
}
