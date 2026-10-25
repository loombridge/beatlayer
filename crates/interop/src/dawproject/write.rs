//! Writing a DAWproject file's XML (the DAWproject spec, R66, R67): the
//! model of the schema's elements as the text of `project.xml`.
//!
//! The text is the same for the same model: attributes and children come
//! out in the order the schema gives them, and numbers are written one way.
//! An element the schema lets carry an attribute the writer isn't given is
//! written without it.

use super::model::{
    BoolParameter, Channel, Clip, Content, ContentKind, Device, DeviceBody, DeviceRole, EqBand,
    Expression, FileReference, Interpolation, MediaFile, MixerRole, Parameter, PointValue, Points,
    Project, RealParameter, Scene, Send, Structure, Target, TimeSignatureParameter, TimeUnit,
    Timeline, TimelineInfo, Track, Unit,
};

/// The XML declaration `project.xml` and `metadata.xml` start with.
const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// An XML text being written.
struct Xml {
    out: String,
    depth: usize,
}

type Attr = (&'static str, String);
type Attrs = Vec<Attr>;

impl Xml {
    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    fn tag(&mut self, name: &str, attrs: &[Attr], close: &str) {
        self.indent();
        self.out.push('<');
        self.out.push_str(name);
        for (key, value) in attrs {
            self.out.push(' ');
            self.out.push_str(key);
            self.out.push_str("=\"");
            escape_into(&mut self.out, value);
            self.out.push('"');
        }
        self.out.push_str(close);
        self.out.push('\n');
    }

    /// An element with children, which `body` writes.
    fn element(&mut self, name: &str, attrs: &[Attr], body: impl FnOnce(&mut Self)) {
        self.tag(name, attrs, ">");
        self.depth += 1;
        body(self);
        self.depth -= 1;
        self.indent();
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    /// An element with no children.
    fn empty(&mut self, name: &str, attrs: &[Attr]) {
        self.tag(name, attrs, "/>");
    }
}

/// Appends `text` as an attribute value: the five predefined entities, a
/// character reference for the white space an attribute would lose, and
/// nothing XML 1.0 forbids.
fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c if (c as u32) < 0x20 || matches!(c as u32, 0xFFFE | 0xFFFF) => {}
            c => out.push(c),
        }
    }
}

/// A number the way the file spells it: shortest digits that read back
/// exactly, no exponent, `inf` and `-inf` for the infinities.
#[must_use]
pub fn number(value: f64) -> String {
    if value == f64::INFINITY {
        "inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-inf".to_owned()
    } else if value == 0.0 {
        "0".to_owned()
    } else {
        value.to_string()
    }
}

fn boolean(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

fn unit_name(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Beats => "beats",
        TimeUnit::Seconds => "seconds",
    }
}

/// The attributes of a nameable element.
fn named(info: &TimelineInfo, attrs: &mut Attrs) {
    if let Some(name) = &info.name {
        attrs.push(("name", name.clone()));
    }
    if let Some(color) = &info.color {
        attrs.push(("color", color.clone()));
    }
    if let Some(comment) = &info.comment {
        attrs.push(("comment", comment.clone()));
    }
}

/// The attributes of a referenceable element: its `id`, then the nameable's.
fn referenceable(info: &TimelineInfo) -> Attrs {
    let mut attrs = Attrs::new();
    if let Some(id) = &info.id {
        attrs.push(("id", id.clone()));
    }
    named(info, &mut attrs);
    attrs
}

/// The attributes of a timeline element.
fn timeline_attrs(info: &TimelineInfo) -> Attrs {
    let mut attrs = referenceable(info);
    if let Some(unit) = info.time_unit {
        attrs.push(("timeUnit", unit_name(unit).to_owned()));
    }
    if let Some(track) = &info.track {
        attrs.push(("track", track.clone()));
    }
    attrs
}

fn real(xml: &mut Xml, element: &str, parameter: &RealParameter) {
    let mut attrs = Attrs::new();
    if let Some(id) = &parameter.id {
        attrs.push(("id", id.clone()));
    }
    if let Some(name) = &parameter.name {
        attrs.push(("name", name.clone()));
    }
    if let Some(id) = parameter.parameter_id {
        attrs.push(("parameterID", id.to_string()));
    }
    if let Some(max) = parameter.max {
        attrs.push(("max", number(max)));
    }
    if let Some(min) = parameter.min {
        attrs.push(("min", number(min)));
    }
    attrs.push((
        "unit",
        parameter.unit.unwrap_or(Unit::Linear).name().to_owned(),
    ));
    if let Some(value) = parameter.value {
        attrs.push(("value", number(value)));
    }
    xml.empty(element, &attrs);
}

fn boolean_parameter(xml: &mut Xml, element: &str, parameter: &BoolParameter) {
    let mut attrs = Attrs::new();
    if let Some(id) = &parameter.id {
        attrs.push(("id", id.clone()));
    }
    if let Some(name) = &parameter.name {
        attrs.push(("name", name.clone()));
    }
    if let Some(id) = parameter.parameter_id {
        attrs.push(("parameterID", id.to_string()));
    }
    if let Some(value) = parameter.value {
        attrs.push(("value", boolean(value)));
    }
    xml.empty(element, &attrs);
}

fn signature(xml: &mut Xml, element: &str, parameter: &TimeSignatureParameter) {
    let mut attrs = Attrs::new();
    if let Some(id) = &parameter.id {
        attrs.push(("id", id.clone()));
    }
    if let Some(name) = &parameter.name {
        attrs.push(("name", name.clone()));
    }
    attrs.push((
        "denominator",
        parameter.denominator.unwrap_or(4).to_string(),
    ));
    attrs.push(("numerator", parameter.numerator.unwrap_or(4).to_string()));
    xml.empty(element, &attrs);
}

fn file(xml: &mut Xml, element: &str, reference: &FileReference) {
    let mut attrs: Attrs = vec![("path", reference.path.clone())];
    if reference.external {
        attrs.push(("external", boolean(true)));
    }
    xml.empty(element, &attrs);
}

fn parameters(xml: &mut Xml, list: &[Parameter]) {
    if list.is_empty() {
        return;
    }
    xml.element("Parameters", &Attrs::new(), |xml| {
        for parameter in list {
            match parameter {
                Parameter::Real(p) => real(xml, "RealParameter", p),
                Parameter::Bool(p) => boolean_parameter(xml, "BoolParameter", p),
                Parameter::Integer { .. }
                | Parameter::Enumerated { .. }
                | Parameter::Signature(_) => {}
            }
        }
    });
}

fn device_attrs(device: &Device, plugin: bool) -> Attrs {
    let mut attrs = referenceable(&device.info);
    if let Some(id) = &device.device_id {
        attrs.push(("deviceID", id.clone()));
    }
    attrs.push(("deviceName", device.device_name.clone().unwrap_or_default()));
    attrs.push((
        "deviceRole",
        match device.role.unwrap_or(DeviceRole::AudioFx) {
            DeviceRole::Instrument => "instrument",
            DeviceRole::NoteFx => "noteFX",
            DeviceRole::AudioFx => "audioFX",
            DeviceRole::Analyzer => "analyzer",
        }
        .to_owned(),
    ));
    if let Some(vendor) = &device.device_vendor {
        attrs.push(("deviceVendor", vendor.clone()));
    }
    if let Some(loaded) = device.loaded {
        attrs.push(("loaded", boolean(loaded)));
    }
    if plugin && let Some(version) = &device.plugin_version {
        attrs.push(("pluginVersion", version.clone()));
    }
    attrs
}

fn band(xml: &mut Xml, band: &EqBand) {
    let mut attrs = Attrs::new();
    attrs.push(("type", band.band_type.clone().unwrap_or_default()));
    if let Some(order) = band.order {
        attrs.push(("order", order.to_string()));
    }
    xml.element("Band", &attrs, |xml| {
        if let Some(parameter) = &band.frequency {
            real(xml, "Freq", parameter);
        }
        if let Some(parameter) = &band.gain {
            real(xml, "Gain", parameter);
        }
        if let Some(parameter) = &band.q {
            real(xml, "Q", parameter);
        }
        if let Some(parameter) = &band.enabled {
            boolean_parameter(xml, "Enabled", parameter);
        }
    });
}

fn device(xml: &mut Xml, device: &Device) {
    let (name, plugin) = match &device.body {
        DeviceBody::Plugin(format) => (format.name(), true),
        DeviceBody::Builtin => ("BuiltinDevice", false),
        DeviceBody::Equalizer { .. } => ("Equalizer", false),
        DeviceBody::Compressor { .. } => ("Compressor", false),
        DeviceBody::NoiseGate { .. } => ("NoiseGate", false),
        DeviceBody::Limiter { .. } => ("Limiter", false),
    };
    let attrs = device_attrs(device, plugin);
    xml.element(name, &attrs, |xml| {
        parameters(xml, &device.parameters);
        if let Some(enabled) = &device.enabled {
            boolean_parameter(xml, "Enabled", enabled);
        }
        if let Some(state) = &device.state {
            file(xml, "State", state);
        }
        // The generic devices' values follow the common children, in the
        // order the schema gives them.
        match &device.body {
            DeviceBody::Equalizer {
                bands,
                input_gain,
                output_gain,
            } => {
                for item in bands {
                    band(xml, item);
                }
                optional(xml, "InputGain", input_gain);
                optional(xml, "OutputGain", output_gain);
            }
            DeviceBody::Compressor {
                attack,
                auto_makeup,
                input_gain,
                output_gain,
                ratio,
                release,
                threshold,
            } => {
                optional(xml, "Attack", attack);
                if let Some(parameter) = auto_makeup {
                    boolean_parameter(xml, "AutoMakeup", parameter);
                }
                optional(xml, "InputGain", input_gain);
                optional(xml, "OutputGain", output_gain);
                optional(xml, "Ratio", ratio);
                optional(xml, "Release", release);
                optional(xml, "Threshold", threshold);
            }
            DeviceBody::NoiseGate {
                attack,
                range,
                ratio,
                release,
                threshold,
            } => {
                optional(xml, "Attack", attack);
                optional(xml, "Range", range);
                optional(xml, "Ratio", ratio);
                optional(xml, "Release", release);
                optional(xml, "Threshold", threshold);
            }
            DeviceBody::Limiter {
                attack,
                input_gain,
                output_gain,
                release,
                threshold,
            } => {
                optional(xml, "Attack", attack);
                optional(xml, "InputGain", input_gain);
                optional(xml, "OutputGain", output_gain);
                optional(xml, "Release", release);
                optional(xml, "Threshold", threshold);
            }
            DeviceBody::Plugin(_) | DeviceBody::Builtin => {}
        }
    });
}

fn optional(xml: &mut Xml, element: &str, parameter: &Option<RealParameter>) {
    if let Some(parameter) = parameter {
        real(xml, element, parameter);
    }
}

fn send(xml: &mut Xml, send: &Send) {
    let mut attrs = Attrs::new();
    if let Some(id) = &send.id {
        attrs.push(("id", id.clone()));
    }
    if let Some(destination) = &send.destination {
        attrs.push(("destination", destination.clone()));
    }
    if let Some(kind) = &send.send_type {
        attrs.push(("type", kind.clone()));
    }
    xml.element("Send", &attrs, |xml| {
        if let Some(enable) = &send.enable {
            boolean_parameter(xml, "Enable", enable);
        }
        if let Some(pan) = &send.pan {
            real(xml, "Pan", pan);
        }
        if let Some(volume) = &send.volume {
            real(xml, "Volume", volume);
        }
    });
}

fn channel(xml: &mut Xml, channel: &Channel) {
    let mut attrs = referenceable(&channel.info);
    if let Some(count) = channel.audio_channels {
        attrs.push(("audioChannels", count.to_string()));
    }
    if let Some(destination) = &channel.destination {
        attrs.push(("destination", destination.clone()));
    }
    if let Some(role) = channel.role {
        attrs.push((
            "role",
            match role {
                MixerRole::Regular => "regular",
                MixerRole::Master => "master",
                MixerRole::Effect => "effect",
                MixerRole::Submix => "submix",
                MixerRole::Vca => "vca",
            }
            .to_owned(),
        ));
    }
    if let Some(solo) = channel.solo {
        attrs.push(("solo", boolean(solo)));
    }
    xml.element("Channel", &attrs, |xml| {
        if !channel.devices.is_empty() {
            xml.element("Devices", &Attrs::new(), |xml| {
                for item in &channel.devices {
                    device(xml, item);
                }
            });
        }
        if let Some(mute) = &channel.mute {
            boolean_parameter(xml, "Mute", mute);
        }
        if let Some(pan) = &channel.pan {
            real(xml, "Pan", pan);
        }
        if !channel.sends.is_empty() {
            xml.element("Sends", &Attrs::new(), |xml| {
                for item in &channel.sends {
                    send(xml, item);
                }
            });
        }
        if let Some(volume) = &channel.volume {
            real(xml, "Volume", volume);
        }
    });
}

fn track(xml: &mut Xml, track: &Track) {
    let mut attrs = referenceable(&track.info);
    if !track.content_types.is_empty() {
        let kinds: Vec<&str> = track
            .content_types
            .iter()
            .map(|kind| match kind {
                ContentKind::Audio => "audio",
                ContentKind::Automation => "automation",
                ContentKind::Notes => "notes",
                ContentKind::Video => "video",
                ContentKind::Markers => "markers",
                ContentKind::Tracks => "tracks",
            })
            .collect();
        attrs.push(("contentType", kinds.join(" ")));
    }
    if let Some(loaded) = track.loaded {
        attrs.push(("loaded", boolean(loaded)));
    }
    xml.element("Track", &attrs, |xml| {
        if let Some(item) = &track.channel {
            channel(xml, item);
        }
        for child in &track.children {
            self::track(xml, child);
        }
    });
}

/// The attributes a clip carries, in the schema's order.
fn clip_attrs(clip: &Clip) -> Attrs {
    let mut attrs = Attrs::new();
    named(&clip.info, &mut attrs);
    let mut push = |key: &'static str, value: Option<f64>| {
        if let Some(value) = value {
            attrs.push((key, number(value)));
        }
    };
    push("time", clip.time);
    push("duration", clip.duration);
    if let Some(unit) = clip.content_time_unit {
        attrs.push(("contentTimeUnit", unit_name(unit).to_owned()));
    }
    let mut push = |key: &'static str, value: Option<f64>| {
        if let Some(value) = value {
            attrs.push((key, number(value)));
        }
    };
    push("playStart", clip.play_start);
    push("playStop", clip.play_stop);
    push("loopStart", clip.loop_start);
    push("loopEnd", clip.loop_end);
    if let Some(unit) = clip.fade_time_unit {
        attrs.push(("fadeTimeUnit", unit_name(unit).to_owned()));
    }
    if let Some(value) = clip.fade_in {
        attrs.push(("fadeInTime", number(value)));
    }
    if let Some(value) = clip.fade_out {
        attrs.push(("fadeOutTime", number(value)));
    }
    if let Some(enable) = clip.enable {
        attrs.push(("enable", boolean(enable)));
    }
    if let Some(reference) = &clip.reference {
        attrs.push(("reference", reference.clone()));
    }
    attrs
}

fn clip(xml: &mut Xml, item: &Clip) {
    let attrs = clip_attrs(item);
    match item.content.as_deref() {
        Some(Content::Timeline(timeline)) => xml.element("Clip", &attrs, |xml| {
            self::timeline(xml, timeline);
        }),
        _ => xml.empty("Clip", &attrs),
    }
}

fn media(xml: &mut Xml, element: &str, item: &MediaFile) {
    let mut attrs = timeline_attrs(&item.info);
    if let Some(duration) = item.duration {
        attrs.push(("duration", number(duration)));
    }
    if let Some(algorithm) = &item.algorithm {
        attrs.push(("algorithm", algorithm.clone()));
    }
    if let Some(channels) = item.channels {
        attrs.push(("channels", channels.to_string()));
    }
    if let Some(rate) = item.sample_rate {
        attrs.push(("sampleRate", rate.to_string()));
    }
    xml.element(element, &attrs, |xml| {
        if let Some(reference) = &item.file {
            file(xml, "File", reference);
        }
    });
}

fn target(xml: &mut Xml, target: &Target) {
    let mut attrs = Attrs::new();
    if let Some(parameter) = &target.parameter {
        attrs.push(("parameter", parameter.clone()));
    }
    if let Some(expression) = target.expression {
        attrs.push((
            "expression",
            match expression {
                Expression::Gain => "gain",
                Expression::Pan => "pan",
                Expression::Transpose => "transpose",
                Expression::Timbre => "timbre",
                Expression::Formant => "formant",
                Expression::Pressure => "pressure",
                Expression::ChannelController => "channelController",
                Expression::ChannelPressure => "channelPressure",
                Expression::PolyPressure => "polyPressure",
                Expression::PitchBend => "pitchBend",
                Expression::ProgramChange => "programChange",
            }
            .to_owned(),
        ));
    }
    if let Some(channel) = target.channel {
        attrs.push(("channel", channel.to_string()));
    }
    if let Some(key) = target.key {
        attrs.push(("key", key.to_string()));
    }
    if let Some(controller) = target.controller {
        attrs.push(("controller", controller.to_string()));
    }
    xml.empty("Target", &attrs);
}

fn point(xml: &mut Xml, point: &PointValue) {
    let time = number(point.time().unwrap_or(0.0));
    match point {
        PointValue::Real {
            value,
            interpolation,
            ..
        } => {
            let mut attrs: Attrs = vec![("time", time), ("value", number(value.unwrap_or(0.0)))];
            if let Some(interpolation) = interpolation {
                attrs.push((
                    "interpolation",
                    match interpolation {
                        Interpolation::Hold => "hold",
                        Interpolation::Linear => "linear",
                    }
                    .to_owned(),
                ));
            }
            xml.empty("RealPoint", &attrs);
        }
        PointValue::Bool { value, .. } => xml.empty(
            "BoolPoint",
            &[("time", time), ("value", boolean(value.unwrap_or(false)))],
        ),
        PointValue::Integer { value, .. } => xml.empty(
            "IntegerPoint",
            &[("time", time), ("value", value.unwrap_or(0).to_string())],
        ),
        PointValue::Enumerated { value, .. } => xml.empty(
            "EnumPoint",
            &[("time", time), ("value", value.unwrap_or(0).to_string())],
        ),
        PointValue::Signature {
            numerator,
            denominator,
            ..
        } => xml.empty(
            "TimeSignaturePoint",
            &[
                ("time", time),
                ("numerator", numerator.unwrap_or(4).to_string()),
                ("denominator", denominator.unwrap_or(4).to_string()),
            ],
        ),
    }
}

fn points(xml: &mut Xml, element: &str, item: &Points) {
    let mut attrs = timeline_attrs(&item.info);
    if let Some(unit) = item.unit {
        attrs.push(("unit", unit.name().to_owned()));
    }
    xml.element(element, &attrs, |xml| {
        target(xml, &item.target);
        for each in &item.points {
            point(xml, each);
        }
    });
}

fn markers(xml: &mut Xml, element: &str, item: &super::model::Markers) {
    xml.element(element, &timeline_attrs(&item.info), |xml| {
        for marker in &item.markers {
            let mut attrs = Attrs::new();
            named(&marker.info, &mut attrs);
            attrs.push(("time", number(marker.time.unwrap_or(0.0))));
            xml.empty("Marker", &attrs);
        }
    });
}

fn timeline(xml: &mut Xml, item: &Timeline) {
    match item {
        Timeline::Clips(clips) => xml.element("Clips", &timeline_attrs(&clips.info), |xml| {
            for each in &clips.clips {
                clip(xml, each);
            }
        }),
        Timeline::Notes(notes) => xml.element("Notes", &timeline_attrs(&notes.info), |xml| {
            for note in &notes.notes {
                let mut attrs: Attrs = vec![
                    ("time", number(note.time.unwrap_or(0.0))),
                    ("duration", number(note.duration.unwrap_or(0.0))),
                    ("channel", note.channel.unwrap_or(0).to_string()),
                    ("key", note.key.unwrap_or(0).to_string()),
                ];
                if let Some(vel) = note.vel {
                    attrs.push(("vel", number(vel)));
                }
                if let Some(rel) = note.rel {
                    attrs.push(("rel", number(rel)));
                }
                match note.content.as_deref() {
                    Some(Content::Timeline(inner)) => xml.element("Note", &attrs, |xml| {
                        timeline(xml, inner);
                    }),
                    _ => xml.empty("Note", &attrs),
                }
            }
        }),
        Timeline::ClipSlot(slot) => {
            let mut attrs = timeline_attrs(&slot.info);
            if let Some(stop) = slot.has_stop {
                attrs.push(("hasStop", boolean(stop)));
            }
            match &slot.clip {
                Some(inner) => xml.element("ClipSlot", &attrs, |xml| clip(xml, inner)),
                None => xml.empty("ClipSlot", &attrs),
            }
        }
        Timeline::Warps(warps) => {
            let mut attrs = timeline_attrs(&warps.info);
            attrs.push((
                "contentTimeUnit",
                unit_name(warps.content_time_unit.unwrap_or(TimeUnit::Seconds)).to_owned(),
            ));
            xml.element("Warps", &attrs, |xml| {
                if let Some(audio) = &warps.audio {
                    media(xml, "Audio", audio);
                }
                for warp in &warps.warps {
                    xml.empty(
                        "Warp",
                        &[
                            ("time", number(warp.time.unwrap_or(0.0))),
                            ("contentTime", number(warp.content_time.unwrap_or(0.0))),
                        ],
                    );
                }
            });
        }
        Timeline::Audio(audio) => media(xml, "Audio", audio),
        Timeline::Video(video) => media(xml, "Video", video),
        Timeline::Markers(list) => markers(xml, "Markers", list),
        Timeline::Points(set) => points(xml, "Points", set),
        Timeline::Lanes(lanes) => xml.element("Lanes", &timeline_attrs(&lanes.info), |xml| {
            for child in &lanes.children {
                timeline(xml, child);
            }
        }),
    }
}

fn scene(xml: &mut Xml, item: &Scene) {
    xml.element("Scene", &referenceable(&item.info), |xml| {
        if let Some(Content::Timeline(inner)) = item.content.as_deref() {
            timeline(xml, inner);
        }
    });
}

/// The text of `project.xml` for a project.
#[must_use]
pub fn project_xml(project: &Project) -> String {
    let mut xml = Xml {
        out: DECLARATION.to_owned(),
        depth: 0,
    };
    let attrs: Attrs = vec![("version", project.version.clone().unwrap_or_default())];
    xml.element("Project", &attrs, |xml| {
        if let Some(application) = &project.application {
            xml.empty(
                "Application",
                &[
                    ("name", application.name.clone().unwrap_or_default()),
                    ("version", application.version.clone().unwrap_or_default()),
                ],
            );
        }
        if let Some(transport) = &project.transport {
            xml.element("Transport", &Attrs::new(), |xml| {
                if let Some(tempo) = &transport.tempo {
                    real(xml, "Tempo", tempo);
                }
                if let Some(parameter) = &transport.time_signature {
                    signature(xml, "TimeSignature", parameter);
                }
            });
        }
        xml.element("Structure", &Attrs::new(), |xml| {
            for item in &project.structure {
                match item {
                    Structure::Track(each) => track(xml, each),
                    Structure::Channel(each) => channel(xml, each),
                }
            }
        });
        if let Some(arrangement) = &project.arrangement {
            xml.element("Arrangement", &referenceable(&arrangement.info), |xml| {
                if !arrangement.lanes.is_empty() {
                    xml.element(
                        "Lanes",
                        &[("timeUnit", unit_name(TimeUnit::Beats).to_owned())],
                        |xml| {
                            for lane in &arrangement.lanes {
                                timeline(xml, lane);
                            }
                        },
                    );
                }
                if let Some(list) = &arrangement.markers {
                    markers(xml, "Markers", list);
                }
                if let Some(set) = &arrangement.tempo_automation {
                    points(xml, "TempoAutomation", set);
                }
                if let Some(set) = &arrangement.signature_automation {
                    points(xml, "TimeSignatureAutomation", set);
                }
            });
        }
        if !project.scenes.is_empty() {
            xml.element("Scenes", &Attrs::new(), |xml| {
                for each in &project.scenes {
                    scene(xml, each);
                }
            });
        }
    });
    xml.out
}

/// The text of `metadata.xml`, holding the set's name as its title (R79).
#[must_use]
pub fn metadata_xml(title: &str) -> String {
    let mut out = DECLARATION.to_owned();
    out.push_str("<MetaData>\n  <Title>");
    let mut escaped = String::new();
    escape_into(&mut escaped, title);
    out.push_str(&escaped);
    out.push_str("</Title>\n</MetaData>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: dawproject R66.
    #[test]
    fn numbers_are_written_one_way() {
        assert_eq!(number(1.0), "1");
        assert_eq!(number(-0.0), "0");
        assert_eq!(number(0.1), "0.1");
        assert_eq!(number(1e-7), "0.0000001");
        assert_eq!(number(f64::NEG_INFINITY), "-inf");
        assert_eq!(number(704_600.5).parse::<f64>().unwrap(), 704_600.5);
    }

    // Spec: dawproject R66. A name with markup and white space reads back as
    // it was.
    #[test]
    fn attribute_text_is_escaped() {
        let mut out = String::new();
        escape_into(&mut out, "a<b>&\"c'\n\u{1}d");
        assert_eq!(out, "a&lt;b&gt;&amp;&quot;c&apos;&#10;d");
    }
}
