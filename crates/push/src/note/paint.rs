//! What Note Mode lights: the pads, the buttons it owns and the touch strip
//! (`docs/specs/push-2-notes.md`, R12, R13, R21, R23 and R29).

use std::collections::BTreeSet;

use crate::content::{HeldStep, StripUse};
use bl_control::ReadModel;
use bl_core::time::Ticks;
use bl_engine::SlotState;
use bl_model::{Clip, MidiData, TrackId, swatch_rgb};

use super::grid::{self, Role, lighter, scaled};
use super::{Area, Down, Fit, Frame, Kind, Melodic, Melody, Note, State, bit, clip, melodic, pads};
use crate::content::Paint;
use crate::controls::cc;
use crate::leds::{Led, fixed};

/// A pad's colour: one of the palette's fixed entries, or any colour.
#[derive(Clone, Copy)]
enum Shade {
    Fixed(u8),
    Rgb([u8; 3]),
}

impl Shade {
    fn led(self, paint: &mut Paint<'_>) -> Led {
        match self {
            Self::Fixed(index) => Led::steady(index),
            Self::Rgb(rgb) => Led::steady(paint.lights.colour(rgb)),
        }
    }
}

/// The clip a step or page is read from, with its data.
type Edited<'a> = Option<(&'a Clip, &'a MidiData)>;

/// The pitches of the clip playing on `track` that sound at its playhead.
fn sounding(model: &ReadModel, track: TrackId) -> BTreeSet<u8> {
    let mut keys = BTreeSet::new();
    for status in model
        .slots
        .iter()
        .filter(|s| s.track == track && s.state == SlotState::Playing)
    {
        let Some(id) = model.document.slot(track, status.scene).map(|c| c.id) else {
            continue;
        };
        let Some((c, midi)) = clip::midi(model, id) else {
            continue;
        };
        let Some(at) = clip::playhead(model, track, status.scene, c, midi) else {
            continue;
        };
        keys.extend(
            midi.notes
                .iter()
                .filter(|n| !n.mute && n.start <= at && at < n.start + n.length)
                .map(|n| n.key),
        );
    }
    keys
}

/// Whether the track records now: a Session recording goes into its slot, or
/// the transport records and the track is armed (R23, R29).
fn records(model: &ReadModel, track: TrackId) -> bool {
    model
        .slots
        .iter()
        .any(|s| s.track == track && s.state == SlotState::Recording)
        || (model.transport.recording && model.document.track(track).is_some_and(|t| t.mixer().arm))
}

impl Note {
    /// What `push.state` and the surface read of Note Mode: its summary, and
    /// the pads with reduced sensitivity (R31).
    pub(super) fn summarise(&mut self, frame: &Frame) {
        let drum = matches!(frame.kind, Kind::Rack { .. } | Kind::Slices { .. });
        let melodic = matches!(frame.kind, Kind::Melodic);
        let (layout, bank) = match frame.kind {
            Kind::Rack { first, .. } => (frame.layout.name(), Some(first)),
            Kind::Slices { .. } => (frame.layout.name(), None),
            Kind::Melodic => (frame.melodic.name(), None),
            Kind::Silent => ("None", None),
        };
        self.strip = self.strip_for(frame);
        let mut reduced = 0;
        if drum {
            for row in 0..8 {
                for column in 0..8 {
                    if matches!(Self::role(frame, row, column), Role::Loop(_)) {
                        reduced |= bit(row, column);
                    }
                }
            }
        }
        self.reduced = reduced;
        self.summary = State {
            track: Some(frame.track),
            layout: layout.to_owned(),
            overlay: frame
                .overlay
                .filter(|_| drum || melodic)
                .map(|overlay| overlay.name().to_owned()),
            overlay_locked: (drum || melodic)
                && self
                    .memory
                    .get(&frame.track)
                    .is_some_and(|memory| memory.lock.is_some()),
            bank,
            selected_pad: frame.selected.filter(|_| drum),
            step_size: frame.size.label().to_owned(),
            edited_clip: frame.clip,
            edited_scene: frame.scene,
            page: self.pair * 2 + 1,
            following: self.following,
            velocity: self.velocity,
            held_steps: crate::content::Content::held_steps(self),
            area: self.area(frame),
            melody: melodic.then(|| Self::melody(frame, self.strip)),
        };
    }

    /// What the layout lets the display show of the clip (push-2-tracks R50
    /// to R54).
    fn area(&self, frame: &Frame) -> Option<Area> {
        let (from, to) = (
            frame.pages.step_start(self.pair, 0),
            frame.pages.step_start(self.pair + 1, 0),
        );
        let (fit, low, high) = match frame.kind {
            Kind::Silent => return None,
            Kind::Rack { .. } | Kind::Slices { .. } => {
                let key = frame.selected.unwrap_or(36);
                match frame.layout {
                    grid::Layout::Pads => (Fit::Folded, 0, 127),
                    _ => (Fit::Pad, key, key),
                }
            }
            Kind::Melodic => {
                let (low, high) = frame.view().range().unwrap_or((frame.low, frame.low));
                match frame.melodic {
                    Melodic::Notes => (Fit::Folded, 0, 127),
                    Melodic::Sequencer => (Fit::Sequencer, low, high),
                    Melodic::SequencerNotes => (Fit::SequencerNotes, low, high),
                }
            }
        };
        Some(Area {
            fit,
            from,
            to,
            low,
            high,
            step: frame.size.ticks(),
        })
    }

    /// What a melodic layout shows, for `push.state` (R32 to R44).
    fn melody(frame: &Frame, strip: Option<StripUse>) -> Melody {
        let (low, high) = frame.view().range().unwrap_or((frame.low, frame.low));
        Melody {
            in_key: frame.pads.in_key,
            fixed: frame.pads.fixed,
            spacing: frame.pads.spacing.name().to_owned(),
            direction: frame.pads.direction.name().to_owned(),
            octave: frame.octave,
            low,
            high,
            range: format!(
                "{} to {}",
                melodic::note_name(low),
                melodic::note_name(high)
            ),
            selected_notes: super::keys::keys_of(frame.keys).collect(),
            modulation: strip == Some(StripUse::Modulation),
        }
    }

    pub(super) fn paint_pads(&mut self, paint: &mut Paint<'_>) {
        let model = paint.model;
        let Some(frame) = self.frame(model) else {
            return;
        };
        let Some(track) = model.document.track(frame.track) else {
            return;
        };
        if matches!(frame.kind, Kind::Silent) {
            return;
        }
        let swatch = |index: u8| swatch_rgb(paint.colours, index);
        let track_colour = swatch(track.colour());
        let clip_colour = frame
            .clip
            .and_then(|id| model.document.clip(id))
            .and_then(|c| c.colour)
            .map_or(track_colour, swatch);
        let edited: Edited<'_> = frame.clip.and_then(|id| clip::midi(model, id));
        let playhead = frame
            .scene
            .zip(edited)
            .and_then(|(scene, (c, midi))| clip::playhead(model, frame.track, scene, c, midi));
        let recording = records(model, frame.track);
        let mut sound = sounding(model, frame.track);
        sound.extend(self.down.values().filter_map(|down| match down {
            Down::Played(note)
            | Down::Picked {
                key: note,
                played: true,
            } => Some(*note),
            _ => None,
        }));
        let held = self.down.values().find_map(Down::step);
        for row in 0..8 {
            for column in 0..8 {
                let shade = match Self::role(&frame, row, column) {
                    Role::Off => continue,
                    Role::Drum(note) => {
                        self.drum_shade(&frame, note, model, track_colour, &sound, &swatch)
                    }
                    Role::Velocity(velocity) if self.velocity == Some(velocity) => {
                        Shade::Fixed(fixed::WHITE)
                    }
                    Role::Velocity(velocity) => {
                        Shade::Rgb(scaled(track_colour, 25 + u16::from(velocity) * 75 / 127))
                    }
                    Role::Loop(page) => self.loop_shade(&frame, page, edited, playhead, recording),
                    Role::Step(index) if matches!(frame.kind, Kind::Melodic) => self
                        .notes_step_shade(&frame, index, edited, playhead, recording, clip_colour),
                    Role::Step(index) => {
                        self.step_shade(&frame, index, edited, playhead, recording, clip_colour)
                    }
                    Role::Key(note) => {
                        Self::key_shade(&frame, note, &sound, recording, track_colour)
                    }
                    Role::Pick(note) => {
                        self.pick_shade(&frame, note, &sound, recording, track_colour, edited, held)
                    }
                    Role::Cell { key, step } => self.cell_shade(
                        &frame,
                        (key, step),
                        edited,
                        playhead,
                        recording,
                        clip_colour,
                    ),
                };
                let led = shade.led(paint);
                paint.lights.pad(row, column, led);
            }
        }
        // The buttons Note Mode owns (R13, R21).
        let (can_up, can_down) = match frame.kind {
            Kind::Rack { first, .. } => (Self::can_move(first, true), Self::can_move(first, false)),
            Kind::Melodic if frame.melodic == Melodic::Notes => (
                melodic::can_octave(frame.scale, frame.pads, frame.octave, true),
                melodic::can_octave(frame.scale, frame.pads, frame.octave, false),
            ),
            Kind::Melodic => {
                let can = |up| {
                    melodic::shifted_low(frame.scale, frame.pads.in_key, frame.low, up, false)
                        .is_some()
                };
                (can(true), can(false))
            }
            _ => (false, false),
        };
        paint.lights.white(54, can_down, false);
        paint.lights.white(55, can_up, false);
        paint
            .lights
            .white(cc::LAYOUT, true, frame.overlay.is_some());
        paint.lights.white(62, edited.is_some(), false);
        paint.lights.white(63, edited.is_some(), false);
        for row in 0..8u8 {
            let colour = if frame.size.row() == row {
                fixed::WHITE
            } else {
                fixed::DARK_GREY
            };
            paint.lights.button(cc::SCENE_1 - row, Led::steady(colour));
        }
        if let Kind::Rack { first, .. } = frame.kind {
            paint.lights.strip(grid::strip_leds(first));
        }
        if matches!(frame.kind, Kind::Melodic) && frame.melodic != Melodic::Notes {
            let range = frame.view().range().unwrap_or((frame.low, frame.low));
            let notes: Vec<u8> = edited
                .map(|(_, midi)| midi.notes.iter().map(|n| n.key).collect())
                .unwrap_or_default();
            paint
                .lights
                .strip(melodic::strip_leds(range.0, range.1, &notes));
        }
        // The arrows: Left and Right where there is a track to select, Up and
        // Down where there is a scene to launch (R45, R46).
        let at = model.columns.iter().position(|t| *t == frame.track);
        paint
            .lights
            .white(cc::LEFT, at.is_some_and(|at| at > 0), false);
        paint.lights.white(
            cc::RIGHT,
            at.is_some_and(|at| at + 1 < model.columns.len()),
            false,
        );
        paint.lights.white(
            cc::UP,
            Self::can_step_scene(model, frame.scene, false),
            false,
        );
        paint.lights.white(
            cc::DOWN,
            Self::can_step_scene(model, frame.scene, true),
            false,
        );
    }

    /// A drum pad's colour: green while it sounds, white while selected,
    /// blue while soloed, grey while muted, else its colour, lighter when no
    /// chain receives its note (R12, R19).
    fn drum_shade(
        &self,
        frame: &Frame,
        note: u8,
        model: &ReadModel,
        track_colour: [u8; 3],
        sound: &BTreeSet<u8>,
        swatch: &dyn Fn(u8) -> [u8; 3],
    ) -> Shade {
        let (filled, own, solo, muted) = match frame.kind {
            Kind::Rack { rack, filled, .. } => {
                let chains = pads::pad_chains(model, rack, note);
                (
                    filled >> note & 1 == 1,
                    pads::pad_colour(model, rack, note),
                    chains.iter().any(|c| c.mixer.solo),
                    !chains.is_empty() && chains.iter().all(|c| !c.mixer.enabled),
                )
            }
            Kind::Slices { filled, .. } => (filled >> note & 1 == 1, None, false, false),
            _ => (true, None, false, false),
        };
        // A pad past the last slice is dark (R75).
        if matches!(frame.kind, Kind::Slices { .. }) && !filled {
            return Shade::Fixed(fixed::BLACK);
        }
        let base = own.map_or(track_colour, swatch);
        if sound.contains(&note) {
            Shade::Fixed(fixed::GREEN)
        } else if frame.selected == Some(note) {
            Shade::Fixed(fixed::WHITE)
        } else if solo {
            Shade::Fixed(fixed::BLUE)
        } else if muted {
            Shade::Fixed(fixed::LIGHT_GREY)
        } else if filled {
            Shade::Rgb(base)
        } else {
            Shade::Rgb(lighter(base))
        }
    }

    /// A loop length pad: unlit outside the loop, grey inside it, white
    /// while its page is in the step area, green while its page plays and red
    /// while it records (R29).
    fn loop_shade(
        &self,
        frame: &Frame,
        page: u8,
        edited: Edited<'_>,
        playhead: Option<Ticks>,
        recording: bool,
    ) -> Shade {
        let Some((c, midi)) = edited else {
            return Shade::Fixed(fixed::BLACK);
        };
        let (from, to) = (
            frame.pages.page_start(page),
            frame.pages.page_start(page + 1),
        );
        let (low, high) = clip::region(c, midi);
        if from >= high || to <= low {
            Shade::Fixed(fixed::BLACK)
        } else if playhead.is_some_and(|at| at >= from && at < to) {
            Shade::Fixed(if recording { fixed::RED } else { fixed::GREEN })
        } else if frame.pages.view_of(page) == self.pair {
            Shade::Fixed(fixed::WHITE)
        } else {
            Shade::Fixed(fixed::LIGHT_GREY)
        }
    }

    /// A step: grey without a note, the clip's colour with one (brighter for
    /// a higher velocity), a lighter version for a deactivated note, green
    /// while it plays and red while the track records (R23).
    fn step_shade(
        &self,
        frame: &Frame,
        index: u8,
        edited: Edited<'_>,
        playhead: Option<Ticks>,
        recording: bool,
        clip_colour: [u8; 3],
    ) -> Shade {
        let start = frame.pages.step_start(self.pair, index);
        let end = start + frame.size.ticks();
        if playhead.is_some_and(|at| at >= start && at < end) {
            return Shade::Fixed(if recording { fixed::RED } else { fixed::GREEN });
        }
        let (Some((_, midi)), Some(key)) = (edited, frame.selected) else {
            return Shade::Fixed(fixed::LIGHT_GREY);
        };
        let notes: Vec<_> = midi
            .notes
            .iter()
            .filter(|n| n.key == key && n.start >= start && n.start < end)
            .collect();
        if notes.is_empty() {
            return Shade::Fixed(fixed::LIGHT_GREY);
        }
        match notes.iter().filter(|n| !n.mute).map(|n| n.velocity).max() {
            Some(velocity) => Shade::Rgb(scaled(clip_colour, 40 + u16::from(velocity) * 60 / 127)),
            None => Shade::Rgb(lighter(clip_colour)),
        }
    }
    /// A pad of 64 Notes: the track's colour for the scale's root, white for
    /// another note of the scale, unlit outside it, green while its note plays
    /// and red while the track records (R34).
    fn key_shade(
        frame: &Frame,
        note: u8,
        sound: &BTreeSet<u8>,
        recording: bool,
        track_colour: [u8; 3],
    ) -> Shade {
        if sound.contains(&note) {
            Shade::Fixed(if recording { fixed::RED } else { fixed::GREEN })
        } else {
            Self::scale_shade(frame, note, track_colour)
        }
    }

    /// The colour a note has by its place in the scale (R34).
    fn scale_shade(frame: &Frame, note: u8, track_colour: [u8; 3]) -> Shade {
        if note % 12 == frame.scale.root {
            Shade::Rgb(track_colour)
        } else if frame.scale.holds(note) {
            Shade::Fixed(fixed::WHITE)
        } else {
            Shade::Fixed(fixed::BLACK)
        }
    }

    /// One of the 32 notes: a lighter version of the track's colour while
    /// selected or while a held step holds it, green while it plays (R43,
    /// R44).
    #[allow(clippy::too_many_arguments)]
    fn pick_shade(
        &self,
        frame: &Frame,
        note: u8,
        sound: &BTreeSet<u8>,
        _recording: bool,
        track_colour: [u8; 3],
        edited: Edited<'_>,
        held: Option<HeldStep>,
    ) -> Shade {
        let in_held_step = held.is_some_and(|step| {
            edited.is_some_and(|(_, midi)| {
                midi.notes.iter().any(|n| {
                    n.key == note && n.start >= step.start && n.start < step.start + step.length
                })
            })
        });
        if sound.contains(&note) {
            Shade::Fixed(fixed::GREEN)
        } else if frame.keys >> note & 1 == 1 || in_held_step {
            Shade::Rgb(lighter(track_colour))
        } else {
            Self::scale_shade(frame, note, track_colour)
        }
    }

    /// A cell of the Melodic Sequencer: its row's colour while empty (white
    /// for the root, grey in the scale, unlit outside it), a step's colours
    /// with a note, green while its step plays (R40, R42).
    fn cell_shade(
        &self,
        frame: &Frame,
        (key, step): (u8, u8),
        edited: Edited<'_>,
        playhead: Option<Ticks>,
        recording: bool,
        clip_colour: [u8; 3],
    ) -> Shade {
        let start = frame.pages.step_start(self.pair, step);
        let end = start + frame.size.ticks();
        if playhead.is_some_and(|at| at >= start && at < end) {
            return Shade::Fixed(if recording { fixed::RED } else { fixed::GREEN });
        }
        let notes: Vec<_> = edited
            .map(|(_, midi)| {
                midi.notes
                    .iter()
                    .filter(|n| n.key == key && n.start >= start && n.start < end)
                    .collect()
            })
            .unwrap_or_default();
        if notes.is_empty() {
            return if key % 12 == frame.scale.root {
                Shade::Fixed(fixed::WHITE)
            } else if frame.scale.holds(key) {
                Shade::Fixed(fixed::LIGHT_GREY)
            } else {
                Shade::Fixed(fixed::BLACK)
            };
        }
        match notes.iter().filter(|n| !n.mute).map(|n| n.velocity).max() {
            Some(velocity) => Shade::Rgb(scaled(clip_colour, 40 + u16::from(velocity) * 60 / 127)),
            None => Shade::Rgb(lighter(clip_colour)),
        }
    }

    /// A step of the Melodic Sequencer + 32 Notes: white while held, green
    /// while it plays, the clip's colour with notes, grey without and light
    /// grey with only deactivated ones (R44).
    fn notes_step_shade(
        &self,
        frame: &Frame,
        index: u8,
        edited: Edited<'_>,
        playhead: Option<Ticks>,
        recording: bool,
        clip_colour: [u8; 3],
    ) -> Shade {
        let start = frame.pages.step_start(self.pair, index);
        let end = start + frame.size.ticks();
        let held = self
            .down
            .values()
            .any(|down| down.step().is_some_and(|step| step.start == start));
        if held {
            return Shade::Fixed(fixed::WHITE);
        }
        if playhead.is_some_and(|at| at >= start && at < end) {
            return Shade::Fixed(if recording { fixed::RED } else { fixed::GREEN });
        }
        let Some((_, midi)) = edited else {
            return Shade::Fixed(fixed::LIGHT_GREY);
        };
        let notes: Vec<_> = midi
            .notes
            .iter()
            .filter(|n| n.start >= start && n.start < end)
            .collect();
        if notes.is_empty() {
            return Shade::Fixed(fixed::LIGHT_GREY);
        }
        match notes.iter().filter(|n| !n.mute).map(|n| n.velocity).max() {
            Some(velocity) => Shade::Rgb(scaled(clip_colour, 40 + u16::from(velocity) * 60 / 127)),
            None => Shade::Rgb([190, 190, 190]),
        }
    }
}
