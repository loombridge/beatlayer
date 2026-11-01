//! Clip Mode: the selected clip on the display, its loop, markers, zoom and
//! audio settings on the encoders, Crop, and the note settings of the steps
//! held in a step sequencing layout (`docs/specs/push-2-tracks.md`, R43 to
//! R58).
//!
//! What the mode shows is read from the set and the Clip View as the read
//! model has them, so Push 2 follows the window (R3); what it keeps itself,
//! the zoom and the marker last touched, is surface state: no part of the
//! set, no undo step (R2).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bl_control::ReadModel;
use bl_core::time::TICKS_PER_QUARTER;
use bl_engine::SlotState;
use bl_model::fader::{fader_gain, fader_position, volume_text};
use bl_model::{
    AudioData, Clip, ClipData, ClipId, MidiData, Note, SceneId, TrackId, TrackKind, WarpMode,
    swatch_rgb,
};
use serde_json::{Value, json};

use crate::content::{Content, Ctx, Gesture, HeldStep, Modifiers, Pad, Paint, Turn};
use crate::controls::{self, cc};
use crate::encoder::{Counting, LIST_STEPS, Moving};
use crate::note::clip as note_clip;
use crate::note::{self, Fit, Kind};
use crate::screen::{AreaBox, Bar, ClipBody, ClipScreen, Column, Graphic, Label};

/// A beat, a quarter note, in ticks.
const BEAT: f64 = TICKS_PER_QUARTER as f64;
/// The gesture key of the note settings: the changes made while steps are
/// held are one undo step, from the first until the last step is let go
/// (R1).
const NOTES_GESTURE: u32 = 0x5400;
/// How many zoom levels there are; each doubles the zoom every four.
const ZOOM_LEVELS: u8 = 24;
/// How many columns of the waveform the display draws.
const WAVE_COLUMNS: usize = 240;
/// A note's velocity that a turn started from stays this long, until the
/// document shows it.
const REST: Duration = Duration::from_millis(500);
/// The colour of the mode's labels.
const WHITE: [u8; 3] = [230, 230, 230];
/// A clip the display can't colour.
const GREY: [u8; 3] = [150, 150, 150];
/// The warp modes, in the Clip View's order, and the names they show.
const WARP_MODES: [(WarpMode, &str); 6] = [
    (WarpMode::Beats, "Beats"),
    (WarpMode::Tones, "Tones"),
    (WarpMode::Texture, "Texture"),
    (WarpMode::RePitch, "Re-Pitch"),
    (WarpMode::Complex, "Complex"),
    (WarpMode::ComplexPro, "Complex Pro"),
];

/// The clip Clip Mode shows, if there is one: the selected track's clip in
/// the selected scene's slot while the Session View is the main view, else
/// the clip the Clip View shows (R43).
#[must_use]
pub fn shown(model: &ReadModel) -> Option<ClipId> {
    match place(model) {
        Place::Clip(shown) => Some(shown.clip.id),
        _ => None,
    }
}

/// Which slot or clip Clip Mode looks at (R43).
enum Place<'a> {
    /// A clip, MIDI or audio.
    Clip(Shown<'a>),
    /// A slot with no clip, on a MIDI or audio track.
    Empty {
        track: TrackId,
        scene: SceneId,
        audio: bool,
    },
    /// Nothing to show.
    Nothing,
}

struct Shown<'a> {
    clip: &'a Clip,
    track: Option<TrackId>,
    scene: Option<SceneId>,
    data: Data<'a>,
}

enum Data<'a> {
    Midi(&'a MidiData),
    Audio(&'a AudioData),
}

fn place(model: &ReadModel) -> Place<'_> {
    let doc = &model.document;
    let (track, scene, clip) = if model.session_view {
        let Some(track) = note_clip::selected_track(model) else {
            return Place::Nothing;
        };
        let Some(scene) =
            note_clip::selected_scene(model).or_else(|| doc.scenes().next().map(|scene| scene.id))
        else {
            return Place::Nothing;
        };
        match doc.slot(track, scene) {
            Some(clip) => (Some(track), Some(scene), clip),
            None => {
                return match doc.track(track).map(bl_model::Track::kind) {
                    Some(kind @ (TrackKind::Audio | TrackKind::Midi)) => Place::Empty {
                        track,
                        scene,
                        audio: kind == TrackKind::Audio,
                    },
                    _ => Place::Nothing,
                };
            }
        }
    } else {
        let Some(clip) = model.clip_view.and_then(|id| doc.clip(id)) else {
            return Place::Nothing;
        };
        let track = doc
            .tracks()
            .find(|track| track.clip(clip.id).is_some())
            .map(bl_model::Track::id);
        (track, None, clip)
    };
    let data = match &clip.data {
        ClipData::Midi(midi) => Data::Midi(midi),
        ClipData::Audio(audio) => Data::Audio(audio),
    };
    Place::Clip(Shown {
        clip,
        track,
        scene,
        data,
    })
}

/// What a position of the clip is counted in: ticks of its contents, or the
/// seconds into the file of an audio clip that isn't warped, whose beat is
/// the set's tempo's (Beatlayer's own).
#[derive(Clone, Copy)]
enum Unit {
    Ticks,
    Seconds(f64),
}

impl Unit {
    fn beats(self, value: f64) -> f64 {
        match self {
            Self::Ticks => value / BEAT,
            Self::Seconds(bpm) => value * bpm / 60.0,
        }
    }

    /// How much one beat is in this unit.
    fn per_beat(self) -> f64 {
        match self {
            Self::Ticks => BEAT,
            Self::Seconds(bpm) => 60.0 / bpm,
        }
    }
}

/// The clip's markers in its unit, the ends that are `null` made whole.
#[derive(Clone, Copy)]
struct Marks {
    start: f64,
    end: f64,
    looping: bool,
    loop_start: f64,
    loop_end: f64,
}

impl Shown<'_> {
    fn unit(&self, model: &ReadModel) -> Unit {
        match self.data {
            Data::Audio(audio) if !audio.warp => Unit::Seconds(model.tempo.get()),
            _ => Unit::Ticks,
        }
    }

    fn waveform<'m>(&self, model: &'m ReadModel) -> Option<&'m bl_control::Waveform> {
        model
            .waveform
            .as_deref()
            .filter(|waveform| waveform.clip == self.clip.id)
    }

    /// Where the clip's contents end, in its unit: its length, or for an
    /// audio clip the end of the file once the peaks say where that is.
    fn contents_end(&self, model: &ReadModel) -> f64 {
        let unit = self.unit(model);
        let length = unit.per_beat() * self.clip.length.get() as f64 / BEAT;
        match &self.data {
            Data::Midi(midi) => midi
                .notes
                .iter()
                .map(|note| (note.start + note.length).get() as f64)
                .fold(length, f64::max),
            Data::Audio(_) => self
                .waveform(model)
                .map_or(length, |waveform| waveform.to * unit.per_beat()),
        }
    }

    fn marks(&self, model: &ReadModel) -> Marks {
        let whole = match &self.data {
            Data::Midi(_) => self.clip.length.get() as f64,
            Data::Audio(_) => self.contents_end(model),
        };
        match &self.data {
            Data::Midi(midi) => Marks {
                start: midi.start.get() as f64,
                end: midi.end.map_or(whole, |end| end.get() as f64),
                looping: midi.looping,
                loop_start: midi.loop_start.get() as f64,
                loop_end: midi.loop_end.map_or(whole, |end| end.get() as f64),
            },
            Data::Audio(audio) => Marks {
                start: audio.start,
                end: audio.end.unwrap_or(whole),
                looping: audio.looping,
                loop_start: audio.loop_start,
                loop_end: audio.loop_end.unwrap_or(whole),
            },
        }
    }

    /// The span of the clip's contents the display can show, in beats.
    fn extent(&self, model: &ReadModel) -> (f64, f64) {
        let unit = self.unit(model);
        let end = unit.beats(self.contents_end(model)).max(1.0);
        match (&self.data, self.waveform(model)) {
            (Data::Audio(_), Some(waveform)) => (waveform.from.min(0.0), end),
            _ => (0.0, end),
        }
    }

    /// Where the clip plays now, in beats of its contents: the time it has
    /// played from its Start marker, brought into its loop (R44).
    fn playhead(&self, model: &ReadModel) -> Option<f64> {
        let (track, scene) = (self.track?, self.scene?);
        model.slots.iter().find(|status| {
            status.track == track && status.scene == scene && status.state == SlotState::Playing
        })?;
        let since = model.clip_time.iter().find(|(t, _)| *t == track)?.1;
        let unit = self.unit(model);
        let marks = self.marks(model);
        let played = unit.per_beat() * since.get() as f64 / BEAT;
        let (from, to) = if marks.looping {
            (marks.loop_start, marks.loop_end)
        } else {
            (marks.start, marks.end)
        };
        let at = marks.start + played;
        Some(unit.beats(if to > from && at >= to {
            from + (at - from).rem_euclid(to - from)
        } else {
            at
        }))
    }

    fn colour(&self, model: &ReadModel, colours: &[[u8; 3]]) -> [u8; 3] {
        let track = self
            .track
            .and_then(|id| model.document.track(id))
            .map(bl_model::Track::colour);
        self.clip
            .colour
            .or(track)
            .map_or(GREY, |index| swatch_rgb(colours, index))
    }
}

/// `value` held between `low` and `high`, `low` winning where they cross: a
/// loop or a clip shorter than a step leaves no room (`f64::clamp` panics).
fn within(value: f64, low: f64, high: f64) -> f64 {
    value.min(high).max(low)
}

/// A position as bars, beats and sixteenths from 1, in 4/4 (Beatlayer's own
/// look).
fn position_text(beats: f64) -> String {
    let sixteenths = (beats * 4.0).round() as i64;
    format!(
        "{}.{}.{}",
        sixteenths.div_euclid(16) + 1,
        sixteenths.rem_euclid(16) / 4 + 1,
        sixteenths.rem_euclid(4) + 1
    )
}

/// A length as bars, beats and sixteenths from 0.
fn length_text(beats: f64) -> String {
    let sixteenths = (beats * 4.0).round() as i64;
    format!(
        "{}.{}.{}",
        sixteenths.div_euclid(16),
        sixteenths.rem_euclid(16) / 4,
        sixteenths.rem_euclid(4)
    )
}

/// What `push.state` reads of Clip Mode (R58).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The clip shown.
    pub clip: Option<ClipId>,
    /// What the mode shows: `midi`, `audio`, `empty_midi`, `empty_audio` or
    /// `nothing`.
    pub shows: &'static str,
    /// The clip's name.
    pub name: String,
    /// Whether its Loop is on, if it has one.
    pub looping: Option<bool>,
    /// The controls on the encoders.
    pub controls: Vec<Control>,
    /// The zoom, 1 for the whole clip.
    pub zoom: f64,
    /// The span of beats the display shows.
    pub view: (f64, f64),
    /// Whether the display is folded to the pitches that hold notes.
    pub folded: bool,
    /// The sequenceable area the pads edit.
    pub area: Option<note::Area>,
    /// The steps held, whose notes the encoders set.
    pub held_steps: Vec<HeldStep>,
    /// The drum pad whose every note the encoders set, while Select holds
    /// it.
    pub select_pad: Option<u8>,
}

/// One control on an encoder, as `push.state` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// The encoder, from 0.
    pub column: u8,
    /// The control's name.
    pub name: String,
    /// Its value as the display shows it.
    pub value: String,
}

/// A note of a held step, or of the pad Select holds, as the encoders read
/// and set it.
#[derive(Clone, Copy, Debug)]
struct Held {
    place: u32,
    key: u8,
    start: i64,
    length: i64,
    velocity: u8,
    /// The grid line the nudge is counted from.
    grid: i64,
}

/// Which notes the encoders set now (R55 to R57).
struct Targets {
    clip: ClipId,
    notes: Vec<Held>,
    /// The held steps with no note, which a turn gives one.
    empties: Vec<HeldStep>,
    /// How long a step is, the unit of Nudge, Length and Fine.
    step: i64,
    select: Option<u8>,
}

/// A note a turn made on an empty step, until the document shows it
/// (R57): what was added, and what the turns since want.
#[derive(Clone, Copy, Debug)]
struct Shadow {
    sent: Values,
    want: Values,
}

#[derive(Clone, Copy, Debug)]
struct Values {
    start: i64,
    length: i64,
    velocity: u8,
}

/// The velocity a note added by a step takes (push-2-notes R22).
const VELOCITY: u8 = 100;

/// The content of Clip Mode.
pub struct ClipMode {
    counts: [Counting; 8],
    gain: Moving,
    /// The zoom level, 0 for the whole clip.
    level: u8,
    /// The encoder of the position last touched.
    anchor: u8,
    state: State,
    /// Whether the note settings' gesture is open.
    gesture: bool,
    /// Whether a pad was pressed while Select was held.
    select: bool,
    shadows: BTreeMap<(ClipId, u8, i64), Shadow>,
    recent: BTreeMap<(u8, i64), (u8, Instant)>,
}

impl Default for ClipMode {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipMode {
    /// Clip Mode before it is entered: the whole clip, no marker touched.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counts: Default::default(),
            gain: Moving::default(),
            level: 0,
            anchor: 1,
            state: State::default(),
            gesture: false,
            select: false,
            shadows: BTreeMap::new(),
            recent: BTreeMap::new(),
        }
    }

    fn zoom(&self) -> f64 {
        2f64.powf(f64::from(self.level) / 4.0)
    }

    /// The beat of the position the encoder last touched (R46).
    fn anchor_at(&self, shown: &Shown<'_>, model: &ReadModel) -> f64 {
        let (unit, marks) = (shown.unit(model), shown.marks(model));
        unit.beats(match (marks.looping, self.anchor) {
            (true, 1) => marks.loop_start,
            (true, 2) => marks.loop_end,
            (_, 2) => marks.end,
            _ => marks.start,
        })
    }

    /// The span of beats the display shows (R44, R46, R55).
    fn view(&self, shown: &Shown<'_>, model: &ReadModel, page: Option<(f64, f64)>) -> (f64, f64) {
        if let Some(page) = page {
            return page;
        }
        let (from, to) = shown.extent(model);
        let span = (to - from) / self.zoom();
        if span >= to - from {
            return (from, to);
        }
        let centre = shown
            .playhead(model)
            .unwrap_or_else(|| self.anchor_at(shown, model));
        let left = (centre - span / 2.0).clamp(from, to - span);
        (left, left + span)
    }

    // ----- which notes the encoders set (R55 to R57) -----

    /// The notes the encoders set while steps are held, or a drum pad is
    /// held with Select (R55, R57).
    fn targets(
        &self,
        shown: &Shown<'_>,
        held_steps: &[HeldStep],
        modifiers: &Modifiers,
        note: Option<&note::State>,
    ) -> Option<Targets> {
        let Data::Midi(midi) = &shown.data else {
            return None;
        };
        let ordered = note_clip::ordered(midi);
        let held = |place: usize, note: &Note, grid: i64| Held {
            place: place as u32,
            key: note.key,
            start: note.start.get(),
            length: note.length.get(),
            velocity: note.velocity,
            grid,
        };
        let steps: Vec<&HeldStep> = held_steps
            .iter()
            .filter(|step| step.clip == Some(shown.clip.id))
            .collect();
        if !steps.is_empty() {
            let mut notes = Vec::new();
            let mut empties = Vec::new();
            for step in &steps {
                let (from, to) = (step.start, step.start + step.length);
                let inside: Vec<Held> = ordered
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.key == step.key && n.start >= from && n.start < to)
                    .map(|(place, n)| held(place, n, step.start.get()))
                    .collect();
                if inside.is_empty() {
                    empties.push(**step);
                }
                notes.extend(inside);
            }
            return Some(Targets {
                clip: shown.clip.id,
                notes,
                empties,
                step: steps[0].length.get(),
                select: None,
            });
        }
        let area = note.and_then(|n| n.area)?;
        let key = note.and_then(|n| n.selected_pad)?;
        if !(self.select && modifiers.has(cc::SELECT)) {
            return None;
        }
        let step = area.step.get().max(1);
        let notes = ordered
            .iter()
            .enumerate()
            .filter(|(_, n)| n.key == key)
            .map(|(place, n)| held(place, n, n.start.get().div_euclid(step) * step))
            .collect::<Vec<_>>();
        (!notes.is_empty()).then_some(Targets {
            clip: shown.clip.id,
            notes,
            empties: Vec::new(),
            step,
            select: Some(key),
        })
    }

    /// The four note controls' values over the held notes: a range shows its
    /// lowest and highest where they differ (R55, R56).
    fn settings(targets: &Targets) -> [Column; 8] {
        let mut columns: [Column; 8] = Default::default();
        let step = targets.step.max(1) as f64;
        let range = |values: Vec<f64>, text: &dyn Fn(f64) -> String| {
            let (low, high) = values
                .iter()
                .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
            if values.is_empty() {
                String::new()
            } else if (high - low).abs() < 1e-9 {
                text(low)
            } else {
                format!("{} to {}", text(low), text(high))
            }
        };
        let nudge: Vec<f64> = targets
            .notes
            .iter()
            .map(|n| (n.start - n.grid) as f64 / step * 100.0)
            .collect();
        let length: Vec<f64> = targets
            .notes
            .iter()
            .map(|n| n.length as f64 / step)
            .collect();
        let velocity: Vec<f64> = targets
            .notes
            .iter()
            .map(|n| f64::from(n.velocity))
            .collect();
        let empty = targets.notes.is_empty();
        let value = |text: String| if empty { "new note".to_owned() } else { text };
        let first = |values: &[f64], of: f64| values.first().map_or(of, |v| *v);
        columns[0] = Column {
            name: "Nudge".to_owned(),
            value: value(range(nudge.clone(), &|v| format!("{v:+.0} %"))),
            graphic: Graphic::Arc(((first(&nudge, 0.0) + 100.0) / 200.0).clamp(0.0, 1.0) as f32),
            ..Column::default()
        };
        columns[1] = Column {
            name: "Length".to_owned(),
            value: value(range(length.clone(), &|v| {
                let whole = v.round().max(0.0);
                format!("{whole:.0} step{}", if whole == 1.0 { "" } else { "s" })
            })),
            graphic: Graphic::Arc((first(&length, 1.0) / 16.0).clamp(0.0, 1.0) as f32),
            ..Column::default()
        };
        columns[2] = Column {
            name: "Fine".to_owned(),
            value: value(range(length.clone(), &|v| format!("{:.0} %", v * 100.0))),
            graphic: Graphic::Arc((first(&length, 1.0) / 16.0).clamp(0.0, 1.0) as f32),
            ..Column::default()
        };
        columns[3] = Column {
            name: "Velocity".to_owned(),
            value: value(range(velocity.clone(), &|v| format!("{v:.0}"))),
            graphic: Graphic::Arc((first(&velocity, 100.0) / 127.0) as f32),
            ..Column::default()
        };
        columns
    }

    /// An encoder turned while notes are held: it changes every held note by
    /// the same amount, each held within its range (R55 to R57).
    fn turn_notes(&mut self, column: u8, steps: i32, targets: &Targets, ctx: &mut Ctx<'_>) {
        // Letting a step go after this is no tap.
        ctx.keep_steps();
        let shift = ctx.modifiers.shift();
        let step = targets.step.max(1);
        let by = |per: f64| (f64::from(steps) * per * if shift { 0.1 } else { 1.0 }).round() as i64;
        let clip = targets.clip;
        match column {
            0 => {
                let delta = by(step as f64 / 100.0);
                self.move_notes(targets, delta, ctx);
                let plus = |values: Values, step_start: i64| Values {
                    start: (values.start + delta)
                        .clamp(step_start - step, step_start + step)
                        .max(0),
                    ..values
                };
                self.touch_empties(targets, ctx, plus);
            }
            1 | 2 => {
                let delta = if column == 1 {
                    i64::from(self.counts[1].take(steps, LIST_STEPS)) * step
                } else {
                    by(step as f64 / 100.0)
                };
                if delta == 0 {
                    return;
                }
                let places: Vec<u32> = targets.notes.iter().map(|n| n.place).collect();
                if !places.is_empty() {
                    self.run(
                        ctx,
                        "note.resize",
                        json!({ "clip": clip, "notes": places, "by": delta }),
                    );
                }
                self.touch_empties(targets, ctx, |values, _| Values {
                    length: (values.length + delta).max(1),
                    ..values
                });
            }
            3 => {
                let delta = steps;
                let now = ctx.at;
                let mut places = Vec::new();
                let mut values = Vec::new();
                for note in &targets.notes {
                    let base = self
                        .recent
                        .get(&(note.key, note.start))
                        .filter(|(_, at)| now.saturating_duration_since(*at) < REST)
                        .map_or(note.velocity, |(v, _)| *v);
                    let next = (i32::from(base) + delta).clamp(1, 127) as u8;
                    self.recent.insert((note.key, note.start), (next, now));
                    places.push(note.place);
                    values.push(next);
                }
                if !places.is_empty() {
                    self.run(
                        ctx,
                        "note.set_value",
                        json!({
                            "field": "velocity",
                            "clip": clip,
                            "notes": places,
                            "values": values,
                        }),
                    );
                }
                self.touch_empties(targets, ctx, |values, _| Values {
                    velocity: (i32::from(values.velocity) + delta).clamp(1, 127) as u8,
                    ..values
                });
            }
            _ => {}
        }
    }

    /// Runs a command of the note settings' gesture.
    fn run(&mut self, ctx: &mut Ctx<'_>, id: &str, args: Value) {
        self.gesture = true;
        ctx.cx.run_held(NOTES_GESTURE, id, args);
    }

    /// Moves the held notes by `delta` ticks, each kept within a step of its
    /// grid line and inside the clip (R55, R56).
    fn move_notes(&mut self, targets: &Targets, delta: i64, ctx: &mut Ctx<'_>) {
        let mut groups: BTreeMap<i64, Vec<u32>> = BTreeMap::new();
        for note in &targets.notes {
            let wanted = (note.start + delta)
                .clamp(note.grid - targets.step, note.grid + targets.step)
                .max(0);
            let moved = wanted - note.start;
            if moved != 0 {
                groups.entry(moved).or_default().push(note.place);
            }
        }
        for (by, places) in groups {
            self.run(
                ctx,
                "note.move",
                json!({ "clip": targets.clip, "notes": places, "by": by }),
            );
        }
    }

    /// Gives each held step with no note one, with the values the turn
    /// gives it, or changes the one a turn already made (R57).
    fn touch_empties(
        &mut self,
        targets: &Targets,
        ctx: &mut Ctx<'_>,
        change: impl Fn(Values, i64) -> Values,
    ) {
        for step in &targets.empties {
            let id = (targets.clip, step.key, step.start.get());
            if let Some(shadow) = self.shadows.get_mut(&id) {
                shadow.want = change(shadow.want, step.start.get());
                continue;
            }
            let values = change(
                Values {
                    start: step.start.get(),
                    length: step.length.get(),
                    velocity: VELOCITY,
                },
                step.start.get(),
            );
            self.shadows.insert(
                id,
                Shadow {
                    sent: values,
                    want: values,
                },
            );
            self.run(
                ctx,
                "note.add",
                json!({
                    "clip": targets.clip,
                    "key": step.key,
                    "start": values.start,
                    "length": values.length,
                    "velocity": values.velocity,
                }),
            );
        }
    }

    /// Carries what the turns since an added note was sent want to the note
    /// once the document shows it (R57).
    fn catch_up(&mut self, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let ids: Vec<_> = self.shadows.keys().copied().collect();
        for id in ids {
            let Some(shadow) = self.shadows.get(&id).copied() else {
                continue;
            };
            let Some(ClipData::Midi(midi)) = model.document.clip(id.0).map(|c| &c.data) else {
                self.shadows.remove(&id);
                continue;
            };
            let found = note_clip::ordered(midi).into_iter().position(|n| {
                n.key == id.1
                    && n.start.get() == shadow.sent.start
                    && n.length.get() == shadow.sent.length
            });
            let Some(place) = found else {
                continue;
            };
            self.shadows.remove(&id);
            let places = [place as u32];
            let (sent, want) = (shadow.sent, shadow.want);
            if want.start != sent.start {
                self.run(
                    ctx,
                    "note.move",
                    json!({ "clip": id.0, "notes": places, "by": want.start - sent.start }),
                );
            }
            if want.length != sent.length {
                self.run(
                    ctx,
                    "note.resize",
                    json!({ "clip": id.0, "notes": places, "by": want.length - sent.length }),
                );
            }
            if want.velocity != sent.velocity {
                self.run(
                    ctx,
                    "note.set_value",
                    json!({ "field": "velocity", "clip": id.0, "notes": places, "value": want.velocity }),
                );
            }
        }
    }

    // ----- the clip's controls (R45 to R49) -----

    /// What the encoders control on `shown` (R48).
    fn columns(&self, shown: &Shown<'_>, model: &ReadModel, shift: bool) -> [Column; 8] {
        let mut columns: [Column; 8] = Default::default();
        let (unit, marks) = (shown.unit(model), shown.marks(model));
        columns[0] = Column {
            name: "Zoom".to_owned(),
            value: format!("{:.1}x", self.zoom()),
            graphic: Graphic::Arc(f32::from(self.level) / f32::from(ZOOM_LEVELS)),
            ..Column::default()
        };
        let beats = |value: f64| unit.beats(value);
        let marker = |name: &str, text: String| Column {
            name: name.to_owned(),
            value: text,
            ..Column::default()
        };
        if marks.looping {
            columns[1] = marker("Loop Position", position_text(beats(marks.loop_start)));
            columns[2] = marker(
                "Loop Length",
                length_text(beats(marks.loop_end - marks.loop_start)),
            );
            columns[3] = marker(
                "Start Offset",
                length_text(beats(marks.start - marks.loop_start)),
            );
        } else {
            columns[1] = marker("Start", position_text(beats(marks.start)));
            columns[2] = marker("End", position_text(beats(marks.end)));
        }
        if let Data::Audio(audio) = &shown.data {
            let mode = WARP_MODES
                .iter()
                .find(|(mode, _)| *mode == audio.warp_mode)
                .map_or("", |(_, name)| *name);
            columns[4] = Column {
                name: "Warp Mode".to_owned(),
                value: String::new(),
                graphic: Graphic::Choice(mode.to_owned()),
                ..Column::default()
            };
            columns[5] = Column {
                name: "Gain".to_owned(),
                value: format!("{} dB", volume_text(audio.gain)),
                graphic: Graphic::Arc(fader_position(audio.gain) as f32),
                ..Column::default()
            };
            columns[6] = Column {
                name: if shift { "Detune" } else { "Transpose" }.to_owned(),
                value: format!("{:+} st {:+.0} ct", audio.transpose, audio.detune),
                graphic: Graphic::Arc(((f64::from(audio.transpose) + 48.0) / 96.0) as f32),
                ..Column::default()
            };
        }
        columns
    }

    /// Runs the commands that move a marker, as one gesture with the encoder
    /// (R45).
    fn marker_turn(
        &mut self,
        column: u8,
        steps: i32,
        shown: &Shown<'_>,
        model: &ReadModel,
        ctx: &mut Ctx<'_>,
    ) {
        let whole = self.counts[usize::from(column)].take(steps, LIST_STEPS);
        if whole == 0 {
            return;
        }
        let unit = shown.unit(model);
        let one = unit.per_beat() * if ctx.modifiers.shift() { 0.25 } else { 1.0 };
        let by = one * f64::from(whole);
        let marks = shown.marks(model);
        let (_, extent_end) = shown.extent(model);
        let limit = extent_end * unit.per_beat();
        let raw_end = match &shown.data {
            Data::Midi(midi) => midi.end.map(|end| end.get() as f64),
            Data::Audio(audio) => audio.end,
        };
        let id = shown.clip.id;
        if marks.looping {
            match column {
                1 => {
                    let length = marks.loop_end - marks.loop_start;
                    let at = (marks.loop_start + by).clamp(0.0, (limit - length).max(0.0));
                    let carry = (marks.start - marks.loop_start).abs() < 1e-6;
                    Self::set_loop(ctx, column, shown, id, at, at + length);
                    if carry {
                        Self::set_region(ctx, column, shown, id, at, raw_end);
                    }
                }
                2 => {
                    let length = (marks.loop_end - marks.loop_start + by).max(one);
                    let end = (marks.loop_start + length).min(limit);
                    Self::set_loop(ctx, column, shown, id, marks.loop_start, end);
                }
                3 => {
                    let at = within(marks.start + by, marks.loop_start, marks.loop_end - one);
                    Self::set_region(ctx, column, shown, id, at, raw_end);
                }
                _ => {}
            }
        } else {
            match column {
                1 => {
                    let at = within(marks.start + by, 0.0, marks.end - one);
                    Self::set_region(ctx, column, shown, id, at, raw_end);
                }
                2 => {
                    let end = within(marks.end + by, marks.start + one, limit);
                    Self::set_region(ctx, column, shown, id, marks.start, Some(end));
                }
                _ => {}
            }
        }
    }

    fn position(shown: &Shown<'_>, value: f64) -> Value {
        match (&shown.data, value) {
            (Data::Midi(_), v) => json!(v.round() as i64),
            (Data::Audio(audio), v) if audio.warp => json!(v.round() as i64),
            (Data::Audio(_), v) => json!(v),
        }
    }

    fn set_loop(
        ctx: &mut Ctx<'_>,
        column: u8,
        shown: &Shown<'_>,
        id: ClipId,
        start: f64,
        end: f64,
    ) {
        let (start, end) = (Self::position(shown, start), Self::position(shown, end));
        match &shown.data {
            Data::Midi(_) => ctx.run_turn(
                column,
                "clip.set_midi_loop",
                json!({ "clip": id, "start": start, "end": end }),
            ),
            Data::Audio(audio) => ctx.run_turn(
                column,
                "clip.set_loop",
                json!({ "clip": id, "start": start, "end": end, "ticks": audio.warp }),
            ),
        }
    }

    fn set_region(
        ctx: &mut Ctx<'_>,
        column: u8,
        shown: &Shown<'_>,
        id: ClipId,
        start: f64,
        end: Option<f64>,
    ) {
        let start = Self::position(shown, start);
        let end = end.map_or(Value::Null, |end| Self::position(shown, end));
        match &shown.data {
            Data::Midi(_) => ctx.run_turn(
                column,
                "clip.set_midi_region",
                json!({ "clip": id, "start": start, "end": end }),
            ),
            Data::Audio(audio) => ctx.run_turn(
                column,
                "clip.set_region",
                json!({ "clip": id, "start": start, "end": end, "ticks": audio.warp }),
            ),
        }
    }

    /// Turns Warp Mode, Gain or Transpose (R47).
    fn audio_turn(
        &mut self,
        column: u8,
        steps: i32,
        audio: &AudioData,
        id: ClipId,
        ctx: &mut Ctx<'_>,
    ) {
        match column {
            4 => {
                let by = self.counts[4].take(steps, LIST_STEPS);
                let now = WARP_MODES
                    .iter()
                    .position(|(mode, _)| *mode == audio.warp_mode)
                    .unwrap_or(0);
                let next = (now as i32 + by).clamp(0, WARP_MODES.len() as i32 - 1) as usize;
                if next != now {
                    ctx.run_turn(
                        4,
                        "clip.set_warp_mode",
                        json!({ "clip": id, "mode": WARP_MODES[next].0 }),
                    );
                }
            }
            5 => {
                let position = self.gain.turn(
                    ctx.at,
                    fader_position(audio.gain),
                    steps,
                    ctx.modifiers.shift(),
                );
                ctx.run_turn(
                    5,
                    "clip.set_gain",
                    json!({ "clip": id, "gain": fader_gain(position) }),
                );
            }
            6 => {
                let by = self.counts[6].take(steps, LIST_STEPS);
                if by == 0 {
                    return;
                }
                if ctx.modifiers.shift() {
                    let detune = (audio.detune + f64::from(by)).clamp(-50.0, 50.0);
                    ctx.run_turn(6, "clip.set_pitch", json!({ "clip": id, "detune": detune }));
                } else {
                    let transpose = (audio.transpose + by).clamp(-48, 48);
                    ctx.run_turn(
                        6,
                        "clip.set_pitch",
                        json!({ "clip": id, "transpose": transpose }),
                    );
                }
            }
            _ => {}
        }
    }

    // ----- the display (R44, R50 to R54) -----

    /// The rows and notes of a MIDI clip's display, and the sequenceable
    /// area and range boxes of the layout (R44, R50 to R54).
    #[allow(clippy::too_many_arguments)]
    fn notes_screen(
        &self,
        paint: &Paint<'_>,
        shown: &Shown<'_>,
        midi: &MidiData,
        view: (f64, f64),
        colour: [u8; 3],
        screen: &mut ClipScreen,
    ) {
        let model = paint.model;
        let layout = paint
            .note
            .filter(|note| note.track == shown.track && !paint.session)
            .and_then(|note| note.area.map(|area| (note, area)));
        let mut keys: Vec<u8> = midi.notes.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        keys.dedup();
        let full = || -> Vec<u8> {
            match (keys.first(), keys.last()) {
                (Some(low), Some(high)) => (*low..=*high).rev().collect(),
                _ => Vec::new(),
            }
        };
        let folded = || -> Vec<u8> { keys.iter().rev().copied().collect() };
        let rows: Vec<u8> = if paint.session {
            folded()
        } else {
            match layout {
                Some((_, area)) => match area.fit {
                    Fit::Folded => folded(),
                    Fit::Pad => vec![area.low],
                    Fit::Sequencer => (area.low..=area.high).rev().collect(),
                    Fit::SequencerNotes => full(),
                },
                None => full(),
            }
        };
        let drum = shown
            .track
            .and_then(|track| match note_clip::classify(model, track) {
                Kind::Rack { rack, .. } => Some(rack),
                _ => None,
            });
        let bars = midi
            .notes
            .iter()
            .filter_map(|n| {
                let row = rows.iter().position(|key| *key == n.key)?;
                let (from, to) = (
                    n.start.get() as f64 / BEAT,
                    (n.start + n.length).get() as f64 / BEAT,
                );
                (to > view.0 && from < view.1).then(|| Bar {
                    row,
                    from,
                    to,
                    opacity: f32::from(n.velocity) / 127.0 * if n.mute { 0.4 } else { 1.0 },
                    colour: drum
                        .and_then(|rack| note::pads::pad_colour(model, rack, n.key))
                        .map_or(colour, |index| swatch_rgb(paint.colours, index)),
                })
            })
            .collect();
        let mut area = None;
        let mut range = None;
        let mut lines = Vec::new();
        if let Some((note, placed)) = layout
            && placed.fit != Fit::Folded
        {
            let (from, to) = (
                placed.from.get() as f64 / BEAT,
                placed.to.get() as f64 / BEAT,
            );
            let row_of = |key: u8| rows.iter().position(|k| *k == key);
            area = match placed.fit {
                Fit::SequencerNotes => {
                    // The area of the selected notes the clip holds (R53).
                    let held: Vec<usize> = note
                        .melody
                        .iter()
                        .flat_map(|m| m.selected_notes.iter().copied())
                        .filter(|key| keys.contains(key))
                        .filter_map(row_of)
                        .collect();
                    held.iter()
                        .min()
                        .zip(held.iter().max())
                        .map(|(top, bottom)| AreaBox {
                            from,
                            to,
                            rows: Some((*top, *bottom)),
                        })
                }
                _ => Some(AreaBox {
                    from,
                    to,
                    rows: None,
                }),
            };
            range = Some((
                f32::from(placed.low) / 128.0,
                (f32::from(placed.high) + 1.0) / 128.0,
            ));
            // Sixteen lines for the pitch ranges of eight that hold notes,
            // thicker where there are more (R51).
            let mut counts = [0u32; 16];
            for n in &midi.notes {
                counts[usize::from(n.key / 8)] += 1;
            }
            let most = counts.iter().copied().max().unwrap_or(0).max(1);
            lines = counts
                .iter()
                .enumerate()
                .filter(|(_, count)| **count > 0)
                .map(|(bin, count)| {
                    (
                        (bin as f32 + 0.5) / 16.0,
                        (4 * *count).div_ceil(most).clamp(1, 4) as u8,
                    )
                })
                .collect();
        }
        screen.body = ClipBody::Notes { rows, bars };
        screen.area = area;
        screen.range = range;
        screen.lines = lines;
    }

    /// An audio clip's waveform across the span shown (R44).
    fn waveform(shown: &Shown<'_>, model: &ReadModel, view: (f64, f64)) -> ClipBody {
        let Some(waveform) = shown.waveform(model) else {
            return ClipBody::Prompt("Building the waveform".to_owned());
        };
        let across = (waveform.to - waveform.from).max(1e-9);
        let points = waveform.peaks.len();
        let at =
            |beat: f64| ((beat - waveform.from) / across * points as f64).clamp(0.0, points as f64);
        let columns = (0..WAVE_COLUMNS)
            .map(|column| {
                let left = view.0 + (view.1 - view.0) * column as f64 / WAVE_COLUMNS as f64;
                let right = view.0 + (view.1 - view.0) * (column + 1) as f64 / WAVE_COLUMNS as f64;
                let (first, last) = (
                    at(left) as usize,
                    (at(right).ceil() as usize).max(at(left) as usize + 1),
                );
                waveform.peaks
                    [first.min(points.saturating_sub(1))..last.min(points).max(first.min(points))]
                    .iter()
                    .fold([0.0f32, 0.0], |[low, high], [l, h]| {
                        [low.min(*l), high.max(*h)]
                    })
            })
            .collect();
        ClipBody::Waveform(columns)
    }
}

impl Content for ClipMode {
    fn name(&self) -> &str {
        "Clip"
    }

    fn enter(&mut self, _why: crate::content::Enter, _ctx: &mut Ctx<'_>) {}

    fn button(&mut self, button: u8, gesture: Gesture, ctx: &mut Ctx<'_>) {
        if gesture != Gesture::Press {
            return;
        }
        let Some(model) = ctx.model else {
            return;
        };
        let Some(column) = controls::upper_column(button) else {
            return;
        };
        match place(model) {
            Place::Clip(shown) => match (column, &shown.data) {
                (1, Data::Midi(midi)) => ctx.cx.run(
                    "clip.set_loop_enabled",
                    json!({ "clip": shown.clip.id, "enabled": !midi.looping }),
                ),
                (1, Data::Audio(audio)) => ctx.cx.run(
                    "clip.set_loop_enabled",
                    json!({ "clip": shown.clip.id, "enabled": !audio.looping }),
                ),
                (2, Data::Midi(_)) => ctx.cx.run("clip.crop", json!({ "clip": shown.clip.id })),
                _ => {}
            },
            Place::Empty {
                track,
                scene,
                audio: true,
            } if column == 0 => ctx.browse_samples(track, scene),
            _ => {}
        }
    }

    fn pad_select(&mut self, _pad: Pad, _ctx: &mut Ctx<'_>) -> bool {
        // The pad is still Note Mode's to select; the encoders then set all
        // of its notes (R57).
        self.select = true;
        false
    }

    fn encoder(&mut self, column: u8, turn: Turn, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let Place::Clip(shown) = place(model) else {
            return;
        };
        let col = usize::from(column);
        let targets = self.targets(&shown, ctx.held_steps, ctx.modifiers, ctx.note);
        match turn {
            Turn::Touch => {
                self.counts[col].reset();
                self.gain.touch();
                if targets.is_none() && (1..=3).contains(&column) {
                    self.anchor = column;
                }
            }
            Turn::Release => {}
            Turn::Steps(steps) => {
                if let Some(targets) = targets {
                    self.turn_notes(column, steps, &targets, ctx);
                    return;
                }
                match column {
                    0 => {
                        let by = self.counts[0].take(steps, LIST_STEPS);
                        let level = i32::from(self.level) + by;
                        self.level = level.clamp(0, i32::from(ZOOM_LEVELS)) as u8;
                    }
                    1..=3 => self.marker_turn(column, steps, &shown, model, ctx),
                    _ => {
                        if let Data::Audio(audio) = &shown.data {
                            self.audio_turn(column, steps, audio, shown.clip.id, ctx);
                        }
                    }
                }
            }
        }
    }

    fn tick(&mut self, ctx: &mut Ctx<'_>) {
        let select = self.select && ctx.modifiers.has(cc::SELECT);
        if !ctx.modifiers.has(cc::SELECT) {
            self.select = false;
        }
        if self.gesture && ctx.held_steps.is_empty() && !select {
            ctx.cx.end_gesture(NOTES_GESTURE);
            self.gesture = false;
            self.shadows.clear();
        }
        self.catch_up(ctx);
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        let model = paint.model;
        let mut state = State {
            zoom: self.zoom(),
            shows: "nothing",
            ..State::default()
        };
        let clip = match place(model) {
            Place::Nothing => ClipScreen {
                name: String::new(),
                colour: GREY,
                body: ClipBody::Prompt("No clip".to_owned()),
                view: (0.0, 1.0),
                playhead: None,
                markers: Vec::new(),
                area: None,
                range: None,
                lines: Vec::new(),
                held: Vec::new(),
            },
            Place::Empty { track, audio, .. } => {
                let colour = model
                    .document
                    .track(track)
                    .map_or(GREY, |t| swatch_rgb(paint.colours, t.colour()));
                state.shows = if audio { "empty_audio" } else { "empty_midi" };
                paint.screen.upper[0] = if audio {
                    Label::new("Load Sample", WHITE, false)
                } else {
                    Label::default()
                };
                ClipScreen {
                    name: String::new(),
                    colour,
                    body: ClipBody::Prompt(
                        if audio {
                            "Empty slot. Load a sample."
                        } else {
                            "This slot is empty."
                        }
                        .to_owned(),
                    ),
                    view: (0.0, 1.0),
                    playhead: None,
                    markers: Vec::new(),
                    area: None,
                    range: None,
                    lines: Vec::new(),
                    held: Vec::new(),
                }
            }
            Place::Clip(shown) => {
                let shift = paint.modifiers.shift();
                let colour = shown.colour(model, paint.colours);
                let unit = shown.unit(model);
                let marks = shown.marks(model);
                let targets = self.targets(&shown, paint.held_steps, paint.modifiers, paint.note);
                // Holding a step zooms the display to the page holding it
                // (R55).
                let page = targets
                    .as_ref()
                    .filter(|t| t.select.is_none())
                    .and_then(|_| {
                        paint
                            .note
                            .and_then(|n| n.area)
                            .map(|a| (a.from.get() as f64 / BEAT, a.to.get() as f64 / BEAT))
                    });
                let view = self.view(&shown, model, page);
                let held: Vec<(f64, f64)> = paint
                    .held_steps
                    .iter()
                    .filter(|step| step.clip == Some(shown.clip.id))
                    .map(|step| {
                        (
                            step.start.get() as f64 / BEAT,
                            (step.start + step.length).get() as f64 / BEAT,
                        )
                    })
                    .collect();
                let mut screen = ClipScreen {
                    name: shown.clip.name.clone(),
                    colour,
                    body: ClipBody::Prompt(String::new()),
                    view,
                    playhead: shown.playhead(model),
                    markers: Vec::new(),
                    area: None,
                    range: None,
                    lines: Vec::new(),
                    held,
                };
                let mut push =
                    |name: &'static str, at: f64| screen.markers.push((name, unit.beats(at)));
                if marks.looping {
                    push("loop_start", marks.loop_start);
                    push("loop_end", marks.loop_end);
                    push("start", marks.start);
                } else {
                    push("start", marks.start);
                    push("end", marks.end);
                }
                match &shown.data {
                    Data::Midi(midi) => {
                        self.notes_screen(paint, &shown, midi, view, colour, &mut screen);
                        state.folded = paint.session
                            || paint
                                .note
                                .and_then(|n| n.area)
                                .is_some_and(|a| a.fit == Fit::Folded);
                        state.shows = "midi";
                        state.looping = Some(midi.looping);
                        state.area = paint
                            .note
                            .filter(|n| n.track == shown.track)
                            .and_then(|n| n.area);
                        paint.screen.upper[2] = Label::new("Crop", WHITE, false);
                    }
                    Data::Audio(audio) => {
                        screen.body = Self::waveform(&shown, model, view);
                        state.shows = "audio";
                        state.looping = Some(audio.looping);
                    }
                }
                paint.screen.upper[1] = Label::new("Loop", WHITE, state.looping == Some(true));
                state.clip = Some(shown.clip.id);
                state.name.clone_from(&shown.clip.name);
                state.view = view;
                let columns = match &targets {
                    Some(targets) => Self::settings(targets),
                    None => self.columns(&shown, model, shift),
                };
                if let Some(targets) = &targets {
                    state.held_steps = paint
                        .held_steps
                        .iter()
                        .filter(|s| s.clip == Some(shown.clip.id))
                        .copied()
                        .collect();
                    state.select_pad = targets.select;
                }
                state.controls = columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.is_empty())
                    .map(|(n, c)| Control {
                        column: n as u8,
                        name: c.name.clone(),
                        value: match &c.graphic {
                            Graphic::Choice(entry) if c.value.is_empty() => entry.clone(),
                            _ => c.value.clone(),
                        },
                    })
                    .collect();
                paint.screen.columns = columns;
                screen
            }
        };
        paint.screen.clip = Some(clip);
        self.state = state;
    }

    fn clip_state(&self) -> Option<State> {
        Some(self.state.clone())
    }
}
