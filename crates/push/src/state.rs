//! What the surface shares with the shell: the Setup values and the swing
//! amount the preferences keep, and a view of each running surface for
//! `push.state` (`docs/specs/push-2.md`, R10, R31 and R34).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::content::{DisplayMode, MixPage, PadMode, StripUse};
use crate::curve::Setup;
use crate::display::Status;
use crate::leds::{Addr, Led};
use crate::recording::FixedLength;
use crate::screen::Screen;
use crate::sysex::MidiMode;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What every Push 2 surface shares, and the shell keeps in the preferences:
/// the Setup values (R31), the swing amount (R10) and the 14 track colours
/// the palette of 70 swatches is made from.
pub struct Shared {
    setup: Mutex<Setup>,
    swing: Mutex<f64>,
    fixed: Mutex<FixedLength>,
    palette: Mutex<Vec<[u8; 3]>>,
    revision: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new(Setup::default(), 0.0, FixedLength::default())
    }
}

impl Shared {
    /// Shared state starting from the preferences' `setup`, `swing` and
    /// `fixed` Fixed Length.
    #[must_use]
    pub fn new(setup: Setup, swing: f64, fixed: FixedLength) -> Self {
        Self {
            setup: Mutex::new(setup.clamped()),
            swing: Mutex::new(swing.clamp(0.0, 1.0)),
            fixed: Mutex::new(fixed.clamped()),
            palette: Mutex::new(bl_model::DEFAULT_TRACK_PALETTE.to_vec()),
            revision: AtomicU64::new(0),
        }
    }

    /// The Setup values.
    #[must_use]
    pub fn setup(&self) -> Setup {
        *lock(&self.setup)
    }

    /// Sets the Setup values, each in its range (R31).
    pub fn set_setup(&self, setup: Setup) {
        *lock(&self.setup) = setup.clamped();
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// The swing amount, 0 to 1 (R10).
    #[must_use]
    pub fn swing(&self) -> f64 {
        *lock(&self.swing)
    }

    /// Sets the swing amount.
    pub fn set_swing(&self, swing: f64) {
        *lock(&self.swing) = swing.clamp(0.0, 1.0);
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// Fixed Length's settings (R50).
    #[must_use]
    pub fn fixed_length(&self) -> FixedLength {
        *lock(&self.fixed)
    }

    /// Sets Fixed Length's settings.
    pub fn set_fixed_length(&self, fixed: FixedLength) {
        *lock(&self.fixed) = fixed.clamped();
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// The 14 track colours the swatches are made from.
    #[must_use]
    pub fn track_colours(&self) -> Vec<[u8; 3]> {
        lock(&self.palette).clone()
    }

    /// Sets the 14 track colours, as the theme gives them.
    pub fn set_track_colours(&self, colours: Vec<[u8; 3]>) {
        *lock(&self.palette) = colours;
    }

    /// Counts every change to the Setup values or the swing amount, so the
    /// shell knows when to save the preferences.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
}

/// The menus that open over the display mode (R23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// Setup (R31).
    Setup,
    /// Scale.
    Scale,
    /// Quantize (notes R58).
    Quantize,
    /// Fixed Length (notes R50).
    FixedLength,
    /// The choices Convert offers for an audio clip (push-2-notes R76).
    Convert,
}

impl Menu {
    /// The menu's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Scale => "Scale",
            Self::Quantize => "Quantize",
            Self::FixedLength => "Fixed Length",
            Self::Convert => "Convert",
        }
    }
}

/// What `push.state` reads of a running surface (R34).
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The firmware version the Device Inquiry reported.
    pub firmware: Option<String>,
    /// Whether Push 2 has its own power supply.
    pub external_power: Option<bool>,
    /// Why the surface isn't driving a Push 2, if it isn't.
    pub error: Option<String>,
    /// The MIDI mode.
    pub midi_mode: MidiMode,
    /// The pad mode.
    pub pad_mode: PadMode,
    /// The display mode.
    pub display_mode: DisplayMode,
    /// Mix's page.
    pub mix_page: MixPage,
    /// The menu that is open.
    pub menu: Option<Menu>,
    /// What Note Mode shows while it is the pad mode.
    pub note: Option<crate::note::State>,
    /// The buttons held, by name.
    pub held: Vec<&'static str>,
    /// The buttons locked on, by name.
    pub locked: Vec<&'static str>,
    /// Whether the colour picker is open.
    pub picker: bool,
    /// What every LED should show.
    pub leds: Vec<Led>,
    /// The touch strip's configuration.
    pub strip: StripUse,
    /// The touch strip's LEDs.
    pub strip_leds: [u8; 31],
    /// The Setup values.
    pub setup: Setup,
    /// What the display shows.
    pub screen: Screen,
    /// What Browse Mode shows, while it is the display mode.
    pub browse: Option<crate::browse::State>,
    /// What Device Mode shows, while it is the display mode.
    pub device: Option<crate::device::State>,
    /// What Session Mode shows, while it is the pad mode.
    pub session: Option<crate::session::State>,
    /// What Mix Mode shows, while it is the display mode.
    pub mix: Option<crate::mix::State>,
    /// What the display's thread is doing.
    pub display: Status,
    /// What the Quantize menu shows while it is open (notes R58).
    pub quantize_menu: Option<QuantizeMenu>,
    /// What Record shows: `off`, `session` or `arrangement` (notes R48).
    pub record: &'static str,
    /// Fixed Length's settings (notes R50).
    pub fixed_length: FixedLength,
    /// The swing amount, 0 to 1 (notes R56).
    pub swing: f64,
    /// Whether Automation Arm is on (notes R61).
    pub automation_arm: bool,
}

/// The Quantize menu's values (notes R58).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizeMenu {
    /// Swing Amount, 0 to 100 %.
    pub swing: u8,
    /// Quantize To, as the grid's note value.
    pub quantize_to: String,
    /// Quantize Amount, 0 to 100 %.
    pub amount: u8,
    /// Record Quantize's value.
    pub record_quantize: String,
    /// Whether the menu stays after Quantize is released.
    pub stays: bool,
}

impl Snapshot {
    pub(crate) fn new() -> Self {
        Self {
            firmware: None,
            external_power: None,
            error: None,
            midi_mode: MidiMode::Live,
            pad_mode: PadMode::default(),
            display_mode: DisplayMode::default(),
            mix_page: MixPage::default(),
            menu: None,
            note: None,
            held: Vec::new(),
            locked: Vec::new(),
            picker: false,
            leds: vec![Led::default(); 256],
            strip: StripUse::default(),
            strip_leds: [0; 31],
            setup: Setup::default(),
            screen: Screen::default(),
            browse: None,
            device: None,
            session: None,
            mix: None,
            display: Status::default(),
            quantize_menu: None,
            record: "off",
            fixed_length: FixedLength::default(),
            swing: 0.0,
            automation_arm: false,
        }
    }

    /// What the LED of the control called `name` shows: a button by its
    /// name (`play`, `lower_1`), a pad as `pad_<row>_<column>`, row from the
    /// bottom and column from the left, both from 0.
    #[must_use]
    pub fn led(&self, name: &str) -> Option<Led> {
        let addr = if let Some(rest) = name.strip_prefix("pad_") {
            let (row, column) = rest.split_once('_')?;
            let (row, column): (u8, u8) = (row.parse().ok()?, column.parse().ok()?);
            (row < 8 && column < 8).then(|| Addr::Pad(crate::controls::pad_note(row, column)))?
        } else {
            Addr::Button(crate::controls::button_named(name)?.number)
        };
        Some(
            self.leds[match addr {
                Addr::Pad(note) => usize::from(note),
                Addr::Button(number) => 128 + usize::from(number),
            }],
        )
    }
}

/// What the shell asks a running surface to do (`push.set_mode`, R34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Switches the pad mode and the display mode as their buttons' taps do.
    SetMode {
        /// The pad mode, if it changes.
        pad: Option<PadMode>,
        /// The display mode and Mix's page, if they change.
        display: Option<(DisplayMode, MixPage)>,
    },
}

/// The shell's handle on one running surface: its snapshot, and the
/// commands it takes. The surface's `probe`.
pub struct Handle {
    snapshot: Mutex<Snapshot>,
    inbox: Mutex<Vec<Command>>,
}

impl Handle {
    pub(crate) fn new() -> Self {
        Self {
            snapshot: Mutex::new(Snapshot::new()),
            inbox: Mutex::new(Vec::new()),
        }
    }

    /// What the surface looked like at its last tick.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        lock(&self.snapshot).clone()
    }

    /// Asks the surface to do `command` on its next tick.
    pub fn send(&self, command: Command) {
        lock(&self.inbox).push(command);
    }

    pub(crate) fn publish(&self, snapshot: Snapshot) {
        *lock(&self.snapshot) = snapshot;
    }

    pub(crate) fn take(&self) -> Vec<Command> {
        std::mem::take(&mut *lock(&self.inbox))
    }
}
