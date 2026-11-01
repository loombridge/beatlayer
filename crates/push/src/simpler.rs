//! Simpler in Device Mode: its main bank over the waveform in each mode, the
//! buttons of Edit Mode, and the slices (`docs/specs/push-2-notes.md`, R67 to
//! R74).
//!
//! The main bank is the first bank of Simpler's banks, drawn here: Start and
//! End are the region's flags, not parameters, and Zoom is the display's
//! alone. Everything else is Simpler's own parameters, which the encoders set
//! as any device's.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bl_control::{ParamView, ReadModel, SampleView};
use bl_devices::sampler as s;
use bl_model::{Device, DeviceDescription, DeviceId, DeviceKind, ParamId};
use serde_json::json;

use crate::content::{Ctx, Paint};
use crate::frame::WIDTH;
use crate::screen::{Column, Graphic, Label, Shade, Waveform};

/// The key of slice 1: C3, as Simpler plays it (sampler R13).
pub const FIRST_KEY: u8 = 60;
/// The gesture key of the region's and the slices' encoders: with the
/// encoder's number added, one undo step a touch (R1).
pub const GESTURE: u32 = 0x5200;
/// The group key of the commands one button runs as one undo step.
const GROUP: u32 = 0x5400;
/// How many frames the narrowest view spans.
const NARROWEST: f64 = 64.0;
/// How many octaves of zoom a full turn of Zoom makes.
const OCTAVES: f64 = 12.0;
/// A turn that rests this long starts again from the document.
const REST: Duration = Duration::from_millis(500);

/// Simpler's mode (sampler R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Classic,
    OneShot,
    Slice,
}

impl Mode {
    fn of(value: f64) -> Self {
        match value as u8 {
            0 => Self::Classic,
            1 => Self::OneShot,
            _ => Self::Slice,
        }
    }

    /// The mode's name as `push.state` reads it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::OneShot => "one_shot",
            Self::Slice => "slice",
        }
    }
}

/// What an encoder of the main bank does (R68).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Zoom,
    Start,
    End,
    Nudge,
    Param(ParamId),
    Empty,
}

/// The upper buttons of Edit Mode (R70).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Loop,
    Trigger,
    PadSlicing,
    SplitSlice,
    WarpAs,
    Half,
    Double,
    Crop,
    Reverse,
}

impl Edit {
    const fn name(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::Trigger => "trigger_gate",
            Self::PadSlicing => "pad_slicing",
            Self::SplitSlice => "split_slice",
            Self::WarpAs => "warp_as",
            Self::Half => "half",
            Self::Double => "double",
            Self::Crop => "crop",
            Self::Reverse => "reverse",
        }
    }
}

const fn edits(mode: Mode) -> &'static [Edit] {
    match mode {
        Mode::Classic => &[
            Edit::Loop,
            Edit::WarpAs,
            Edit::Half,
            Edit::Double,
            Edit::Crop,
            Edit::Reverse,
        ],
        Mode::OneShot => &[
            Edit::Trigger,
            Edit::WarpAs,
            Edit::Half,
            Edit::Double,
            Edit::Crop,
            Edit::Reverse,
        ],
        Mode::Slice => &[
            Edit::Trigger,
            Edit::PadSlicing,
            Edit::SplitSlice,
            Edit::WarpAs,
            Edit::Half,
            Edit::Double,
            Edit::Reverse,
        ],
    }
}

const fn controls(mode: Mode, by: Option<ParamId>) -> [Control; 8] {
    use Control::{End, Nudge, Param, Start, Zoom};
    match mode {
        Mode::Classic => [
            Zoom,
            Param(s::MODE),
            Start,
            End,
            Param(s::START),
            Param(s::LENGTH),
            Param(s::LOOP_LENGTH),
            Param(s::GAIN),
        ],
        Mode::OneShot => [
            Zoom,
            Param(s::MODE),
            Start,
            End,
            Param(s::FADE_IN),
            Param(s::FADE_OUT),
            Param(s::TRANSPOSE),
            Param(s::GAIN),
        ],
        Mode::Slice => [
            Zoom,
            Param(s::MODE),
            Start,
            End,
            Param(s::SLICE_BY),
            match by {
                Some(param) => Param(param),
                None => Control::Empty,
            },
            Param(s::PLAYBACK),
            Nudge,
        ],
    }
}

/// What the main bank's encoder `column` sets, if it is a parameter: `None`
/// for Zoom, Start, End and Nudge, and `Some(None)` for an empty one.
#[must_use]
pub fn control_param(view: &View<'_>, column: u8) -> Option<Option<ParamId>> {
    match Simpler::controls(view)[usize::from(column)] {
        Control::Param(id) => Some(Some(id)),
        Control::Empty => Some(None),
        _ => None,
    }
}

/// Simpler's description, made once: its parameters' descriptors.
fn description() -> &'static DeviceDescription {
    static DESCRIPTION: OnceLock<DeviceDescription> = OnceLock::new();
    DESCRIPTION.get_or_init(|| s::sampler_type().description)
}

/// Whether `device` is a Simpler.
#[must_use]
pub fn is_simpler(device: &Device) -> bool {
    matches!(
        &device.kind,
        DeviceKind::Native { device_type } if device_type == s::SAMPLER
    )
}

/// A slice as it sounds: where it plays from and to, and how fast (R74).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Played {
    /// The first frame played.
    pub start: u64,
    /// The frame it stops at.
    pub end: u64,
    /// How many frames of the sample a second of playing goes through.
    pub per_second: f64,
}

/// A Simpler as the read model has it.
pub struct View<'a> {
    device: &'a Device,
    sample: Option<&'a SampleView>,
    mode: Mode,
    /// The Start and End flags, in frames.
    region: (u64, u64),
    frames: u64,
    rate: f64,
}

impl<'a> View<'a> {
    /// `device`, if it is a Simpler.
    #[must_use]
    pub fn of(model: &'a ReadModel, device: DeviceId) -> Option<Self> {
        let device = model.document.device(device)?;
        if !is_simpler(device) {
            return None;
        }
        let sample = model.samples.get(&device.id);
        let frames = sample.map_or(0, |sample| sample.frames);
        let mut view = Self {
            device,
            sample,
            mode: Mode::Classic,
            region: (0, frames),
            frames,
            rate: sample.map_or(1.0, |sample| f64::from(sample.rate.max(1))),
        };
        view.mode = Mode::of(view.value(s::MODE));
        view.region = device
            .region
            .map_or((0, frames), |region| region.frames_in(frames));
        Some(view)
    }

    /// Whether a sample is loaded and can be read.
    #[must_use]
    pub fn has_sample(&self) -> bool {
        self.sample.is_some() && self.frames > 0
    }

    /// The device's mode.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    fn value(&self, id: ParamId) -> f64 {
        let description = description();
        description
            .value(&self.device.params, id)
            .or_else(|| description.param(id).map(|p| p.default))
            .unwrap_or(0.0)
    }

    /// A parameter as an encoder turns it.
    #[must_use]
    pub fn param(&self, id: ParamId) -> Option<ParamView> {
        let descriptor = description().param(id)?;
        let value = self.value(id);
        Some(ParamView {
            param: id,
            name: descriptor.name.clone(),
            value,
            text: descriptor.format(value),
            held: false,
            descriptor: descriptor.clone(),
        })
    }

    /// The edges of the slices: the region's start, where each slice after
    /// the first starts, and the region's end.
    fn bounds(&self) -> Vec<u64> {
        let mut bounds = vec![self.region.0];
        if let Some(sample) = self.sample {
            bounds.extend(sample.slices.iter().copied());
        }
        bounds.push(self.region.1);
        bounds
    }

    /// How many slices there are in Slice mode (R75): up to 64.
    #[must_use]
    pub fn slice_count(&self) -> usize {
        if self.mode == Mode::Slice && self.has_sample() {
            (self.bounds().len() - 1).min(s::MAX_SLICES)
        } else {
            0
        }
    }

    /// The start and end of slice `index`.
    fn slice(&self, index: usize) -> Option<(u64, u64)> {
        let bounds = self.bounds();
        Some((*bounds.get(index)?, *bounds.get(index + 1)?))
    }

    /// Where slice `index` plays from and to, and how many frames of the
    /// sample a second of it plays: Thru plays on to the region's end
    /// (sampler R23).
    #[must_use]
    pub fn played(&self, index: usize) -> Option<Played> {
        let (start, end) = self.slice(index).filter(|_| index < self.slice_count())?;
        let thru = self.value(s::PLAYBACK) as usize == 2;
        Some(Played {
            start,
            end: if thru { self.region.1 } else { end },
            per_second: self.rate * 2f64.powf(self.value(s::TRANSPOSE) / 12.0),
        })
    }

    /// Where slice `index` starts, if it is a slice placed by hand: the
    /// first starts at the Start flag, and automatic slices aren't placed by
    /// hand (R73).
    #[must_use]
    pub fn manual_start(&self, index: usize) -> Option<u64> {
        let sample = self.sample?;
        let frame = *sample.slices.get(index.checked_sub(1)?)?;
        self.device.slices.contains(&frame).then_some(frame)
    }

    /// Whether Slice By is Manual (R74).
    #[must_use]
    pub fn manual(&self) -> bool {
        self.value(s::SLICE_BY) as usize == 3
    }

    /// The region's length in seconds.
    fn seconds(&self) -> f64 {
        (self.region.1.saturating_sub(self.region.0)) as f64 / self.rate
    }
}

/// The nearest power of two bars the region comes to at `bpm` (R70).
fn guess_bars(seconds: f64, bpm: f64) -> f64 {
    let bars = (seconds * bpm / 60.0 / 4.0).max(f64::MIN_POSITIVE);
    2f64.powf(bars.log2().round().clamp(-4.0, 6.0))
}

/// How a number of bars reads on the button.
fn bars_text(bars: f64) -> String {
    if bars < 1.0 {
        format!("1/{} bar", (1.0 / bars).round() as u32)
    } else if bars < 1.5 {
        "1 bar".to_owned()
    } else {
        format!("{} bars", bars as u32)
    }
}

/// A flag or a slice's start an encoder is turning: steps that arrive before
/// the document shows the last change add up from where it left it.
#[derive(Default)]
struct Drag {
    value: Option<f64>,
    last: Option<Instant>,
}

impl Drag {
    fn turn(&mut self, now: Instant, current: u64, by: f64) -> f64 {
        let rested = self
            .last
            .is_none_or(|at| now.saturating_duration_since(at) >= REST);
        let base = match self.value {
            Some(value) if !rested => value,
            _ => current as f64,
        };
        let value = base + by;
        self.value = Some(value);
        self.last = Some(now);
        value
    }
}

/// What `push.state` reads of Simpler's main bank and Edit Mode (R67 to R76).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Whether the Simpler has no sample.
    pub empty: bool,
    /// Its mode: `classic`, `one_shot` or `slice`.
    pub mode: &'static str,
    /// Whether the main bank is on the encoders.
    pub main: bool,
    /// How many times the waveform is zoomed in.
    pub zoom: f64,
    /// The control Zoom zooms around: `start`, `end`, `s_start`, `s_length` or
    /// `slice`.
    pub around: &'static str,
    /// The Start and End flags, in frames.
    pub region: (u64, u64),
    /// How many slices Slice mode has.
    pub slices: usize,
    /// The selected slice, from 0.
    pub selected: usize,
    /// Whether Pad Slicing is on.
    pub pad_slicing: bool,
    /// The upper buttons of Edit Mode, with the button each is on from 1.
    pub buttons: Vec<EditState>,
}

/// One of Edit Mode's buttons, as `push.state` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct EditState {
    /// The button, from 1.
    pub button: usize,
    /// What it does: `loop`, `trigger_gate`, `pad_slicing`, `split_slice`,
    /// `warp_as`, `half`, `double`, `crop` or `reverse`.
    pub name: &'static str,
    /// Its text.
    pub text: String,
    /// Whether the setting it stands for is on.
    pub on: bool,
}

/// Simpler's surface state: what the display shows that isn't the set's.
pub struct Simpler {
    /// How far Zoom is turned, from 0 to 1.
    zoom: f64,
    /// The position control touched last, which Zoom zooms around.
    around: Option<Control>,
    pad_slicing: bool,
    drag: Drag,
}

impl Default for Simpler {
    fn default() -> Self {
        Self::new()
    }
}

impl Simpler {
    /// Simpler's state before anything is touched.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            zoom: 0.0,
            around: None,
            pad_slicing: false,
            drag: Drag {
                value: None,
                last: None,
            },
        }
    }

    /// Whether Pad Slicing is on (R74).
    #[must_use]
    pub const fn pad_slicing(&self) -> bool {
        self.pad_slicing
    }

    fn by(view: &View<'_>) -> Option<ParamId> {
        match view.value(s::SLICE_BY) as usize {
            0 => Some(s::SLICE_SENSITIVITY),
            1 => Some(s::SLICE_DIVISION),
            2 => Some(s::SLICE_REGIONS),
            _ => None,
        }
    }

    fn controls(view: &View<'_>) -> [Control; 8] {
        controls(view.mode, Self::by(view))
    }

    /// The selected slice, from 0 (R73).
    fn selected(view: &View<'_>, key: Option<u8>) -> usize {
        let count = view.slice_count().max(1);
        usize::from(key.unwrap_or(FIRST_KEY).saturating_sub(FIRST_KEY)).min(count - 1)
    }

    /// The frame Zoom zooms around: the position control touched last (R69).
    fn anchor(&self, view: &View<'_>, selected: usize) -> (f64, &'static str) {
        let (start, end) = view.region;
        let span = (end - start) as f64;
        let played = (view.value(s::START) / 100.0, view.value(s::LENGTH) / 100.0);
        let slice = || {
            (
                view.slice(selected).map_or(start, |slice| slice.0) as f64,
                "slice",
            )
        };
        match self.around {
            Some(Control::End) => (end as f64, "end"),
            Some(Control::Param(id)) if id == s::START => {
                (start as f64 + played.0 * span, "s_start")
            }
            Some(Control::Param(id)) if id == s::LENGTH => (
                start as f64 + (played.0 + played.1).min(1.0) * span,
                "s_length",
            ),
            Some(Control::Start) => (start as f64, "start"),
            // With slices Zoom goes to the selected one, unless a flag was
            // touched last (R69).
            _ if view.mode == Mode::Slice => slice(),
            _ => (start as f64, "start"),
        }
    }

    /// The part of the sample the display shows: its first frame and how many
    /// frames it spans (R69).
    fn window(&self, view: &View<'_>, selected: usize) -> (f64, f64) {
        let frames = view.frames as f64;
        let narrowest = NARROWEST.min(frames);
        let span = (frames / 2f64.powf(self.zoom * OCTAVES)).clamp(narrowest, frames);
        let (anchor, _) = self.anchor(view, selected);
        let first = (anchor - span / 2.0).clamp(0.0, (frames - span).max(0.0));
        (first, span)
    }

    /// An encoder turned by `steps`: the controls that aren't parameters
    /// (R68, R69, R73). Parameters are the device mode's.
    pub fn turn(
        &mut self,
        column: u8,
        steps: i32,
        view: &View<'_>,
        key: Option<u8>,
        ctx: &mut Ctx<'_>,
    ) {
        let control = Self::controls(view)[usize::from(column)];
        let selected = Self::selected(view, key);
        let (_, span) = self.window(view, selected);
        // A flag or a slice moves 1/200 of what the display shows a step,
        // a tenth of that with Shift (R9).
        let by = span * crate::encoder::fraction(steps, ctx.modifiers.shift());
        let now = ctx.cx.now();
        let device = view.device.id;
        match control {
            Control::Zoom => {
                self.zoom = (self.zoom + crate::encoder::fraction(steps, ctx.modifiers.shift()))
                    .clamp(0.0, 1.0);
            }
            Control::Start => {
                let top = view.region.1.saturating_sub(1);
                let to = self
                    .drag
                    .turn(now, view.region.0, by)
                    .clamp(0.0, top as f64) as u64;
                ctx.cx.run_held(
                    GESTURE + u32::from(column),
                    "device.set_sample_region",
                    json!({ "device": device, "start": to }),
                );
            }
            Control::End => {
                let bottom = view.region.0 + 1;
                let to = self
                    .drag
                    .turn(now, view.region.1, by)
                    .clamp(bottom as f64, view.frames as f64) as u64;
                ctx.cx.run_held(
                    GESTURE + u32::from(column),
                    "device.set_sample_region",
                    json!({ "device": device, "end": to }),
                );
            }
            Control::Nudge => {
                self.nudge(column, by, view, selected, ctx);
            }
            Control::Param(_) | Control::Empty => {}
        }
    }

    /// Moves the selected slice's start, if it is one placed by hand: the
    /// first slice starts at the Start flag and an automatic slice isn't
    /// placed by hand (R73).
    fn nudge(&mut self, column: u8, by: f64, view: &View<'_>, selected: usize, ctx: &mut Ctx<'_>) {
        let Some(start) = view.manual_start(selected) else {
            return;
        };
        let bounds = view.bounds();
        let (low, high) = (bounds[selected - 1] + 1, bounds[selected + 1] - 1);
        if low > high {
            return;
        }
        let to = self
            .drag
            .turn(ctx.cx.now(), start, by)
            .clamp(low as f64, high as f64) as u64;
        if to != start {
            ctx.cx.run_held(
                GESTURE + u32::from(column),
                "device.move_slice",
                json!({ "device": view.device.id, "from": start, "to": to }),
            );
        }
    }

    /// A finger came on encoder `column` of the main bank: the next step
    /// starts from the document, and if it is a position control, Zoom
    /// zooms around it (R69).
    pub fn touch(&mut self, column: u8, view: &View<'_>) {
        self.drag = Drag::default();
        let control = Self::controls(view)[usize::from(column)];
        let position = match control {
            Control::Start | Control::End | Control::Nudge => true,
            Control::Param(id) => id == s::START || id == s::LENGTH,
            _ => false,
        };
        if position {
            self.around = Some(control);
        }
    }

    /// The upper buttons of Edit Mode, after the first, which goes back
    /// (R70): each one's text, whether its setting is on, whether it can be
    /// pressed.
    fn buttons(&self, view: &View<'_>, bpm: f64) -> Vec<(Edit, String, bool, bool)> {
        let on = |id| view.value(id) >= 0.5;
        edits(view.mode)
            .iter()
            .map(|&edit| match edit {
                Edit::Loop => (edit, "Loop".to_owned(), on(s::LOOP), true),
                Edit::Trigger => {
                    let text = if on(s::TRIGGER) { "Trigger" } else { "Gate" };
                    (edit, text.to_owned(), false, true)
                }
                Edit::PadSlicing => (
                    edit,
                    "Pad Slicing".to_owned(),
                    self.pad_slicing && view.manual(),
                    view.manual(),
                ),
                Edit::SplitSlice => (edit, "Split Slice".to_owned(), false, true),
                Edit::WarpAs => (
                    edit,
                    format!("Warp as {}", bars_text(guess_bars(view.seconds(), bpm))),
                    view.device.warp.warp,
                    view.has_sample(),
                ),
                Edit::Half => (edit, "÷2".to_owned(), false, true),
                Edit::Double => (edit, "×2".to_owned(), false, true),
                Edit::Crop => (edit, "Crop".to_owned(), false, view.has_sample()),
                Edit::Reverse => (edit, "Reverse".to_owned(), false, view.has_sample()),
            })
            .collect()
    }

    /// An upper button of Edit Mode pressed: `column` is the button from 0,
    /// of which the first is Back (R70).
    pub fn press(
        &mut self,
        column: usize,
        view: &View<'_>,
        key: Option<u8>,
        bpm: f64,
        ctx: &mut Ctx<'_>,
    ) {
        let Some((edit, _, _, enabled)) = column
            .checked_sub(1)
            .and_then(|at| self.buttons(view, bpm).into_iter().nth(at))
        else {
            return;
        };
        if !enabled {
            return;
        }
        let device = view.device.id;
        let toggle = |ctx: &mut Ctx<'_>, id: ParamId| {
            let on = view.value(id) >= 0.5;
            ctx.cx.run(
                "device.set_param",
                json!({ "device": device, "param": id, "value": if on { 0.0 } else { 1.0 } }),
            );
        };
        match edit {
            Edit::Loop => toggle(ctx, s::LOOP),
            Edit::Trigger => toggle(ctx, s::TRIGGER),
            Edit::PadSlicing => {
                self.pad_slicing = !self.pad_slicing;
            }
            Edit::SplitSlice => {
                let selected = Self::selected(view, key);
                if let Some((from, to)) = view.slice(selected)
                    && to - from >= 2
                {
                    ctx.cx.run(
                        "device.add_slice",
                        json!({ "device": device, "frame": from + (to - from) / 2 }),
                    );
                }
            }
            Edit::WarpAs => {
                // Warping on and the tempo that fits the bars in one undo
                // step (R70).
                let sample_bpm = guess_bars(view.seconds(), bpm) * 4.0 * 60.0 / view.seconds();
                ctx.cx.run_held(
                    GROUP,
                    "device.set_warp",
                    json!({ "device": device, "warp": true }),
                );
                ctx.cx.run_released(
                    GROUP,
                    "device.set_sample_bpm",
                    json!({ "device": device, "bpm": sample_bpm.clamp(1.0, 1000.0) }),
                );
            }
            Edit::Half => ctx.cx.run(
                "device.scale_tempo",
                json!({ "device": device, "factor": 0.5 }),
            ),
            Edit::Double => ctx.cx.run(
                "device.scale_tempo",
                json!({ "device": device, "factor": 2.0 }),
            ),
            Edit::Crop => ctx
                .cx
                .run("device.crop_sample", json!({ "device": device })),
            Edit::Reverse => ctx
                .cx
                .run("device.reverse_sample", json!({ "device": device })),
        }
    }

    /// Pad Slicing ends when Slice By isn't Manual or the Simpler isn't
    /// slicing (R74).
    pub fn settle(&mut self, view: &View<'_>) {
        self.pad_slicing &= view.mode == Mode::Slice && view.manual();
    }

    /// Fills the upper buttons of Edit Mode, after Back at the first.
    pub fn paint_edit(
        &self,
        view: &View<'_>,
        bpm: f64,
        colour: [u8; 3],
        upper: &mut [Label; 8],
    ) -> Vec<EditState> {
        let mut state = Vec::new();
        for (at, (edit, text, on, enabled)) in self.buttons(view, bpm).into_iter().enumerate() {
            let ink = if enabled { colour } else { [110, 110, 110] };
            upper[at + 1] = Label::new(text.clone(), ink, on);
            state.push(EditState {
                button: at + 2,
                name: edit.name(),
                text,
                on,
            });
        }
        state
    }

    /// Draws the main bank: the columns of R68 over the waveform of R67.
    pub fn paint(&self, view: &View<'_>, key: Option<u8>, paint: &mut Paint<'_>) -> State {
        let selected = Self::selected(view, key);
        let (first, span) = self.window(view, selected);
        let (_, around) = self.anchor(view, selected);
        let count = view.slice_count();
        let state = State {
            empty: !view.has_sample(),
            mode: view.mode.name(),
            main: true,
            zoom: 2f64.powf(self.zoom * OCTAVES),
            around,
            region: view.region,
            slices: count,
            selected,
            pad_slicing: self.pad_slicing,
            buttons: Vec::new(),
        };
        if !view.has_sample() {
            paint.screen.title = Some("Press Browse to load a sample".to_owned());
            return state;
        }
        for (n, control) in Self::controls(view).iter().enumerate() {
            paint.screen.columns[n] = self.column(*control, view, selected);
            paint.screen.shades[n] = Shade::Normal;
        }
        paint.screen.waveform = Some(Self::waveform(view, first, span, selected));
        state
    }

    fn column(&self, control: Control, view: &View<'_>, selected: usize) -> Column {
        let seconds = |frame: u64| format!("{:.3} s", frame as f64 / view.rate);
        let (name, value) = match control {
            Control::Zoom => (
                "Zoom".to_owned(),
                format!("×{:.1}", 2f64.powf(self.zoom * OCTAVES)),
            ),
            Control::Start => ("Start".to_owned(), seconds(view.region.0)),
            Control::End => ("End".to_owned(), seconds(view.region.1)),
            Control::Nudge => (
                "Nudge".to_owned(),
                if view.slice_count() > 0 {
                    format!("slice {}", selected + 1)
                } else {
                    String::new()
                },
            ),
            Control::Param(id) => match view.param(id) {
                Some(param) => (param.name, param.text),
                None => return Column::default(),
            },
            Control::Empty => return Column::default(),
        };
        Column {
            name,
            value,
            graphic: Graphic::None,
            dot: crate::screen::Dot::None,
            touched: false,
        }
    }

    /// The waveform across the display, from `first` for `span` frames
    /// (R67).
    fn waveform(view: &View<'_>, first: f64, span: f64, selected: usize) -> Waveform {
        let width = WIDTH as f64;
        let px = |frame: f64| ((frame - first) / span * width) as f32;
        let peaks = view.sample.map_or_else(Vec::new, |sample| {
            let channels = usize::from(sample.peaks.channels());
            (0..WIDTH)
                .map(|x| {
                    let from = (first + span * x as f64 / width) as u64;
                    let to = ((first + span * (x + 1) as f64 / width) as u64).max(from + 1);
                    (0..channels)
                        .filter_map(|channel| sample.peaks.min_max(channel, from..to))
                        .fold((0i8, 0i8), |(low, high), [lo, hi]| {
                            (low.min(lo), high.max(hi))
                        })
                })
                .collect()
        });
        let (start, end) = view.region;
        let span_region = (end - start) as f64;
        let played = (view.mode == Mode::Classic).then(|| {
            let from = start as f64 + view.value(s::START) / 100.0 * span_region;
            let to = (start as f64
                + ((view.value(s::START) + view.value(s::LENGTH)) / 100.0).min(1.0) * span_region)
                .min(end as f64);
            (from, to)
        });
        let looped = played
            .filter(|_| view.value(s::LOOP) >= 0.5)
            .map(|(from, to)| to - (to - from) * view.value(s::LOOP_LENGTH) / 100.0);
        let bounds = view.bounds();
        let slices = if view.slice_count() > 1 {
            bounds[1..bounds.len() - 1]
                .iter()
                .map(|frame| px(*frame as f64))
                .collect()
        } else {
            Vec::new()
        };
        Waveform {
            peaks,
            region: (px(start as f64), px(end as f64)),
            played: played.map(|(from, to)| (px(from), px(to))),
            looped: looped.map(px),
            slices,
            selected: view
                .slice(selected)
                .filter(|_| view.slice_count() > 0)
                .map(|(from, to)| (px(from as f64), px(to as f64))),
        }
    }
}
