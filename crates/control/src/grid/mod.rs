//! The grid surface and the Launchpad X, Launchpad Mini MK3 and Launchpad
//! Pro MK3 profiles of it (`docs/specs/supported-controllers.md`, R6 to R34).
//!
//! One surface plays all three: its pads are the session ring's slots, its
//! scene buttons launch the ring's scenes, its arrows move the ring, its
//! track buttons stop, solo, mute, arm or select the ring's tracks, its grid
//! faders set the mixer or the device in focus, and a drum rack on the
//! selected track gives it the drum layout. A profile says which messages
//! and buttons the controller has; [`Grid`] holds what is the same.
//!
//! What a control reaches is the shell's to work out when a message arrives
//! ([`Cx::control`]); what the surface shows comes from the read model on
//! each tick, and only the lights that changed are sent.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use bl_core::midi::MidiMessage;
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_engine::SlotState;
use bl_model::{
    ClipData, DEFAULT_TRACK_PALETTE, Document, RemoteMode, SceneId, TrackId, TrackKind, swatch_rgb,
};
use serde_json::json;

use crate::Hit;
use crate::surface::{
    ControlSurface, Cx, DrumView, Kind, ReadModel, Recognise, Ring, RingStep, Role, SurfaceState,
};

mod palette;
#[cfg(feature = "testing")]
pub mod sim;

pub use palette::{PALETTE, nearest};
use palette::{colour, dimmed};

/// How long a press of Capture MIDI may last and still be Session Record
/// (R22).
const TAP: Duration = Duration::from_millis(500);

/// How far ahead of now the clock is stamped: two ticks, so the controller's
/// flashing never waits for a late tick (R11).
const AHEAD: Duration = Duration::from_millis(50);

/// The ring is eight by eight on every grid controller (R6).
const RING: Ring = Ring {
    tracks: 8,
    scenes: 8,
};

/// The first note the drum rack's pad view can start on (drum racks R11).
const LAST_FIRST_PAD: u8 = 112;

/// The controller numbers of the DAW faders: Beatlayer's own (R21).
const FADER_CC: u8 = 21;

/// Which controller a surface is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Model {
    X,
    Mini,
    Pro,
}

impl Model {
    /// The byte after the maker's in the SysEx header (R18, R25, R29).
    const fn id(self) -> u8 {
        match self {
            Self::X => 0x0C,
            Self::Mini => 0x0D,
            Self::Pro => 0x0E,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::X => "Launchpad X",
            Self::Mini => "Launchpad Mini MK3",
            Self::Pro => "Launchpad Pro MK3",
        }
    }
}

const MAKER: [u8; 3] = [0x00, 0x20, 0x29];

/// The Launchpad Mini MK3 as the chooser offers it (R1, R25).
#[must_use]
pub fn mini() -> Kind {
    kind(
        Model::Mini,
        "launchpad-mini-mk3",
        "lpminimk3 daw",
        make_mini,
    )
}
/// The Launchpad Pro MK3 as the chooser offers it (R1, R29).
#[must_use]
pub fn pro() -> Kind {
    kind(Model::Pro, "launchpad-pro-mk3", "lppromk3 daw", make_pro)
}
/// The Launchpad X as the chooser offers it (R1, R18).
#[must_use]
pub fn launchpad_x() -> Kind {
    kind(Model::X, "launchpad-x", "lpx daw", make_x)
}

fn kind(
    model: Model,
    id: &'static str,
    port: &'static str,
    make: fn() -> Box<dyn ControlSurface>,
) -> Kind {
    Kind::new(id, model.name(), true, true, Some(RING), Vec::new(), make)
        .recognised(Recognise { port, maker: MAKER })
}

fn make_x() -> Box<dyn ControlSurface> {
    Box::new(Grid::new(Model::X))
}

fn make_mini() -> Box<dyn ControlSurface> {
    Box::new(Grid::new(Model::Mini))
}

fn make_pro() -> Box<dyn ControlSurface> {
    Box::new(Grid::new(Model::Pro))
}

/// Where a light is: the Session layout's pads, the drum layout's pads, or a
/// button (R19, R23).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    /// A pad of the Session layout: notes on channels 1 to 3.
    Pad,
    /// A pad of the drum layout: notes on channels 9 to 11.
    Drum,
    /// A button: controllers on channels 1 to 3.
    Button,
}

/// How a pad or button is lit (R19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Light {
    /// Steady, in a palette entry.
    Static(u8),
    /// Flashing between a static entry and another, in time with the clock.
    Flash {
        /// The static colour it flashes from.
        a: u8,
        /// The colour it flashes to.
        b: u8,
    },
    /// Pulsing between dark and a palette entry, in time with the clock.
    Pulse(u8),
}

/// The messages that give a pad or button a light, or put it out.
fn light_bytes(layer: Layer, number: u8, light: Option<Light>) -> Vec<Vec<u8>> {
    let base = match layer {
        Layer::Pad => 0x90,
        Layer::Drum => 0x98,
        Layer::Button => 0xB0,
    };
    match light {
        None => vec![vec![base, number, 0]],
        Some(Light::Static(c)) => vec![vec![base, number, c]],
        Some(Light::Flash { a, b }) => {
            vec![vec![base, number, a], vec![base + 1, number, b]]
        }
        Some(Light::Pulse(c)) => vec![vec![base + 2, number, c]],
    }
}

/// What a button's controller number is, per controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    Arrow(Dir),
    Scene(u8),
    Session,
    Note,
    /// The Launchpad X's Custom, or one of the Mini's own layouts.
    Own,
    Capture,
    StopSoloMute,
    TrackSelect(u8),
    /// The Pro's track controls 1 to 8.
    Control(u8),
    Shift,
    Clear,
    Duplicate,
    Quantise,
    Play,
    Record,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// The scene buttons' controller numbers, top to bottom (R19).
const SCENE: [u8; 8] = [89, 79, 69, 59, 49, 39, 29, 19];

impl Model {
    fn button(self, cc: u8) -> Option<Button> {
        if let Some(row) = SCENE.iter().position(|n| *n == cc) {
            let row = row as u8;
            return Some(match self {
                Self::Mini if row == 7 => Button::StopSoloMute,
                _ => Button::Scene(row),
            });
        }
        Some(match (self, cc) {
            (Self::X | Self::Mini, 91) => Button::Arrow(Dir::Up),
            (Self::X | Self::Mini, 92) => Button::Arrow(Dir::Down),
            (Self::X | Self::Mini, 93) => Button::Arrow(Dir::Left),
            (Self::X | Self::Mini, 94) => Button::Arrow(Dir::Right),
            (Self::X | Self::Mini, 95) => Button::Session,
            (Self::X, 96) => Button::Note,
            (Self::X, 97) => Button::Own,
            (Self::X, 98) => Button::Capture,
            (Self::Mini, 96..=98) => Button::Own,
            (Self::Pro, 90) => Button::Shift,
            (Self::Pro, 91) => Button::Arrow(Dir::Left),
            (Self::Pro, 92) => Button::Arrow(Dir::Right),
            (Self::Pro, 93) => Button::Session,
            (Self::Pro, 94) => Button::Note,
            (Self::Pro, 95..=98) => Button::Own,
            (Self::Pro, 80) => Button::Arrow(Dir::Up),
            (Self::Pro, 70) => Button::Arrow(Dir::Down),
            (Self::Pro, 60) => Button::Clear,
            (Self::Pro, 50) => Button::Duplicate,
            (Self::Pro, 40) => Button::Quantise,
            (Self::Pro, 20) => Button::Play,
            (Self::Pro, 10) => Button::Record,
            (Self::Pro, 101..=108) => Button::TrackSelect(cc - 101),
            (Self::Pro, 1..=8) => Button::Control(cc),
            _ => return None,
        })
    }

    /// The arrow's controller number.
    const fn arrow(self, dir: Dir) -> u8 {
        match (self, dir) {
            (Self::Pro, Dir::Up) => 80,
            (Self::Pro, Dir::Down) => 70,
            (Self::Pro, Dir::Left) => 91,
            (Self::Pro, Dir::Right) => 92,
            (_, Dir::Up) => 91,
            (_, Dir::Down) => 92,
            (_, Dir::Left) => 93,
            (_, Dir::Right) => 94,
        }
    }
}

/// What the grid's layout, as far as the surface knows, is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Session,
    /// The Pro's Fader layout, on this page (R31).
    Fader(u8),
    /// Note mode: the drum layout where the selected track has a drum rack.
    Note,
    /// One of the controller's own (Custom, Drums, Keys, User, Chord…).
    Own,
}

/// What a row of track buttons does (R13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackFn {
    Stop,
    Solo,
    Mute,
    Arm,
    Select,
}

impl TrackFn {
    const fn name(self) -> &'static str {
        match self {
            Self::Stop => "Clip Stop",
            Self::Solo => "Solo",
            Self::Mute => "Mute",
            Self::Arm => "Rec Arm",
            Self::Select => "Select",
        }
    }

    const fn role(self, column: u8) -> Role {
        match self {
            Self::Stop => Role::Stop(column),
            Self::Solo => Role::Solo(column),
            Self::Mute => Role::Mute(column),
            Self::Arm => Role::Arm(column),
            Self::Select => Role::Select(column),
        }
    }
}

/// What the Launchpad X's scene buttons choose in Mixer, top to bottom
/// (R20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mix {
    Volume,
    Pan,
    SendA,
    SendB,
    Stop,
    Mute,
    Solo,
    Arm,
}

const MIXES: [Mix; 8] = [
    Mix::Volume,
    Mix::Pan,
    Mix::SendA,
    Mix::SendB,
    Mix::Stop,
    Mix::Mute,
    Mix::Solo,
    Mix::Arm,
];

impl Mix {
    const fn name(self) -> &'static str {
        match self {
            Self::Volume => "Volume",
            Self::Pan => "Pan",
            Self::SendA => "Send A",
            Self::SendB => "Send B",
            Self::Stop => "Stop Clip",
            Self::Mute => "Mute",
            Self::Solo => "Solo",
            Self::Arm => "Record Arm",
        }
    }

    const fn faders(self) -> Option<Faders> {
        match self {
            Self::Volume => Some(Faders::Volume),
            Self::Pan => Some(Faders::Pan),
            Self::SendA => Some(Faders::Send(0)),
            Self::SendB => Some(Faders::Send(1)),
            _ => None,
        }
    }

    const fn track_fn(self) -> Option<TrackFn> {
        match self {
            Self::Stop => Some(TrackFn::Stop),
            Self::Mute => Some(TrackFn::Mute),
            Self::Solo => Some(TrackFn::Solo),
            Self::Arm => Some(TrackFn::Arm),
            _ => None,
        }
    }
}

/// What the Mini's Stop/Solo/Mute button has the bottom row do (R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cycle {
    Clips,
    Stop,
    Solo,
    Mute,
}

impl Cycle {
    const fn next(self) -> Self {
        match self {
            Self::Clips => Self::Stop,
            Self::Stop => Self::Solo,
            Self::Solo => Self::Mute,
            Self::Mute => Self::Clips,
        }
    }

    const fn track_fn(self) -> Option<TrackFn> {
        match self {
            Self::Clips => None,
            Self::Stop => Some(TrackFn::Stop),
            Self::Solo => Some(TrackFn::Solo),
            Self::Mute => Some(TrackFn::Mute),
        }
    }
}

/// What a set of grid faders reaches (R14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Faders {
    Volume,
    Pan,
    /// A send, from 0.
    Send(u8),
    Device,
}

impl Faders {
    const fn name(self) -> &'static str {
        match self {
            Self::Volume => "Volume",
            Self::Pan => "Pan",
            Self::Send(_) => "Send",
            Self::Device => "Device",
        }
    }

    const fn role(self, fader: u8) -> Role {
        match self {
            Self::Volume => Role::Volume(fader),
            Self::Pan => Role::Pan(fader),
            Self::Send(send) => Role::Send {
                column: fader,
                send,
            },
            Self::Device => Role::Device(fader),
        }
    }
}

/// A Mixer button held on the Launchpad X: what showed before it, and
/// whether anything else was pressed while it was held (R20).
#[derive(Clone, Copy, Debug)]
struct Peek {
    before: Option<Mix>,
    used: bool,
}

/// What the surface knows of the controller's state, and what it has told
/// the controller.
struct Grid {
    model: Model,
    layout: Layout,
    /// The Launchpad X's Mixer function, if Mixer shows.
    mixer: Option<Mix>,
    peek: Option<Peek>,
    /// The controller number of the Mixer button held.
    peek_button: Option<u8>,
    cycle: Cycle,
    /// The Pro's track function for the Track Select row, if one is chosen.
    row_fn: Option<TrackFn>,
    /// The send the Pro's Sends page shows, from 0.
    send: u8,
    shift: bool,
    clear: bool,
    duplicate: bool,
    quantise: bool,
    /// Whether the Pro's Session is held, showing Session Overview (R33).
    overview: bool,
    capture_down: Option<Instant>,
    /// The layout the next tick tells the controller to show.
    layout_dirty: bool,
    /// What each light was last sent as.
    lit: BTreeMap<(Layer, u8), Light>,
    /// Pads whose press went to a slot, so their release does.
    slots_down: HashSet<u8>,
    /// Drum pads held, with the note they play.
    drums_down: BTreeMap<u8, u8>,
    /// Whether the controller was told its drum layout is on.
    drum_mode: Option<bool>,
    /// The fader set up last, as the message that did it.
    fader_config: Option<Vec<u8>>,
    /// Each fader's position as last sent or taken.
    fader_at: [Option<u8>; 8],
    /// When the next clock pulse is due.
    clock: Option<Instant>,
    last: Option<ReadModel>,
    /// The row of the Control Surfaces table the surface runs in.
    row: usize,
}

impl Grid {
    fn new(model: Model) -> Self {
        Self {
            model,
            layout: Layout::Session,
            mixer: None,
            peek: None,
            peek_button: None,
            cycle: Cycle::Clips,
            row_fn: None,
            send: 0,
            shift: false,
            clear: false,
            duplicate: false,
            quantise: false,
            overview: false,
            capture_down: None,
            layout_dirty: false,
            lit: BTreeMap::new(),
            slots_down: HashSet::new(),
            drums_down: BTreeMap::new(),
            drum_mode: None,
            fader_config: None,
            fader_at: [None; 8],
            clock: None,
            last: None,
            row: 0,
        }
    }

    fn sysex(&self, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xF0, 0x00, 0x20, 0x29, 0x02, self.model.id()];
        bytes.extend_from_slice(body);
        bytes.push(0xF7);
        bytes
    }

    /// The layout message that shows the Session layout (R18, R25, R29).
    fn session_layout(&self) -> Vec<u8> {
        match self.model {
            Model::Pro => self.sysex(&[0x00, 0x00, 0x00, 0x00]),
            _ => self.sysex(&[0x00, 0x00]),
        }
    }

    // What the surface shows -------------------------------------------

    /// The faders the grid shows now, if any.
    fn faders(&self) -> Option<Faders> {
        match (self.model, self.layout) {
            (Model::X, Layout::Session) => self.mixer?.faders(),
            (Model::Pro, Layout::Fader(page)) => Some(match page {
                0 => Faders::Volume,
                1 => Faders::Pan,
                2 => Faders::Send(self.send),
                _ => Faders::Device,
            }),
            _ => None,
        }
    }

    /// What the grid's bottom row does instead of holding clips: the Mini's
    /// cycle and the X's Mixer functions (R20, R27).
    fn bottom_row(&self) -> Option<TrackFn> {
        match self.model {
            Model::X => self.mixer?.track_fn(),
            Model::Mini => self.cycle.track_fn(),
            Model::Pro => None,
        }
    }

    /// Whether the drum layout is what the controller shows.
    fn drum_visible(&self, model: &ReadModel) -> bool {
        self.layout == Layout::Note && model.drum.is_some()
    }

    /// The mode `control_surface.list` reports (R4).
    fn mode(&self) -> String {
        let mut parts = Vec::new();
        let layout = match self.layout {
            Layout::Session => match (self.model, self.mixer) {
                (Model::X, Some(mix)) => format!("Mixer: {}", mix.name()),
                _ => "Session".to_owned(),
            },
            Layout::Fader(page) => format!(
                "Fader: {}",
                ["Volume", "Pan", "Sends", "Device"][usize::from(page.min(3))]
            ),
            Layout::Note if self.last.as_ref().is_some_and(|m| m.drum.is_some()) => {
                "Drum".to_owned()
            }
            Layout::Note => "Note".to_owned(),
            Layout::Own => "Custom".to_owned(),
        };
        parts.push(layout);
        match self.model {
            Model::Pro => parts.push(format!(
                "track buttons: {}",
                self.row_fn.unwrap_or(TrackFn::Select).name()
            )),
            _ => {
                if let Some(f) = self.bottom_row() {
                    parts.push(format!("track buttons: {}", f.name()));
                }
            }
        }
        if let Some(f) = self.faders() {
            parts.push(format!("faders: {}", f.name()));
        }
        parts.join("; ")
    }

    // Colours ------------------------------------------------------------

    fn track_colour(doc: &Document, track: TrackId) -> [u8; 3] {
        let swatch = doc.track(track).map_or(0, bl_model::Track::colour);
        swatch_rgb(&DEFAULT_TRACK_PALETTE, swatch)
    }

    fn clip_colour(doc: &Document, track: TrackId, scene: SceneId) -> u8 {
        let rgb = match doc.slot(track, scene).and_then(|clip| clip.colour) {
            Some(swatch) => swatch_rgb(&DEFAULT_TRACK_PALETTE, swatch),
            None => Self::track_colour(doc, track),
        };
        nearest(rgb)
    }

    // Rendering ----------------------------------------------------------

    /// The state of a slot, from what the engine reports.
    fn state_of(model: &ReadModel, track: TrackId, scene: SceneId) -> Option<SlotState> {
        model
            .slots
            .iter()
            .find(|s| s.track == track && s.scene == scene)
            .map(|s| s.state)
    }

    /// A pad's slot as it shows (R9).
    fn slot_light(model: &ReadModel, track: TrackId, scene: SceneId) -> Option<Light> {
        let doc = &model.document;
        let clip = doc.slot(track, scene).is_some();
        let armed = doc.track(track).is_some_and(|t| t.mixer().arm);
        let colour_of = || Self::clip_colour(doc, track, scene);
        match (Self::state_of(model, track, scene), clip) {
            (Some(SlotState::Queued), true) => Some(Light::Flash {
                a: colour_of(),
                b: colour::GREEN,
            }),
            // A launch that waits for a slot with no clip is a recording.
            (Some(SlotState::Queued), false) => Some(Light::Flash {
                a: if armed { colour::DIM_RED } else { 0 },
                b: colour::RED,
            }),
            (Some(SlotState::Playing | SlotState::QueuedToStop), _) => {
                Some(Light::Pulse(colour::GREEN))
            }
            (Some(SlotState::Recording), _) => Some(Light::Pulse(colour::RED)),
            (None, true) => Some(Light::Static(colour_of())),
            (None, false) if armed => Some(Light::Static(colour::DIM_RED)),
            (None, false) => None,
        }
    }

    /// A scene button as it shows (R9).
    fn scene_light(model: &ReadModel, scene: SceneId) -> Option<Light> {
        let doc = &model.document;
        if model
            .slots
            .iter()
            .any(|s| s.scene == scene && s.state == SlotState::Playing)
        {
            return Some(Light::Static(colour::GREEN));
        }
        doc.tracks()
            .any(|t| doc.slot(t.id(), scene).is_some())
            .then_some(Light::Static(colour::DIM_WHITE))
    }

    /// A track button as it shows its track's state (R13).
    fn track_fn_light(model: &ReadModel, f: TrackFn, track: TrackId) -> Option<Light> {
        let doc = &model.document;
        let t = doc.track(track)?;
        let kind = t.kind();
        let lit = match f {
            TrackFn::Stop => {
                if !(kind.holds_clips() || kind == TrackKind::Group) {
                    return None;
                }
                let playing = model.slots.iter().any(|s| {
                    s.track == track
                        && matches!(
                            s.state,
                            SlotState::Playing | SlotState::QueuedToStop | SlotState::Recording
                        )
                });
                if playing {
                    colour::RED
                } else {
                    colour::DIM_RED
                }
            }
            TrackFn::Solo => {
                let on = t.mixer().solo;
                if on { colour::BLUE } else { colour::DIM_BLUE }
            }
            TrackFn::Mute => {
                let on = !t.mixer().mute;
                if on {
                    colour::YELLOW
                } else {
                    colour::DIM_YELLOW
                }
            }
            TrackFn::Arm => {
                if !kind.holds_clips() {
                    return None;
                }
                if t.mixer().arm {
                    colour::RED
                } else {
                    colour::DIM_RED
                }
            }
            TrackFn::Select => {
                let rgb = Self::track_colour(doc, track);
                let selected = model.selection.tracks.contains(&track);
                if selected { nearest(rgb) } else { dimmed(rgb) }
            }
        };
        Some(Light::Static(lit))
    }

    /// The ring of a row, if the read model has one.
    fn ring<'a>(&self, model: &'a ReadModel) -> Option<&'a crate::RingView> {
        model.row(self.row)?.ring.as_ref()
    }

    /// Every light the surface wants on now.
    fn render(&self, model: &ReadModel) -> BTreeMap<(Layer, u8), Light> {
        let mut out = BTreeMap::new();
        let ring = self.ring(model);
        let mut put = |layer, number, light: Option<Light>| {
            if let Some(light) = light {
                out.insert((layer, number), light);
            }
        };

        // The grid.
        if self.overview {
            self.overview_lights(model, ring, &mut put);
        } else if let Some(ring) = ring {
            for r in 0..8u8 {
                for c in 0..8u8 {
                    let note = pad_note(c, r);
                    let (Some(&track), Some(&scene)) = (
                        ring.tracks.get(usize::from(c)),
                        ring.scenes.get(usize::from(r)),
                    ) else {
                        continue;
                    };
                    let light = match self.bottom_row() {
                        Some(f) if r == 7 => Self::track_fn_light(model, f, track),
                        _ => Self::slot_light(model, track, scene),
                    };
                    put(Layer::Pad, note, light);
                }
                // A bottom row given to a track function shows it past the
                // last scene too.
            }
            if let (Some(f), true) = (self.bottom_row(), ring.scenes.len() < 8) {
                for (c, &track) in ring.tracks.iter().enumerate() {
                    put(
                        Layer::Pad,
                        pad_note(c as u8, 7),
                        Self::track_fn_light(model, f, track),
                    );
                }
            }
        }

        // The drum layout, whatever shows (the controller keeps it).
        if let Some(drum) = model.drum {
            for r in 0..8u8 {
                for c in 0..8u8 {
                    let Some(note) = drum_note(&drum, c, 7 - r) else {
                        continue;
                    };
                    let light = if drum.selected == Some(note) {
                        Some(Light::Static(colour::BLUE))
                    } else if drum.filled >> note & 1 == 1 {
                        Some(Light::Static(colour::YELLOW))
                    } else {
                        None
                    };
                    put(Layer::Drum, pad_note(c, r), light);
                }
            }
        }

        // The buttons.
        self.button_lights(model, ring, &mut put);
        out
    }

    fn overview_lights(
        &self,
        model: &ReadModel,
        ring: Option<&crate::RingView>,
        put: &mut impl FnMut(Layer, u8, Option<Light>),
    ) {
        let doc = &model.document;
        let scenes: Vec<SceneId> = doc.scenes().map(|s| s.id).collect();
        let at = |id: Option<TrackId>| id.and_then(|t| model.columns.iter().position(|x| *x == t));
        let ring_block = ring.map(|r| {
            (
                at(r.first_track).unwrap_or(0) / 8,
                r.first_scene
                    .and_then(|s| scenes.iter().position(|x| *x == s))
                    .unwrap_or(0)
                    / 8,
            )
        });
        for r in 0..8usize {
            for c in 0..8usize {
                let tracks = model.columns.iter().skip(c * 8).take(8);
                let block: Vec<SceneId> = scenes.iter().skip(r * 8).take(8).copied().collect();
                if c * 8 >= model.columns.len() || block.is_empty() {
                    continue;
                }
                let playing = tracks.clone().any(|t| {
                    model.slots.iter().any(|s| {
                        s.track == *t && block.contains(&s.scene) && s.state == SlotState::Playing
                    })
                });
                let light = if ring_block == Some((c, r)) {
                    Light::Static(colour::BROWN)
                } else if playing {
                    Light::Pulse(colour::GREEN)
                } else {
                    Light::Static(colour::BLUE)
                };
                put(Layer::Pad, pad_note(c as u8, r as u8), Some(light));
            }
        }
    }

    fn button_lights(
        &self,
        model: &ReadModel,
        ring: Option<&crate::RingView>,
        put: &mut impl FnMut(Layer, u8, Option<Light>),
    ) {
        let white = |on: bool| {
            Some(Light::Static(if on {
                colour::WHITE
            } else {
                colour::DIM_WHITE
            }))
        };
        let moves = ring.map(|r| r.moves).unwrap_or_default();
        // The arrows: lit while the ring, or the pad view, can move that way.
        let drum = self.drum_visible(model).then_some(model.drum).flatten();
        for dir in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
            let can = match drum {
                Some(d) => match dir {
                    Dir::Up | Dir::Right => d.first < LAST_FIRST_PAD,
                    Dir::Down | Dir::Left => d.first > 0,
                },
                None => match dir {
                    Dir::Up => moves.up,
                    Dir::Down => moves.down,
                    Dir::Left => moves.left,
                    Dir::Right => moves.right,
                },
            };
            put(
                Layer::Button,
                self.model.arrow(dir),
                can.then_some(Light::Static(colour::WHITE)),
            );
        }
        let scene_at = |r: usize| ring.and_then(|ring| ring.scenes.get(r).copied());
        match self.model {
            Model::X if self.mixer.is_some() => {
                for (cc, mix) in SCENE.iter().zip(MIXES) {
                    put(Layer::Button, *cc, white(self.mixer == Some(mix)));
                }
            }
            Model::Mini => {
                // The bottom button is Stop/Solo/Mute, not a scene's (R26).
                for (row, cc) in SCENE.iter().enumerate().take(7) {
                    let light = scene_at(row).and_then(|s| Self::scene_light(model, s));
                    put(Layer::Button, *cc, light);
                }
                let cycle = match self.cycle {
                    Cycle::Clips => colour::WHITE,
                    Cycle::Stop => colour::RED,
                    Cycle::Solo => colour::BLUE,
                    Cycle::Mute => colour::YELLOW,
                };
                put(Layer::Button, SCENE[7], Some(Light::Static(cycle)));
            }
            _ => {
                for (row, cc) in SCENE.iter().enumerate() {
                    let light = scene_at(row).and_then(|s| Self::scene_light(model, s));
                    put(Layer::Button, *cc, light);
                }
            }
        }
        match self.model {
            Model::X => {
                let recording = model.session_record;
                put(
                    Layer::Button,
                    98,
                    Some(Light::Static(if recording {
                        colour::RED
                    } else {
                        colour::DIM_WHITE
                    })),
                );
            }
            Model::Mini => {}
            Model::Pro => self.pro_lights(model, ring, put),
        }
    }

    fn pro_lights(
        &self,
        model: &ReadModel,
        ring: Option<&crate::RingView>,
        put: &mut impl FnMut(Layer, u8, Option<Light>),
    ) {
        let dim = Some(Light::Static(colour::DIM_WHITE));
        put(Layer::Button, 90, dim);
        for cc in [60, 50, 40] {
            put(Layer::Button, cc, dim);
        }
        put(
            Layer::Button,
            20,
            Some(Light::Static(if model.transport.playing {
                colour::GREEN
            } else {
                colour::DIM_GREEN
            })),
        );
        put(
            Layer::Button,
            10,
            Some(Light::Static(if model.session_record {
                colour::RED
            } else {
                colour::DIM_RED
            })),
        );
        // The track controls: the chosen one bright.
        let chosen = |n: u8| match n {
            1 => self.row_fn == Some(TrackFn::Arm),
            2 => self.row_fn == Some(TrackFn::Mute),
            3 => self.row_fn == Some(TrackFn::Solo),
            4 => self.layout == Layout::Fader(0),
            5 => self.layout == Layout::Fader(1),
            6 => self.layout == Layout::Fader(2),
            7 => self.layout == Layout::Fader(3),
            _ => self.row_fn == Some(TrackFn::Stop),
        };
        for n in 1..=8u8 {
            let gold = self.shift && matches!(n, 1 | 2 | 3 | 6);
            put(
                Layer::Button,
                n,
                Some(Light::Static(if gold {
                    colour::GOLD
                } else if chosen(n) {
                    colour::WHITE
                } else {
                    colour::DIM_WHITE
                })),
            );
        }
        if self.shift {
            put(Layer::Button, 10, Some(Light::Static(colour::GOLD)));
        }
        // The Track Select row.
        if self.layout == Layout::Fader(3) {
            let focus = self.focus_device(model);
            for (i, device) in self.devices(model).iter().enumerate() {
                put(
                    Layer::Button,
                    101 + i as u8,
                    Some(Light::Static(if focus == Some(*device) {
                        colour::WHITE
                    } else {
                        colour::DIM_WHITE
                    })),
                );
            }
        } else if let Some(ring) = ring {
            let f = self.row_fn.unwrap_or(TrackFn::Select);
            for (c, &track) in ring.tracks.iter().enumerate() {
                put(
                    Layer::Button,
                    101 + c as u8,
                    Self::track_fn_light(model, f, track),
                );
            }
        }
    }

    /// The device in focus on this row.
    fn focus_device(&self, model: &ReadModel) -> Option<bl_model::DeviceId> {
        Some(model.row(self.row)?.focus.as_ref()?.device)
    }

    /// The first eight devices of the focus device's track (R31).
    fn devices(&self, model: &ReadModel) -> Vec<bl_model::DeviceId> {
        let doc = &model.document;
        self.focus_device(model)
            .and_then(|device| doc.device_track(device))
            .and_then(|track| doc.track(track))
            .map(|track| track.devices().take(8).map(|d| d.id).collect())
            .unwrap_or_default()
    }
}

mod events;

/// The pad at a column and a row counted from the top, as the Session
/// layout numbers it: ten times the row counted from the bottom, plus the
/// column (R19).
const fn pad_note(column: u8, row: u8) -> u8 {
    10 * (8 - row) + column + 1
}

/// The column and row from the top of a pad's note, if it is one.
const fn pad_at(note: u8) -> Option<(u8, u8)> {
    let (tens, ones) = (note / 10, note % 10);
    if tens >= 1 && tens <= 8 && ones >= 1 && ones <= 8 {
        Some((ones - 1, 8 - tens))
    } else {
        None
    }
}

/// The note the drum layout's pad at a column and a row from the bottom
/// plays: four 4 × 4 areas, the lower left the pads the pad view shows, the
/// one above it the next sixteen and the right-hand ones the thirty-two
/// after those, each area bottom row first, left to right (R16).
fn drum_note(drum: &DrumView, column: u8, row_from_bottom: u8) -> Option<u8> {
    let area = (column / 4) * 2 + row_from_bottom / 4;
    let offset = area * 16 + (row_from_bottom % 4) * 4 + column % 4;
    let note = u16::from(drum.first) + u16::from(offset);
    u8::try_from(note).ok().filter(|n| *n < 128)
}
