//! Track commands that run on the shell (WP-2.19b): Duplicate Track Without
//! Clips (IMP-12), and the whole-track copies of WP-6.52: Duplicate, Copy,
//! Cut and Paste on the selected tracks.
//!
//! A duplicate is made from the document's own commands, inside one
//! transaction, so it's one undo step and each part of it is checked as any
//! edit is.

use bl_model::{
    Automation, AutomationTarget, ChainId, Clip, ClipId, Command, Device, DeviceId, Document,
    EditError, Editor, FrozenTrack, Input, MidiTo, Mixer, NewTakeLane, Output, Routing, SceneId,
    TakeLane, Track, TrackDelay, TrackId, TrackKind,
};

use crate::routing::{return_letter, track_label};

/// The tracks a duplicate copies: those given, less Main and any inside
/// another one given (its group's copy holds its copy), in display order.
#[must_use]
pub fn duplicated(doc: &Document, tracks: &[TrackId]) -> Vec<TrackId> {
    let inside_another = |id: TrackId| {
        let mut parent = doc.parent(id);
        while let Some(group) = parent {
            if tracks.contains(&group) {
                return true;
            }
            parent = doc.parent(group);
        }
        false
    };
    doc.track_order()
        .into_iter()
        .filter(|&id| {
            tracks.contains(&id)
                && doc.track(id).is_some_and(|t| t.kind() != TrackKind::Main)
                && !inside_another(id)
        })
        .collect()
}

/// One of a track's sends: the return track it feeds, that return's place
/// among the set's return tracks, and the level.
#[derive(Clone, Debug, PartialEq)]
pub struct SendTo {
    /// The return track.
    pub to: TrackId,
    /// Its place among the set's return tracks, counted from 0.
    pub position: usize,
    /// The level, as a linear gain above 0.
    pub level: f64,
}

/// What a track holds, as data: what a copy keeps (the track context menu
/// spec, R13), so a copy can be made in another set as well as this one.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackData {
    /// The track's ID in the set it came from, which the references in it
    /// name.
    pub id: TrackId,
    /// The track's kind.
    pub kind: TrackKind,
    /// Its name.
    pub name: String,
    /// Its mixer settings.
    pub mixer: Mixer,
    /// Its sends.
    pub sends: Vec<SendTo>,
    /// Its In/Out settings.
    pub routing: Routing,
    /// Its devices, with the IDs they had.
    pub devices: Vec<Device>,
    /// Its Arrangement clips, with the IDs they had.
    pub clips: Vec<Clip>,
    /// Its Session clips: the scene each sits in, counted from 0, and the
    /// clip.
    pub slots: Vec<(usize, Clip)>,
    /// The scenes whose slot's Clip Stop button was removed, counted from 0.
    pub removed_stop_buttons: Vec<usize>,
    /// Its take lanes.
    pub take_lanes: Vec<TakeLane>,
    /// Its automation: the parameter each envelope drives, and the envelope.
    pub automation: Vec<(AutomationTarget, Automation)>,
    /// Its delay.
    pub delay: TrackDelay,
    /// Whether it keeps its monitoring latency.
    pub keep_monitoring_latency: bool,
    /// Whether it bypasses the set's tuning (WP-6.9).
    pub tuning_bypass: bool,
    /// Its frozen audio, if it is frozen.
    pub frozen: Option<FrozenTrack>,
    /// The tracks inside it, if it is a group.
    pub children: Vec<TrackData>,
}

/// Whole tracks on the clipboard (R14, R15): what they hold, and what is
/// needed to paste them into another set (the set exchange spec, R5 to R7).
#[derive(Clone, Debug, PartialEq)]
pub struct CopiedTracks {
    /// The tracks.
    pub tracks: Vec<TrackData>,
    /// The return tracks of the set they came from, in order.
    pub returns: Vec<TrackId>,
    /// Which set they came from, as the shell counts its sets.
    pub set: u64,
    /// The media table entries of the files they play: the hash, the path
    /// on disk and the size (R15).
    pub media: Vec<(bl_model::ContentHash, std::path::PathBuf, u64)>,
}

impl TrackData {
    /// Whether this track or one inside it plays the file with `hash`:
    /// through a clip, a take lane, its frozen audio or a device's sample.
    #[must_use]
    pub fn plays(&self, hash: &bl_model::ContentHash) -> bool {
        // shortcut: finds the hash in the saved text of what the track holds,
        // since the model has no walk over every place a file is named;
        // replace it with one when a second caller needs it.
        let held = serde_json::to_string(&(
            &self.devices,
            &self.clips,
            &self.slots,
            &self.take_lanes,
            &self.frozen,
        ))
        .unwrap_or_default();
        held.contains(&hash.to_string()) || self.children.iter().any(|child| child.plays(hash))
    }
}

/// What a track holds, as data: the copy the clipboard keeps and the
/// duplicate is made from (R13).
#[must_use]
pub fn snapshot(doc: &Document, id: TrackId) -> Option<TrackData> {
    let track = doc.track(id)?;
    let returns: Vec<TrackId> = doc.returns().collect();
    Some(TrackData {
        id: track.id(),
        kind: track.kind(),
        name: track.name().to_owned(),
        mixer: *track.mixer(),
        sends: track
            .sends()
            .map(|(to, level)| SendTo {
                position: returns.iter().position(|&r| r == to).unwrap_or(0),
                to,
                level,
            })
            .collect(),
        routing: track.routing().clone(),
        devices: track.devices().cloned().collect(),
        clips: track.clips().cloned().collect(),
        slots: track
            .slots()
            .map(|(scene, clip)| (doc.scene_position(scene).unwrap_or(0), clip.clone()))
            .collect(),
        removed_stop_buttons: track
            .removed_stop_buttons()
            .filter_map(|scene| doc.scene_position(scene))
            .collect(),
        take_lanes: track.take_lanes().cloned().collect(),
        automation: track
            .automation()
            .map(|(target, automation)| (*target, automation.clone()))
            .collect(),
        delay: track.delay(),
        keep_monitoring_latency: track.keep_monitoring_latency(),
        tuning_bypass: track.tuning_bypass(),
        frozen: track.frozen().cloned(),
        children: track.children().filter_map(|c| snapshot(doc, c)).collect(),
    })
}

/// The new IDs a copy holds: each original and its copy.
#[derive(Default)]
struct Copies {
    tracks: Vec<(TrackId, TrackId)>,
    devices: Vec<(DeviceId, DeviceId)>,
    chains: Vec<(ChainId, ChainId)>,
    clips: Vec<(ClipId, ClipId)>,
    /// The return tracks of the set the tracks came from, when they are
    /// pasted into another set: IDs are counters in each set, so an ID the
    /// tracks name means nothing in this one (the set exchange spec, R6).
    foreign: Option<Vec<TrackId>>,
}

impl Copies {
    /// The copy of a track, if it was copied.
    fn track(&self, id: TrackId) -> Option<TrackId> {
        self.tracks.iter().find(|(o, _)| *o == id).map(|(_, c)| *c)
    }

    /// The copy of a device, if it was copied.
    fn device(&self, id: DeviceId) -> Option<DeviceId> {
        self.devices.iter().find(|(o, _)| *o == id).map(|(_, c)| *c)
    }

    /// The copy of a chain, if it was copied.
    fn chain(&self, id: ChainId) -> Option<ChainId> {
        self.chains.iter().find(|(o, _)| *o == id).map(|(_, c)| *c)
    }

    /// The copy of a clip, if it was copied.
    fn clip(&self, id: ClipId) -> Option<ClipId> {
        self.clips.iter().find(|(o, _)| *o == id).map(|(_, c)| *c)
    }

    /// Whether the tracks are pasted into another set than they came from.
    fn is_foreign(&self) -> bool {
        self.foreign.is_some()
    }

    /// The copy of a device, or the device itself when it's still in the
    /// set the copy is made in.
    fn device_or_same(&self, doc: &Document, id: DeviceId) -> Option<DeviceId> {
        self.device(id)
            .or_else(|| (!self.is_foreign() && doc.device(id).is_some()).then_some(id))
    }

    /// The copy of a chain, or the chain itself when it's still in the set
    /// the copy is made in.
    fn chain_or_same(&self, doc: &Document, id: ChainId) -> Option<ChainId> {
        self.chain(id)
            .or_else(|| (!self.is_foreign() && doc.chain(id).is_some()).then_some(id))
    }
}

/// Where a reference to a track goes in a copy: the copy when the track was
/// copied too, the same track when the copy is made in the set it came from
/// and the set still has it, and nothing otherwise (the set exchange spec,
/// R6).
fn settle_track(copies: &Copies, doc: &Document, track: TrackId) -> Option<TrackId> {
    copies
        .track(track)
        .or_else(|| (!copies.is_foreign() && doc.track(track).is_some()).then_some(track))
}

/// Where a reference to a return track goes in a copy: the copy when it was
/// copied too, and then the same return track when the set still has it
/// (or, in another set, the return at the same place among the returns),
/// the return at `position` when it doesn't, and nothing when the set has
/// no return there (the set exchange spec, R6).
fn settle_return(
    copies: &Copies,
    doc: &Document,
    to: TrackId,
    position: Option<usize>,
) -> Option<TrackId> {
    let mut returns = doc.returns();
    match &copies.foreign {
        Some(source) => copies
            .track(to)
            .or_else(|| returns.nth(source.iter().position(|&r| r == to)?)),
        None => copies
            .track(to)
            .or_else(|| doc.track(to).map(|_| to))
            .or_else(|| returns.nth(position?)),
    }
}

/// Where a send goes in a copy (the set exchange spec, R6).
fn settle_send(copies: &Copies, editor: &Editor, send: &SendTo) -> Option<TrackId> {
    settle_return(copies, editor.document(), send.to, Some(send.position))
}

/// Where a track's input goes in a copy: the copy of the track it names
/// when that track was copied too, the same track when the set still has
/// it, and the kind's default when it has neither (the set exchange spec,
/// R6).
fn settle_input(copies: &Copies, doc: &Document, input: &Input) -> Option<Input> {
    match input {
        Input::Track {
            track,
            tap,
            chain,
            output,
        } => Some(Input::Track {
            track: settle_track(copies, doc, *track)?,
            tap: *tap,
            chain: chain.and_then(|chain| copies.chain_or_same(doc, chain)),
            output: output.and_then(|output| {
                Some(bl_model::DeviceOutput {
                    device: copies.device_or_same(doc, output.device)?,
                    output: output.output,
                })
            }),
        }),
        other => Some(other.clone()),
    }
}

/// Where a track's output goes in a copy: the copy of the track it names
/// when that track was copied too, the same track when the set still has
/// it, and Main when it has neither (the set exchange spec, R6).
fn settle_output(copies: &Copies, doc: &Document, output: &Output) -> Option<Output> {
    match output {
        Output::Track { track } => Some(Output::Track {
            track: settle_track(copies, doc, *track)?,
        }),
        other => Some(*other),
    }
}

/// Where an automation target goes in a copy: the copy of the device or
/// chain it names, and nothing when it wasn't copied and is in another set
/// (the set exchange spec, R6).
fn settle_target(
    copies: &Copies,
    doc: &Document,
    target: &AutomationTarget,
) -> Option<AutomationTarget> {
    match target {
        AutomationTarget::Mixer { param } => Some(AutomationTarget::Mixer { param: *param }),
        // A send's envelope follows the send, which keeps its return track.
        AutomationTarget::Send { to } => {
            settle_return(copies, doc, *to, None).map(|to| AutomationTarget::Send { to })
        }
        AutomationTarget::Device { device, param } => Some(AutomationTarget::Device {
            device: copies.device_or_same(doc, *device)?,
            param: *param,
        }),
        AutomationTarget::Chain { chain, param } => Some(AutomationTarget::Chain {
            chain: copies.chain_or_same(doc, *chain)?,
            param: *param,
        }),
        AutomationTarget::ChainSend { chain, to } => Some(AutomationTarget::ChainSend {
            chain: copies.chain_or_same(doc, *chain)?,
            to: copies.chain_or_same(doc, *to)?,
        }),
        AutomationTarget::Clip { .. }
        | AutomationTarget::MidiCc { .. }
        | AutomationTarget::MidiPitchBend
        | AutomationTarget::MidiChannelPressure => Some(*target),
    }
}

/// Each chain of a device and of its copy, in the same order.
fn chain_pairs(original: &Device, copy: &Device) -> Vec<(ChainId, ChainId)> {
    let mut out = Vec::new();
    let mut stack = vec![(original, copy)];
    while let Some((original, copy)) = stack.pop() {
        for (chain, copied) in original.chains.iter().zip(copy.chains.iter()) {
            out.push((chain.id, copied.id));
            for (device, copied) in chain.devices.iter().zip(copied.devices.iter()) {
                stack.push((device, copied));
            }
        }
    }
    out
}

/// The scene at `position`, adding empty scenes at the end of the set when
/// it has fewer (the set exchange spec, R5).
fn scene_at(editor: &mut Editor, position: usize) -> Result<SceneId, EditError> {
    while editor.document().scene_count() <= position {
        let id: SceneId = editor.new_id();
        let index = editor.document().scene_count();
        editor.apply(Command::CreateScene { id, index })?;
    }
    Ok(editor
        .document()
        .scenes()
        .nth(position)
        .expect("the scene is there")
        .id)
}

/// Creates a copy of `data` in the editor, at `index` in `parent`'s list
/// (or among the returns), with its devices and its clips under new IDs.
fn create(
    editor: &mut Editor,
    data: &TrackData,
    parent: Option<TrackId>,
    index: usize,
    copies: &mut Copies,
) -> Result<TrackId, EditError> {
    let id: TrackId = editor.new_id();
    let parent = if data.kind == TrackKind::Return {
        None
    } else {
        parent
    };
    editor.apply(Command::CreateTrack {
        id,
        kind: data.kind,
        name: data.name.clone(),
        parent,
        index,
    })?;
    copies.tracks.push((data.id, id));
    for (index, device) in data.devices.iter().enumerate() {
        // New IDs for the device and everything inside its racks.
        let mut copy = device.clone();
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        copy.floating = device.floating;
        let devices: Vec<(DeviceId, DeviceId)> = device
            .walk()
            .map(|d| d.id)
            .zip(copy.walk().map(|d| d.id))
            .collect();
        copies.devices.extend(devices);
        copies.chains.extend(chain_pairs(device, &copy));
        // The mappings and the sidechain follow the copies, once they are
        // all there (`finish` and `map_copies`).
        copy.clear_mappings();
        if let Some(sidechain) = copy.sidechain.as_mut() {
            sidechain.input = Input::None;
        }
        if let Some(midi) = copy.midi_sidechain.as_mut() {
            midi.source = None;
        }
        editor.apply(Command::InsertDevice {
            track: id,
            index,
            device: copy,
        })?;
    }
    for clip in &data.clips {
        let original = clip.id;
        let mut clip = clip.clone();
        clip.id = editor.new_id();
        copies.clips.push((original, clip.id));
        remap_clip(&mut clip, copies, editor.document());
        editor.apply(Command::CreateClip { track: id, clip })?;
    }
    for (position, clip) in &data.slots {
        let scene = scene_at(editor, *position)?;
        let original = clip.id;
        let mut clip = clip.clone();
        clip.id = editor.new_id();
        copies.clips.push((original, clip.id));
        remap_clip(&mut clip, copies, editor.document());
        editor.apply(Command::CreateSlotClip {
            track: id,
            scene,
            clip,
        })?;
    }
    // Spec: comping R9. The lanes and their clips come under new IDs.
    for lane in &data.take_lanes {
        let lane_id = editor.new_id();
        editor.apply(Command::AddTakeLane {
            track: id,
            lane: NewTakeLane {
                id: Some(lane_id),
                name: lane.name.clone(),
                clips: Vec::new(),
            },
            index: None,
        })?;
        for clip in &lane.clips {
            let original = clip.id;
            let mut clip = clip.clone();
            clip.id = editor.new_id();
            copies.clips.push((original, clip.id));
            remap_clip(&mut clip, copies, editor.document());
            editor.apply(Command::CreateTakeClip {
                lane: lane_id,
                clip,
            })?;
        }
    }
    for (index, child) in data.children.iter().enumerate() {
        create(editor, child, Some(id), index, copies)?;
    }
    Ok(id)
}

/// Points a clip's envelopes at the copies of the devices and returns they
/// drive, and drops the ones whose device or return the copy has no match
/// for.
fn remap_clip(clip: &mut Clip, copies: &Copies, doc: &Document) {
    clip.envelopes.retain_mut(|envelope| {
        let target = match envelope.target {
            AutomationTarget::Device { device, param } => copies
                .device_or_same(doc, device)
                .map(|device| AutomationTarget::Device { device, param }),
            AutomationTarget::Send { to } => {
                settle_return(copies, doc, to, None).map(|to| AutomationTarget::Send { to })
            }
            other => Some(other),
        };
        target.is_some_and(|target| {
            envelope.target = target;
            true
        })
    });
}

/// Gives a copy everything that names other objects: its mixer, its sends,
/// its routing, its take lanes, its automation and its frozen audio.
fn finish(editor: &mut Editor, data: &TrackData, copies: &Copies) -> Result<(), EditError> {
    let Some(id) = copies.track(data.id) else {
        return Ok(());
    };
    let mixer = data.mixer;
    editor.apply(Command::SetVolume {
        track: id,
        volume: mixer.volume,
    })?;
    editor.apply(Command::SetPan {
        track: id,
        pan: mixer.pan,
    })?;
    if data.kind.has_mute_and_solo() {
        editor.apply(Command::SetMute {
            track: id,
            mute: mixer.mute,
        })?;
        editor.apply(Command::SetSolo {
            track: id,
            solo: mixer.solo,
        })?;
    }
    if data.kind.has_arm() {
        editor.apply(Command::SetArm {
            track: id,
            arm: mixer.arm,
        })?;
    }
    if data.kind == TrackKind::Return {
        editor.apply(Command::SetSendTap {
            track: id,
            tap: mixer.tap,
        })?;
    }
    if !mixer.crossfade.is_none() {
        editor.apply(Command::SetCrossfadeAssign {
            track: id,
            assign: mixer.crossfade,
        })?;
    }
    if !mixer.pan_mode.is_stereo() {
        editor.apply(Command::SetPanMode {
            track: id,
            mode: mixer.pan_mode,
        })?;
    }
    if mixer.split_left != 0.0 {
        editor.apply(Command::SetSplitPanLeft {
            track: id,
            pan: mixer.split_left,
        })?;
    }
    if mixer.split_right != 0.0 {
        editor.apply(Command::SetSplitPanRight {
            track: id,
            pan: mixer.split_right,
        })?;
    }
    if !data.keep_monitoring_latency {
        editor.apply(Command::SetKeepMonitoringLatency {
            track: id,
            enabled: false,
        })?;
    }
    if data.tuning_bypass {
        editor.apply(Command::SetTuningBypass {
            track: id,
            bypass: true,
        })?;
    }
    if !data.delay.is_default() {
        editor.apply(Command::SetTrackDelay {
            track: id,
            delay: data.delay,
        })?;
    }
    for send in &data.sends {
        let Some(to) = settle_send(copies, editor, send) else {
            continue;
        };
        editor.apply(Command::SetSend {
            track: id,
            to,
            level: send.level,
        })?;
    }
    let routing = &data.routing;
    if data.kind.holds_clips() {
        if let Some(input) = settle_input(copies, editor.document(), &routing.input) {
            editor.apply(Command::SetInput {
                track: id,
                input,
                channel: routing.channel,
            })?;
        }
        editor.apply(Command::SetMonitor {
            track: id,
            monitor: routing.monitor,
        })?;
    }
    if routing.output != Output::Default
        && let Some(output) = settle_output(copies, editor.document(), &routing.output)
    {
        editor.apply(Command::SetOutput { track: id, output })?;
    }
    if data.kind == TrackKind::Midi {
        let to = match &routing.midi_output {
            Some(MidiTo::Track(track)) => {
                settle_track(copies, editor.document(), *track).map(MidiTo::Track)
            }
            Some(other) => Some(other.clone()),
            None => None,
        };
        editor.apply(Command::SetMidiOutput { track: id, to })?;
    }
    for position in &data.removed_stop_buttons {
        let scene = scene_at(editor, *position)?;
        editor.apply(Command::SetSlotStopButton {
            track: id,
            scene,
            stop_button: false,
        })?;
    }
    for device in &data.devices {
        let Some(copy) = copies.device(device.id) else {
            continue;
        };
        if let Some(sidechain) = device.sidechain.as_ref()
            && let Some(input) = settle_input(copies, editor.document(), &sidechain.input)
        {
            editor.apply(Command::SetSidechainInput {
                device: copy,
                input,
            })?;
        }
        if let Some(midi) = device.midi_sidechain.as_ref() {
            let source = midi
                .source
                .and_then(|track| settle_track(copies, editor.document(), track));
            editor.apply(Command::SetMidiSidechainSource {
                device: copy,
                track: source,
            })?;
        }
    }
    for (target, automation) in &data.automation {
        let Some(target) = settle_target(copies, editor.document(), target) else {
            continue;
        };
        for point in automation.envelope.points() {
            editor.apply(Command::SetAutomationBreakpoint {
                track: id,
                target,
                at: point.at,
                value: point.value,
            })?;
        }
        for point in automation.envelope.points() {
            if point.bend != 0.0 {
                editor.apply(Command::SetAutomationCurve {
                    track: id,
                    target,
                    at: point.at,
                    bend: point.bend,
                })?;
            }
        }
    }
    if let Some(frozen) = &data.frozen {
        let mut frozen = frozen.clone();
        for region in &mut frozen.clips {
            if let Some(copy) = copies.clip(region.clip) {
                region.clip = copy;
            }
        }
        for tail in &mut frozen.tails {
            if let Some(copy) = copies.clip(tail.clip) {
                tail.clip = copy;
            }
            tail.id = editor.new_id();
        }
        if let Some(output) = frozen.output.as_mut()
            && let Some(copy) = copies.clip(output.id)
        {
            output.id = copy;
        }
        editor.apply(Command::FreezeTracks {
            frozen: vec![(id, frozen)],
        })?;
    }
    for child in &data.children {
        finish(editor, child, copies)?;
    }
    Ok(())
}

/// Points the copies' modulator mappings at the copies of their targets
/// (the modulation spec, R19): a mapping whose target was copied too goes
/// to the copy, and one whose target wasn't isn't copied. The mappings are
/// those of the devices in `data`.
fn map_copies(editor: &mut Editor, data: &[TrackData], copies: &Copies) -> Result<(), EditError> {
    use bl_model::modulator::ModTarget;
    for track in data {
        for device in track.devices.iter().flat_map(Device::walk) {
            let Some(copy) = copies.device(device.id) else {
                continue;
            };
            for mapping in &device.mappings {
                let target = match mapping.target {
                    ModTarget::Mixer { track, param } => copies
                        .track(track)
                        .map(|track| ModTarget::Mixer { track, param }),
                    ModTarget::Send { track, to } => copies
                        .track(track)
                        .map(|track| ModTarget::Send { track, to }),
                    ModTarget::Device { device, param } => copies
                        .device(device)
                        .map(|device| ModTarget::Device { device, param }),
                };
                if let Some(target) = target {
                    editor.apply(Command::MapModulator {
                        device: copy,
                        slot: mapping.slot,
                        target,
                    })?;
                }
            }
        }
        map_copies(editor, &track.children, copies)?;
    }
    Ok(())
}

/// Duplicates the tracks, each straight after itself, with everything they
/// hold under new IDs (the track context menu spec, R12). Returns the
/// copies, in display order.
///
/// # Errors
///
/// If nothing can be duplicated, or a command is refused; the set is then
/// unchanged.
pub fn duplicate_tracks(
    editor: &mut Editor,
    tracks: &[TrackId],
    label: &'static str,
) -> Result<Vec<TrackId>, String> {
    let originals = duplicated(editor.document(), tracks);
    if originals.is_empty() {
        return Err("there's no track to duplicate".to_owned());
    }
    let mut copies = Vec::new();
    let mut renames = Vec::new();
    editor
        .transact(label, |editor| {
            let mut ids = Copies::default();
            let mut data = Vec::new();
            // The last first, so each original's place stays where it was.
            for &original in originals.iter().rev() {
                let doc = editor.document();
                let parent = doc.parent(original);
                let kind = doc.track(original).map(Track::kind);
                let siblings: Vec<TrackId> = match (kind, parent) {
                    (Some(TrackKind::Return), _) => doc.returns().collect(),
                    (_, Some(group)) => doc
                        .track(group)
                        .map(|g| g.children().collect())
                        .unwrap_or_default(),
                    (_, None) => doc.top_level().collect(),
                };
                let index = siblings
                    .iter()
                    .position(|&t| t == original)
                    .map_or(0, |i| i + 1);
                let Some(track) = snapshot(doc, original) else {
                    continue;
                };
                let copy = create(editor, &track, parent, index, &mut ids)?;
                renames.push((copy, original));
                copies.push(copy);
                data.push(track);
            }
            // Everything that names the copies, now that they are all there.
            data.reverse();
            for track in &data {
                finish(editor, track, &ids)?;
            }
            map_copies(editor, &data, &ids)?;
            for (copy, original) in &renames {
                rename_like(editor, *copy, *original)?;
            }
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;
    let order = editor.document().track_order();
    copies.sort_by_key(|copy| order.iter().position(|t| t == copy));
    Ok(copies)
}

/// Where the copies go: the tracks that aren't return tracks go in
/// `parent`'s list (or the top level) at `index`, and the return tracks
/// among the returns at `returns`.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    /// The group the copies go in, or the top level.
    pub parent: Option<TrackId>,
    /// The place in that list, counted from 0.
    pub index: usize,
    /// The place among the return tracks, counted from 0.
    pub returns: usize,
}

/// Makes a copy of each track in `data` in the editor, with everything the
/// tracks hold under new IDs (R13). The copies are made in two passes: the
/// tracks, their devices and their clips first, then everything that names
/// them, so a reference to a track copied later still finds its copy.
/// `source_returns` is the return tracks of the set the tracks came from
/// when that isn't this one, and the references the copies make to tracks
/// not copied are then settled by place, not by ID.
///
/// # Errors
///
/// If a command is refused; the set is then unchanged.
pub fn copy_tracks(
    editor: &mut Editor,
    data: &[TrackData],
    place: Place,
    source_returns: Option<Vec<TrackId>>,
    label: &'static str,
) -> Result<Vec<TrackId>, String> {
    if data.is_empty() {
        return Err("there's nothing on the clipboard to paste".to_owned());
    }
    let mut ids = Copies {
        foreign: source_returns,
        ..Copies::default()
    };
    let mut copies = Vec::with_capacity(data.len());
    let mut index = place.index;
    let mut returns = place.returns;
    editor
        .transact(label, |editor| {
            for track in data {
                // A return track goes among the returns, the others in the
                // list they were pasted into.
                let at = if track.kind == TrackKind::Return {
                    let at = returns;
                    returns += 1;
                    at
                } else {
                    let at = index;
                    index += 1;
                    at
                };
                copies.push(create(editor, track, place.parent, at, &mut ids)?);
            }
            for track in data {
                finish(editor, track, &ids)?;
            }
            map_copies(editor, data, &ids)?;
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;
    Ok(copies)
}

/// Duplicates `tracks` without their clips, as one undo step named `label`:
/// each copy straight after its track, with its name, mixer, sends,
/// routing, and devices (new IDs, the same settings), and a group's copy
/// holding copies of its tracks. Returns the copies, in display order.
///
/// # Errors
///
/// If nothing can be duplicated, or a command is refused; the set is then
/// unchanged.
pub fn duplicate_without_clips(
    editor: &mut Editor,
    tracks: &[TrackId],
    label: &'static str,
) -> Result<Vec<TrackId>, String> {
    let originals = duplicated(editor.document(), tracks);
    if originals.is_empty() {
        return Err("there's no track to duplicate".to_owned());
    }
    let mut copies = Vec::new();
    editor
        .transact(label, |editor| {
            let mut renames = Vec::new();
            let mut devices = Vec::new();
            // The last first, so each original's place stays where it was.
            for &original in originals.iter().rev() {
                let doc = editor.document();
                let parent = doc.parent(original);
                let kind = doc.track(original).map(Track::kind);
                let siblings: Vec<TrackId> = match (kind, parent) {
                    (Some(TrackKind::Return), _) => doc.returns().collect(),
                    (_, Some(group)) => doc
                        .track(group)
                        .map(|g| g.children().collect())
                        .unwrap_or_default(),
                    (_, None) => doc.top_level().collect(),
                };
                let index = siblings
                    .iter()
                    .position(|&t| t == original)
                    .map_or(0, |i| i + 1);
                let copy = copy_track(editor, original, parent, index, &mut renames, &mut devices)?;
                copies.push(copy);
            }
            map_modulators(editor, &renames, &devices)?;
            for (copy, original) in renames {
                rename_like(editor, copy, original)?;
            }
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;
    let order = editor.document().track_order();
    copies.sort_by_key(|copy| order.iter().position(|t| t == copy));
    Ok(copies)
}

/// Copies `original` to `index` in `parent`'s list (or the returns), with
/// its settings and devices, and a group's tracks inside it; notes each
/// copy for renaming.
fn copy_track(
    editor: &mut Editor,
    original: TrackId,
    parent: Option<TrackId>,
    index: usize,
    renames: &mut Vec<(TrackId, TrackId)>,
    devices: &mut Vec<(DeviceId, DeviceId)>,
) -> Result<TrackId, EditError> {
    let source = editor
        .document()
        .track(original)
        .cloned()
        .expect("the original exists");
    let id: TrackId = editor.new_id();
    editor.apply(Command::CreateTrack {
        id,
        kind: source.kind(),
        name: source.name().to_owned(),
        parent: if source.kind() == TrackKind::Return {
            None
        } else {
            parent
        },
        index,
    })?;
    renames.push((id, original));
    let mixer = *source.mixer();
    editor.apply(Command::SetVolume {
        track: id,
        volume: mixer.volume,
    })?;
    editor.apply(Command::SetPan {
        track: id,
        pan: mixer.pan,
    })?;
    if source.kind().has_mute_and_solo() {
        editor.apply(Command::SetMute {
            track: id,
            mute: mixer.mute,
        })?;
        editor.apply(Command::SetSolo {
            track: id,
            solo: mixer.solo,
        })?;
    }
    if source.kind().has_arm() {
        editor.apply(Command::SetArm {
            track: id,
            arm: mixer.arm,
        })?;
    }
    if source.kind() == TrackKind::Return {
        editor.apply(Command::SetSendTap {
            track: id,
            tap: mixer.tap,
        })?;
    }
    for (to, level) in source.sends() {
        editor.apply(Command::SetSend {
            track: id,
            to,
            level,
        })?;
    }
    for (index, device) in source.devices().enumerate() {
        // New IDs for the device and everything inside its racks.
        let mut copy = device.clone();
        copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
        copy.floating = device.floating;
        // Its mappings come once the copies of its targets are there
        // (`map_copies`).
        let pairs: Vec<(DeviceId, DeviceId)> = device
            .walk()
            .map(|d| d.id)
            .zip(copy.walk().map(|d| d.id))
            .collect();
        copy.clear_mappings();
        devices.extend(pairs);
        editor.apply(Command::InsertDevice {
            track: id,
            index,
            device: copy,
        })?;
    }
    let children: Vec<TrackId> = source.children().collect();
    for (index, child) in children.into_iter().enumerate() {
        copy_track(editor, child, Some(id), index, renames, devices)?;
    }
    // The routing last, once the devices and the tracks inside are there.
    let routing = source.routing().clone();
    if source.kind().holds_clips() {
        editor.apply(Command::SetInput {
            track: id,
            input: routing.input,
            channel: routing.channel,
        })?;
        editor.apply(Command::SetMonitor {
            track: id,
            monitor: routing.monitor,
        })?;
    }
    if routing.output != Output::Default {
        editor.apply(Command::SetOutput {
            track: id,
            output: routing.output,
        })?;
    }
    if source.kind() == TrackKind::Midi {
        editor.apply(Command::SetMidiOutput {
            track: id,
            to: routing.midi_output,
        })?;
    }
    Ok(id)
}

/// Maps the copies of modulators to the copies of their targets (the
/// modulation spec, R19): a mapping whose target was duplicated too goes to
/// the copy, and one whose target wasn't isn't copied. `tracks` and
/// `devices` pair each copy with its original.
fn map_modulators(
    editor: &mut Editor,
    tracks: &[(TrackId, TrackId)],
    devices: &[(DeviceId, DeviceId)],
) -> Result<(), EditError> {
    use bl_model::modulator::ModTarget;
    let track_copy =
        |original: TrackId| tracks.iter().find(|(_, o)| *o == original).map(|(c, _)| *c);
    let device_copy = |original: DeviceId| {
        devices
            .iter()
            .find(|(o, _)| *o == original)
            .map(|(_, c)| *c)
    };
    for &(original, copy) in devices {
        let mappings = editor
            .document()
            .device(original)
            .map(|d| d.mappings.clone())
            .unwrap_or_default();
        for mapping in mappings {
            let target = match mapping.target {
                ModTarget::Mixer { track, param } => {
                    track_copy(track).map(|track| ModTarget::Mixer { track, param })
                }
                ModTarget::Send { track, to } => {
                    track_copy(track).map(|track| ModTarget::Send { track, to })
                }
                ModTarget::Device { device, param } => {
                    device_copy(device).map(|device| ModTarget::Device { device, param })
                }
            };
            if let Some(target) = target {
                editor.apply(Command::MapModulator {
                    device: copy,
                    slot: mapping.slot,
                    target,
                })?;
            }
        }
    }
    Ok(())
}

/// Gives `copy` its original's name, numbered for its own place where the
/// original's name starts with the original's number or letter (Live
/// duplicates "4 Audio" as "5 Audio", observed); any other name as it is.
fn rename_like(editor: &mut Editor, copy: TrackId, original: TrackId) -> Result<(), EditError> {
    let doc = editor.document();
    let Some(name) = doc.track(original).map(|t| t.name().to_owned()) else {
        return Ok(());
    };
    let Some((head, rest)) = name.split_once(' ') else {
        return Ok(());
    };
    let own = |track: TrackId| {
        if doc.track(track).map(Track::kind) == Some(TrackKind::Return) {
            let index = doc.returns().position(|r| r == track).unwrap_or(0);
            return_letter(index)
        } else {
            track_label(doc, track)
                .split_once('-')
                .map(|(number, _)| number.to_owned())
                .unwrap_or_default()
        }
    };
    if head != own(original) {
        return Ok(());
    }
    let renamed = format!("{} {rest}", own(copy));
    if renamed != name {
        editor.apply(Command::RenameTrack {
            track: copy,
            name: renamed,
        })?;
    }
    Ok(())
}

/// The tracks from `tracks` that can be grouped, in display order:
/// tracks whose kind satisfies `can_be_grouped()` and that exist in the document,
/// excluding Main and Return tracks, and excluding tracks that are already inside
/// another track in `tracks`.
#[must_use]
pub fn groupable(doc: &Document, tracks: &[TrackId]) -> Vec<TrackId> {
    let inside_another = |id: TrackId| {
        let mut parent = doc.parent(id);
        while let Some(group) = parent {
            if tracks.contains(&group) {
                return true;
            }
            parent = doc.parent(group);
        }
        false
    };
    doc.track_order()
        .into_iter()
        .filter(|&id| {
            tracks.contains(&id)
                && doc.track(id).is_some_and(|t| t.kind().can_be_grouped())
                && !inside_another(id)
        })
        .collect()
}

/// The audio and MIDI tracks `tracks` name for linking, in display order:
/// each one itself, and each group track the audio and MIDI tracks inside it,
/// at any depth (the linked-track editing spec, R2 to R4).
#[must_use]
pub fn linkable(doc: &Document, tracks: &[TrackId]) -> Vec<TrackId> {
    let named = |id: TrackId| {
        tracks.contains(&id) || {
            let mut parent = doc.parent(id);
            std::iter::from_fn(|| {
                let group = parent?;
                parent = doc.parent(group);
                Some(group)
            })
            .any(|group| tracks.contains(&group))
        }
    };
    doc.track_order()
        .into_iter()
        .filter(|&id| {
            named(id)
                && doc
                    .track(id)
                    .is_some_and(|t| matches!(t.kind(), TrackKind::Audio | TrackKind::Midi))
        })
        .collect()
}

/// Groups `tracks` into a new group track named "Group" as one undo step named `label`.
/// Inserts the group track where the first track was, moves each track into it in order,
/// and returns the new group track ID.
///
/// # Errors
///
/// If no tracks can be grouped, or if a command fails; the set remains unchanged.
pub fn group_tracks(
    editor: &mut Editor,
    tracks: &[TrackId],
    label: &'static str,
) -> Result<TrackId, String> {
    let members = groupable(editor.document(), tracks);
    if members.is_empty() {
        return Err("there's no track to group".to_owned());
    }
    let first = members[0];
    let doc = editor.document();
    let parent = doc.parent(first);
    let siblings: Vec<TrackId> = match parent {
        Some(group) => doc
            .track(group)
            .map(|g| g.children().collect())
            .unwrap_or_default(),
        None => doc.top_level().collect(),
    };
    let insert_index = siblings
        .iter()
        .position(|&t| t == first)
        .unwrap_or(siblings.len());

    let group_id: TrackId = editor.new_id();

    editor
        .transact(label, |editor| {
            editor.apply(Command::CreateTrack {
                id: group_id,
                kind: TrackKind::Group,
                name: "Group".to_owned(),
                parent,
                index: insert_index,
            })?;
            for (i, &member) in members.iter().enumerate() {
                editor.apply(Command::MoveTrack {
                    track: member,
                    parent: Some(group_id),
                    index: i,
                })?;
            }
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;

    Ok(group_id)
}

/// Ungroups `group` as one undo step named `label`.
/// Moves all children of `group` to where `group` was, in order, deletes `group`,
/// and returns the former members in order.
///
/// # Errors
///
/// If `group` is not a group track or doesn't exist, or if a command fails.
pub fn ungroup_tracks(
    editor: &mut Editor,
    group: TrackId,
    label: &'static str,
) -> Result<Vec<TrackId>, String> {
    let doc = editor.document();
    let track = doc
        .track(group)
        .ok_or_else(|| "track not found".to_owned())?;
    if track.kind() != TrackKind::Group {
        return Err("selected track is not a group track".to_owned());
    }
    let children: Vec<TrackId> = track.children().collect();
    let parent = doc.parent(group);
    let siblings: Vec<TrackId> = match parent {
        Some(p) => doc
            .track(p)
            .map(|g| g.children().collect())
            .unwrap_or_default(),
        None => doc.top_level().collect(),
    };
    let insert_index = siblings
        .iter()
        .position(|&t| t == group)
        .unwrap_or(siblings.len());

    editor
        .transact(label, |editor| {
            for (i, &child) in children.iter().enumerate() {
                editor.apply(Command::MoveTrack {
                    track: child,
                    parent,
                    index: insert_index + i,
                })?;
            }
            editor.apply(Command::DeleteTrack { track: group })?;
            Ok::<_, EditError>(())
        })
        .map_err(|e| e.to_string())?;

    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bl_core::time::Ticks;
    use bl_model::{AudioData, ClipData, ContentHash, DeviceKind, ParamId};

    fn add(editor: &mut Editor, kind: TrackKind, name: &str, parent: Option<TrackId>) -> TrackId {
        let id = editor.new_id();
        let doc = editor.document();
        let index = match (kind, parent) {
            (TrackKind::Return, _) => doc.returns().len(),
            (_, Some(group)) => doc.track(group).unwrap().children().len(),
            (_, None) => doc.top_level().len(),
        };
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: name.to_owned(),
                parent,
                index,
            })
            .unwrap();
        id
    }

    /// A clip on a track, as the tests use them.
    fn clip(editor: &mut Editor, track: TrackId, start: i64, length: i64) -> ClipId {
        let id = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    Ticks(start),
                    Ticks(length),
                    ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
                ),
            })
            .unwrap();
        id
    }

    /// A MIDI clip on a track, as the tests use them.
    fn midi_clip(editor: &mut Editor, track: TrackId, start: i64, length: i64) -> ClipId {
        let id = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    Ticks(start),
                    Ticks(length),
                    ClipData::Midi(bl_model::MidiData::default()),
                ),
            })
            .unwrap();
        id
    }

    /// A device on a track, as the tests use them.
    fn device(editor: &mut Editor, track: TrackId) -> DeviceId {
        let id = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: Device::new(
                    id,
                    "Compressor",
                    DeviceKind::Native {
                        device_type: "beatlayer.squeeze".to_owned(),
                    },
                )
                .with_sidechain(),
            })
            .unwrap();
        id
    }

    // Spec: track-context-menu R12, R13
    #[test]
    fn a_duplicate_holds_what_its_track_holds_under_new_ids() {
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let ret = add(&mut editor, TrackKind::Return, "A Reverb", None);
        let clip = clip(&mut editor, one, 0, 960_000);
        let device = device(&mut editor, one);
        editor
            .apply(Command::SetSend {
                track: one,
                to: ret,
                level: 0.5,
            })
            .unwrap();
        editor
            .apply(Command::AddTakeLane {
                track: one,
                lane: NewTakeLane {
                    id: None,
                    name: "Take 1".to_owned(),
                    clips: Vec::new(),
                },
                index: None,
            })
            .unwrap();
        let lane = editor
            .document()
            .track(one)
            .unwrap()
            .take_lanes()
            .next()
            .unwrap()
            .id;
        let take: ClipId = editor.new_id();
        editor
            .apply(Command::CreateTakeClip {
                lane,
                clip: Clip::new(
                    take,
                    Ticks(0),
                    Ticks(960_000),
                    ClipData::Audio(AudioData::new(ContentHash([8; 32]))),
                ),
            })
            .unwrap();
        editor
            .apply(Command::SetAutomationBreakpoint {
                track: one,
                target: AutomationTarget::Device {
                    device,
                    param: ParamId(1),
                },
                at: Ticks(0),
                value: 0.5,
            })
            .unwrap();
        editor.clear_history().unwrap();

        let copies = duplicate_tracks(&mut editor, &[one], "Duplicate Track(s)").unwrap();
        assert_eq!(copies.len(), 1);
        let copy = copies[0];
        let doc = editor.document();
        let original = doc.track(one).unwrap();
        let made = doc.track(copy).unwrap();
        assert_ne!(copy, one, "the copy has its own ID");
        assert_eq!(made.mixer(), original.mixer());
        assert_eq!(
            made.sends().collect::<Vec<_>>(),
            original.sends().collect::<Vec<_>>()
        );
        assert_eq!(made.take_lanes().count(), original.take_lanes().count());
        let made_lane = made.take_lanes().next().unwrap();
        assert_ne!(made_lane.id, lane, "the lane has its own ID");
        assert_eq!(made_lane.clips.len(), 1);
        assert_ne!(made_lane.clips[0].id, take, "so does its clip");
        doc.check_invariants().unwrap();
        assert_eq!(made.clips().count(), 1, "the clip came with it");
        assert_ne!(
            made.clips().next().unwrap().id,
            clip,
            "the clip has a new ID"
        );
        assert_eq!(made.devices().count(), 1, "the device came with it");
        assert_ne!(
            made.devices().next().unwrap().id,
            device,
            "the device has a new ID"
        );
        assert_eq!(made.automation().count(), 1, "the automation came with it");
        // The copy goes straight after its track, and is named for its place.
        let order = doc.track_order();
        assert_eq!(order[1], copy);
        assert_eq!(doc.track(copy).unwrap().name(), "2 Audio");
        assert_eq!(editor.undo_history().next(), Some("Duplicate Track(s)"));
    }

    // Spec: track-context-menu R12 (a group, a return track)
    #[test]
    fn a_duplicated_group_holds_copies_of_its_tracks_and_a_return_among_the_returns() {
        let mut editor = Editor::new();
        let group = add(&mut editor, TrackKind::Group, "Group", None);
        let inner = add(&mut editor, TrackKind::Audio, "1 Audio", Some(group));
        let midi = add(&mut editor, TrackKind::Midi, "2 MIDI", Some(group));
        let ret = add(&mut editor, TrackKind::Return, "A Reverb", None);
        clip(&mut editor, inner, 0, 960_000);
        midi_clip(&mut editor, midi, 0, 960_000);
        editor.clear_history().unwrap();

        let copies = duplicate_tracks(&mut editor, &[group, ret], "Duplicate Track(s)").unwrap();
        assert_eq!(copies.len(), 2);
        let doc = editor.document();
        let group_copy = doc.track(copies[0]).unwrap();
        assert_eq!(group_copy.kind(), TrackKind::Group);
        let inner_copy = group_copy.children().next().expect("its track came too");
        assert_eq!(
            doc.track(inner_copy).unwrap().clips().count(),
            1,
            "the clip came with it"
        );
        let midi_copy = group_copy
            .children()
            .nth(1)
            .expect("its MIDI track came too");
        assert_eq!(
            doc.track(midi_copy).unwrap().clips().count(),
            1,
            "the MIDI clip came with it"
        );
        let return_copy = doc.track(copies[1]).unwrap();
        assert_eq!(return_copy.kind(), TrackKind::Return);
        // The return's copy is among the returns, after the original.
        let returns: Vec<TrackId> = doc.returns().collect();
        assert_eq!(returns, [ret, copies[1]]);
        assert_eq!(doc.track(copies[1]).unwrap().name(), "B Reverb");
    }

    // Spec: track-context-menu R13 (references between the tracks copied)
    #[test]
    fn a_sidechain_between_two_copied_tracks_points_at_the_copies() {
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let two = add(&mut editor, TrackKind::Audio, "2 Audio", None);
        let outside = add(&mut editor, TrackKind::Audio, "3 Audio", None);
        let device = device(&mut editor, two);
        editor
            .apply(Command::SetSidechainInput {
                device,
                input: Input::Track {
                    track: one,
                    tap: bl_model::Tap::PostFx,
                    chain: None,
                    output: None,
                },
            })
            .unwrap();
        editor.clear_history().unwrap();

        let copies = duplicate_tracks(&mut editor, &[one, two], "Duplicate Track(s)").unwrap();
        let doc = editor.document();
        let made = doc.track(copies[1]).unwrap();
        let copy_device = made.devices().next().unwrap();
        let Some(Input::Track { track, .. }) = copy_device.sidechain.as_ref().map(|s| &s.input)
        else {
            panic!("the copy kept its sidechain");
        };
        assert_eq!(*track, copies[0], "it points at the copy");

        // A sidechain on a track that isn't copied stays as it is.
        let outside_device = {
            let id = editor.new_id();
            editor
                .apply(Command::InsertDevice {
                    track: outside,
                    index: 0,
                    device: Device::new(
                        id,
                        "Compressor",
                        DeviceKind::Native {
                            device_type: "beatlayer.squeeze".to_owned(),
                        },
                    )
                    .with_sidechain(),
                })
                .unwrap();
            id
        };
        editor
            .apply(Command::SetSidechainInput {
                device: outside_device,
                input: Input::Track {
                    track: one,
                    tap: bl_model::Tap::PostFx,
                    chain: None,
                    output: None,
                },
            })
            .unwrap();
        let copies = duplicate_tracks(&mut editor, &[outside], "Duplicate Track(s)").unwrap();
        let doc = editor.document();
        let made = doc.track(copies[0]).unwrap();
        let copy_device = made.devices().next().unwrap();
        let Some(Input::Track { track, .. }) = copy_device.sidechain.as_ref().map(|s| &s.input)
        else {
            panic!("the copy kept its sidechain");
        };
        assert_eq!(*track, one, "it still points at the track it did");
    }

    #[test]
    fn a_duplicate_goes_after_its_track_numbered_for_its_place() {
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let two = add(&mut editor, TrackKind::Audio, "Kick", None);
        let ret = add(&mut editor, TrackKind::Return, "A Reverb", None);
        let copies = duplicate_without_clips(&mut editor, &[one, two, ret], "Duplicate").unwrap();
        let doc = editor.document();
        let names: Vec<&str> = doc
            .track_order()
            .into_iter()
            .map(|t| doc.track(t).unwrap().name())
            .collect();
        assert_eq!(
            names,
            [
                "1 Audio", "2 Audio", "Kick", "Kick", "A Reverb", "B Reverb", "Main"
            ]
        );
        assert_eq!(copies.len(), 3);
        assert_eq!(editor.undo_history().len(), 4);
        assert_eq!(editor.undo_history().next(), Some("Duplicate"));
    }

    // Spec: modulation R19
    #[test]
    fn a_duplicated_track_maps_its_own_copies() {
        use bl_model::modulator::{ModTarget, WAVE_MOD};
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let two = add(&mut editor, TrackKind::Audio, "Other", None);
        let device = |editor: &mut Editor, track, device_type: &str| {
            let id: DeviceId = editor.new_id();
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device: bl_model::Device::new(
                        id,
                        device_type,
                        bl_model::DeviceKind::Native {
                            device_type: device_type.to_owned(),
                        },
                    ),
                })
                .unwrap();
            id
        };
        let wave = device(&mut editor, one, WAVE_MOD);
        let eq = device(&mut editor, one, "beatlayer.eq");
        let outside = device(&mut editor, two, "beatlayer.eq");
        let map = |editor: &mut Editor, slot, target| {
            editor
                .apply(Command::MapModulator {
                    device: wave,
                    slot,
                    target,
                })
                .unwrap();
        };
        map(
            &mut editor,
            0,
            ModTarget::Device {
                device: eq,
                param: bl_model::ParamId(1),
            },
        );
        map(
            &mut editor,
            1,
            ModTarget::Mixer {
                track: one,
                param: bl_model::MixerParam::Pan,
            },
        );
        map(
            &mut editor,
            2,
            ModTarget::Device {
                device: outside,
                param: bl_model::ParamId(2),
            },
        );
        let copies = duplicate_without_clips(&mut editor, &[one], "Duplicate").unwrap();
        let doc = editor.document();
        let copy = doc.track(copies[0]).unwrap();
        let mut devices = copy.devices();
        let (copy_eq, copy_wave) = {
            let first = devices.next().unwrap();
            let second = devices.next().unwrap();
            if bl_model::modulator::is_modulator_kind(&first.kind) {
                (second, first)
            } else {
                (first, second)
            }
        };
        let targets: Vec<(u8, ModTarget)> = copy_wave
            .mappings
            .iter()
            .map(|m| (m.slot, m.target))
            .collect();
        assert_eq!(
            targets,
            [
                (
                    0,
                    ModTarget::Device {
                        device: copy_eq.id,
                        param: bl_model::ParamId(1)
                    }
                ),
                (
                    1,
                    ModTarget::Mixer {
                        track: copies[0],
                        param: bl_model::MixerParam::Pan
                    }
                ),
            ],
            "the copies of its targets, and not a target outside the copy"
        );
        assert_eq!(
            doc.device(wave).unwrap().mappings.len(),
            3,
            "the original keeps its own"
        );
    }

    #[test]
    fn group_and_ungroup_tracks_round_trip() {
        let mut editor = Editor::new();
        let one = add(&mut editor, TrackKind::Audio, "1 Audio", None);
        let two = add(&mut editor, TrackKind::Midi, "2 MIDI", None);
        let three = add(&mut editor, TrackKind::Audio, "3 Audio", None);

        let initial_doc = editor.document().clone();

        // Group `two` and `three`
        let group = group_tracks(&mut editor, &[two, three], "Group Tracks").unwrap();
        let doc = editor.document();
        let names: Vec<&str> = doc
            .track_order()
            .into_iter()
            .map(|t| doc.track(t).unwrap().name())
            .collect();
        assert_eq!(names, ["1 Audio", "Group", "2 MIDI", "3 Audio", "Main"]);
        assert_eq!(doc.output_of(two), Some(group));
        assert_eq!(doc.output_of(three), Some(group));
        assert_eq!(doc.output_of(group), Some(doc.main_id()));
        assert_eq!(editor.undo_history().next(), Some("Group Tracks"));

        // Undo grouping
        editor.undo().unwrap();
        assert_eq!(editor.document(), &initial_doc);

        // Redo grouping
        editor.redo().unwrap();
        assert_eq!(
            editor.document().track_order(),
            [one, group, two, three, editor.document().main_id()]
        );

        // Ungroup
        let ungrouped = ungroup_tracks(&mut editor, group, "Ungroup Tracks").unwrap();
        assert_eq!(ungrouped, [two, three]);
        let doc = editor.document();
        assert_eq!(doc.track_order(), [one, two, three, doc.main_id()]);
        assert_eq!(doc.output_of(two), Some(doc.main_id()));
        assert_eq!(doc.output_of(three), Some(doc.main_id()));
        assert_eq!(editor.undo_history().next(), Some("Ungroup Tracks"));

        // Undo ungroup
        editor.undo().unwrap();
        let doc = editor.document();
        assert_eq!(doc.track_order(), [one, group, two, three, doc.main_id()]);
        assert_eq!(doc.output_of(two), Some(group));

        // Redo ungroup
        editor.redo().unwrap();
        assert_eq!(
            editor.document().track_order(),
            [one, two, three, editor.document().main_id()]
        );
    }
}
