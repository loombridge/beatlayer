//! The Push 2 surface: it starts and stops the device, reads its controls,
//! lights its LEDs and fills its display (`docs/specs/push-2.md`, R1 to R36).

use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_control::{ControlSurface, Cx, ReadModel, SlotState, SurfaceState};
use bl_core::midi::MidiMessage;
use bl_model::TrackId;
use serde_json::{Value, json};

use crate::buttons::{self, Buttons};
use crate::content::{
    Action, Content, Contents, Ctx, DisplayMode, Enter, Gesture, Lights, MakeContents, MixPage,
    Modifiers, Pad, PadEvent, PadMode, Paint, StripEvent, StripUse, Target, Turn,
};
use crate::controls::{self, BUTTONS, LedKind, STRIP_TOUCH, cc};
use crate::curve::{Setup, Workflow};
use crate::display::{Claim, Display};
use crate::encoder::{self, Counting, LIST_STEPS, Moving};
use crate::leds::{Addr, Led, Leds, Palette, fixed};
use crate::screen::{Column, Graphic, Label, Screen};
use crate::state::{Command, Handle, Menu, Shared, Snapshot};
use crate::sysex::{self, MidiMode, Reply};

/// How long a reply to a command may take before the command is sent again,
/// and then before the surface carries on without it (R4).
const REPLY: Duration = Duration::from_millis(500);
/// How long a notification stays (R19).
const NOTIFICATION: Duration = Duration::from_millis(1500);
/// Two taps of footswitch 2 within this are a double tap (R29).
const DOUBLE_TAP: Duration = Duration::from_millis(300);
/// How far ahead MIDI clock is stamped.
const CLOCK_AHEAD: Duration = Duration::from_millis(60);
/// The gesture key of encoder `n` is this plus `n`.
const ENCODER_GESTURE: u32 = 0x5000;
/// What the aftertouch thresholds are (R30).
const AFTERTOUCH: (u16, u16) = (1220, 1570);

/// What a command to Push 2 waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Identity,
    Mode,
    Statistics,
}

struct Step {
    bytes: Vec<u8>,
    expect: Option<Expect>,
}

struct Waiting {
    expect: Expect,
    sent: Instant,
    retried: bool,
    bytes: Vec<u8>,
}

enum Phase {
    /// Sending R4's sequence.
    Starting {
        queue: VecDeque<Step>,
        waiting: Option<Waiting>,
    },
    /// Running.
    Live,
    /// Not a Push 2, or something else that ends it.
    Failed,
}

/// Which modes' contents an event goes to.
#[derive(Clone, Copy)]
enum Who {
    Pads,
    /// Session Mode's pads, whichever pad mode is active.
    SessionPads,
    Display,
    Both,
}

/// The colour picker (R27).
struct Picker {
    target: Target,
    /// The swatch the target has now.
    current: Option<u8>,
}

/// The Push 2 surface.
pub struct Push {
    shared: Arc<Shared>,
    handle: Arc<Handle>,
    claim: Claim,
    contents: Contents,
    phase: Phase,
    user_mode: bool,
    /// Whether the sustain pedal (footswitch 1) is down.
    sustain_down: bool,
    firmware: Option<String>,
    external_power: Option<bool>,
    error: Option<String>,
    leds: Leds,
    palette: Palette,
    buttons: Buttons,
    pad_mode: PadMode,
    display_mode: DisplayMode,
    mix_page: MixPage,
    momentary_pad: Option<PadMode>,
    momentary_display: Option<(DisplayMode, MixPage)>,
    menu: Option<Menu>,
    picker: Option<Picker>,
    strip: StripUse,
    strip_sent: StripUse,
    aftertouch: bool,
    aftertouch_sent: bool,
    screen: Screen,
    notification: Option<(String, Instant)>,
    display: Option<Display>,
    /// The Setup values Push 2 has been sent.
    applied: Setup,
    model: Option<ReadModel>,
    touched: [bool; 11],
    gestures: [bool; 11],
    master: Moving,
    cue: Moving,
    swing: Moving,
    tempo: Option<(f64, Instant)>,
    setup_counts: [Counting; 8],
    last_pad: Pad,
    /// Pads pressed while the colour picker was open: their release and
    /// pressure belong to nobody.
    swallowed: std::collections::BTreeSet<(u8, u8)>,
    clock: Option<Instant>,
    playing: bool,
    count_in: Option<Instant>,
    footswitch: Option<Instant>,
    actions: Vec<Action>,
}

impl Push {
    /// A surface that starts from `shared`, claims its display with `claim`
    /// and makes the content of its modes with `contents`.
    #[must_use]
    pub fn new(shared: Arc<Shared>, claim: Claim, contents: &MakeContents) -> Self {
        let applied = shared.setup();
        Self {
            shared,
            handle: Arc::new(Handle::new()),
            claim,
            contents: contents(),
            phase: Phase::Live,
            user_mode: false,
            sustain_down: false,
            firmware: None,
            external_power: None,
            error: None,
            leds: Leds::default(),
            palette: Palette::default(),
            buttons: Buttons::default(),
            pad_mode: PadMode::default(),
            display_mode: DisplayMode::default(),
            mix_page: MixPage::default(),
            momentary_pad: None,
            momentary_display: None,
            menu: None,
            picker: None,
            strip: StripUse::default(),
            strip_sent: StripUse::default(),
            aftertouch: false,
            aftertouch_sent: false,
            screen: Screen::default(),
            notification: None,
            display: None,
            applied,
            model: None,
            touched: [false; 11],
            gestures: [false; 11],
            master: Moving::default(),
            cue: Moving::default(),
            swing: Moving::default(),
            tempo: None,
            setup_counts: Default::default(),
            last_pad: Pad { row: 0, column: 0 },
            swallowed: std::collections::BTreeSet::new(),
            clock: None,
            playing: false,
            count_in: None,
            footswitch: None,
            actions: Vec::new(),
        }
    }

    fn send(cx: &mut Cx, bytes: Vec<u8>) {
        debug_assert!(sysex::allowed(&bytes), "{bytes:02X?}");
        // Whatever is not on the list never goes out (R5).
        if sysex::allowed(&bytes) {
            cx.send(bytes);
        }
    }

    fn pad_content(&mut self) -> &mut dyn Content {
        match self.pad_mode {
            PadMode::Note => self.contents.pads[0].as_mut(),
            PadMode::Session => self.contents.pads[1].as_mut(),
        }
    }

    fn display_content(&mut self) -> &mut dyn Content {
        self.contents.display[self.display_mode.index()].as_mut()
    }

    // ----- starting and stopping (R4) -----

    /// R4's sequence, in order.
    fn sequence(&mut self) -> VecDeque<Step> {
        let setup = self.shared.setup();
        self.applied = setup;
        let mut steps = VecDeque::new();
        let expecting = |bytes: Vec<u8>, expect| Step {
            bytes,
            expect: Some(expect),
        };
        steps.push_back(expecting(sysex::DEVICE_INQUIRY.to_vec(), Expect::Identity));
        steps.push_back(expecting(sysex::midi_mode(MidiMode::Live), Expect::Mode));
        steps.push_back(expecting(sysex::request_statistics(), Expect::Statistics));
        // The palette entries it uses: the theme's 14 track colours, and
        // Reapply Color Palette.
        let colours = self.shared.track_colours();
        for rgb in colours {
            self.palette.index(rgb, &|_| false);
        }
        let mut writes = self.palette.take_writes();
        if writes.is_empty() {
            writes.push(sysex::reapply_palette());
        }
        let mut more: Vec<Vec<u8>> = writes;
        more.push(sysex::led_brightness(setup.led_value()));
        more.push(sysex::display_brightness(setup.display_value()));
        more.extend(crate::curve::messages(&setup.curve()));
        more.push(sysex::pad_parameters(AFTERTOUCH.0, AFTERTOUCH.1));
        more.push(sysex::aftertouch_mode(self.aftertouch));
        more.push(sysex::touch_strip_config(self.strip.flags()));
        self.aftertouch_sent = self.aftertouch;
        self.strip_sent = self.strip;
        for bytes in more {
            steps.push_back(Step {
                bytes,
                expect: None,
            });
        }
        steps
    }

    /// Sends steps until one waits for a reply; when none is left, the
    /// surface is live.
    fn advance(&mut self, cx: &mut Cx) {
        let now = cx.now();
        let Phase::Starting { queue, waiting } = &mut self.phase else {
            return;
        };
        while waiting.is_none() {
            let Some(step) = queue.pop_front() else {
                break;
            };
            Self::send(cx, step.bytes.clone());
            if let Some(expect) = step.expect {
                *waiting = Some(Waiting {
                    expect,
                    sent: now,
                    retried: false,
                    bytes: step.bytes,
                });
            }
        }
        if waiting.is_none() && queue.is_empty() {
            self.phase = Phase::Live;
            // Every LED (R13), and the first frame, which the display's
            // thread sends once it has claimed its display (R17).
            self.leds.forget();
            self.flush_leds(cx);
        }
    }

    fn flush_leds(&mut self, cx: &mut Cx) {
        for bytes in self.palette.take_writes() {
            Self::send(cx, bytes);
        }
        let strip = self.strip == StripUse::Range;
        for bytes in self.leds.flush(strip) {
            if sysex::is_sysex(&bytes) {
                Self::send(cx, bytes);
            } else {
                cx.send(bytes);
            }
        }
    }

    fn reply(&mut self, bytes: &[u8], cx: &mut Cx) {
        let reply = sysex::parse(bytes);
        let waiting = match &self.phase {
            Phase::Starting { waiting, .. } => waiting.as_ref().map(|w| w.expect),
            _ => None,
        };
        let satisfied = match (&reply, waiting) {
            (Reply::Identity { firmware }, _) => {
                self.firmware = Some(firmware.clone());
                waiting == Some(Expect::Identity)
            }
            (Reply::OtherDevice, Some(Expect::Identity)) => {
                self.error =
                    Some("the device that answered the Device Inquiry isn't a Push 2".to_owned());
                self.phase = Phase::Failed;
                self.publish();
                return;
            }
            (Reply::Statistics { external_power }, _) => {
                self.external_power = Some(*external_power);
                waiting == Some(Expect::Statistics)
            }
            (Reply::MidiMode(_), Some(Expect::Mode)) => true,
            _ => false,
        };
        if satisfied && let Phase::Starting { waiting, .. } = &mut self.phase {
            *waiting = None;
            self.advance(cx);
        }
        self.publish();
    }

    fn wait_timeouts(&mut self, cx: &mut Cx) {
        let now = cx.now();
        let Phase::Starting { waiting, .. } = &mut self.phase else {
            return;
        };
        let Some(wait) = waiting else {
            return;
        };
        if now.saturating_duration_since(wait.sent) < REPLY {
            return;
        }
        if wait.retried {
            *waiting = None;
            self.advance(cx);
        } else {
            wait.retried = true;
            wait.sent = now;
            let bytes = wait.bytes.clone();
            Self::send(cx, bytes);
        }
    }

    // ----- commands the controls run -----

    fn run(cx: &mut Cx, id: &str, args: Value) {
        cx.run(id, args);
    }

    fn notify(&mut self, now: Instant, text: impl Into<String>) {
        self.notification = Some((text.into(), now + NOTIFICATION));
    }

    fn apply(&mut self, now: Instant) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::Notify(text) => self.notify(now, text),
                Action::Strip(how) => self.strip = how,
                Action::Picker(target) => self.open_picker(target),
                Action::Aftertouch(poly) => self.aftertouch = poly,
            }
        }
    }

    /// Runs `f` on the active pad mode's content and then the display
    /// mode's, with a context.
    fn with_contents(
        &mut self,
        cx: &mut Cx,
        who: Who,
        mut f: impl FnMut(&mut dyn Content, &mut Ctx<'_>),
    ) {
        let modifiers = self.buttons.modifiers();
        let setup = self.shared.setup();
        let mut actions = std::mem::take(&mut self.actions);
        {
            let pad = match self.pad_mode {
                PadMode::Note => self.contents.pads[0].as_mut(),
                PadMode::Session => self.contents.pads[1].as_mut(),
            };
            let display = self.contents.display[self.display_mode.index()].as_mut();
            let chosen: Vec<&mut dyn Content> = match who {
                Who::Pads => vec![pad],
                Who::SessionPads => vec![self.contents.pads[1].as_mut()],
                Who::Display => vec![display],
                Who::Both => vec![pad, display],
            };
            for content in chosen {
                let mut ctx = Ctx {
                    cx,
                    model: self.model.as_ref(),
                    modifiers: &modifiers,
                    setup,
                    actions: &mut actions,
                };
                f(content, &mut ctx);
            }
        }
        self.actions = actions;
        self.apply(cx.now());
    }

    // ----- the ring's tracks (R26) -----

    fn ring_tracks(&self, row: usize) -> Vec<TrackId> {
        self.model
            .as_ref()
            .and_then(|model| model.row(row))
            .and_then(|row| row.ring.as_ref())
            .map(|ring| ring.tracks.clone())
            .unwrap_or_default()
    }

    fn lower_claimed(&mut self) -> bool {
        self.display_content().claims_lower()
    }

    // ----- the colour picker (R27) -----

    fn open_picker(&mut self, target: Target) {
        let current = match &target {
            Target::Track(track) => self
                .model
                .as_ref()
                .and_then(|m| m.document.track(*track))
                .map(bl_model::Track::colour),
            Target::Command { current, .. } => *current,
        };
        self.picker = Some(Picker { target, current });
    }

    /// The pad of the outer ring that holds swatch `n` of the first two
    /// rows: clockwise from the bottom left (R27).
    fn ring_pad(n: u8) -> (u8, u8) {
        match n {
            0..=6 => (n, 0),
            7..=13 => (7, n - 7),
            14..=20 => (7 - (n - 14), 7),
            _ => (0, 7 - (n - 21)),
        }
    }

    fn pick(&mut self, cx: &mut Cx, row: u8, column: u8) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        if let Some(n) = (0..28u8).find(|&n| Self::ring_pad(n) == (row, column)) {
            let (id, args) = picker.target.colour_command(n);
            cx.run(&id, args);
        } else {
            // An inner pad changes nothing and leaves the picker open.
            self.picker = Some(picker);
        }
    }

    // ----- buttons (R7, R21 to R26) -----

    fn is_core(&mut self, button: u8) -> bool {
        matches!(
            button,
            cc::TAP_TEMPO
                | cc::METRONOME
                | cc::PLAY
                | cc::UNDO
                | cc::DOUBLE_LOOP
                | cc::SETUP
                | cc::SCALE
                | cc::USER
                | cc::NOTE
                | cc::SESSION
                | cc::DEVICE
                | cc::BROWSE
                | cc::MIX
                | cc::CLIP
                | cc::SHIFT
        ) || (controls::lower_column(button).is_some() && !self.lower_claimed())
    }

    fn button_down(&mut self, button: u8, at: Instant, cx: &mut Cx) {
        let others = self.buttons.modifiers();
        // A locked Mute, Solo or Stop Clip still modifies pads and the
        // contents, but core buttons only combine with a modifier held
        // down (R25), so Play, Undo and the modes work while one is locked.
        let others_held = others
            .0
            .iter()
            .any(|&m| m != button && self.buttons.is_down(m));
        let events = self.buttons.press(button, at);
        if self.user_mode && button != cc::USER {
            return;
        }
        if matches!(events.first(), Some(buttons::Event::Unlock(_))) {
            self.with_contents(cx, Who::Both, |content, ctx| {
                content.button(button, Gesture::Unlock, ctx)
            });
            return;
        }
        // Any button closes the colour picker, with no change (R27).
        if self.picker.take().is_some() {
            self.buttons.mark_used(button);
            return;
        }
        // Tapping a display mode's button while a menu is open closes the
        // menu and leaves the mode as it was (R23).
        if self.menu.is_some() && DisplayMode::ALL.iter().any(|m| m.button() == button) {
            self.menu = None;
            self.buttons.mark_used(button);
            return;
        }
        if self.stop_clip(button, cx) {
            return;
        }
        if self.is_core(button) {
            if others_held {
                self.combination(button, &others, cx);
                self.buttons.mark_used(button);
            } else if button == cc::TAP_TEMPO {
                Self::run(cx, "transport.tap_tempo", json!({}));
                self.buttons.mark_used(button);
            }
            return;
        }
        self.with_contents(cx, Who::Both, |content, ctx| {
            content.button(button, Gesture::Press, ctx)
        });
    }

    /// What a core button does with a modifier held: the combination the
    /// specs give it, and nothing else (R25).
    fn combination(&mut self, button: u8, modifiers: &Modifiers, cx: &mut Cx) {
        if !modifiers.shift() {
            return;
        }
        match button {
            cc::PLAY => {
                let playing = self.model.as_ref().is_some_and(|m| m.transport.playing);
                if playing {
                    Self::run(cx, "transport.play_stop", json!({}));
                } else {
                    // Shift with Play while stopped goes to 1.1.1 without
                    // starting (R12).
                    Self::run(cx, "transport.return_to_start", json!({}));
                }
            }
            cc::UNDO => Self::run(cx, "edit.redo", json!({})),
            button => {
                if let Some(column) = controls::lower_column(button)
                    && let Some(&track) = self.ring_tracks(cx.row()).get(usize::from(column))
                {
                    self.open_picker(Target::Track(track));
                }
            }
        }
    }

    fn button_event(&mut self, event: buttons::Event, cx: &mut Cx) {
        match event {
            buttons::Event::Press(_) | buttons::Event::Unlock(_) => {}
            buttons::Event::Release {
                button,
                tap,
                held,
                used,
            } => self.button_up(button, tap, held, used, cx),
            buttons::Event::Hold(button) => self.hold(button, cx),
            buttons::Event::Lock(button) => {
                self.with_contents(cx, Who::Both, |content, ctx| {
                    content.button(button, Gesture::Lock, ctx)
                });
            }
        }
    }

    fn hold(&mut self, button: u8, cx: &mut Cx) {
        if self.user_mode {
            return;
        }
        for mode in [PadMode::Note, PadMode::Session] {
            if mode.button() == button && mode != self.pad_mode {
                self.momentary_pad = Some(self.pad_mode);
                self.pad_mode = mode;
            }
        }
        for mode in DisplayMode::ALL {
            if mode.button() == button && mode.momentary() && mode != self.display_mode {
                self.momentary_display = Some((self.display_mode, self.mix_page));
                self.display_mode = mode;
            }
        }
        if !self.is_core(button) {
            self.with_contents(cx, Who::Both, |content, ctx| {
                content.button(button, Gesture::Hold, ctx)
            });
        }
    }

    fn button_up(&mut self, button: u8, tap: bool, held: bool, used: bool, cx: &mut Cx) {
        if self.user_mode {
            if button == cc::USER && !used {
                self.set_user_mode(false, cx);
            }
            return;
        }
        if held {
            if [PadMode::Note.button(), PadMode::Session.button()].contains(&button)
                && let Some(base) = self.momentary_pad.take()
            {
                self.pad_mode = base;
            }
            if let Some((mode, page)) = self.momentary_display.take() {
                if DisplayMode::ALL.iter().any(|m| m.button() == button) {
                    self.display_mode = mode;
                    self.mix_page = page;
                } else {
                    self.momentary_display = Some((mode, page));
                }
            }
        }
        if !self.is_core(button) {
            self.with_contents(cx, Who::Both, |content, ctx| {
                content.button(button, Gesture::Release, ctx)
            });
            if tap {
                self.with_contents(cx, Who::Both, |content, ctx| {
                    content.button(button, Gesture::Tap, ctx)
                });
                // Add Track leads to Browse Mode with the track kinds
                // (push-2-tracks R12).
                if button == cc::ADD_TRACK {
                    self.display_mode = DisplayMode::Browse;
                    self.enter(Enter::AddTrack, cx);
                }
                // Stop Clip stops the selected track's clip (push-2-tracks
                // R68).
                if button == cc::STOP_CLIP
                    && let Some(&track) = self
                        .model
                        .as_ref()
                        .and_then(|model| model.selection.tracks.first())
                {
                    Self::run(cx, "session.stop_track", json!({ "track": track }));
                }
            }
            return;
        }
        if used {
            return;
        }
        match button {
            cc::PLAY => Self::run(cx, "transport.play_stop", json!({})),
            cc::METRONOME => {
                let on = self
                    .model
                    .as_ref()
                    .is_some_and(|m| m.document.settings().metronome.enabled);
                Self::run(
                    cx,
                    "transport.set_metronome_enabled",
                    json!({ "enabled": !on }),
                );
            }
            cc::UNDO => Self::run(cx, "edit.undo", json!({})),
            cc::DOUBLE_LOOP => Self::run(cx, "clip.duplicate_loop", json!({})),
            cc::SETUP => self.toggle_menu(Menu::Setup),
            cc::SCALE => {
                if self.pad_content().scale_available() {
                    self.toggle_menu(Menu::Scale);
                }
            }
            cc::USER if tap => self.set_user_mode(true, cx),
            cc::NOTE if tap => self.pad_mode = PadMode::Note,
            cc::SESSION if tap => self.pad_mode = PadMode::Session,
            number if tap => {
                if let Some(mode) = DisplayMode::ALL.into_iter().find(|m| m.button() == number) {
                    if mode == DisplayMode::Mix && self.display_mode == DisplayMode::Mix {
                        self.mix_page = match self.mix_page {
                            MixPage::Track => MixPage::Global,
                            MixPage::Global => MixPage::Track,
                        };
                    }
                    self.show_display(mode, cx);
                } else if let Some(column) = controls::lower_column(number)
                    && let Some(&track) = self.ring_tracks(cx.row()).get(usize::from(column))
                {
                    Self::run(cx, "session.select", json!({ "track": track }));
                }
            }
            number => {
                // A lower display button held, not tapped, selects as well.
                if let Some(column) = controls::lower_column(number)
                    && let Some(&track) = self.ring_tracks(cx.row()).get(usize::from(column))
                {
                    Self::run(cx, "session.select", json!({ "track": track }));
                }
            }
        }
    }

    // ----- Stop Clip, in every mode (push-2-tracks R68) -----

    fn stop_clip_on(&self) -> bool {
        self.buttons.is_down(cc::STOP_CLIP) || self.buttons.locked().any(|b| b == cc::STOP_CLIP)
    }

    /// The Stop Clip combinations on a press: Shift stops every clip, and a
    /// lower display button, with Stop Clip held or locked, stops its track.
    /// Whether the press was one.
    fn stop_clip(&mut self, button: u8, cx: &mut Cx) -> bool {
        if button == cc::STOP_CLIP && self.buttons.is_down(cc::SHIFT) {
            Self::run(cx, "session.stop_all", json!({}));
            return true;
        }
        let Some(column) = controls::lower_column(button) else {
            return false;
        };
        if !self.stop_clip_on() {
            return false;
        }
        self.buttons.mark_used(button);
        if let Some(&track) = self.ring_tracks(cx.row()).get(usize::from(column)) {
            Self::run(cx, "session.stop_track", json!({ "track": track }));
        }
        true
    }

    /// Whether `track` has a clip that plays or records now.
    fn plays(model: &ReadModel, track: TrackId) -> bool {
        model.slots.iter().any(|s| {
            s.track == track
                && matches!(
                    s.state,
                    SlotState::Playing | SlotState::QueuedToStop | SlotState::Recording
                )
        })
    }

    fn toggle_menu(&mut self, menu: Menu) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
    }

    /// Lets go of the sustain pedal if it is down: its release can no longer
    /// arrive once the surface stops or Push 2 is in User mode.
    fn release_sustain(&mut self, cx: &mut Cx) {
        if std::mem::take(&mut self.sustain_down)
            && let Some(message) = MidiMessage::from_bytes(&[0xB0, 64, 0])
        {
            cx.play(message);
        }
    }

    fn set_user_mode(&mut self, on: bool, cx: &mut Cx) {
        if on {
            self.release_sustain(cx);
        }
        self.user_mode = on;
        self.buttons.clear();
        self.picker = None;
        self.momentary_pad = None;
        self.momentary_display = None;
        if on {
            // Every LED off but User, which is full (R33): sent first, since
            // Push 2 ignores the Live port's LEDs once it is in User mode,
            // except User's own.
            self.leds.clear();
            self.leds.set(
                Addr::Button(cc::USER),
                Led::steady(crate::leds::white::FULL),
            );
            self.flush_leds(cx);
            Self::send(cx, sysex::midi_mode(MidiMode::User));
        } else {
            Self::send(cx, sysex::midi_mode(MidiMode::Live));
            // Everything again, and a frame, in the modes it had (R33).
            self.leds.forget();
        }
    }

    // ----- pads, strip, encoders (R8, R9, R11) -----

    fn pad_down(&mut self, row: u8, column: u8, velocity: u8, cx: &mut Cx) {
        self.buttons.use_modifiers(0);
        if self.picker.is_some() {
            self.swallowed.insert((row, column));
            self.pick(cx, row, column);
            return;
        }
        self.last_pad = Pad { row, column };
        let pad = self.last_pad;
        self.with_contents(cx, Who::Pads, |content, ctx| {
            content.pad(pad, PadEvent::On(velocity), ctx)
        });
    }

    fn pad_event(&mut self, pad: Pad, event: PadEvent, cx: &mut Cx) {
        let key = (pad.row, pad.column);
        if self.picker.is_some()
            || (matches!(event, PadEvent::Off) && self.swallowed.remove(&key))
            || (matches!(event, PadEvent::Pressure(_)) && self.swallowed.contains(&key))
        {
            return;
        }
        self.with_contents(cx, Who::Pads, |content, ctx| content.pad(pad, event, ctx));
        // A pad pressed in Session Mode and let go after the pad mode changed
        // still gets its release, which a Gate clip needs (push-2-tracks R61).
        if matches!(event, PadEvent::Off) && self.pad_mode != PadMode::Session {
            self.with_contents(cx, Who::SessionPads, |content, ctx| {
                content.pad(pad, event, ctx)
            });
        }
    }

    fn strip_event(&mut self, event: StripEvent, cx: &mut Cx) {
        if matches!(event, StripEvent::Touch) {
            self.buttons.use_modifiers(0);
        }
        self.with_contents(cx, Who::Pads, |content, ctx| content.strip(event, ctx));
    }

    fn touch(&mut self, index: u8, touched: bool, cx: &mut Cx) {
        let i = usize::from(index);
        self.touched[i] = touched;
        let now = cx.now();
        if touched {
            self.buttons.use_modifiers(0);
            self.setup_counts.iter_mut().for_each(Counting::reset);
            match index {
                controls::MASTER => {
                    self.master.touch();
                    self.cue.touch();
                }
                controls::TEMPO => {
                    self.tempo = None;
                    let bpm = self.model.as_ref().map_or(120.0, |m| m.tempo.get());
                    self.notify(now, format!("Tempo {bpm:.1} BPM"));
                }
                controls::SWING => {
                    self.swing.touch();
                    let swing = self.shared.swing();
                    self.notify(now, format!("Swing {:.0} %", swing * 100.0));
                }
                _ => {}
            }
        } else if std::mem::take(&mut self.gestures[i]) {
            // One touch is one undo step (R9).
            cx.end_gesture(ENCODER_GESTURE + u32::from(index));
        }
        if index < 8 && self.menu != Some(Menu::Setup) {
            let turn = if touched { Turn::Touch } else { Turn::Release };
            self.with_contents(cx, Who::Display, |content, ctx| {
                content.encoder(index, turn, ctx)
            });
        }
    }

    fn turned(&mut self, index: u8, value: u8, cx: &mut Cx) {
        let steps = encoder::steps(value);
        if steps == 0 {
            return;
        }
        self.buttons.use_modifiers(0);
        let now = cx.now();
        let shift = self.buttons.modifiers().shift();
        let control = ENCODER_GESTURE + u32::from(index);
        match index {
            controls::MASTER => {
                let Some(model) = &self.model else {
                    return;
                };
                self.gestures[usize::from(index)] = true;
                if shift {
                    let now_value =
                        bl_model::fader::fader_position(model.document.settings().cue_volume);
                    let position = self.cue.turn(now, now_value, steps, true);
                    let volume = bl_model::fader::fader_gain(position);
                    cx.run_held(control, "mixer.set_cue_volume", json!({ "volume": volume }));
                    // The browser's preview keeps a volume of its own, which
                    // follows the knob (push-2-tracks R10).
                    cx.run_held(control, "browser.set_preview", json!({ "volume": volume }));
                } else {
                    let main = model.document.main();
                    let now_value = bl_model::fader::fader_position(main.mixer().volume);
                    let id = model.document.main_id();
                    let position = self.master.turn(now, now_value, steps, false);
                    cx.run_held(
                        control,
                        "mixer.set_volume",
                        json!({ "track": id, "volume": bl_model::fader::fader_gain(position) }),
                    );
                }
            }
            controls::TEMPO => {
                let Some(model) = &self.model else {
                    return;
                };
                let map = model.document.tempo_map();
                if map.segments().len() != 1 {
                    return;
                }
                let base = match self.tempo {
                    Some((bpm, at))
                        if now.saturating_duration_since(at) < Duration::from_millis(500) =>
                    {
                        bpm
                    }
                    _ => model.tempo.get(),
                };
                let by = f64::from(steps) * if shift { 0.1 } else { 1.0 };
                let (low, high) = bl_model::remote::TEMPO_RANGE;
                let bpm = ((base + by) * 10.0).round() / 10.0;
                let bpm = bpm.clamp(low, high);
                self.tempo = Some((bpm, now));
                if let Ok(bpm) = bl_core::time::Bpm::new(bpm) {
                    self.gestures[usize::from(index)] = true;
                    let tempo = serde_json::to_value(bl_core::time::TempoMap::constant(bpm))
                        .unwrap_or(Value::Null);
                    cx.run_held(control, "song.set_tempo", json!({ "tempo": tempo }));
                }
                self.notify(now, format!("Tempo {bpm:.1} BPM"));
            }
            controls::SWING => {
                let swing = self.swing.turn(now, self.shared.swing(), steps, shift);
                self.shared.set_swing(swing);
                self.notify(now, format!("Swing {:.0} %", swing * 100.0));
            }
            column if self.menu == Some(Menu::Setup) => self.setup_turn(column, steps, shift),
            column => {
                self.with_contents(cx, Who::Display, |content, ctx| {
                    content.encoder(column, Turn::Steps(steps), ctx)
                });
            }
        }
    }

    /// The Setup menu's encoders (R31): 1 to 3 the pad values, 6 and 7 the
    /// brightnesses, 8 Workflow.
    fn setup_turn(&mut self, column: u8, steps: i32, shift: bool) {
        let mut setup = self.shared.setup();
        let counts = &mut self.setup_counts[usize::from(column)];
        let per_percent = if shift { 20 } else { 2 };
        let by = |value: u8, counts: &mut Counting, per: i32, high: i32| {
            (i32::from(value) + counts.take(steps, per)).clamp(0, high) as u8
        };
        match column {
            0 => setup.sensitivity = by(setup.sensitivity, counts, LIST_STEPS, 10),
            1 => setup.gain = by(setup.gain, counts, LIST_STEPS, 10),
            2 => setup.dynamics = by(setup.dynamics, counts, LIST_STEPS, 10),
            5 => setup.display_light = by(setup.display_light, counts, per_percent, 100),
            6 => setup.led_brightness = by(setup.led_brightness, counts, per_percent, 100),
            7 => {
                if counts.take(steps, LIST_STEPS) != 0 {
                    setup.workflow = match setup.workflow {
                        Workflow::Scene => Workflow::Clip,
                        Workflow::Clip => Workflow::Scene,
                    };
                }
            }
            _ => return,
        }
        if setup != self.shared.setup() {
            self.shared.set_setup(setup);
        }
    }

    // ----- footswitches (R28, R29) -----

    fn footswitch(&mut self, number: u8, value: u8, at: Instant, cx: &mut Cx) {
        let down = value >= 64;
        if number == cc::FOOTSWITCH_1 {
            self.sustain_down = down;
            if let Some(message) = MidiMessage::from_bytes(&[0xB0, 64, if down { 127 } else { 0 }])
            {
                cx.play(message);
            }
        } else if down {
            match self.footswitch.take() {
                Some(first) if at.saturating_duration_since(first) <= DOUBLE_TAP => {
                    self.synthetic(cc::NEW, cx);
                }
                _ => self.footswitch = Some(at),
            }
        }
    }

    /// A press and release of `button` that wasn't on Push 2 (R29).
    fn synthetic(&mut self, button: u8, cx: &mut Cx) {
        for gesture in [Gesture::Press, Gesture::Release, Gesture::Tap] {
            self.with_contents(cx, Who::Both, |content, ctx| {
                content.button(button, gesture, ctx)
            });
        }
    }

    // ----- the messages (R6) -----

    fn input(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx) {
        let (Some(&status), Some(&a)) = (bytes.first(), bytes.get(1)) else {
            return;
        };
        let b = bytes.get(2).copied().unwrap_or(0);
        match status & 0xF0 {
            0x90 | 0x80 => {
                let on = status & 0xF0 == 0x90 && b > 0;
                if let Some((row, column)) = controls::pad_of(a) {
                    if self.user_mode {
                        return;
                    }
                    if on {
                        self.pad_down(row, column, b, cx);
                    } else {
                        self.pad_event(Pad { row, column }, PadEvent::Off, cx);
                    }
                } else if let Some(encoder) = controls::encoder_touched(a) {
                    if !self.user_mode {
                        self.touch(encoder.index, on, cx);
                    }
                } else if a == STRIP_TOUCH && !self.user_mode {
                    self.strip_event(
                        if on {
                            StripEvent::Touch
                        } else {
                            StripEvent::Release
                        },
                        cx,
                    );
                }
            }
            0xB0 => {
                if a == cc::FOOTSWITCH_1 || a == cc::FOOTSWITCH_2 {
                    if !self.user_mode {
                        self.footswitch(a, b, at, cx);
                    }
                } else if let Some(encoder) = controls::encoder_turned(a) {
                    if !self.user_mode {
                        self.turned(encoder.index, b, cx);
                    }
                } else if a == 1 {
                    if !self.user_mode {
                        self.strip_event(StripEvent::Position(f32::from(b) / 127.0), cx);
                    }
                } else if controls::button(a).is_some() {
                    if b >= 64 {
                        self.button_down(a, at, cx);
                    } else {
                        for event in self.buttons.release(a, at) {
                            self.button_event(event, cx);
                        }
                    }
                }
            }
            0xE0 if !self.user_mode => {
                let value = u16::from(b) << 7 | u16::from(a);
                self.strip_event(StripEvent::Position(f32::from(value >> 6) / 255.0), cx);
            }
            0xD0 if !self.user_mode => {
                let pad = self.last_pad;
                self.pad_event(pad, PadEvent::ChannelPressure(a), cx);
            }
            0xA0 if !self.user_mode => {
                if let Some((row, column)) = controls::pad_of(a) {
                    self.pad_event(Pad { row, column }, PadEvent::Pressure(b), cx);
                }
            }
            _ => {}
        }
    }

    // ----- each tick -----

    fn commands(&mut self, cx: &mut Cx) {
        for command in self.handle.take() {
            let Command::SetMode { pad, display } = command;
            if let Some(pad) = pad {
                self.pad_mode = pad;
            }
            if let Some((mode, page)) = display {
                self.show_display(mode, cx);
                self.mix_page = page;
            }
            self.menu = None;
        }
    }

    /// Makes `mode` the display mode, which its content enters afresh when
    /// it is Browse (push-2-tracks R5).
    fn show_display(&mut self, mode: DisplayMode, cx: &mut Cx) {
        self.display_mode = mode;
        if mode == DisplayMode::Browse {
            self.enter(Enter::Button, cx);
        }
    }

    fn enter(&mut self, why: Enter, cx: &mut Cx) {
        self.with_contents(cx, Who::Display, |content, ctx| content.enter(why, ctx));
    }

    fn setup_changes(&mut self, cx: &mut Cx) {
        if self.strip != self.strip_sent {
            self.strip_sent = self.strip;
            Self::send(cx, sysex::touch_strip_config(self.strip.flags()));
        }
        if self.aftertouch != self.aftertouch_sent {
            self.aftertouch_sent = self.aftertouch;
            Self::send(cx, sysex::aftertouch_mode(self.aftertouch));
        }
        let now = self.shared.setup();
        if now == self.applied {
            return;
        }
        let was = self.applied;
        self.applied = now;
        if (was.sensitivity, was.gain, was.dynamics) != (now.sensitivity, now.gain, now.dynamics) {
            for bytes in crate::curve::messages(&now.curve()) {
                Self::send(cx, bytes);
            }
        }
        if was.led_brightness != now.led_brightness {
            Self::send(cx, sysex::led_brightness(now.led_value()));
        }
        if was.display_light != now.display_light {
            Self::send(cx, sysex::display_brightness(now.display_value()));
        }
    }

    /// MIDI clock to the Live port, 24 to the quarter note at the song's
    /// tempo, each stamped with its moment, and Start with the transport
    /// (R15).
    fn clock(&mut self, model: &ReadModel, cx: &mut Cx) {
        let now = cx.now();
        if model.transport.playing && !self.playing {
            cx.send_at(now, vec![0xFA]);
        }
        self.playing = model.transport.playing;
        let interval = Duration::from_secs_f64(60.0 / (model.tempo.get() * 24.0));
        // A host that was stalled for a long time starts the clock afresh,
        // rather than sending every tick it missed.
        let mut next = self
            .clock
            .filter(|&at| at + Duration::from_secs(1) > now)
            .unwrap_or(now);
        while next < now + CLOCK_AHEAD {
            cx.send_at(next.max(now), vec![0xF8]);
            next += interval;
        }
        self.clock = Some(next);
    }

    fn paint(&mut self, model: &ReadModel, cx: &mut Cx) {
        let now = cx.now();
        let modifiers = self.buttons.modifiers();
        self.screen = Screen::default();
        self.leds.clear();
        let colours = self.shared.track_colours();
        let swatch = |index: u8| bl_model::swatch_rgb(&colours, index);

        // The ring's tracks label the lower buttons (R26).
        let claimed = self.lower_claimed();
        let tracks = self.ring_tracks(cx.row());
        if !claimed {
            for (column, &track) in tracks.iter().take(8).enumerate() {
                if let Some(t) = model.document.track(track) {
                    self.screen.lower[column] = Label::new(
                        t.name(),
                        swatch(t.colour()),
                        model.selection.tracks.contains(&track),
                    );
                    if let Some((_, fraction)) = model.progress.iter().find(|(id, _)| *id == track)
                    {
                        self.screen.progress[column] = Some(*fraction);
                    }
                }
            }
        }

        // The modes draw the pads, the buttons they own and the display.
        {
            let (pad, display) = match (self.pad_mode, self.display_mode) {
                (PadMode::Note, mode) => (0, mode.index()),
                (PadMode::Session, mode) => (1, mode.index()),
            };
            for content in [
                self.contents.pads[pad].as_mut(),
                self.contents.display[display].as_mut(),
            ] {
                content.paint(&mut Paint {
                    screen: &mut self.screen,
                    lights: Lights {
                        leds: &mut self.leds,
                        palette: &mut self.palette,
                    },
                    model,
                    modifiers: &modifiers,
                    colours: &colours,
                    row: cx.row(),
                });
            }
        }

        // A display button's LED follows its label: off when empty, its
        // colour dim, full while selected (R18), unless the mode lit it.
        let stopping = self.stop_clip_on();
        let mut lights = Lights {
            leds: &mut self.leds,
            palette: &mut self.palette,
        };
        // With Stop Clip held or locked, the lower buttons of the tracks that
        // play a clip pulse (push-2-tracks R68).
        if stopping {
            for (column, &track) in tracks.iter().take(8).enumerate() {
                if let Some(t) = model.document.track(track)
                    && Self::plays(model, track)
                {
                    let rgb = swatch(t.colour());
                    let (full, dimmed) = (lights.colour(rgb), lights.colour(dim(rgb)));
                    lights.button(controls::lower(column as u8), Led::pulsing(full, dimmed));
                }
            }
        }
        for column in 0..8u8 {
            for (labels, button) in [
                (&self.screen.upper, controls::upper(column)),
                (&self.screen.lower, controls::lower(column)),
            ] {
                let label = &labels[usize::from(column)];
                if label.is_empty() || lights.leds.get(Addr::Button(button)) != Led::default() {
                    continue;
                }
                let rgb = if label.selected {
                    label.colour
                } else {
                    dim(label.colour)
                };
                let index = lights.colour(rgb);
                lights.button(button, Led::steady(index));
            }
        }

        // The buttons the surface itself owns (R12, R13, R21, R22).
        let playing = model.transport.playing;
        lights.button(
            cc::PLAY,
            Led::steady(if playing {
                fixed::GREEN
            } else {
                fixed::DARK_GREY
            }),
        );
        lights.button(cc::TAP_TEMPO, Led::level(true, false));
        lights.button(
            cc::METRONOME,
            Led::switch(model.document.settings().metronome.enabled),
        );
        lights.button(cc::UNDO, Led::level(true, false));
        lights.button(cc::DOUBLE_LOOP, Led::level(true, false));
        lights.button(cc::SETUP, Led::level(true, self.menu == Some(Menu::Setup)));
        let scale = self.contents.pads[match self.pad_mode {
            PadMode::Note => 0,
            PadMode::Session => 1,
        }]
        .scale_available();
        lights.button(cc::SCALE, Led::level(scale, self.menu == Some(Menu::Scale)));
        lights.button(cc::USER, Led::level(true, false));
        lights.button(cc::SHIFT, Led::level(true, false));
        for mode in [PadMode::Note, PadMode::Session] {
            lights.button(mode.button(), Led::level(true, mode == self.pad_mode));
        }
        for mode in DisplayMode::ALL {
            lights.button(mode.button(), Led::level(true, mode == self.display_mode));
        }
        // Held and locked buttons are full while they are (R13).
        for button in self.buttons.held().chain(self.buttons.locked()) {
            if controls::button(button).is_some_and(|b| b.led == LedKind::White) {
                lights.button(button, Led::steady(crate::leds::white::FULL));
            }
        }

        // Stop Clip: dim red, full while held and pulsing while locked
        // (push-2-tracks R68).
        {
            let mut lights = Lights {
                leds: &mut self.leds,
                palette: &mut self.palette,
            };
            let red = [255, 0, 0];
            let (full, dimmed) = (lights.colour(red), lights.colour(dim(red)));
            let led = if self.buttons.locked().any(|b| b == cc::STOP_CLIP) {
                Led::pulsing(full, dimmed)
            } else if self.buttons.is_down(cc::STOP_CLIP) {
                Led::steady(full)
            } else {
                Led::steady(dimmed)
            };
            lights.button(cc::STOP_CLIP, led);
        }

        // The menus (R23, R31).
        match self.menu {
            Some(Menu::Setup) => self.paint_setup(),
            Some(Menu::Scale) => self.screen.title = Some("Scale".to_owned()),
            None => {}
        }

        // The count-in bar, flashing on each beat (R19).
        if model.transport.counting_in {
            let started = *self.count_in.get_or_insert(now);
            let beat = Duration::from_secs_f64(60.0 / model.tempo.get());
            let into = now.saturating_duration_since(started).as_secs_f64() % beat.as_secs_f64();
            self.screen.count_in = Some(into < beat.as_secs_f64() / 2.0);
        } else {
            self.count_in = None;
        }

        // Which encoders are touched (R20).
        for (column, touched) in self.touched.iter().take(8).enumerate() {
            if *touched {
                self.screen.columns[column].touched = true;
            }
        }

        if let Some((text, until)) = &self.notification {
            if now < *until {
                self.screen.notification = Some(text.clone());
            } else {
                self.notification = None;
            }
        }

        if self.picker.is_some() {
            self.paint_picker();
        }

        if self.user_mode {
            self.screen = Screen {
                title: Some("Push 2 is in User Mode".to_owned()),
                ..Screen::default()
            };
            self.leds.clear();
            self.leds.set(
                Addr::Button(cc::USER),
                Led::steady(crate::leds::white::FULL),
            );
        }
    }

    fn paint_setup(&mut self) {
        let setup = self.shared.setup();
        let list = |name: &str, value: u8| Column {
            name: name.to_owned(),
            value: value.to_string(),
            graphic: Graphic::Arc(f32::from(value) / 10.0),
            ..Column::default()
        };
        let percent = |name: &str, value: u8| Column {
            name: name.to_owned(),
            value: format!("{value} %"),
            graphic: Graphic::Arc(f32::from(value) / 100.0),
            ..Column::default()
        };
        self.screen.columns = Default::default();
        self.screen.title = None;
        self.screen.columns[0] = list("Pad Sensitivity", setup.sensitivity);
        self.screen.columns[1] = list("Pad Gain", setup.gain);
        self.screen.columns[2] = list("Pad Dynamics", setup.dynamics);
        self.screen.columns[5] = percent("Display Light", setup.display_light);
        self.screen.columns[6] = percent("LED Brightness", setup.led_brightness);
        let workflow = match setup.workflow {
            Workflow::Scene => "Scene",
            Workflow::Clip => "Clip",
        };
        self.screen.columns[7] = Column {
            name: "Workflow".to_owned(),
            value: workflow.to_owned(),
            graphic: Graphic::Choice(workflow.to_owned()),
            ..Column::default()
        };
        self.screen.curve = Some(setup.curve().to_vec());
    }

    fn paint_picker(&mut self) {
        let Some(picker) = &self.picker else {
            return;
        };
        let colours = self.shared.track_colours();
        let current = picker.current;
        let mut lights = Lights {
            leds: &mut self.leds,
            palette: &mut self.palette,
        };
        for row in 0..8 {
            for column in 0..8 {
                lights.pad(row, column, Led::default());
            }
        }
        for n in 0..28u8 {
            let (row, column) = Self::ring_pad(n);
            let index = lights.colour(bl_model::swatch_rgb(&colours, n));
            let led = if current == Some(n) {
                Led::blinking(index, fixed::BLACK)
            } else {
                Led::steady(index)
            };
            lights.pad(row, column, led);
        }
    }

    fn publish(&mut self) {
        let mut snapshot = Snapshot::new();
        snapshot.firmware = self.firmware.clone();
        snapshot.external_power = self.external_power;
        snapshot.error = self.error.clone();
        snapshot.midi_mode = if self.user_mode {
            MidiMode::User
        } else {
            MidiMode::Live
        };
        snapshot.pad_mode = self.pad_mode;
        snapshot.display_mode = self.display_mode;
        snapshot.mix_page = self.mix_page;
        snapshot.menu = self.menu;
        snapshot.held = self
            .buttons
            .held()
            .filter_map(|n| controls::button(n).map(|b| b.name))
            .collect();
        snapshot.locked = self
            .buttons
            .locked()
            .filter_map(|n| controls::button(n).map(|b| b.name))
            .collect();
        snapshot.picker = self.picker.is_some();
        for addr in all_leds() {
            let slot = match addr {
                Addr::Pad(note) => usize::from(note),
                Addr::Button(number) => 128 + usize::from(number),
            };
            snapshot.leds[slot] = self.leds.get(addr);
        }
        snapshot.strip = self.strip;
        snapshot.strip_leds = *self.leds.strip();
        snapshot.setup = self.shared.setup();
        snapshot.screen = self.screen.clone();
        snapshot.browse = self.display_content().browse_state();
        if self.pad_mode == PadMode::Session {
            snapshot.session = self.pad_content().session_state();
        }
        snapshot.display = self
            .display
            .as_ref()
            .map(Display::status)
            .unwrap_or_default();
        self.handle.publish(snapshot);
    }
}

fn all_leds() -> impl Iterator<Item = Addr> {
    (36..100u8)
        .map(Addr::Pad)
        .chain(BUTTONS.iter().map(|b| Addr::Button(b.number)))
}

/// A colour dimmed for a button that isn't selected.
pub(crate) fn dim([r, g, b]: [u8; 3]) -> [u8; 3] {
    let scale = |c: u8| (u16::from(c) * 35 / 100) as u8;
    [scale(r), scale(g), scale(b)]
}

impl ControlSurface for Push {
    fn start(&mut self, cx: &mut Cx) {
        self.display = Some(Display::start(Arc::clone(&self.claim)));
        let queue = self.sequence();
        self.phase = Phase::Starting {
            queue,
            waiting: None,
        };
        self.advance(cx);
        self.publish();
    }

    fn stop(&mut self, cx: &mut Cx) {
        // Every LED off, the display dark, and Push 2 left in Live mode
        // (R4).
        self.release_sustain(cx);
        self.leds.clear();
        self.leds.forget();
        for bytes in self.leds.flush(false) {
            cx.send(bytes);
        }
        Self::send(cx, sysex::display_brightness(0));
        Self::send(cx, sysex::midi_mode(MidiMode::Live));
        self.display = None;
    }

    fn message(&mut self, at: Instant, bytes: &[u8], cx: &mut Cx) {
        if sysex::is_sysex(bytes) {
            self.reply(bytes, cx);
        } else if matches!(self.phase, Phase::Live) {
            self.input(at, bytes, cx);
        }
    }

    fn tick(&mut self, model: &ReadModel, cx: &mut Cx) {
        self.model = Some(model.clone());
        self.wait_timeouts(cx);
        if matches!(self.phase, Phase::Live) {
            self.commands(cx);
            if !self.user_mode {
                for event in self.buttons.tick(cx.now()) {
                    self.button_event(event, cx);
                }
                if let Some(first) = self.footswitch
                    && cx.now().saturating_duration_since(first) >= DOUBLE_TAP
                {
                    self.footswitch = None;
                    self.synthetic(cc::RECORD, cx);
                }
                self.setup_changes(cx);
            }
            self.clock(model, cx);
            self.paint(model, cx);
            self.flush_leds(cx);
            if let Some(display) = &self.display {
                display.show(&self.screen);
            }
        }
        self.publish();
    }

    fn state(&self) -> SurfaceState {
        SurfaceState {
            ring: Some(bl_control::Ring {
                tracks: 8,
                scenes: 8,
            }),
            mode: Some(match (self.user_mode, self.display_mode, self.mix_page) {
                (true, ..) => "User Mode".to_owned(),
                (_, DisplayMode::Mix, MixPage::Global) => {
                    format!("{}, Global Mix", self.pad_mode.name())
                }
                (_, DisplayMode::Mix, MixPage::Track) => {
                    format!("{}, Track Mix", self.pad_mode.name())
                }
                (_, mode, _) => format!("{}, {}", self.pad_mode.name(), mode.name()),
            }),
            watch: Vec::new(),
        }
    }

    fn probe(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(Arc::clone(&self.handle) as Arc<dyn Any + Send + Sync>)
    }
}
