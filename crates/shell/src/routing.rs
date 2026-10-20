//! The In/Out section (WP-2.19; the routing spec): what each of a track's
//! choosers lists, in Live's order, which entry is chosen, and the
//! registered command each entry runs, worked out from the set so the
//! window, its tests, and an agent's view of the mixer share them.
//!
//! The lists, their order, and their defaults were observed in Live 12.4.6
//! on macOS on 12 October 2026 (the routing spec, sources). Entries for
//! what another package brings (the audio interface's outputs) are listed
//! where Live lists them, disabled, and name the package. MIDI From lists the
//! MIDI input ports the computer offers (WP-2.2).

use bl_model::{
    Chain, Channel, Device, DeviceId, DeviceOutput, DeviceRole, Document, Input, MidiTo, Monitor,
    Output, RackKind, Tap, TrackId, TrackKind,
};
use bl_shell_spec::commands::APP_SETTINGS;
use serde_json::{Value, json};

use crate::Invocation;

/// What the In/Out choosers list from outside the set: how many input and
/// output channels the open audio device has (zero while none is open), the
/// MIDI input ports the computer offers, and the extra audio outputs each
/// device in the set offers past its main one (WP-5.45).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interface {
    /// The device's input channels.
    pub inputs: u16,
    /// The device's output channels.
    pub outputs: u16,
    /// The names of the MIDI input ports, in the order the Settings window
    /// lists them (the MIDI I/O spec, R7).
    pub midi_inputs: Vec<String>,
    /// The names of the MIDI output ports whose Track switch is on, in the
    /// order the Settings window lists them (the external devices spec, R1).
    pub midi_outputs: Vec<String>,
    /// Each device's extra audio outputs, by the names its type gives them,
    /// in the order it declares them: an instrument's second stereo output,
    /// one of the eight-slot drum sampler's individual outputs (WP-5.45).
    /// A device that offers none is left out.
    pub device_outputs: std::collections::BTreeMap<DeviceId, Vec<String>>,
}

/// One line of a chooser's list.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A choice.
    Choice {
        /// What it reads.
        label: String,
        /// The command choosing it runs, or `None` if it's disabled.
        run: Option<Invocation>,
        /// Whether it's the chosen one.
        chosen: bool,
        /// The package that brings it, if it's disabled until then.
        pending: Option<&'static str>,
    },
    /// A separator line.
    Separator,
}

impl Entry {
    fn choice(label: impl Into<String>, run: Invocation, chosen: bool) -> Self {
        Self::Choice {
            label: label.into(),
            run: Some(run),
            chosen,
            pending: None,
        }
    }

    /// What it reads, if it's a choice.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Choice { label, .. } => Some(label),
            Self::Separator => None,
        }
    }
}

/// A chooser in the In/Out section: its accessible name, what it shows,
/// and its list.
#[derive(Clone, Debug, PartialEq)]
pub struct Chooser {
    /// Its name for assistive technology, as Live's Info View names it.
    pub name: &'static str,
    /// What it shows: the chosen entry's label, or the channel's.
    pub text: String,
    /// Its list, separators included; empty for a chooser with nothing to
    /// choose.
    pub entries: Vec<Entry>,
    /// The package that brings it, if the whole chooser is disabled until
    /// then.
    pub pending: Option<&'static str>,
}

impl Chooser {
    /// Whether it can be opened.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.pending.is_none()
            && self
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Choice { run: Some(_), .. }))
    }

    /// The labels of its list, a separator as `─`.
    #[must_use]
    pub fn labels(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.label().unwrap_or("─"))
            .collect()
    }

    /// The command the entry labelled `label` runs.
    #[must_use]
    pub fn run_for(&self, label: &str) -> Option<&Invocation> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Choice {
                label: l,
                run: Some(run),
                ..
            } if l == label => Some(run),
            _ => None,
        })
    }
}

/// A track's Monitor switch: In, Auto, and Off, one of them chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorSwitch {
    /// The chosen state.
    pub chosen: Monitor,
    /// The command each button runs: In, Auto, Off.
    pub runs: [(Monitor, Invocation); 3],
}

/// One track's In/Out section, top to bottom.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "made for a drawn column and dropped in the same frame, never stored"
)]
pub enum InOut {
    /// An audio or MIDI track: its input type and channel, Monitor, and
    /// its output type and channel. A MIDI track's are MIDI From and MIDI
    /// To.
    Clips {
        /// Audio From or MIDI From.
        input: Chooser,
        /// Its channel.
        input_channel: Chooser,
        /// Monitor.
        monitor: MonitorSwitch,
        /// Audio To or MIDI To.
        output: Chooser,
        /// Its channel.
        output_channel: Chooser,
    },
    /// A group or a return: Audio To and its channel.
    Output {
        /// Audio To.
        output: Chooser,
        /// Its channel.
        output_channel: Chooser,
    },
    /// Main: Cue Out and Main Out, which list the audio interface's
    /// outputs.
    Main {
        /// Cue Out.
        cue: Chooser,
        /// Main Out.
        main: Chooser,
    },
}

/// What a track is called in another track's chooser: its number and name
/// for an audio, MIDI, or group track (`4-Audio` for `4 Audio`), its letter
/// and name for a return (`A-Reverb` for `A Reverb`), and `Main`
/// (observed). A name's own leading number or letter, which new tracks'
/// names carry, isn't repeated.
#[must_use]
pub fn track_label(doc: &Document, track: TrackId) -> String {
    let Some(t) = doc.track(track) else {
        return String::new();
    };
    match t.kind() {
        TrackKind::Main => "Main".to_owned(),
        TrackKind::Return => {
            let index = doc.returns().position(|r| r == track).unwrap_or(0);
            let letter = return_letter(index);
            let name = t
                .name()
                .strip_prefix(letter.as_str())
                .and_then(|rest| rest.strip_prefix(' '))
                .unwrap_or(t.name());
            format!("{letter}-{name}")
        }
        _ => {
            let number = doc
                .track_order()
                .into_iter()
                .filter(|&id| doc.track(id).is_some_and(|t| t.kind().can_be_grouped()))
                .position(|id| id == track)
                .map_or(0, |i| i + 1);
            let name = t
                .name()
                .split_once(' ')
                .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                .map_or(t.name(), |(_, rest)| rest);
            format!("{number}-{name}")
        }
    }
}

/// A return track's letter: A to Z, then AA, AB, and on, as there's no
/// limit on return tracks (IMP-01).
#[must_use]
pub fn return_letter(index: usize) -> String {
    let mut letters = Vec::new();
    let mut n = index + 1;
    while n > 0 {
        n -= 1;
        letters.push(char::from(b'A' + (n % 26) as u8));
        n /= 26;
    }
    letters.iter().rev().collect()
}

fn set_input(track: TrackId, input: Value) -> Invocation {
    Invocation::new(
        "routing.set_input",
        json!({ "track": track, "input": input }),
    )
}

fn set_output(track: TrackId, output: Value) -> Invocation {
    Invocation::new(
        "routing.set_output",
        json!({ "track": track, "output": output }),
    )
}

fn configure(page: &str) -> Entry {
    Entry::choice(
        "Configure…",
        Invocation::new(APP_SETTINGS, json!({ "page": page })),
        false,
    )
}

const fn tap_label(tap: Tap) -> &'static str {
    match tap {
        Tap::PreFx => "Pre FX",
        Tap::PostFx => "Post FX",
        Tap::PostMixer => "Post Mixer",
    }
}

/// `track`'s In/Out section, or `None` if there's no such track. The
/// interface's channels come from `interface`, so the Ext. In channel and
/// Main Out choosers list what the open device has.
#[must_use]
pub fn in_out(doc: &Document, track: TrackId, interface: &Interface) -> Option<InOut> {
    let t = doc.track(track)?;
    Some(match t.kind() {
        TrackKind::Audio => InOut::Clips {
            input: audio_from(doc, track),
            input_channel: input_channel(
                doc,
                track,
                t.routing().input.clone(),
                None,
                false,
                interface,
            ),
            monitor: monitor(track, t.routing().monitor),
            output: audio_to(doc, track),
            output_channel: output_channel(track, t.routing().output, interface.outputs),
        },
        TrackKind::Midi => InOut::Clips {
            input: midi_from(doc, track, &interface.midi_inputs),
            input_channel: input_channel(
                doc,
                track,
                t.routing().input.clone(),
                t.routing().channel,
                true,
                interface,
            ),
            monitor: monitor(track, t.routing().monitor),
            output: midi_to(doc, track, &interface.midi_outputs),
            output_channel: midi_to_channel(track, t.routing().midi_output.as_ref()),
        },
        TrackKind::Group | TrackKind::Return => InOut::Output {
            output: audio_to(doc, track),
            output_channel: output_channel(track, t.routing().output, interface.outputs),
        },
        TrackKind::Main => InOut::Main {
            cue: interface_outputs(
                "Cue Output",
                "routing.set_cue_out",
                t.routing().cue_out,
                interface.outputs,
            ),
            main: interface_outputs(
                "Main Output",
                "routing.set_main_out",
                t.routing().main_out,
                interface.outputs,
            ),
        },
    })
}

/// Cue Out or Main Out: Configure… ─ the interface's output pairs
/// (`1/2`, `3/4`, …), the chosen one lit. With no outputs open it lists
/// Configure… alone, as Live does with no audio device (observed).
fn interface_outputs(
    name: &'static str,
    command: &'static str,
    chosen: Channel,
    outputs: u16,
) -> Chooser {
    let mut entries = vec![configure("audio"), Entry::Separator];
    entries.extend(channel_pairs(outputs).into_iter().map(|channel| {
        Entry::choice(
            channel.label(),
            Invocation::new(command, json!({ "channel": channel_value(channel) })),
            channel == chosen,
        )
    }));
    let mut chooser = chooser(name, entries);
    if chooser.text.is_empty() {
        chosen.label().clone_into(&mut chooser.text);
    }
    chooser
}

/// The stereo pairs among `channels` channels, `1/2`, `3/4`, ….
fn channel_pairs(channels: u16) -> Vec<Channel> {
    (1..channels).step_by(2).map(Channel::Pair).collect()
}

/// A `Channel` as the routing commands' JSON takes it.
fn channel_value(channel: Channel) -> Value {
    match channel {
        Channel::Mono(channel) => json!({ "type": "mono", "channel": channel }),
        Channel::Pair(channel) => json!({ "type": "pair", "channel": channel }),
    }
}

/// The interface's inputs or outputs as Live lists them: the single
/// channels (`1`, `2`, …), then the stereo pairs (`1/2`, `3/4`, …), each
/// running `run`'s command.
fn interface_channels(
    channels: u16,
    chosen: Channel,
    run: impl Fn(Channel) -> Invocation,
) -> Vec<Entry> {
    (1..=channels)
        .map(Channel::Mono)
        .chain(channel_pairs(channels))
        .map(|channel| Entry::choice(channel.label(), run(channel), channel == chosen))
        .collect()
}

/// Audio From (observed): Ext. In · Configure… ─ Resampling ─ the group,
/// audio, and return tracks, the MIDI tracks that hold an instrument, and
/// Main, in display order, without the track itself ─ No Input.
fn audio_from(doc: &Document, track: TrackId) -> Chooser {
    let input = doc.track(track).map(|t| t.routing().input.clone());
    let current_from = match input {
        Some(Input::Track { track, .. }) => Some(track),
        _ => None,
    };
    // Re-choosing Ext. In keeps the channel it has.
    let ext_in = match input {
        Some(Input::External { channel }) => {
            json!({ "type": "external", "channel": channel_value(channel) })
        }
        _ => json!({ "type": "external" }),
    };
    let mut entries = vec![
        Entry::choice(
            "Ext. In",
            set_input(track, ext_in),
            matches!(input, Some(Input::External { .. })),
        ),
        configure("audio"),
        Entry::Separator,
        Entry::choice(
            "Resampling",
            set_input(track, json!({ "type": "resampling" })),
            input == Some(Input::Resampling),
        ),
        Entry::Separator,
    ];
    for id in doc.track_order() {
        let Some(source) = doc.track(id) else {
            continue;
        };
        // A MIDI track is a source once it holds an instrument (the racks
        // spec, R38).
        let listed = matches!(
            source.kind(),
            TrackKind::Audio | TrackKind::Group | TrackKind::Return | TrackKind::Main
        ) || source.kind() == TrackKind::Midi
            && source.devices().any(|d| d.role == DeviceRole::Instrument);
        if id == track || !listed {
            continue;
        }
        entries.push(Entry::choice(
            track_label(doc, id),
            set_input(track, json!({ "type": "track", "track": id })),
            current_from == Some(id),
        ));
    }
    entries.push(Entry::Separator);
    entries.push(Entry::choice(
        "No Input",
        set_input(track, json!({ "type": "none" })),
        input == Some(Input::None),
    ));
    chooser("Input Type", entries)
}

/// MIDI From (observed): All Ins · Computer Keyboard · the input ports ·
/// Configure… ─ the other MIDI tracks ─ No Input (the MIDI I/O spec, R7). A
/// port the track names that the computer doesn't offer now is listed too,
/// chosen, because the track keeps the name (R8).
fn midi_from(doc: &Document, track: TrackId, ports: &[String]) -> Chooser {
    let input = doc.track(track).map(|t| t.routing().input.clone());
    let current_from = match input {
        Some(Input::Track { track, .. }) => Some(track),
        _ => None,
    };
    let chosen_port = match &input {
        Some(Input::Port { name }) => Some(name.as_str()),
        _ => None,
    };
    let mut entries = vec![
        Entry::choice(
            "All Ins",
            set_input(track, json!({ "type": "all_ins" })),
            input == Some(Input::AllIns),
        ),
        Entry::choice(
            "Computer Keyboard",
            set_input(track, json!({ "type": "computer_keyboard" })),
            input == Some(Input::ComputerKeyboard),
        ),
    ];
    let absent = chosen_port.filter(|name| !ports.iter().any(|p| p == name));
    for name in ports.iter().map(String::as_str).chain(absent) {
        entries.push(Entry::choice(
            name,
            set_input(track, json!({ "type": "port", "name": name })),
            chosen_port == Some(name),
        ));
    }
    entries.push(configure("tempo-midi"));
    entries.push(Entry::Separator);
    for id in doc.track_order() {
        if id != track && doc.track(id).map(bl_model::Track::kind) == Some(TrackKind::Midi) {
            entries.push(Entry::choice(
                track_label(doc, id),
                set_input(track, json!({ "type": "track", "track": id })),
                current_from == Some(id),
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(Entry::choice(
        "No Input",
        set_input(track, json!({ "type": "none" })),
        input == Some(Input::None),
    ));
    chooser("Input Type", entries)
}

/// The input channel: Pre FX · Post FX · Post Mixer with a track chosen
/// (observed, Post Mixer the default); the interface's inputs with Ext. In
/// (the single channels, then the pairs); All Channels, then 1 to 16, with a
/// MIDI port (the MIDI I/O spec, R7); All Channels, fixed, with All Ins or the
/// computer keyboard; nothing otherwise.
fn input_channel(
    doc: &Document,
    track: TrackId,
    input: Input,
    channel: Option<u8>,
    midi: bool,
    interface: &Interface,
) -> Chooser {
    match input {
        Input::Track {
            track: from,
            tap,
            chain,
            output,
        } => {
            let mut entries: Vec<Entry> = [Tap::PreFx, Tap::PostFx, Tap::PostMixer]
                .into_iter()
                .map(|t| {
                    Entry::choice(
                        tap_label(t),
                        set_input(
                            track,
                            json!({ "type": "track", "track": from, "tap": tap_name(t) }),
                        ),
                        chain.is_none() && output.is_none() && t == tap,
                    )
                })
                .collect();
            // Audio From goes on with the chains of the track's racks
            // (the racks spec, R38, R39).
            if !midi {
                for (rack, c) in rack_chains(doc, from) {
                    for t in [Tap::PreFx, Tap::PostFx, Tap::PostMixer] {
                        entries.push(Entry::choice(
                            chain_tap_label(rack, c, t),
                            set_input(
                                track,
                                json!({ "type": "track", "track": from, "tap": tap_name(t),
                                        "chain": c.id }),
                            ),
                            chain == Some(c.id) && t == tap,
                        ));
                    }
                }
                // Then the extra outputs of the track's instruments, at any
                // depth in its racks (WP-5.45).
                for (device, index, name) in device_outputs(doc, from, interface) {
                    entries.push(Entry::choice(
                        instrument_output_label(device, name),
                        set_input(
                            track,
                            json!({ "type": "track", "track": from,
                                    "output": { "device": device.id, "output": index } }),
                        ),
                        output
                            == Some(DeviceOutput {
                                device: device.id,
                                output: index,
                            }),
                    ));
                }
            }
            Chooser {
                name: "Input Channel",
                text: entries
                    .iter()
                    .find_map(|entry| match entry {
                        Entry::Choice {
                            label,
                            chosen: true,
                            ..
                        } => Some(label.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| tap_label(tap).to_owned()),
                entries,
                pending: None,
            }
        }
        Input::External { channel } => Chooser {
            name: "Input Channel",
            text: channel.label(),
            entries: interface_channels(interface.inputs, channel, |channel| {
                set_input(
                    track,
                    json!({ "type": "external", "channel": channel_value(channel) }),
                )
            }),
            pending: None,
        },
        Input::Port { name } if midi => {
            let set = |channel: Option<u8>| {
                Invocation::new(
                    "routing.set_input",
                    json!({ "track": track, "input": { "type": "port", "name": name },
                            "channel": channel }),
                )
            };
            let mut entries = vec![Entry::choice("All Channels", set(None), channel.is_none())];
            entries.extend(
                (1..=16).map(|n| Entry::choice(n.to_string(), set(Some(n)), channel == Some(n))),
            );
            Chooser {
                name: "Input Channel",
                text: channel.map_or_else(|| "All Channels".to_owned(), |n| n.to_string()),
                entries,
                pending: None,
            }
        }
        Input::AllIns | Input::ComputerKeyboard if midi => Chooser {
            name: "Input Channel",
            text: "All Channels".to_owned(),
            entries: Vec::new(),
            pending: None,
        },
        _ => Chooser {
            name: "Input Channel",
            text: String::new(),
            entries: Vec::new(),
            pending: None,
        },
    }
}

fn monitor(track: TrackId, chosen: Monitor) -> MonitorSwitch {
    let run = |state: &str| {
        Invocation::new(
            "routing.set_monitor",
            json!({ "track": track, "monitor": state }),
        )
    };
    MonitorSwitch {
        chosen,
        runs: [
            (Monitor::In, run("in")),
            (Monitor::Auto, run("auto")),
            (Monitor::Off, run("off")),
        ],
    }
}

/// Audio To (observed): Ext. Out · Configure… ─ Main, the track's group,
/// and the audio tracks in display order, without the track itself ─
/// Sends Only. A group's list puts a separator after Main. Choosing Ext. Out
/// again keeps the channel it has.
fn audio_to(doc: &Document, track: TrackId) -> Chooser {
    let Some(t) = doc.track(track) else {
        return chooser("Output Type", Vec::new());
    };
    let goes_to = doc.output_of(track);
    let output = t.routing().output;
    let main = doc.main_id();
    let ext_out = match output {
        Output::External { channel } => {
            json!({ "type": "external", "channel": channel_value(channel) })
        }
        _ => json!({ "type": "external" }),
    };
    let mut entries = vec![
        Entry::choice(
            "Ext. Out",
            set_output(track, ext_out),
            matches!(output, Output::External { .. }),
        ),
        configure("audio"),
        Entry::Separator,
        Entry::choice(
            "Main",
            set_output(track, json!({ "type": "main" })),
            goes_to == Some(main) && output != Output::SendsOnly,
        ),
    ];
    if t.kind() == TrackKind::Group {
        entries.push(Entry::Separator);
    }
    if let Some(group) = doc.parent(track) {
        entries.push(Entry::choice(
            track_label(doc, group),
            set_output(track, json!({ "type": "track", "track": group })),
            goes_to == Some(group),
        ));
    }
    for id in doc.track_order() {
        if id != track && doc.track(id).map(bl_model::Track::kind) == Some(TrackKind::Audio) {
            entries.push(Entry::choice(
                track_label(doc, id),
                set_output(track, json!({ "type": "track", "track": id })),
                goes_to == Some(id),
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(Entry::choice(
        "Sends Only",
        set_output(track, json!({ "type": "sends_only" })),
        output == Output::SendsOnly,
    ));
    chooser("Output Type", entries)
}

/// MIDI To (observed): the output ports whose Track switch is on (the
/// external devices spec, R1; Beatlayer's own place for them), Configure…
/// ─ the other MIDI tracks ─ No Output. A track that keeps the name of a
/// port that is away or has Track off lists it too, dimmed (R2). Choosing
/// a port takes channel 1, and choosing the same port again keeps its
/// channel.
fn midi_to(doc: &Document, track: TrackId, ports: &[String]) -> Chooser {
    let to = doc
        .track(track)
        .and_then(|t| t.routing().midi_output.clone());
    let set = |value: Value| {
        Invocation::new(
            "routing.set_midi_output",
            json!({ "track": track, "to": value }),
        )
    };
    let chosen_port = match &to {
        Some(MidiTo::Port { name, .. }) => Some(name.as_str()),
        _ => None,
    };
    let mut entries = Vec::new();
    for name in ports {
        let run = match &to {
            Some(MidiTo::Port {
                name: kept,
                channel,
            }) if kept == name => set(json!({ "name": name, "channel": channel })),
            _ => set(json!({ "name": name, "channel": 1 })),
        };
        entries.push(Entry::choice(name, run, chosen_port == Some(name.as_str())));
    }
    if let Some(name) = chosen_port.filter(|name| !ports.iter().any(|p| p == name)) {
        entries.push(Entry::Choice {
            label: name.to_owned(),
            run: None,
            chosen: true,
            pending: None,
        });
    }
    entries.push(configure("tempo-midi"));
    entries.push(Entry::Separator);
    for id in doc.track_order() {
        if id != track && doc.track(id).map(bl_model::Track::kind) == Some(TrackKind::Midi) {
            entries.push(Entry::choice(
                track_label(doc, id),
                set(json!(id)),
                to == Some(MidiTo::Track(id)),
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(Entry::choice("No Output", set(Value::Null), to.is_none()));
    chooser("Output Type", entries)
}

/// The channel of MIDI To: 1 to 16 with a port, Track In with a track, and
/// nothing with No Output.
fn midi_to_channel(track: TrackId, to: Option<&MidiTo>) -> Chooser {
    match to {
        Some(MidiTo::Track(_)) => Chooser {
            name: "Output Channel",
            text: "Track In".to_owned(),
            entries: vec![Entry::Choice {
                label: "Track In".to_owned(),
                run: None,
                chosen: true,
                pending: None,
            }],
            pending: None,
        },
        Some(MidiTo::Port { name, channel }) => Chooser {
            name: "Output Channel",
            text: channel.to_string(),
            entries: (1..=16u8)
                .map(|n| {
                    Entry::choice(
                        n.to_string(),
                        Invocation::new(
                            "routing.set_midi_output",
                            json!({ "track": track, "to": { "name": name, "channel": n } }),
                        ),
                        n == *channel,
                    )
                })
                .collect(),
            pending: None,
        },
        None => Chooser {
            name: "Output Channel",
            text: String::new(),
            entries: Vec::new(),
            pending: None,
        },
    }
}

/// The output channel: Track In with a track chosen (observed; in Live 12
/// it lists Track In alone even when that track's devices have sidechain
/// inputs, which take their source themselves),
/// the interface's outputs with Ext. Out (the single channels, then the
/// pairs; dimmed with no device open), nothing otherwise. A group as the
/// output has no channel.
fn output_channel(track: TrackId, output: Output, outputs: u16) -> Chooser {
    match output {
        Output::Track { .. } => Chooser {
            name: "Output Channel",
            text: "Track In".to_owned(),
            entries: vec![Entry::Choice {
                label: "Track In".to_owned(),
                run: None,
                chosen: true,
                pending: None,
            }],
            pending: None,
        },
        Output::External { channel } => {
            // A choice past the device's outputs is kept and drawn dimmed,
            // like the chooser with no device open (unobserved).
            let last = match channel {
                Channel::Mono(c) => c,
                Channel::Pair(c) => c.saturating_add(1),
            };
            Chooser {
                name: "Output Channel",
                text: channel.label(),
                entries: if last <= outputs {
                    interface_channels(outputs, channel, |channel| {
                        set_output(
                            track,
                            json!({ "type": "external", "channel": channel_value(channel) }),
                        )
                    })
                } else {
                    Vec::new()
                },
                pending: None,
            }
        }
        _ => Chooser {
            name: "Output Channel",
            text: String::new(),
            entries: Vec::new(),
            pending: None,
        },
    }
}

/// The chains a track's racks offer to Audio From, in the order the chooser
/// lists them (the racks spec, R38, R39): the racks in signal order, each
/// chain followed by the racks inside it, with its rack. Instrument and
/// audio effect racks only; a drum rack offers its return chains instead
/// (the drum racks spec, R10).
fn rack_chains(doc: &Document, track: TrackId) -> Vec<(&Device, &Chain)> {
    fn walk<'a>(devices: impl Iterator<Item = &'a Device>, out: &mut Vec<(&'a Device, &'a Chain)>) {
        for rack in devices {
            let offered = matches!(
                rack.rack_kind(),
                Some(RackKind::Instrument | RackKind::AudioEffect)
            );
            let drum = rack.rack_kind() == Some(RackKind::Drum);
            for chain in &rack.chains {
                if offered || (drum && chain.is_return) {
                    out.push((rack, chain));
                }
                walk(chain.devices.iter(), out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(t) = doc.track(track) {
        walk(t.devices(), &mut out);
    }
    out
}

/// A chain tap as the chooser reads it: the rack's title, the chain's name
/// and the tap, with vertical bars between (the racks spec, R39).
fn chain_tap_label(rack: &Device, chain: &Chain, tap: Tap) -> String {
    format!("{} | {} | {}", rack.title(), chain.name, tap_label(tap))
}

/// The devices on a track, at any depth in its racks, that offer extra audio
/// outputs, each with the output's place and name, in signal order
/// (WP-5.45). Only what its device type declares: a device whose type the
/// host hasn't met offers none. Manual 17.5.2.5 leaves the order and the
/// label open, and speaks of instruments alone; a plug-in that offers extra
/// audio ports has them listed too, whatever its role.
fn device_outputs<'a>(
    doc: &'a Document,
    track: TrackId,
    interface: &'a Interface,
) -> Vec<(&'a Device, u32, &'a str)> {
    let mut out = Vec::new();
    let Some(t) = doc.track(track) else {
        return out;
    };
    for device in t.devices_deep() {
        let Some(names) = interface.device_outputs.get(&device.id) else {
            continue;
        };
        for (index, name) in names.iter().enumerate() {
            out.push((device, index as u32, name.as_str()));
        }
    }
    out
}

/// A device's extra output as the chooser reads it: the device's title and
/// the output's name, with a vertical bar between (WP-5.45).
fn instrument_output_label(device: &Device, name: &str) -> String {
    format!("{} | {}", device.title(), name)
}

/// A chooser showing its chosen entry.
fn chooser(name: &'static str, entries: Vec<Entry>) -> Chooser {
    let text = entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Choice {
                label,
                chosen: true,
                ..
            } => Some(label.clone()),
            _ => None,
        })
        .unwrap_or_default();
    Chooser {
        name,
        text,
        entries,
        pending: None,
    }
}

fn set_sidechain_input(device: DeviceId, input: Value) -> Invocation {
    Invocation::new(
        "device.set_sidechain_input",
        json!({ "device": device, "input": input }),
    )
}

const fn tap_name(tap: Tap) -> &'static str {
    match tap {
        Tap::PreFx => "pre_fx",
        Tap::PostFx => "post_fx",
        Tap::PostMixer => "post_mixer",
    }
}

/// A device's sidechain Source chooser (WP-2.19b; observed in Live 12.4.6):
/// the audio and group tracks in display order, the device's own track
/// among them, the returns, and Main ─ No Input. Choosing a track takes it
/// Post FX. `None` for a device without a sidechain input.
#[must_use]
pub fn sidechain_source(doc: &Document, device: DeviceId) -> Option<Chooser> {
    let input = doc.device(device)?.sidechain.as_ref()?.input.clone();
    let current = match input {
        Input::Track { track, .. } => Some(track),
        _ => None,
    };
    let mut entries = Vec::new();
    for id in doc.track_order() {
        let listed = doc.track(id).is_some_and(|t| {
            matches!(
                t.kind(),
                TrackKind::Audio | TrackKind::Group | TrackKind::Return | TrackKind::Main
            )
        });
        if listed {
            entries.push(Entry::choice(
                track_label(doc, id),
                set_sidechain_input(
                    device,
                    json!({ "type": "track", "track": id, "tap": tap_name(Tap::PostFx) }),
                ),
                current == Some(id),
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(Entry::choice(
        "No Input",
        set_sidechain_input(device, json!({ "type": "none" })),
        input == Input::None,
    ));
    let mut chooser = chooser("Sidechain Source", entries);
    if current.is_none() {
        "No Input".clone_into(&mut chooser.text);
    }
    Some(chooser)
}

/// The choosers of an external instrument's section (the external devices
/// spec, R9): MIDI To, its channel, and Audio From.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalChoosers {
    /// MIDI To: the output ports whose Track switch is on, No Output, and
    /// Configure… at the end.
    pub midi_to: Chooser,
    /// The MIDI channel, 1 to 16.
    pub channel: Chooser,
    /// Audio From: the interface's input channels, the single channels then
    /// the pairs, and Configure… at the end.
    pub audio_from: Chooser,
}

fn set_external(device: DeviceId, change: Value) -> Invocation {
    let mut args = json!({ "device": device });
    if let (Some(args), Value::Object(change)) = (args.as_object_mut(), change) {
        args.extend(change);
    }
    Invocation::new("device.set_external", args)
}

/// An external device's choosers (observed in Live 12.4.6 only as far as the
/// manual names them): MIDI To lists the output ports, a port the device
/// keeps that is away or has Track off dimmed, then No Output; Audio From
/// lists the open device's inputs. `None` for a device without routing to
/// hardware.
#[must_use]
pub fn external_choosers(
    doc: &Document,
    device: DeviceId,
    interface: &Interface,
) -> Option<ExternalChoosers> {
    let external = doc.device(device)?.external.clone()?;
    let kept = external.port.as_deref();
    let mut midi_to = Vec::new();
    for name in &interface.midi_outputs {
        midi_to.push(Entry::choice(
            name,
            set_external(
                device,
                json!({ "midi_to": { "type": "port", "name": name } }),
            ),
            kept == Some(name.as_str()),
        ));
    }
    if let Some(name) = kept.filter(|name| !interface.midi_outputs.iter().any(|p| p == name)) {
        midi_to.push(Entry::Choice {
            label: name.to_owned(),
            run: None,
            chosen: true,
            pending: None,
        });
    }
    midi_to.push(Entry::choice(
        "No Output",
        set_external(device, json!({ "midi_to": { "type": "none" } })),
        kept.is_none(),
    ));
    midi_to.push(Entry::Separator);
    midi_to.push(configure("tempo-midi"));
    let mut audio_from = interface_channels(interface.inputs, external.audio_from, |channel| {
        set_external(device, json!({ "audio_from": channel_value(channel) }))
    });
    audio_from.push(Entry::Separator);
    audio_from.push(configure("audio"));
    let mut audio = chooser("External Audio From", audio_from);
    if audio.text.is_empty() {
        external.audio_from.label().clone_into(&mut audio.text);
    }
    Some(ExternalChoosers {
        midi_to: chooser("External MIDI To", midi_to),
        channel: Chooser {
            name: "External MIDI Channel",
            text: external.channel.to_string(),
            entries: (1..=16u8)
                .map(|n| {
                    Entry::choice(
                        n.to_string(),
                        set_external(device, json!({ "channel": n })),
                        n == external.channel,
                    )
                })
                .collect(),
            pending: None,
        },
        audio_from: audio,
    })
}

fn set_midi_sidechain_source(device: DeviceId, track: Value) -> Invocation {
    let args = if track.is_null() {
        json!({ "device": device })
    } else {
        json!({ "device": device, "track": track })
    };
    Invocation::new("device.set_midi_sidechain_source", args)
}

/// A device's MIDI sidechain source chooser (WP-5.27; the routing spec,
/// R34): the MIDI tracks in display order, the device's own among them,
/// then ─ No Input. `None` for a device without a MIDI sidechain input.
#[must_use]
pub fn midi_sidechain_source(doc: &Document, device: DeviceId) -> Option<Chooser> {
    let current = doc.device(device)?.midi_sidechain.as_ref()?.source;
    let mut entries = Vec::new();
    for id in doc.track_order() {
        if doc.track(id).map(bl_model::Track::kind) == Some(TrackKind::Midi) {
            entries.push(Entry::choice(
                track_label(doc, id),
                set_midi_sidechain_source(device, json!(id)),
                current == Some(id),
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(Entry::choice(
        "No Input",
        set_midi_sidechain_source(device, Value::Null),
        current.is_none(),
    ));
    let mut chooser = chooser("MIDI Sidechain Source", entries);
    if current.is_none() {
        "No Input".clone_into(&mut chooser.text);
    }
    Some(chooser)
}

/// A device's sidechain channel chooser: Pre FX · Post FX · Post Mixer with
/// a track as the source, empty with No Input (observed). `None` for a
/// device without a sidechain input.
#[must_use]
pub fn sidechain_channel(doc: &Document, device: DeviceId) -> Option<Chooser> {
    let input = doc.device(device)?.sidechain.as_ref()?.input.clone();
    Some(match input {
        Input::Track { track, tap, .. } => Chooser {
            name: "Sidechain Source Channel",
            text: tap_label(tap).to_owned(),
            entries: [Tap::PreFx, Tap::PostFx, Tap::PostMixer]
                .into_iter()
                .map(|t| {
                    Entry::choice(
                        tap_label(t),
                        set_sidechain_input(
                            device,
                            json!({ "type": "track", "track": track, "tap": tap_name(t) }),
                        ),
                        t == tap,
                    )
                })
                .collect(),
            pending: None,
        },
        _ => Chooser {
            name: "Sidechain Source Channel",
            text: String::new(),
            entries: Vec::new(),
            pending: None,
        },
    })
}
