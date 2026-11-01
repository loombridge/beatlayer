//! What the later packages plug into each mode: the pads, the display and
//! the encoders of Note, Session, Device, Browse, Mix and Clip
//! (`docs/specs/push-2.md`, R21 and R22).
//!
//! The surface routes the controls that behave the same in every mode
//! itself. Everything else goes to the [`Content`] of the pad mode or the
//! display mode that is active: its pads, its encoders, its buttons, the
//! touch strip. Until a package builds a mode's content, the mode shows its
//! name and its encoders control nothing ([`Named`]).

use std::collections::BTreeSet;

use bl_control::{Cx, ReadModel};
use serde_json::{Value, json};

use crate::controls::{LedKind, cc};
use crate::curve::Setup;
use crate::leds::{Addr, Led, Leds, Palette};
use crate::screen::Screen;

/// The pad mode (R21).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadMode {
    /// Note Mode, where the surface starts.
    #[default]
    Note,
    /// Session Mode.
    Session,
}

impl PadMode {
    /// The mode's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Session => "Session",
        }
    }

    /// The mode's button.
    #[must_use]
    pub const fn button(self) -> u8 {
        match self {
            Self::Note => cc::NOTE,
            Self::Session => cc::SESSION,
        }
    }
}

/// The display mode (R22).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayMode {
    /// Device, where the surface starts.
    #[default]
    Device,
    /// Browse, which has no hold.
    Browse,
    /// Mix.
    Mix,
    /// Clip.
    Clip,
}

impl DisplayMode {
    /// Every display mode, in the order of their buttons.
    pub const ALL: [Self; 4] = [Self::Device, Self::Browse, Self::Mix, Self::Clip];

    /// The mode's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Device => "Device",
            Self::Browse => "Browse",
            Self::Mix => "Mix",
            Self::Clip => "Clip",
        }
    }

    /// The mode's button.
    #[must_use]
    pub const fn button(self) -> u8 {
        match self {
            Self::Device => cc::DEVICE,
            Self::Browse => cc::BROWSE,
            Self::Mix => cc::MIX,
            Self::Clip => cc::CLIP,
        }
    }

    /// Whether holding the button switches while it is held (R22).
    #[must_use]
    pub const fn momentary(self) -> bool {
        !matches!(self, Self::Browse)
    }

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Device => 0,
            Self::Browse => 1,
            Self::Mix => 2,
            Self::Clip => 3,
        }
    }
}

/// Mix's two pages (R22).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MixPage {
    /// Track Mix, the first time.
    #[default]
    Track,
    /// Global Mix.
    Global,
}

/// A pad, by its row from the bottom and its column from the left (R6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    /// The row from the bottom, 0 to 7.
    pub row: u8,
    /// The column from the left, 0 to 7.
    pub column: u8,
}

/// What a pad did (R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadEvent {
    /// Pressed, with the velocity the curve gave, 1 to 127.
    On(u8),
    /// Released.
    Off,
    /// Held, with its pressure: polyphonic key pressure on this pad.
    Pressure(u8),
    /// Held, with the hardest-pressed pad's pressure (channel pressure).
    ChannelPressure(u8),
}

/// What an encoder did (R9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// A finger came on it.
    Touch,
    /// The finger left it.
    Release,
    /// It turned by this many steps: positive clockwise.
    Steps(i32),
}

/// What the touch strip did (R11).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StripEvent {
    /// A finger came on it.
    Touch,
    /// The finger left it.
    Release,
    /// Its position, 0 at the bottom to 1 at the top.
    Position(f32),
}

/// What a button did (R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Went down.
    Press,
    /// Came up.
    Release,
    /// Came up within 300 ms of going down, with no other control used.
    Tap,
    /// Has been down for 300 ms.
    Hold,
    /// Mute, Solo or Stop Clip held alone for 1 s: locked on (R24).
    Lock,
    /// A locked button pressed again.
    Unlock,
}

/// The buttons that are modifiers (R25).
pub const MODIFIERS: [u8; 10] = [
    cc::SHIFT,
    cc::SELECT,
    cc::DELETE,
    cc::DUPLICATE,
    cc::MUTE,
    cc::SOLO,
    cc::STOP_CLIP,
    cc::QUANTIZE,
    cc::RECORD,
    cc::LAYOUT,
];

/// The modifiers held now, locked ones included (R24, R25).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(pub BTreeSet<u8>);

impl Modifiers {
    /// Whether the button with controller number `button` is held or locked.
    #[must_use]
    pub fn has(&self, button: u8) -> bool {
        self.0.contains(&button)
    }

    /// Whether Shift is held.
    #[must_use]
    pub fn shift(&self) -> bool {
        self.has(cc::SHIFT)
    }

    /// Whether any is held.
    #[must_use]
    pub fn any(&self) -> bool {
        !self.0.is_empty()
    }
}

/// What the colour picker colours (R27).
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A track, by `track.set_colour`.
    Track(bl_model::TrackId),
    /// Anything else: `command` runs with `colour` added to `args`. A drum
    /// pad or a clip names its own command here.
    Command {
        /// The command's ID.
        command: String,
        /// Its arguments, without the colour.
        args: Value,
        /// The swatch it has now, which blinks.
        current: Option<u8>,
    },
}

impl Target {
    /// The command that gives the target swatch `colour`.
    pub(crate) fn colour_command(&self, colour: u8) -> (String, Value) {
        match self {
            Self::Track(track) => (
                "track.set_colour".to_owned(),
                json!({ "tracks": [track], "colour": colour }),
            ),
            Self::Command { command, args, .. } => {
                let mut args = args.clone();
                if let Value::Object(map) = &mut args {
                    map.insert("colour".to_owned(), json!(colour));
                }
                (command.clone(), args)
            }
        }
    }
}

/// Why a display mode's content is entered afresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    /// Its button was pressed.
    Button,
    /// Add Track was pressed (push-2-tracks R12).
    AddTrack,
    /// Add Device was pressed (push-2-tracks R25).
    AddDevice,
}

/// How the touch strip is configured (R11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StripUse {
    /// Pitch bend, one LED, back to the centre on release: flags 0x68. What
    /// Push 2 starts in.
    #[default]
    PitchBend,
    /// Modulation, a bar from the bottom that stays: flags 0x0C.
    Modulation,
    /// A range, lit by Beatlayer: flags 0x03.
    Range,
}

impl StripUse {
    /// The configuration flags Set Touch Strip Configuration takes.
    #[must_use]
    pub const fn flags(self) -> u8 {
        match self {
            Self::PitchBend => 0x68,
            Self::Modulation => 0x0C,
            Self::Range => 0x03,
        }
    }
}

/// What a content hears of the world and asks of it while it handles an
/// event.
pub struct Ctx<'a> {
    /// Commands to run and messages to send, as for any surface.
    pub cx: &'a mut Cx,
    /// The set and the engine's state as of the last tick.
    pub model: Option<&'a ReadModel>,
    /// The modifiers held or locked.
    pub modifiers: &'a Modifiers,
    /// The Setup values.
    pub setup: Setup,
    /// Mix's page, for Mix Mode (R22).
    pub mix_page: MixPage,
    pub(crate) actions: &'a mut Vec<Action>,
}

/// What a content asks the surface to do besides running commands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Notify(String),
    Strip(StripUse),
    Picker(Target),
    Aftertouch(bool),
    /// An encoder's turns are a gesture the surface ends at its release.
    Gesture(u8),
}

impl Ctx<'_> {
    /// Shows `text` in a box over the display for 1.5 s (R19).
    pub fn notify(&mut self, text: impl Into<String>) {
        self.actions.push(Action::Notify(text.into()));
    }

    /// Gives the touch strip the configuration its use needs (R11).
    pub fn set_strip(&mut self, how: StripUse) {
        self.actions.push(Action::Strip(how));
    }

    /// Opens the colour picker on `target` (R27).
    pub fn open_picker(&mut self, target: Target) {
        self.actions.push(Action::Picker(target));
    }

    /// Asks for a command that is part of the gesture on encoder `column`:
    /// its changes, from touch to release, are one undo step (R9).
    pub fn run_turn(&mut self, column: u8, id: &str, args: Value) {
        self.actions.push(Action::Gesture(column));
        self.cx.run_held(
            crate::surface::ENCODER_GESTURE + u32::from(column),
            id,
            args,
        );
    }

    /// Selects `track` as a click on its title bar does: the Session View's
    /// column when it has one, and the selection the surfaces read.
    pub fn select_track(&mut self, track: bl_model::TrackId) {
        if self.model.is_some_and(|m| m.columns.contains(&track)) {
            self.cx.run("session.select", json!({ "track": track }));
        }
        self.cx
            .run("arrangement.select_tracks", json!({ "tracks": [track] }));
    }

    /// Chooses channel pressure (false) or polyphonic key pressure for the
    /// pads' aftertouch (R8).
    pub fn set_aftertouch(&mut self, polyphonic: bool) {
        self.actions.push(Action::Aftertouch(polyphonic));
    }
}

/// The LEDs and the palette, for a content to light the pads and buttons it
/// owns.
pub struct Lights<'a> {
    pub(crate) leds: &'a mut Leds,
    pub(crate) palette: &'a mut Palette,
}

impl Lights<'_> {
    /// The palette index that shows `rgb` (R14).
    pub fn colour(&mut self, rgb: [u8; 3]) -> u8 {
        let in_use: BTreeSet<u8> = self.leds.colours().collect();
        self.palette.index(rgb, &|index| in_use.contains(&index))
    }

    /// Sets the pad in row `row` from the bottom and column `column`.
    pub fn pad(&mut self, row: u8, column: u8, led: Led) {
        self.leds
            .set(Addr::Pad(crate::controls::pad_note(row, column)), led);
    }

    /// Sets the button with controller number `number`.
    pub fn button(&mut self, number: u8, led: Led) {
        self.leds.set(Addr::Button(number), led);
    }

    /// Lights a white button: dim when it does something, off when it
    /// doesn't, full while its function is active (R13).
    pub fn white(&mut self, number: u8, available: bool, active: bool) {
        debug_assert!(
            crate::controls::button(number).is_some_and(|b| b.led == LedKind::White),
            "{number} isn't a white button"
        );
        self.button(number, Led::level(available, active));
    }

    /// Lights the touch strip's 31 LEDs, levels 0 to 7, the bottom first
    /// (R11).
    pub fn strip(&mut self, levels: [u8; 31]) {
        self.leds.set_strip(levels);
    }
}

/// What a content draws each tick.
pub struct Paint<'a> {
    /// The display's content: the content of a display mode fills it.
    pub screen: &'a mut Screen,
    /// The pads and buttons.
    pub lights: Lights<'a>,
    /// The set and the engine's state.
    pub model: &'a ReadModel,
    /// The modifiers held or locked.
    pub modifiers: &'a Modifiers,
    /// Mix's page, for Mix Mode (R22).
    pub mix_page: MixPage,
    /// The surface's row in the Control Surfaces table, from 0.
    pub row: usize,
    /// The 14 track colours the swatches are made from.
    pub colours: &'a [[u8; 3]],
}

/// The content of one pad mode or display mode.
pub trait Content: Send {
    /// The mode's name, which the placeholder shows.
    fn name(&self) -> &str;

    /// A button the surface doesn't handle itself, or one it handles and
    /// reports too: a modifier's own action comes as [`Gesture::Tap`] only
    /// when it wasn't used in a combination (R25).
    fn button(&mut self, _button: u8, _gesture: Gesture, _ctx: &mut Ctx<'_>) {}

    /// A pad (R8).
    fn pad(&mut self, _pad: Pad, _event: PadEvent, _ctx: &mut Ctx<'_>) {}

    /// One of the eight track encoders, `column` from 0 (R9).
    fn encoder(&mut self, _column: u8, _turn: Turn, _ctx: &mut Ctx<'_>) {}

    /// The touch strip (R11).
    fn strip(&mut self, _event: StripEvent, _ctx: &mut Ctx<'_>) {}

    /// The display mode is entered afresh, by its button or by another
    /// control that leads to it.
    fn enter(&mut self, _why: Enter, _ctx: &mut Ctx<'_>) {}

    /// A pad pressed while Select is held, offered to the display mode
    /// before the pad mode; true when it took it (push-2-tracks R38).
    fn pad_select(&mut self, _pad: Pad, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    /// What Mix Mode shows, for `push.state` (push-2-tracks R42).
    fn mix_state(
        &self,
        _model: &ReadModel,
        _page: MixPage,
        _row: usize,
    ) -> Option<crate::mix::State> {
        None
    }

    /// What Browse Mode shows, for `push.state` (push-2-tracks R14).
    fn browse_state(&self) -> Option<crate::browse::State> {
        None
    }

    /// What Device Mode shows, for `push.state` (push-2-tracks R27).
    fn device_state(&self) -> Option<crate::device::State> {
        None
    }

    /// What Session Mode shows, for `push.state` (push-2-tracks R73).
    fn session_state(&self) -> Option<crate::session::State> {
        None
    }

    /// Draws the mode: the display's columns and labels, the pads, the
    /// buttons it owns.
    fn paint(&mut self, _paint: &mut Paint<'_>) {}

    /// Whether the mode uses the lower display buttons itself rather than
    /// leaving them to the ring's tracks (R26).
    fn claims_lower(&self) -> bool {
        false
    }

    /// Whether Scale opens its menu: Note Mode with a melodic layout (R23).
    fn scale_available(&self) -> bool {
        false
    }
}

/// A mode whose content isn't built yet: it shows its name, and its encoders
/// control nothing (R21, R22).
pub struct Named {
    name: &'static str,
    titled: bool,
}

impl Named {
    /// The placeholder of a display mode, which writes its name on the
    /// display.
    #[must_use]
    pub const fn display(name: &'static str) -> Self {
        Self { name, titled: true }
    }

    /// The placeholder of a pad mode, which leaves the display alone.
    #[must_use]
    pub const fn pads(name: &'static str) -> Self {
        Self {
            name,
            titled: false,
        }
    }
}

impl Content for Named {
    fn name(&self) -> &str {
        self.name
    }

    fn paint(&mut self, paint: &mut Paint<'_>) {
        if self.titled {
            paint.screen.title = Some(self.name.to_owned());
        }
    }
}

/// The contents of the two pad modes and the four display modes.
pub struct Contents {
    /// Note, then Session.
    pub pads: [Box<dyn Content>; 2],
    /// Device, Browse, Mix, then Clip.
    pub display: [Box<dyn Content>; 4],
}

impl Contents {
    /// The placeholders: each mode shows its name.
    #[must_use]
    pub fn placeholders() -> Self {
        Self {
            pads: [
                Box::new(Named::pads(PadMode::Note.name())),
                Box::new(Named::pads(PadMode::Session.name())),
            ],
            display: DisplayMode::ALL
                .map(|mode| Box::new(Named::display(mode.name())) as Box<dyn Content>),
        }
    }
}

/// Makes the contents a surface starts with.
pub type MakeContents = std::sync::Arc<dyn Fn() -> Contents + Send + Sync>;
