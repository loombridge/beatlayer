//! The rules for a track's In/Out settings (the routing spec): which
//! inputs and outputs each kind of track takes. A routing may make a loop,
//! as Live allows (observed); the engine's compiler breaks it.

use std::collections::BTreeSet;

use crate::clip::Clip;
use crate::command::check_clip_envelope_value;
use crate::device::DeviceRole;
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::{ChainId, TrackId};
use crate::rack::RackKind;
use crate::track::{Channel, Input, MidiTo, Output, Routing, Track, TrackKind};

impl Document {
    /// The devices that work with hardware outside the computer and are
    /// switched on, each as the name its title bar shows and its ID: what an
    /// offline render silences (the external devices spec, R16).
    #[must_use]
    pub fn external_devices(&self) -> Vec<String> {
        self.tracks()
            .flat_map(|track| track.devices_deep())
            .filter(|device| device.external.is_some() && device.enabled)
            .map(|device| format!("{} ({})", device.title(), device.id))
            .collect()
    }

    /// Where a track's audio goes in the end: Main, a group or another
    /// audio track, or nowhere (Sends Only, Ext. Out). `None` for Main
    /// itself.
    #[must_use]
    pub fn output_of(&self, track: TrackId) -> Option<TrackId> {
        let t = self.track(track)?;
        match t.kind() {
            TrackKind::Main => None,
            TrackKind::Return => match t.routing.output {
                Output::Default | Output::Main => Some(self.main),
                Output::Track { track } => Some(track),
                Output::External { .. } | Output::SendsOnly => None,
            },
            _ => match t.routing.output {
                Output::Default => Some(self.parent(track).unwrap_or(self.main)),
                Output::Main => Some(self.main),
                Output::Track { track } => Some(track),
                Output::External { .. } | Output::SendsOnly => None,
            },
        }
    }

    /// Checks a track's In/Out settings against its kind and the tracks
    /// they name.
    pub(crate) fn check_routing(&self, id: TrackId) -> Result<(), ModelError> {
        let track = self.track_ref(id)?;
        check_input(self, track)?;
        check_output(self, track)?;
        check_main_out(track)?;
        if let Some(to) = &track.routing.midi_output {
            if track.kind != TrackKind::Midi {
                return Err(wrong_kind(track, "a MIDI track"));
            }
            match to {
                MidiTo::Track(to) => {
                    self.check_target(track, *to, |kind| kind == TrackKind::Midi)?;
                }
                MidiTo::Port { name, channel } => check_port(name, *channel)?,
            }
        }
        for device in track.devices_deep() {
            if let Some(sidechain) = &device.sidechain {
                check_sidechain_input(self, track, sidechain.input.clone())?;
            }
            // Spec: external-devices R9, R12
            if let Some(external) = &device.external {
                check_port(external.port.as_deref().unwrap_or("-"), external.channel)?;
                if !external.hardware_latency.is_valid() {
                    return Err(ModelError::InvalidInput(
                        "the hardware latency is out of range",
                    ));
                }
            }
            if let Some(source) = device.midi_sidechain.as_ref().and_then(|s| s.source) {
                check_midi_sidechain_source(self, track, source)?;
            }
        }
        Ok(())
    }

    /// Checks every parameter a track's automation drives: a control of its
    /// mixer, one of its sends to a return track, or a parameter of a device
    /// in its chain. What a loaded document must pass.
    pub(crate) fn check_automation(&self, id: TrackId) -> Result<(), ModelError> {
        let track = self.track_ref(id)?;
        for target in track.automation.keys() {
            self.check_automation_target(id, *target)?;
        }
        Ok(())
    }

    /// Checks every envelope the clips on `id` carry (WP-3.15): each names a
    /// parameter of the track — one envelope per parameter — and its
    /// breakpoints follow the automation rules. What a loaded document, and
    /// a clip put on the track, must pass.
    pub(crate) fn check_clip_envelopes(&self, id: TrackId, clip: &Clip) -> Result<(), ModelError> {
        self.track_ref(id)?;
        let mut seen = BTreeSet::new();
        let in_slot = self
            .track_ref(id)?
            .slots()
            .any(|(_, slotted)| slotted.id == clip.id);
        for envelope in clip.envelopes() {
            if !seen.insert((envelope.target, envelope.kind)) {
                return Err(ModelError::DuplicateEnvelope { clip: clip.id });
            }
            // An Arrangement clip's envelopes only modulate (WP-3.12).
            if envelope.kind == crate::EnvelopeKind::Automation && !in_slot {
                return Err(ModelError::NotSlotClip(clip.id));
            }
            // A MIDI controller's envelope only modulates (WP-2.29).
            if envelope.kind == crate::EnvelopeKind::Automation && envelope.target.is_midi() {
                return Err(ModelError::MidiControllerNotAParameter);
            }
            crate::command::check_clip_setting_kind(envelope.target, envelope.kind)?;
            self.check_clip_target(id, clip, envelope.target)?;
            envelope.envelope.check()?;
            if envelope.target.is_midi() || envelope.target.is_clip() {
                for point in envelope.envelope.points() {
                    check_clip_envelope_value(envelope.target, point.value)?;
                }
            }
        }
        Ok(())
    }

    fn check_target(
        &self,
        track: &Track,
        to: TrackId,
        allowed: impl Fn(TrackKind) -> bool,
    ) -> Result<(), ModelError> {
        let target = self.track(to).ok_or(ModelError::NoSuchTrack(to))?;
        if to == track.id || !allowed(target.kind) {
            return Err(ModelError::CannotRoute {
                track: track.id,
                to,
            });
        }
        Ok(())
    }

    /// Puts every routing that names `gone` out of use, for a track that's
    /// deleted, as Live does (observed): outputs to it become Sends Only,
    /// inputs from it No Input, MIDI outputs to it No Output, and devices'
    /// sidechain sources No Input, their External switch left as it was
    /// (WP-2.19b).
    pub(crate) fn forget_routes_to(&mut self, gone: &BTreeSet<TrackId>) {
        let touched: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|(_, t)| {
                names_any(&t.routing, gone)
                    || t.devices_deep()
                        .any(|d| listens_to(d, gone) || hears(d, gone))
            })
            .map(|(&id, _)| id)
            .collect();
        for id in touched {
            if let Some(track) = self.tracks.get_mut(&id) {
                let track = crate::shared::Shared::make_mut(track);
                for device in track.devices.iter_mut() {
                    if device.walk().any(|d| listens_to(d, gone) || hears(d, gone)) {
                        crate::shared::Shared::make_mut(device).walk_mut(&mut |device| {
                            if listens_to(device, gone)
                                && let Some(sidechain) = &mut device.sidechain
                            {
                                sidechain.input = Input::None;
                            }
                            if hears(device, gone)
                                && let Some(sidechain) = &mut device.midi_sidechain
                            {
                                sidechain.source = None;
                            }
                        });
                    }
                }
                let routing = &mut track.routing;
                if let Output::Track { track } = routing.output
                    && gone.contains(&track)
                {
                    routing.output = Output::SendsOnly;
                }
                if let Input::Track { track, .. } = &routing.input
                    && gone.contains(track)
                {
                    routing.input = Input::None;
                }
                if routing
                    .midi_output
                    .as_ref()
                    .and_then(MidiTo::track)
                    .is_some_and(|to| gone.contains(&to))
                {
                    routing.midi_output = None;
                }
            }
        }
    }

    /// Checks that a MIDI track an audio track takes as its input holds an
    /// instrument, which makes it a source (the racks spec, R38). Once set,
    /// the input stays if the instrument goes, and carries silence.
    pub(crate) fn check_instrument_source(
        &self,
        track: TrackId,
        input: &Input,
    ) -> Result<(), ModelError> {
        let Input::Track { track: from, .. } = input else {
            return Ok(());
        };
        let silent = self
            .track(track)
            .is_some_and(|t| t.kind == TrackKind::Audio)
            && self.track(*from).is_some_and(|source| {
                source.kind == TrackKind::Midi
                    && !source.devices().any(|d| d.role == DeviceRole::Instrument)
            });
        if silent {
            return Err(ModelError::CannotRoute { track, to: *from });
        }
        Ok(())
    }

    /// Puts every input that takes an extra output of a device in `gone` out
    /// of use, as for a deleted track (the routing spec, R7).
    pub(crate) fn forget_device_outputs(&mut self, gone: &BTreeSet<crate::DeviceId>) {
        let touched: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|(_, t)| {
                matches!(&t.routing.input, Input::Track { output: Some(output), .. }
                    if gone.contains(&output.device))
            })
            .map(|(&id, _)| id)
            .collect();
        for id in touched {
            if let Some(track) = self.tracks.get_mut(&id) {
                crate::shared::Shared::make_mut(track).routing.input = Input::None;
            }
        }
    }

    /// Puts every input that taps a chain in `gone` out of use, for a chain
    /// or a rack that left its track: No Input, as for a deleted track
    /// (the racks spec, R41).
    pub(crate) fn forget_chain_taps(&mut self, gone: &BTreeSet<ChainId>) {
        let touched: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|(_, t)| taps_any(&t.routing.input, gone))
            .map(|(&id, _)| id)
            .collect();
        for id in touched {
            if let Some(track) = self.tracks.get_mut(&id) {
                crate::shared::Shared::make_mut(track).routing.input = Input::None;
            }
        }
    }

    /// Brings a moved track's output up to date with where it is now: a
    /// track going into a group that was routed to Main goes to the group
    /// (as Live does), and a track routed to its group goes to its new
    /// group or Main.
    pub(crate) fn settle_output(&mut self, id: TrackId, old_parent: Option<TrackId>) {
        let parent = self.parent(id);
        let Some(track) = self.tracks.get_mut(&id) else {
            return;
        };
        let output = track.routing.output;
        let settled = match output {
            Output::Main if parent.is_some() && old_parent.is_none() => Output::Default,
            Output::Track { track } if Some(track) == parent => Output::Default,
            Output::Track { track } if Some(track) == old_parent => Output::Default,
            other => other,
        };
        if settled != output {
            crate::shared::Shared::make_mut(track).routing.output = settled;
        }
    }

    /// `output` as it's stored: Main from a top-level track, or the group
    /// a track is in, is [`Output::Default`].
    pub(crate) fn normal_output(&self, id: TrackId, output: Output) -> Output {
        let parent = self.parent(id);
        match output {
            Output::Main if parent.is_none() => Output::Default,
            Output::Track { track } if Some(track) == parent => Output::Default,
            other => other,
        }
    }
}

/// Whether a device's sidechain takes its source from a track in `gone`.
fn listens_to(device: &crate::Device, gone: &BTreeSet<TrackId>) -> bool {
    device
        .sidechain
        .as_ref()
        .is_some_and(|s| matches!(&s.input, Input::Track { track, .. } if gone.contains(track)))
}

/// Whether a device's MIDI sidechain takes its source from a track in
/// `gone`.
fn hears(device: &crate::Device, gone: &BTreeSet<TrackId>) -> bool {
    device
        .midi_sidechain
        .as_ref()
        .and_then(|s| s.source)
        .is_some_and(|track| gone.contains(&track))
}

/// Whether an input taps a chain in `gone`.
fn taps_any(input: &Input, gone: &BTreeSet<ChainId>) -> bool {
    matches!(input, Input::Track { chain: Some(chain), .. } if gone.contains(chain))
}

fn names_any(routing: &Routing, gone: &BTreeSet<TrackId>) -> bool {
    matches!(routing.output, Output::Track { track } if gone.contains(&track))
        || matches!(&routing.input, Input::Track { track, .. } if gone.contains(track))
        || routing
            .midi_output
            .as_ref()
            .and_then(MidiTo::track)
            .is_some_and(|to| gone.contains(&to))
}

fn wrong_kind(track: &Track, needs: &'static str) -> ModelError {
    ModelError::WrongTrackKind {
        track: track.id,
        kind: track.kind,
        needs,
    }
}

/// An audio track takes Ext. In (one channel or a stereo pair), Resampling,
/// another audio or group track, a return, Main, or nothing; a MIDI track
/// All Ins, the computer keyboard, a port, another MIDI track, or nothing;
/// other kinds take no input. A port needs a name, and a channel belongs to
/// a port (the routing spec, R2, R16).
fn check_input(doc: &Document, track: &Track) -> Result<(), ModelError> {
    let input = &track.routing.input;
    if let Input::Port { name } = input
        && name.is_empty()
    {
        return Err(ModelError::InvalidInput("a port needs a name"));
    }
    if let Some(channel) = track.routing.channel {
        if !matches!(input, Input::Port { .. }) {
            return Err(ModelError::InvalidInput("only a port input has a channel"));
        }
        if !(1..=16).contains(&channel) {
            return Err(ModelError::InvalidInput("the MIDI channel must be 1 to 16"));
        }
    }
    match (track.kind, input) {
        (TrackKind::Audio | TrackKind::Midi, Input::None)
        | (TrackKind::Audio, Input::External { .. } | Input::Resampling)
        | (TrackKind::Midi, Input::AllIns | Input::ComputerKeyboard | Input::Port { .. }) => Ok(()),
        (
            TrackKind::Audio,
            Input::Track {
                track: from,
                chain,
                output,
                ..
            },
        ) => {
            // A MIDI track is an Audio From source (the racks spec, R38);
            // `check_instrument_source` asks for its instrument when the
            // input is set, not when a set loads.
            doc.check_target(track, *from, |kind| {
                matches!(
                    kind,
                    TrackKind::Audio
                        | TrackKind::Midi
                        | TrackKind::Group
                        | TrackKind::Return
                        | TrackKind::Main
                )
            })?;
            if chain.is_some() && output.is_some() {
                return Err(ModelError::InvalidInput(
                    "a track's input takes a chain or an instrument output, not both",
                ));
            }
            if let Some(output) = output {
                check_device_output(doc, *from, output)?;
            }
            chain.map_or(Ok(()), |chain| check_chain_tap(doc, *from, chain))
        }
        (
            TrackKind::Midi,
            Input::Track {
                track: from,
                chain,
                output,
                ..
            },
        ) => {
            if chain.is_some() {
                return Err(ModelError::InvalidInput("only an audio track taps a chain"));
            }
            if output.is_some() {
                return Err(ModelError::InvalidInput(
                    "only an audio track takes an instrument output",
                ));
            }
            doc.check_target(track, *from, |kind| kind == TrackKind::Midi)
        }
        (TrackKind::Audio, _) => Err(wrong_kind(track, "a MIDI track")),
        (TrackKind::Midi, _) => Err(wrong_kind(track, "an audio track")),
        (_, Input::None) => Ok(()),
        _ => Err(wrong_kind(track, "an audio or MIDI track")),
    }
}

/// A device output names a device on the source track, at any depth in its
/// racks: the model doesn't know which devices have extra outputs, which is
/// the host's account of them (the routing spec, R16).
fn check_device_output(
    doc: &Document,
    from: TrackId,
    output: &crate::DeviceOutput,
) -> Result<(), ModelError> {
    let source = doc.track(from).ok_or(ModelError::NoSuchTrack(from))?;
    source
        .devices_deep()
        .find(|d| d.id == output.device)
        .ok_or(ModelError::NoSuchDevice(output.device))?;
    Ok(())
}

/// A chain tap names a chain of an instrument or audio effect rack on the
/// source track (the racks spec, R38), or a return chain of a drum rack (the
/// drum racks spec, R10).
fn check_chain_tap(doc: &Document, from: TrackId, chain: ChainId) -> Result<(), ModelError> {
    if doc.chain_track(chain) != Some(from) {
        return Err(ModelError::NoSuchChain(chain));
    }
    let returned = doc.chain(chain).is_some_and(|c| c.is_return);
    match doc.chain_rack(chain).and_then(crate::Device::rack_kind) {
        Some(RackKind::Instrument | RackKind::AudioEffect) => Ok(()),
        Some(RackKind::Drum) if returned => Ok(()),
        _ => Err(ModelError::InvalidInput(
            "only a chain of an instrument or audio effect rack, or a drum rack's return chain, \
             can be tapped",
        )),
    }
}

/// Audio, group, and return tracks go to Main, the group they're in, an
/// audio track, their sends only, or Ext. Out; MIDI tracks (which have no
/// instrument yet) and Main where they go.
fn check_output(doc: &Document, track: &Track) -> Result<(), ModelError> {
    let output = track.routing.output;
    let routed = matches!(
        track.kind,
        TrackKind::Audio | TrackKind::Group | TrackKind::Return
    );
    match (track.kind, output) {
        (_, Output::Default) => Ok(()),
        (_, Output::Main | Output::External { .. } | Output::SendsOnly) if routed => Ok(()),
        (_, Output::Track { track: to }) if routed => {
            let parent = doc.parent(track.id);
            doc.check_target(track, to, |kind| kind == TrackKind::Audio)
                .or_else(|error| {
                    if Some(to) == parent {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
        }
        (TrackKind::Main, _) => Err(ModelError::MainTrack),
        _ => Err(wrong_kind(track, "an audio, group, or return track")),
    }
}

/// A MIDI output port a track or the external instrument names: a name, and
/// a channel from 1 to 16.
pub(crate) fn check_port(name: &str, channel: u8) -> Result<(), ModelError> {
    if name.is_empty() {
        return Err(ModelError::InvalidInput("a port needs a name"));
    }
    if !(1..=16).contains(&channel) {
        return Err(ModelError::InvalidInput("the MIDI channel must be 1 to 16"));
    }
    Ok(())
}

/// Main Out and Cue Out belong to Main and take a stereo pair of the
/// interface's outputs; every other track keeps them at their default.
fn check_main_out(track: &Track) -> Result<(), ModelError> {
    let (main_out, cue_out) = (track.routing.main_out, track.routing.cue_out);
    if track.kind != TrackKind::Main {
        return if main_out.is_first_pair() && cue_out.is_first_pair() {
            Ok(())
        } else {
            Err(wrong_kind(track, "the Main track"))
        };
    }
    if [main_out, cue_out]
        .iter()
        .all(|c| matches!(c, Channel::Pair(_)))
    {
        Ok(())
    } else {
        Err(ModelError::NotStereoPair)
    }
}

/// A sidechain's Source: No Input, or an audio, group, or return track or
/// Main, the device's own track included (observed). Ext. In isn't listed
/// in Live without an audio device, and is refused until it's observed with
/// one.
fn check_sidechain_input(doc: &Document, track: &Track, input: Input) -> Result<(), ModelError> {
    match input {
        Input::None => Ok(()),
        Input::Track { chain: Some(_), .. } => Err(ModelError::InvalidInput(
            "a sidechain takes a track, not a chain",
        )),
        Input::Track {
            output: Some(_), ..
        } => Err(ModelError::InvalidInput(
            "a sidechain takes a track, not an instrument output",
        )),
        Input::Track { track: from, .. } => {
            let source = doc.track(from).ok_or(ModelError::NoSuchTrack(from))?;
            if matches!(
                source.kind,
                TrackKind::Audio | TrackKind::Group | TrackKind::Return | TrackKind::Main
            ) {
                Ok(())
            } else {
                Err(ModelError::CannotRoute {
                    track: track.id,
                    to: from,
                })
            }
        }
        _ => Err(wrong_kind(track, "a device's sidechain to take a track")),
    }
}

/// A MIDI sidechain's source: a MIDI track, the device's own included.
fn check_midi_sidechain_source(
    doc: &Document,
    track: &Track,
    source: TrackId,
) -> Result<(), ModelError> {
    let from = doc.track(source).ok_or(ModelError::NoSuchTrack(source))?;
    if from.kind == TrackKind::Midi {
        Ok(())
    } else {
        Err(ModelError::CannotRoute {
            track: track.id,
            to: source,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::command::Command;
    use crate::device::{Device, DeviceKind, DeviceRole};
    use crate::editor::Editor;
    use crate::error::ModelError;
    use crate::ids::{ChainId, DeviceId, TrackId};
    use crate::rack::{Chain, RackKind};
    use crate::track::{Channel, Input, MidiTo, Monitor, Output, Routing, Tap, TrackKind};
    use crate::{Document, SendTap};

    fn add(editor: &mut Editor, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
        let id = editor.new_id();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => editor.document().returns().len(),
            (_, Some(group)) => editor.document().track(group).unwrap().children().len(),
            (_, None) => editor.document().top_level().len(),
        };
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent,
                index,
            })
            .unwrap();
        id
    }

    /// An instrument on `track`, named `name`.
    fn instrument(editor: &mut Editor, track: TrackId, name: &str) -> DeviceId {
        let id = editor.new_id();
        let device = Device::new(
            id,
            name,
            DeviceKind::Native {
                device_type: "beatlayer.kitbag".to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device,
            })
            .unwrap();
        id
    }

    fn input(editor: &mut Editor, track: TrackId, input: Input) -> Result<(), ModelError> {
        editor
            .apply(Command::SetInput {
                track,
                input,
                channel: None,
            })
            .map(|_| ())
            .map_err(|e| match e {
                crate::EditError::Command(e) => e,
                other => panic!("{other}"),
            })
    }

    fn output(editor: &mut Editor, track: TrackId, output: Output) -> Result<(), ModelError> {
        editor
            .apply(Command::SetOutput { track, output })
            .map(|_| ())
            .map_err(|e| match e {
                crate::EditError::Command(e) => e,
                other => panic!("{other}"),
            })
    }

    // Spec: routing R1
    #[test]
    fn a_new_tracks_routing_is_lives() {
        let mut editor = Editor::new();
        let audio = add(&mut editor, TrackKind::Audio, None);
        let midi = add(&mut editor, TrackKind::Midi, None);
        let doc = editor.document();
        let routing = doc.track(audio).unwrap().routing().clone();
        assert_eq!(
            routing.input,
            Input::External {
                channel: Channel::Pair(1)
            }
        );
        assert_eq!(routing.monitor, Monitor::Auto);
        assert_eq!(routing.output, Output::Default);
        let routing = doc.track(midi).unwrap().routing().clone();
        assert_eq!(routing.input, Input::AllIns);
        assert_eq!(routing.monitor, Monitor::Auto);
        assert_eq!(routing.midi_output, None);
        assert_eq!(doc.output_of(audio), Some(doc.main_id()));
        assert_eq!(Tap::default(), Tap::PostMixer);
        assert_eq!(doc.track(doc.main_id()).unwrap().mixer().tap, SendTap::Post);
    }

    // Spec: routing R2, R3, R4
    #[test]
    fn each_kind_takes_the_inputs_and_outputs_its_choosers_list() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        let m = add(&mut editor, TrackKind::Midi, None);
        let n = add(&mut editor, TrackKind::Midi, None);
        let g = add(&mut editor, TrackKind::Group, None);
        let inner = add(&mut editor, TrackKind::Audio, Some(g));
        let r = add(&mut editor, TrackKind::Return, None);
        let main = editor.document().main_id();
        let tap = Tap::PreFx;

        for from in [b, g, r, main] {
            input(
                &mut editor,
                a,
                Input::Track {
                    track: from,
                    tap,
                    chain: None,
                    output: None,
                },
            )
            .unwrap();
        }
        for ok in [
            Input::Resampling,
            Input::External {
                channel: Channel::Mono(3),
            },
            Input::None,
        ] {
            input(&mut editor, a, ok).unwrap();
        }
        assert!(matches!(
            input(
                &mut editor,
                a,
                Input::Track {
                    track: a,
                    tap,
                    chain: None,
                    output: None
                }
            ),
            Err(ModelError::CannotRoute { .. })
        ));
        assert!(
            input(
                &mut editor,
                a,
                Input::Track {
                    track: m,
                    tap,
                    chain: None,
                    output: None
                }
            )
            .is_err()
        );
        assert!(input(&mut editor, a, Input::AllIns).is_err());
        input(
            &mut editor,
            m,
            Input::Track {
                track: n,
                tap,
                chain: None,
                output: None,
            },
        )
        .unwrap();
        input(&mut editor, m, Input::ComputerKeyboard).unwrap();
        assert!(
            input(
                &mut editor,
                m,
                Input::Track {
                    track: a,
                    tap,
                    chain: None,
                    output: None
                }
            )
            .is_err()
        );
        // An instrument output goes to an audio track's input, never a MIDI
        // track's (WP-5.45).
        let elsewhere = editor.new_id();
        assert!(
            input(
                &mut editor,
                m,
                Input::Track {
                    track: n,
                    tap,
                    chain: None,
                    output: Some(crate::DeviceOutput {
                        device: elsewhere,
                        output: 0,
                    }),
                },
            )
            .is_err()
        );
        assert!(input(&mut editor, m, Input::Resampling).is_err());
        assert!(
            input(
                &mut editor,
                g,
                Input::External {
                    channel: Channel::Pair(1)
                }
            )
            .is_err()
        );
        assert!(
            input(
                &mut editor,
                r,
                Input::External {
                    channel: Channel::Pair(2)
                }
            )
            .is_err()
        );

        // Audio To: Main, an audio track, the track's group, Sends Only, Ext. Out.
        for to in [
            Output::Main,
            Output::Track { track: b },
            Output::SendsOnly,
            Output::External {
                channel: Channel::Pair(3),
            },
            Output::External {
                channel: Channel::Mono(2),
            },
        ] {
            output(&mut editor, a, to).unwrap();
            output(&mut editor, r, to).unwrap();
            output(&mut editor, g, to).unwrap();
        }
        output(&mut editor, g, Output::Track { track: inner }).unwrap();
        output(&mut editor, inner, Output::Main).unwrap();
        output(&mut editor, inner, Output::Track { track: g }).unwrap();
        assert_eq!(
            editor.document().track(inner).unwrap().routing().output,
            Output::Default,
            "a track's own group is its default"
        );
        assert!(output(&mut editor, a, Output::Track { track: g }).is_err());
        assert!(output(&mut editor, a, Output::Track { track: r }).is_err());
        assert!(output(&mut editor, a, Output::Track { track: a }).is_err());
        assert!(output(&mut editor, m, Output::SendsOnly).is_err());
        assert!(output(&mut editor, main, Output::SendsOnly).is_err());
        output(&mut editor, a, Output::Main).unwrap();
        assert_eq!(
            editor.document().track(a).unwrap().routing().output,
            Output::Default,
            "Main from the top level is the default"
        );

        editor
            .apply(Command::SetMidiOutput {
                track: m,
                to: Some(MidiTo::Track(n)),
            })
            .unwrap();
        assert!(
            editor
                .apply(Command::SetMidiOutput {
                    track: m,
                    to: Some(MidiTo::Track(a))
                })
                .is_err()
        );
        assert!(
            editor
                .apply(Command::SetMidiOutput {
                    track: a,
                    to: Some(MidiTo::Track(m))
                })
                .is_err()
        );
        assert!(
            editor
                .apply(Command::SetMonitor {
                    track: g,
                    monitor: Monitor::In
                })
                .is_err()
        );
        editor.document().check_invariants().unwrap();
    }

    // Spec: routing R2, WP-2.13
    #[test]
    fn changes_to_ext_in_and_main_out_are_routing_edits_undo_removes() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let main = editor.document().main_id();
        for channel in [
            Channel::Mono(3),
            Channel::Pair(3),
            Channel::Mono(1),
            Channel::Pair(1),
        ] {
            input(&mut editor, a, Input::External { channel }).unwrap();
            assert_eq!(
                editor.document().track(a).unwrap().routing().input,
                Input::External { channel }
            );
        }
        editor
            .apply(Command::SetMainOut {
                channel: Channel::Pair(3),
            })
            .unwrap();
        editor
            .apply(Command::SetCueOut {
                channel: Channel::Pair(2),
            })
            .unwrap();
        let routing = editor.document().track(main).unwrap().routing().clone();
        assert_eq!(routing.main_out, Channel::Pair(3));
        assert_eq!(routing.cue_out, Channel::Pair(2));
        assert_eq!(
            Command::SetMainOut {
                channel: Channel::Pair(1)
            }
            .label(),
            "Change \"Routing\""
        );
        assert!(matches!(
            editor.apply(Command::SetMainOut {
                channel: Channel::Mono(1)
            }),
            Err(crate::EditError::Command(ModelError::NotStereoPair))
        ));
        editor.undo().unwrap();
        assert_eq!(
            editor.document().track(main).unwrap().routing().cue_out,
            Channel::Pair(1),
            "undo restores Cue Out"
        );

        // Saved and loaded, and an unloadable set is refused.
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            saved["main"]["routing"]["main_out"],
            serde_json::json!({ "type": "pair", "channel": 3 })
        );
        let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(&loaded, editor.document());
        let mut broken = saved;
        broken["tracks"][0]["routing"] = serde_json::json!({
            "input": { "type": "none" },
            "monitor": "off",
            "main_out": { "type": "pair", "channel": 3 },
        });
        assert!(
            serde_json::from_value::<Document>(broken).is_err(),
            "Main Out on another track doesn't load"
        );
    }

    // Spec: routing R5
    #[test]
    fn a_loop_is_kept_in_the_set() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        input(
            &mut editor,
            a,
            Input::Track {
                track: b,
                tap: Tap::PostMixer,
                chain: None,
                output: None,
            },
        )
        .unwrap();
        output(&mut editor, a, Output::Track { track: b }).unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(&loaded, editor.document());
    }

    // Spec: routing R6, R7
    #[test]
    fn grouping_and_deleting_carry_routings_as_live_does() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        let c = add(&mut editor, TrackKind::Audio, None);
        let g = add(&mut editor, TrackKind::Group, None);
        let r = add(&mut editor, TrackKind::Return, None);
        output(&mut editor, b, Output::Track { track: c }).unwrap();
        output(&mut editor, r, Output::Track { track: c }).unwrap();
        input(
            &mut editor,
            a,
            Input::Track {
                track: c,
                tap: Tap::PostFx,
                chain: None,
                output: None,
            },
        )
        .unwrap();
        // Grouping a track routed to Main sends it to the group; one routed
        // elsewhere stays.
        for track in [a, b] {
            editor
                .apply(Command::MoveTrack {
                    track,
                    parent: Some(g),
                    index: 0,
                })
                .unwrap();
        }
        let doc = editor.document();
        assert_eq!(doc.output_of(a), Some(g));
        assert_eq!(doc.output_of(b), Some(c));
        // Out of the group again, it goes to Main.
        editor
            .apply(Command::MoveTrack {
                track: a,
                parent: None,
                index: 0,
            })
            .unwrap();
        assert_eq!(
            editor.document().output_of(a),
            Some(editor.document().main_id())
        );

        let before = editor.document().clone();
        editor.apply(Command::DeleteTrack { track: c }).unwrap();
        let doc = editor.document();
        assert_eq!(doc.track(b).unwrap().routing().output, Output::SendsOnly);
        assert_eq!(doc.track(r).unwrap().routing().output, Output::SendsOnly);
        assert_eq!(doc.track(a).unwrap().routing().input, Input::None);
        doc.check_invariants().unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.document(), &before, "undo restores the routings");
    }

    // Spec: routing R8
    #[test]
    fn routing_is_saved_only_where_it_differs_from_a_new_tracks() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let m = add(&mut editor, TrackKind::Midi, None);
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert!(saved["tracks"][0].get("routing").is_none());
        assert!(saved["tracks"][1].get("routing").is_none());
        editor
            .apply(Command::SetMonitor {
                track: a,
                monitor: Monitor::In,
            })
            .unwrap();
        input(&mut editor, a, Input::Resampling).unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            saved["tracks"][0]["routing"],
            serde_json::json!({ "input": { "type": "resampling" }, "monitor": "in" })
        );
        let loaded: Document = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(&loaded, editor.document());
        assert_eq!(
            loaded.track(m).unwrap().routing().clone(),
            Routing::new(TrackKind::Midi)
        );
        // A routing that breaks a rule doesn't load.
        let mut broken = saved;
        broken["tracks"][0]["routing"]["input"] = serde_json::json!({ "type": "all_ins" });
        assert!(serde_json::from_value::<Document>(broken).is_err());
    }

    // Spec: routing R3, R8 (WP-2.13)
    #[test]
    fn ext_out_and_its_channel_save_and_load() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let b = add(&mut editor, TrackKind::Audio, None);
        let c = add(&mut editor, TrackKind::Audio, None);
        for (track, channel) in [
            (a, Channel::Pair(1)),
            (b, Channel::Pair(3)),
            (c, Channel::Mono(3)),
        ] {
            output(&mut editor, track, Output::External { channel }).unwrap();
        }
        let saved = serde_json::to_value(editor.document()).unwrap();
        // The first pair is a new choice's, so it's left out.
        assert_eq!(
            saved["tracks"][0]["routing"]["output"],
            serde_json::json!({ "type": "external" })
        );
        assert_eq!(
            saved["tracks"][2]["routing"]["output"],
            serde_json::json!({ "type": "external", "channel": { "type": "mono", "channel": 3 } })
        );
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(&loaded, editor.document());
        assert_eq!(
            loaded.output_of(b),
            None,
            "Ext. Out goes nowhere in the set"
        );
    }

    // Spec: routing R32, R33
    #[test]
    fn a_midi_sidechain_takes_a_midi_track_and_forgets_a_deleted_one() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let m = add(&mut editor, TrackKind::Midi, None);
        let n = add(&mut editor, TrackKind::Midi, None);
        let id = editor.new_id();
        let kind = crate::DeviceKind::Native {
            device_type: "test.key".to_owned(),
        };
        editor
            .apply(Command::InsertDevice {
                track: a,
                index: 0,
                device: crate::Device::new(id, "Key", kind).with_midi_sidechain(),
            })
            .unwrap();
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            saved["tracks"][0]["devices"][0]["midi_sidechain"],
            serde_json::json!({}),
            "a new device's MIDI sidechain has no source and saves none"
        );
        let set = |editor: &mut Editor, track| {
            editor.apply(Command::SetMidiSidechainSource { device: id, track })
        };
        assert!(
            set(&mut editor, Some(a)).is_err(),
            "an audio track isn't a source"
        );
        set(&mut editor, Some(m)).unwrap();
        set(&mut editor, Some(n)).unwrap();
        let loaded: Document =
            serde_json::from_value(serde_json::to_value(editor.document()).unwrap()).unwrap();
        assert_eq!(&loaded, editor.document());
        let before = editor.document().clone();
        editor.apply(Command::DeleteTrack { track: n }).unwrap();
        let source = |editor: &Editor| editor.document().device(id).unwrap().midi_sidechain.clone();
        assert_eq!(source(&editor).unwrap().source, None);
        editor.undo().unwrap();
        assert_eq!(editor.document(), &before);
        assert_eq!(source(&editor).unwrap().source, Some(n));
        let plain = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track: m,
                index: 0,
                device: crate::Device::new(
                    plain,
                    "Plain",
                    crate::DeviceKind::Native {
                        device_type: "test.plain".to_owned(),
                    },
                ),
            })
            .unwrap();
        assert!(matches!(
            editor.apply(Command::SetMidiSidechainSource {
                device: plain,
                track: None
            }),
            Err(crate::EditError::Command(ModelError::NoMidiSidechain(_)))
        ));
        set(&mut editor, None).unwrap();
    }

    // Spec: routing R9 (IMP-01)
    #[test]
    fn fifty_return_tracks_save_and_load() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let returns: Vec<TrackId> = (0..50)
            .map(|_| add(&mut editor, TrackKind::Return, None))
            .collect();
        for (i, &to) in returns.iter().enumerate() {
            editor
                .apply(Command::SetSend {
                    track: a,
                    to,
                    level: (i + 1) as f64 / 50.0,
                })
                .unwrap();
        }
        let doc = editor.document();
        assert_eq!(doc.returns().len(), 50);
        let loaded: Document = serde_json::from_str(&serde_json::to_string(doc).unwrap()).unwrap();
        assert_eq!(&loaded, doc);
        assert_eq!(loaded.track(a).unwrap().send(returns[49]), 1.0);
    }

    fn sidechained(editor: &mut Editor, track: TrackId) -> crate::DeviceId {
        let id = editor.new_id();
        let kind = crate::DeviceKind::Native {
            device_type: "test.key".to_owned(),
        };
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: crate::Device::new(id, "Key", kind).with_sidechain(),
            })
            .unwrap();
        id
    }

    // Spec: routing R22, R23, R25
    #[test]
    fn sidechains_take_tracks_and_forget_deleted_ones() {
        let mut editor = Editor::new();
        let a = add(&mut editor, TrackKind::Audio, None);
        let c = add(&mut editor, TrackKind::Audio, None);
        let g = add(&mut editor, TrackKind::Group, None);
        let m = add(&mut editor, TrackKind::Midi, None);
        let r = add(&mut editor, TrackKind::Return, None);
        let main = editor.document().main_id();
        let key = sidechained(&mut editor, a);
        let plain = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track: c,
                index: 0,
                device: crate::Device::new(
                    plain,
                    "Plain",
                    crate::DeviceKind::Native {
                        device_type: "test.plain".to_owned(),
                    },
                ),
            })
            .unwrap();

        // A new device's sidechain: External off, No Input, Mix 100 %, SC
        // Gain 0 dB, and nothing of it saved.
        let new = editor
            .document()
            .device(key)
            .unwrap()
            .sidechain
            .clone()
            .unwrap();
        assert_eq!(
            (new.enabled, new.input, new.gain, new.mix, new.listen),
            (false, Input::None, 1.0, 1.0, false)
        );
        let saved = serde_json::to_value(editor.document()).unwrap();
        assert_eq!(
            saved["tracks"][0]["devices"][0]["sidechain"],
            serde_json::json!({})
        );
        assert!(saved["tracks"][1]["devices"][0].get("sidechain").is_none());

        // Source: another track, the device's own, a group, a return, or
        // Main; not a MIDI track, Ext. In, or Resampling.
        let set = |editor: &mut Editor, input| {
            editor.apply(Command::SetSidechainInput { device: key, input })
        };
        for track in [c, a, g, r, main] {
            set(
                &mut editor,
                Input::Track {
                    track,
                    tap: Tap::PostFx,
                    chain: None,
                    output: None,
                },
            )
            .unwrap();
        }
        for bad in [
            Input::Track {
                track: m,
                tap: Tap::PreFx,
                chain: None,
                output: None,
            },
            Input::External {
                channel: Channel::Pair(1),
            },
            Input::Resampling,
        ] {
            assert!(set(&mut editor, bad).is_err());
        }
        assert!(matches!(
            editor.apply(Command::SetSidechainInput {
                device: plain,
                input: Input::None
            }),
            Err(crate::EditError::Command(ModelError::NoSidechain(_)))
        ));
        for bad in [
            Command::SetSidechainGain {
                device: key,
                gain: -1.0,
            },
            Command::SetSidechainGain {
                device: key,
                gain: f64::NAN,
            },
            Command::SetSidechainMix {
                device: key,
                mix: 1.5,
            },
        ] {
            assert!(editor.apply(bad).is_err());
        }
        set(
            &mut editor,
            Input::Track {
                track: c,
                tap: Tap::PostFx,
                chain: None,
                output: None,
            },
        )
        .unwrap();
        for command in [
            Command::SetSidechainEnabled {
                device: key,
                enabled: true,
            },
            Command::SetSidechainGain {
                device: key,
                gain: 2.0,
            },
            Command::SetSidechainMix {
                device: key,
                mix: 0.5,
            },
            Command::SetSidechainListen {
                device: key,
                listen: true,
            },
        ] {
            editor.apply(command).unwrap();
        }
        let loaded: Document =
            serde_json::from_value(serde_json::to_value(editor.document()).unwrap()).unwrap();
        assert_eq!(&loaded, editor.document());
        editor.document().check_invariants().unwrap();

        // Deleting the source leaves No Input, External still on; undo
        // brings both back.
        let before = editor.document().clone();
        editor.apply(Command::DeleteTrack { track: c }).unwrap();
        let sidechain = editor
            .document()
            .device(key)
            .unwrap()
            .sidechain
            .clone()
            .unwrap();
        assert_eq!(sidechain.input, Input::None);
        assert!(sidechain.enabled);
        editor.document().check_invariants().unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.document(), &before);

        // A saved sidechain that names a MIDI track doesn't load.
        let mut broken = serde_json::to_value(&before).unwrap();
        broken["tracks"][0]["devices"][0]["sidechain"]["input"] =
            serde_json::json!({ "type": "track", "track": m });
        assert!(serde_json::from_value::<Document>(broken).is_err());
    }

    /// An audio track with an audio effect rack of two chains, the first
    /// holding another audio effect rack of one chain, and an audio track to
    /// tap them.
    // Spec: routing R2, R7, R16. Taking an instrument's extra output is saved
    // with the set; deleting the device leaves No Input, and undo restores
    // both.
    #[test]
    fn instrument_outputs_forget_deleted_devices() {
        let mut editor = Editor::new();
        let source = add(&mut editor, TrackKind::Midi, None);
        let tapper = add(&mut editor, TrackKind::Audio, None);
        let device = instrument(&mut editor, source, "Impulse");
        let taken = Input::Track {
            track: source,
            tap: Tap::PostMixer,
            chain: None,
            output: Some(crate::DeviceOutput { device, output: 3 }),
        };
        input(&mut editor, tapper, taken.clone()).unwrap();
        assert_eq!(
            editor.document().track(tapper).unwrap().routing().input,
            taken
        );
        // It saves and loads.
        let saved = serde_json::to_value(editor.document()).unwrap();
        let loaded: Document = serde_json::from_value(saved).unwrap();
        assert_eq!(&loaded, editor.document());
        // A device the source track doesn't hold is refused.
        let other = add(&mut editor, TrackKind::Midi, None);
        let elsewhere = instrument(&mut editor, other, "Elsewhere");
        assert!(matches!(
            input(
                &mut editor,
                tapper,
                Input::Track {
                    track: source,
                    tap: Tap::PostMixer,
                    chain: None,
                    output: Some(crate::DeviceOutput {
                        device: elsewhere,
                        output: 0
                    }),
                }
            ),
            Err(ModelError::NoSuchDevice(_))
        ));
        // Deleting the device leaves No Input; undo brings both back.
        let before = editor.document().clone();
        editor.apply(Command::RemoveDevice { device }).unwrap();
        assert_eq!(
            editor.document().track(tapper).unwrap().routing().input,
            Input::None
        );
        editor.document().check_invariants().unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.document(), &before);
    }

    fn racks_to_tap(editor: &mut Editor) -> (TrackId, TrackId, DeviceId, [ChainId; 3]) {
        let source = add(editor, TrackKind::Audio, None);
        let tapper = add(editor, TrackKind::Audio, None);
        let (rack, inner_rack): (DeviceId, DeviceId) = (editor.new_id(), editor.new_id());
        let [a, b, inner]: [ChainId; 3] = std::array::from_fn(|_| editor.new_id());
        let nested =
            Device::rack(inner_rack, RackKind::AudioEffect).with_chain(Chain::new(inner, "Inner"));
        let outer = Device::rack(rack, RackKind::AudioEffect)
            .with_chain(Chain::new(a, "A").with_devices(vec![nested]))
            .with_chain(Chain::new(b, "B"));
        editor
            .apply(Command::InsertDevice {
                track: source,
                index: 0,
                device: outer,
            })
            .unwrap();
        (source, tapper, rack, [a, b, inner])
    }

    fn tap_chain(editor: &mut Editor, tapper: TrackId, source: TrackId, chain: ChainId) {
        input(
            editor,
            tapper,
            Input::Track {
                track: source,
                tap: Tap::PostFx,
                chain: Some(chain),
                output: None,
            },
        )
        .unwrap();
    }

    // Spec: racks R41
    #[test]
    fn chain_taps_forget_deleted_chains() {
        let mut editor = Editor::new();
        let (source, tapper, rack, [a, b, inner]) = racks_to_tap(&mut editor);
        let tap = |chain| Input::Track {
            track: source,
            tap: Tap::PostFx,
            chain: Some(chain),
            output: None,
        };
        // Any depth is tapped, and saved with the set.
        for chain in [a, b, inner] {
            tap_chain(&mut editor, tapper, source, chain);
            assert_eq!(
                editor.document().track(tapper).unwrap().routing().input,
                tap(chain)
            );
            let saved = serde_json::to_value(editor.document()).unwrap();
            let loaded: Document = serde_json::from_value(saved).unwrap();
            assert_eq!(&loaded, editor.document());
        }
        // Not the tapping track's own, nor a chain that isn't on the track.
        assert!(input(&mut editor, source, tap(a)).is_err());
        let other = add(&mut editor, TrackKind::Audio, None);
        assert!(matches!(
            input(
                &mut editor,
                other,
                Input::Track {
                    track: tapper,
                    tap: Tap::PostFx,
                    chain: Some(a),
                    output: None,
                }
            ),
            Err(ModelError::NoSuchChain(_))
        ));

        // Deleting the chain, an inner chain's rack or the rack, or moving
        // them to another track, leaves No Input; undo brings both back.
        let after_taps = |editor: &Editor| {
            editor
                .document()
                .track(tapper)
                .unwrap()
                .routing()
                .input
                .clone()
        };
        for (chain, command) in [
            (b, Command::RemoveChain { chain: b }),
            (inner, Command::RemoveChain { chain: a }),
            (inner, Command::RemoveDevice { device: rack }),
            (
                inner,
                Command::MoveDevice {
                    device: rack,
                    track: other,
                    index: 0,
                },
            ),
        ] {
            tap_chain(&mut editor, tapper, source, chain);
            let before = editor.document().clone();
            editor.apply(command).unwrap();
            assert_eq!(after_taps(&editor), Input::None);
            editor.document().check_invariants().unwrap();
            editor.undo().unwrap();
            assert_eq!(editor.document(), &before, "undo restores the tap");
        }
        // A chain that stays is still tapped when another one goes.
        tap_chain(&mut editor, tapper, source, a);
        editor.apply(Command::RemoveChain { chain: b }).unwrap();
        assert_eq!(after_taps(&editor), tap(a));

        // A track that goes takes its chains' taps too.
        editor
            .apply(Command::DeleteTrack { track: source })
            .unwrap();
        assert_eq!(after_taps(&editor), Input::None);
    }

    // Spec: racks R41
    #[test]
    fn a_chain_moved_into_a_rack_that_offers_no_taps_is_forgotten() {
        let mut editor = Editor::new();
        let tapper = add(&mut editor, TrackKind::Audio, None);
        let source = add(&mut editor, TrackKind::Midi, None);
        let (notes, keys): (DeviceId, DeviceId) = (editor.new_id(), editor.new_id());
        let pad: ChainId = editor.new_id();
        for (index, device) in [
            (0, Device::rack(notes, RackKind::MidiEffect)),
            (
                1,
                Device::rack(keys, RackKind::Instrument).with_chain(Chain::new(pad, "Pad")),
            ),
        ] {
            editor
                .apply(Command::InsertDevice {
                    track: source,
                    index,
                    device,
                })
                .unwrap();
        }
        tap_chain(&mut editor, tapper, source, pad);
        editor
            .apply(Command::MoveChain {
                chain: pad,
                rack: notes,
                index: 0,
            })
            .unwrap();
        assert_eq!(
            editor.document().track(tapper).unwrap().routing().input,
            Input::None
        );
        let loaded: Document =
            serde_json::from_value(serde_json::to_value(editor.document()).unwrap()).unwrap();
        assert_eq!(&loaded, editor.document());
    }

    // Spec: racks R38
    #[test]
    fn only_audio_inputs_tap_instrument_and_audio_effect_racks() {
        let mut editor = Editor::new();
        let (_, tapper, _, [a, ..]) = racks_to_tap(&mut editor);
        let midi = add(&mut editor, TrackKind::Midi, None);
        let drum_track = add(&mut editor, TrackKind::Midi, None);
        let (keys, drums): (DeviceId, DeviceId) = (editor.new_id(), editor.new_id());
        let (keys_chain, drum_chain): (ChainId, ChainId) = (editor.new_id(), editor.new_id());
        for (track, device) in [
            (
                midi,
                Device::rack(keys, RackKind::Instrument).with_chain(Chain::new(keys_chain, "Pad")),
            ),
            (
                drum_track,
                Device::rack(drums, RackKind::Drum).with_chain(Chain::new(drum_chain, "Kick")),
            ),
        ] {
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device,
                })
                .unwrap();
        }
        let tap = |track, chain| Input::Track {
            track,
            tap: Tap::PreFx,
            chain: Some(chain),
            output: None,
        };
        // A MIDI track's instrument rack is tapped from an audio track.
        input(&mut editor, tapper, tap(midi, keys_chain)).unwrap();
        assert!(
            input(&mut editor, tapper, tap(drum_track, drum_chain)).is_err(),
            "a drum rack offers its return chains instead"
        );
        let audio = add(&mut editor, TrackKind::Audio, None);
        assert!(
            input(&mut editor, midi, tap(audio, a)).is_err(),
            "MIDI From"
        );
        // A chain tap is not a sidechain source.
        let sidechain: DeviceId = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track: tapper,
                index: 0,
                device: Device::new(
                    sidechain,
                    "Compressor",
                    DeviceKind::Native {
                        device_type: "test".to_owned(),
                    },
                )
                .with_sidechain(),
            })
            .unwrap();
        assert!(
            editor
                .apply(Command::SetSidechainInput {
                    device: sidechain,
                    input: tap(midi, keys_chain),
                })
                .is_err()
        );
    }

    // Spec: racks R38
    #[test]
    fn a_midi_track_is_an_audio_source_once_it_holds_an_instrument() {
        let mut editor = Editor::new();
        let audio = add(&mut editor, TrackKind::Audio, None);
        let midi = add(&mut editor, TrackKind::Midi, None);
        let from = |track| Input::Track {
            track,
            tap: Tap::PostMixer,
            chain: None,
            output: None,
        };
        assert!(input(&mut editor, audio, from(midi)).is_err());
        let instrument = Device::new(
            editor.new_id(),
            "Sampler",
            DeviceKind::Native {
                device_type: "sampler".to_owned(),
            },
        )
        .with_role(DeviceRole::Instrument);
        let id = instrument.id;
        editor
            .apply(Command::InsertDevice {
                track: midi,
                index: 0,
                device: instrument,
            })
            .unwrap();
        input(&mut editor, audio, from(midi)).unwrap();
        // The input stays, and the set loads, once the instrument is gone.
        editor.apply(Command::RemoveDevice { device: id }).unwrap();
        assert_eq!(
            editor.document().track(audio).unwrap().routing().input,
            from(midi)
        );
        let loaded: Document =
            serde_json::from_value(serde_json::to_value(editor.document()).unwrap()).unwrap();
        assert_eq!(&loaded, editor.document());
    }
}
