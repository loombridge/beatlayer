//! The sampler devices' sample displays (WP-5.15, IMP-28; the multisampler
//! spec, R54 to R58 and R61): the waveform of the edited sample with its
//! flags, markers, loop brace and slices; zoom and pan; Shift-drag to
//! select, a click to audition, Esc to clear the selection; and the context
//! menu's edits.
//!
//! Simpler's display cell and the multisampler's Sample tab draw through
//! the same [`Displays`]. Every change is a registered command
//! (`device.set_param`, `device.set_layer`, `device.edit_sample`,
//! `device.audition_sample`, ...), which an agent runs too. A drag on a flag
//! or marker is one undo step however far it goes.

use std::collections::HashMap;
use std::sync::Arc;

use bl_media::{Peaks, SampleData};
use bl_model::{ContentHash, DeviceId, LayerId, SampleEdit};
use bl_shell::device_commands::{DeviceFrame, DevicePanel};
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::{Align2, Color32, Id, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How many points a pointer moves before a press is a drag, not a click.
const DRAG_THRESHOLD: f32 = 3.0;

/// How fast a vertical drag zooms: the exponent per point.
const ZOOM_PER_POINT: f64 = 0.01;

/// The fewest frames a view spans.
const MIN_SPAN: f64 = 64.0;

/// The height of one row of Simpler's Warp controls (WP-5.15b).
const WARP_ROW: f32 = 16.0;

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// A small button in Simpler's Warp row (WP-5.15b): draws `text`, and runs
/// when it is clicked or when `key` is pressed while it has the keyboard.
#[expect(clippy::too_many_arguments, reason = "a small button's parts")]
fn warp_button(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    text: &str,
    label: &str,
    on: bool,
    key: Key,
) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let c = &theme.colors;
    let (face, ink) = if on {
        (c.accent.0, c.accent_text.0)
    } else {
        (c.control.0, c.text.0)
    };
    ui.painter().rect_filled(rect, theme.sizes.corner, face);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        ink,
    );
    crate::device_panel::focus_ring(ui, theme, rect, &response);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(label.to_owned());
    });
    response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(key)))
}

/// A small value box in Simpler's Warp row (WP-5.15b): Arrow Up and Down step
/// it by one, a vertical drag moves it, and it says which way the person
/// asked for.
fn warp_number(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    label: &str,
    text: String,
) -> Option<f64> {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, c.window.0);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        c.text.0,
    );
    crate::device_panel::focus_ring(ui, theme, rect, &response);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::SpinButton);
        node.set_label(label.to_owned());
    });
    if response.dragged() {
        // Dragging up raises the value, as Live's number boxes do.
        return Some(-f64::from(ui.input(|i| i.pointer.delta().y)) / 2.0);
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowUp)) {
        Some(1.0)
    } else if response.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowDown)) {
        Some(-1.0)
    } else {
        None
    }
}

/// What a warp mode reads (the warp modes spec, R1).
fn mode_name(mode: bl_model::WarpMode) -> &'static str {
    use bl_model::WarpMode;
    match mode {
        WarpMode::Beats => "Beats",
        WarpMode::Tones => "Tones",
        WarpMode::Texture => "Texture",
        WarpMode::RePitch => "Re-Pitch",
        WarpMode::Complex => "Complex",
        WarpMode::ComplexPro => "Complex Pro",
    }
}

/// The warp mode after `mode`, in the order the chooser offers them (R1).
fn next_mode(mode: bl_model::WarpMode) -> bl_model::WarpMode {
    use bl_model::WarpMode;
    match mode {
        WarpMode::Beats => WarpMode::Tones,
        WarpMode::Tones => WarpMode::Texture,
        WarpMode::Texture => WarpMode::RePitch,
        WarpMode::RePitch => WarpMode::Complex,
        WarpMode::Complex => WarpMode::ComplexPro,
        WarpMode::ComplexPro => WarpMode::Beats,
    }
}

/// What Beats' Preserve reads (R2).
fn preserve_name(preserve: bl_model::Preserve) -> &'static str {
    use bl_model::Preserve;
    match preserve {
        Preserve::Transients => "Transients",
        Preserve::Bar => "1 Bar",
        Preserve::Half => "Half",
        Preserve::Quarter => "Quarter",
        Preserve::Eighth => "Eighth",
        Preserve::Sixteenth => "16th",
        Preserve::ThirtySecond => "32nd",
    }
}

/// The Preserve after `preserve`, in R2's order.
fn next_preserve(preserve: bl_model::Preserve) -> bl_model::Preserve {
    use bl_model::Preserve;
    match preserve {
        Preserve::Transients => Preserve::Bar,
        Preserve::Bar => Preserve::Half,
        Preserve::Half => Preserve::Quarter,
        Preserve::Quarter => Preserve::Eighth,
        Preserve::Eighth => Preserve::Sixteenth,
        Preserve::Sixteenth => Preserve::ThirtySecond,
        Preserve::ThirtySecond => Preserve::Transients,
    }
}

/// What Beats' Transient Loop Mode reads (R2).
fn loop_name(loop_mode: bl_model::TransientLoopMode) -> &'static str {
    use bl_model::TransientLoopMode;
    match loop_mode {
        TransientLoopMode::LoopOff => "Loop Off",
        TransientLoopMode::LoopForward => "Loop Forward",
        TransientLoopMode::LoopBackAndForth => "Back & Forth",
    }
}

/// The Transient Loop Mode after `loop_mode`, in R2's order.
fn next_loop(loop_mode: bl_model::TransientLoopMode) -> bl_model::TransientLoopMode {
    use bl_model::TransientLoopMode;
    match loop_mode {
        TransientLoopMode::LoopOff => TransientLoopMode::LoopForward,
        TransientLoopMode::LoopForward => TransientLoopMode::LoopBackAndForth,
        TransientLoopMode::LoopBackAndForth => TransientLoopMode::LoopOff,
    }
}

/// The value of a device's parameter, from its panel.
pub(crate) fn param_value(panel: &DevicePanel, id: bl_model::ParamId) -> f64 {
    panel
        .description
        .params
        .iter()
        .position(|p| p.id == id)
        .map_or(0.0, |i| panel.value(i))
}

/// Which channels the waveform draws (R14): B, M, L and R.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channels {
    /// Both, one above the other.
    #[default]
    Both,
    /// Their sum.
    Mono,
    /// The left.
    Left,
    /// The right.
    Right,
}

/// What a flag's context menu draws.
pub type FlagMenu<'a> = Box<dyn Fn(&mut Ui, &mut Vec<UiEvent>) + 'a>;

/// A flag or marker the person drags.
pub struct Flag<'a> {
    /// What assistive technology reads.
    pub name: &'static str,
    /// Where it is, in frames of the edited sample at its own rate.
    pub frame: u64,
    /// Whether it is a loop's (drawn in the focus colour).
    pub loop_marker: bool,
    /// The undo step's name for a drag on it.
    pub gesture: &'static str,
    /// The commands a drag to a frame runs.
    pub drag: Box<dyn Fn(u64) -> Vec<(&'static str, Value)> + 'a>,
    /// Its context menu, if it has one.
    pub menu: Option<FlagMenu<'a>>,
}

/// What a display shows now.
pub struct Shown<'a> {
    /// The sampler.
    pub device: DeviceId,
    /// The multisampler's layer; none for Simpler.
    pub layer: Option<LayerId>,
    /// The sample's source, by content hash.
    pub media: ContentHash,
    /// The edits made to it.
    pub edits: &'a [SampleEdit],
    /// What the Status Bar reads while the pointer is over the waveform.
    pub readout: String,
    /// The flags and markers.
    pub handles: Vec<Flag<'a>>,
    /// The loop brace, in frames.
    pub brace: Option<(u64, u64)>,
    /// The slices, each with its frame and whether it was made by hand.
    pub slices: &'a [(u64, bool)],
    /// Whether this is Simpler's display, whose menu offers Normalize
    /// Volumes (R60).
    pub snapshot: bool,
    /// Whether the sample is dropped on the display to replace or add a
    /// layer (R61).
    pub takes_drops: bool,
    /// Whether Slice mode is active.
    pub slice_mode: bool,
}

/// A press on the waveform and what it became.
struct Press {
    origin: Pos2,
    shift: bool,
    /// The frame under the press, and the view when it began.
    frame: f64,
    first: f64,
    span: f64,
    moved: bool,
}

/// One display's state.
struct State {
    media: ContentHash,
    layer: Option<LayerId>,
    edits: Vec<SampleEdit>,
    audio: Arc<SampleData>,
    peaks: Option<Arc<Peaks>>,
    first: f64,
    span: f64,
    vertical: f32,
    channels: Channels,
    selection: Option<(u64, u64)>,
    press: Option<Press>,
    dragging: Option<usize>,
    auditioning: bool,
    /// The slices worked out for Simpler, and what they were worked out
    /// from.
    slices: Vec<(u64, bool)>,
    slice_key: Option<(usize, Vec<u64>)>,
    selected_slice: Option<u64>,
    dragging_slice: Option<(u64, u64)>,
}

/// The sample displays of a window's devices.
#[derive(Default)]
pub struct Displays {
    states: HashMap<DeviceId, State>,
}

impl std::fmt::Debug for Displays {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Displays").finish_non_exhaustive()
    }
}

impl Displays {
    /// Which channels a device's display draws.
    #[must_use]
    pub fn channels(&self, device: DeviceId) -> Channels {
        self.states
            .get(&device)
            .map_or(Channels::Both, |s| s.channels)
    }

    /// Chooses the channels a device's display draws.
    pub fn set_channels(&mut self, device: DeviceId, channels: Channels) {
        if let Some(state) = self.states.get_mut(&device) {
            state.channels = channels;
        }
    }

    /// Loads the edited sample into `device`'s state when it is new or its
    /// edits changed, keeping the view where the sample is the same one.
    /// Returns whether the display has audio to draw.
    fn ensure(
        &mut self,
        shell: &Shell,
        device: DeviceId,
        layer: Option<LayerId>,
        media: ContentHash,
        edits: &[SampleEdit],
    ) -> bool {
        if let Some(state) = self.states.get(&device)
            && state.media == media
            && state.layer == layer
            && state.edits == edits
        {
            return true;
        }
        let Some(audio) = shell.edited_audio(media, edits) else {
            self.states.remove(&device);
            return false;
        };
        let peaks = Peaks::from_samples(&audio).ok().map(Arc::new);
        let total = audio.frames().max(1) as f64;
        let old = self.states.remove(&device);
        // A held audition outlives the reload, so the button's release
        // still ends it.
        let auditioning = old.as_ref().is_some_and(|s| s.auditioning);
        let kept = old.filter(|s| s.media == media && s.layer == layer);
        let (first, span, vertical, channels) = match &kept {
            Some(s) => (
                s.first.min(total - 1.0),
                s.span.min(total),
                s.vertical,
                s.channels,
            ),
            None => (0.0, total, 1.0, Channels::Both),
        };
        self.states.insert(
            device,
            State {
                media,
                layer,
                edits: edits.to_vec(),
                audio,
                peaks,
                first: first.min((total - span).max(0.0)),
                span,
                vertical,
                channels,
                // Frames of the old list mean nothing in the new sample.
                selection: None,
                press: None,
                dragging: None,
                auditioning,
                slices: Vec::new(),
                slice_key: None,
                selected_slice: None,
                dragging_slice: None,
            },
        );
        true
    }

    /// Simpler's display in its cell (R54): the waveform, the region's
    /// flags, the loop brace in Classic mode and the slices in Slice mode.
    pub fn snapshot(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        cell: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        use bl_devices::sampler as s;
        let Some(panel) = frame.panel.as_ref() else {
            return;
        };
        let Some(device) = shell.editor().document().device(frame.id) else {
            return;
        };
        let rect = cell.shrink(2.0);
        // The Warp controls sit along the cell's top edge (WP-5.15b), above
        // the waveform.
        let warp_height = (WARP_ROW * 2.0 + 2.0).min(rect.height() * 0.5);
        let (warp_rect, rect) = (
            Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + warp_height)),
            Rect::from_min_max(pos2(rect.left(), rect.top() + warp_height), rect.max),
        );
        self.warp(ui, theme, device, warp_rect, events);
        let Some(media) = device.media_at(0) else {
            ui.painter_at(rect).text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Drop a sample here",
                theme.small_font(),
                theme.colors.text_dim.0,
            );
            self.states.remove(&frame.id);
            return;
        };
        let edits = device.sample_edits.clone();
        if !self.ensure(shell, frame.id, None, media, &edits) {
            ui.painter_at(rect).text(
                rect.center(),
                Align2::CENTER_CENTER,
                "The sample's file is missing",
                theme.small_font(),
                theme.colors.text_dim.0,
            );
            return;
        }
        let value = |id| param_value(panel, id);
        let state = self.states.get_mut(&frame.id).expect("just loaded");
        let frames = state.audio.frames() as u64;
        let at = |share: f64| (share.clamp(0.0, 1.0) * frames as f64) as u64;
        let (start, length) = (value(s::START) / 100.0, value(s::LENGTH) / 100.0);
        let (region_start, region_end) = (at(start), at(start + length));
        let slice_mode = value(s::MODE) as usize == 2;
        // The slices Slice mode cuts the sample into (the sampler spec,
        // R14), worked out again when what they depend on changes.
        if slice_mode {
            let tempo = &shell.editor().document().settings().tempo;
            let bpm = if device.warp.warp {
                device
                    .warp
                    .bpm
                    .unwrap_or_else(|| tempo.bpm_at(bl_core::time::Ticks::ZERO).get())
            } else {
                tempo.bpm_at(bl_core::time::Ticks::ZERO).get()
            };
            let sixteenth = f64::from(state.audio.sample_rate()) * 60.0 / bpm / 4.0;
            let slice_by = value(s::SLICE_BY) as usize;
            let key = (
                Arc::as_ptr(&state.audio) as usize,
                [
                    s::START,
                    s::LENGTH,
                    s::SLICE_BY,
                    s::SLICE_SENSITIVITY,
                    s::SLICE_DIVISION,
                    s::SLICE_REGIONS,
                ]
                .iter()
                .map(|id| value(*id).to_bits())
                .chain([sixteenth.to_bits()])
                .chain(device.slices.iter().copied())
                .collect::<Vec<_>>(),
            );
            if state.slice_key.as_ref() != Some(&key) {
                if slice_by == 3 {
                    state.slices = device
                        .slices
                        .iter()
                        .copied()
                        .filter(|&f| f > region_start && f < region_end)
                        .map(|frame| (frame, true))
                        .collect();
                } else {
                    let bounds = bl_devices::slice_boundaries(
                        &state.audio,
                        &value,
                        sixteenth,
                        &device.slices,
                    );
                    let inner = bounds.len().saturating_sub(1);
                    let manual_set: std::collections::HashSet<u64> =
                        device.slices.iter().copied().collect();
                    state.slices = bounds
                        .into_iter()
                        .skip(1)
                        .take(inner.saturating_sub(1))
                        .map(|frame| {
                            let is_manual = manual_set.contains(&frame);
                            (frame, is_manual)
                        })
                        .collect();
                }
                state.slice_key = Some(key);
            }
        } else {
            state.slices.clear();
            state.slice_key = None;
        }
        let slices = state.slices.clone();
        let device_id = frame.id;
        let share = move |frame: u64| frame as f64 / frames.max(1) as f64 * 100.0;
        let set = move |param: bl_model::ParamId, value: f64| {
            (
                "device.set_param",
                json!({ "device": device_id, "param": param, "value": value }),
            )
        };
        let mut handles = vec![
            Flag {
                name: "Region Start",
                frame: region_start,
                loop_marker: false,
                gesture: "Change Sample Region",
                drag: Box::new(move |frame| {
                    let end = share(region_end);
                    let start = share(frame.min(region_end.saturating_sub(1)));
                    // The flag moves alone: the end stays where it was.
                    vec![set(s::START, start), set(s::LENGTH, end - start)]
                }),
                menu: None,
            },
            Flag {
                name: "Region End",
                frame: region_end,
                loop_marker: false,
                gesture: "Change Sample Region",
                drag: Box::new(move |frame| {
                    let frame = frame.max(region_start + 1);
                    vec![set(
                        s::LENGTH,
                        (share(frame) - share(region_start)).max(0.0),
                    )]
                }),
                menu: None,
            },
        ];
        let brace =
            (!slice_mode && value(s::MODE) as usize == 0 && value(s::LOOP) >= 0.5).then(|| {
                let loop_start = region_end
                    - ((value(s::LOOP_LENGTH) / 100.0) * (region_end - region_start) as f64) as u64;
                handles.push(Flag {
                    name: "Loop Start",
                    frame: loop_start,
                    loop_marker: true,
                    gesture: "Change Loop",
                    drag: Box::new(move |frame| {
                        let span = (region_end - region_start).max(1) as f64;
                        let share = (region_end
                            .saturating_sub(frame.clamp(region_start, region_end))
                            as f64
                            / span
                            * 100.0)
                            .clamp(0.0, 100.0);
                        vec![set(s::LOOP_LENGTH, share)]
                    }),
                    menu: None,
                });
                (loop_start, region_end)
            });
        let readout = format!(
            "{}: {} frames, {} Hz, {} channel{}",
            frame.title,
            frames,
            state.audio.sample_rate(),
            state.audio.channel_count(),
            if state.audio.channel_count() == 1 {
                ""
            } else {
                "s"
            }
        );
        let shown = Shown {
            device: frame.id,
            layer: None,
            media,
            edits: &edits,
            readout,
            handles,
            brace,
            slices: &slices,
            snapshot: true,
            takes_drops: false,
            slice_mode,
        };
        self.show(ui, shell, theme, &shown, rect, events);
    }

    /// Simpler's Warp controls (WP-5.15b, the sampler spec R18), along the
    /// sample cell's top edge: the Warp switch, the warp mode with its own
    /// controls, the sample's tempo, and Live's ÷2 and ×2. Each runs the
    /// registered command an agent runs too.
    fn warp(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        device: &bl_model::Device,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        use bl_model::WarpMode;
        let device_id = device.id;
        let w = &device.warp;
        let mut x = rect.left();
        let mut row = rect.top();
        let mut place = move |width: f32| {
            let r = Rect::from_min_size(pos2(x, row), egui::vec2(width, WARP_ROW));
            x += width + 2.0;
            r
        };
        // The Warp switch.
        let on = w.warp;
        if warp_button(
            ui,
            theme,
            Id::new(("warp", device_id)),
            place(26.0),
            "Warp",
            "Warp",
            on,
            Key::Enter,
        ) {
            invoke(
                events,
                "device.set_warp",
                json!({ "device": device_id, "warp": !on }),
            );
        }
        // The warp mode, which cycles on Enter.
        let next = next_mode(w.mode);
        if warp_button(
            ui,
            theme,
            Id::new(("warp-mode", device_id)),
            place(42.0),
            mode_name(w.mode),
            "Warp Mode",
            false,
            Key::Enter,
        ) {
            invoke(
                events,
                "device.set_warp_mode",
                json!({ "device": device_id, "mode": next }),
            );
        }
        // The sample's tempo: Arrow Up and Down step it by one BPM.
        let bpm = w.bpm.unwrap_or(120.0);
        if let Some(step) = warp_number(
            ui,
            theme,
            Id::new(("warp-bpm", device_id)),
            place(40.0),
            "Sample BPM",
            format!("{bpm:.0} BPM"),
        ) {
            let bpm = (bpm + step).clamp(1.0, 1000.0);
            invoke(
                events,
                "device.set_sample_bpm",
                json!({ "device": device_id, "bpm": bpm }),
            );
        }
        // Live's ÷2 and ×2, which halve and double the sample's tempo.
        if warp_button(
            ui,
            theme,
            Id::new(("warp-half", device_id)),
            place(18.0),
            "÷2",
            "Halve Tempo",
            false,
            Key::Enter,
        ) {
            invoke(
                events,
                "device.scale_tempo",
                json!({ "device": device_id, "factor": 0.5 }),
            );
        }
        if warp_button(
            ui,
            theme,
            Id::new(("warp-double", device_id)),
            place(18.0),
            "×2",
            "Double Tempo",
            false,
            Key::Enter,
        ) {
            invoke(
                events,
                "device.scale_tempo",
                json!({ "device": device_id, "factor": 2.0 }),
            );
        }
        // The mode's own controls (R18), on a second row.
        row = rect.top() + WARP_ROW + 2.0;
        x = rect.left();
        let mut place = move |width: f32| {
            let r = Rect::from_min_size(pos2(x, row), egui::vec2(width, WARP_ROW));
            x += width + 2.0;
            r
        };
        match w.mode {
            WarpMode::Beats => {
                let beats = w.settings.beats;
                if warp_button(
                    ui,
                    theme,
                    Id::new(("warp-preserve", device_id)),
                    place(52.0),
                    preserve_name(beats.preserve),
                    "Preserve",
                    false,
                    Key::Enter,
                ) {
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "beats": { "preserve": next_preserve(beats.preserve) },
                        }),
                    );
                }
                if warp_button(
                    ui,
                    theme,
                    Id::new(("warp-loop", device_id)),
                    place(46.0),
                    loop_name(beats.transient_loop_mode),
                    "Loop Mode",
                    false,
                    Key::Enter,
                ) {
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "beats": { "transient_loop_mode": next_loop(beats.transient_loop_mode) },
                        }),
                    );
                }
                if let Some(step) = warp_number(
                    ui,
                    theme,
                    Id::new(("warp-env", device_id)),
                    place(34.0),
                    "Envelope",
                    beats.transient_envelope.to_string(),
                ) {
                    let value = (f64::from(beats.transient_envelope) + step).clamp(0.0, 100.0);
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "beats": { "transient_envelope": value.round() as u32 },
                        }),
                    );
                }
            }
            WarpMode::Tones => {
                let grain = w.settings.tones.grain_size;
                if let Some(step) = warp_number(
                    ui,
                    theme,
                    Id::new(("warp-grain", device_id)),
                    place(46.0),
                    "Grain Size",
                    format!("{grain:.0} ms"),
                ) {
                    let value = (grain + step).clamp(10.0, 100.0);
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "tones": { "grain_size": value },
                        }),
                    );
                }
            }
            WarpMode::Texture => {
                let texture = w.settings.texture;
                if let Some(step) = warp_number(
                    ui,
                    theme,
                    Id::new(("warp-grain", device_id)),
                    place(46.0),
                    "Grain Size",
                    format!("{:.0} ms", texture.grain_size),
                ) {
                    let value = (texture.grain_size + step).clamp(10.0, 500.0);
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "texture": { "grain_size": value },
                        }),
                    );
                }
                if let Some(step) = warp_number(
                    ui,
                    theme,
                    Id::new(("warp-flux", device_id)),
                    place(40.0),
                    "Flux",
                    format!("{:.0} %", texture.flux),
                ) {
                    let value = (texture.flux + step).clamp(0.0, 100.0);
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "texture": { "flux": value },
                        }),
                    );
                }
            }
            WarpMode::ComplexPro => {
                let pro = w.settings.complex_pro;
                if let Some(step) = warp_number(
                    ui,
                    theme,
                    Id::new(("warp-formants", device_id)),
                    place(46.0),
                    "Formants",
                    format!("{:.0} %", pro.formants),
                ) {
                    let value = (pro.formants + step).clamp(0.0, 100.0);
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "complex_pro": { "formants": value },
                        }),
                    );
                }
                if let Some(step) = warp_number(
                    ui,
                    theme,
                    Id::new(("warp-envelope", device_id)),
                    place(40.0),
                    "Envelope",
                    pro.envelope.to_string(),
                ) {
                    let value = (f64::from(pro.envelope) + step).clamp(8.0, 512.0);
                    invoke(
                        events,
                        "device.set_warp_settings",
                        json!({
                            "device": device_id,
                            "complex_pro": { "envelope": value.round() as u32 },
                        }),
                    );
                }
            }
            // Re-Pitch and Complex have no controls of their own (R2).
            WarpMode::RePitch | WarpMode::Complex => {}
        }
    }

    /// Draws a display in `rect` and runs what the person does in it.
    #[expect(clippy::too_many_lines, reason = "one display's drawing and gestures")]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        shown: &Shown<'_>,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let device = shown.device;
        if !self.ensure(shell, device, shown.layer, shown.media, shown.edits) {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "The sample's file is missing",
                theme.small_font(),
                c.text_dim.0,
            );
            return;
        }
        let state = self.states.get_mut(&device).expect("just loaded");
        let total = state.audio.frames().max(1) as f64;
        let width = f64::from(rect.width().max(1.0));
        let id = Id::new(("sample-display", device));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        // Flags sit on top of the area, so a press on one is theirs.
        let handle_rects: Vec<Rect> = shown
            .handles
            .iter()
            .map(|h| {
                let x = rect.left() + ((h.frame as f64 - state.first) / state.span * width) as f32;
                Rect::from_min_max(pos2(x - 4.0, rect.top()), pos2(x + 4.0, rect.bottom()))
            })
            .collect();
        let slice_handle_rects: Vec<Rect> = if shown.slice_mode {
            shown
                .slices
                .iter()
                .map(|&(frame, _)| {
                    let x =
                        rect.left() + ((frame as f64 - state.first) / state.span * width) as f32;
                    Rect::from_min_max(pos2(x - 4.0, rect.top()), pos2(x + 4.0, rect.bottom()))
                })
                .collect()
        } else {
            Vec::new()
        };
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let on_handle = pointer.is_some_and(|p| {
            handle_rects.iter().any(|r| r.contains(p))
                || slice_handle_rects.iter().any(|r| r.contains(p))
        });
        let frame_at = |state: &State, x: f32| {
            (state.first + f64::from(x - rect.left()) / width * state.span).clamp(0.0, total)
        };

        // Zoom with Cmd or Ctrl and the wheel (R54), about the pointer.
        if response.hovered() {
            let zoom = ui.input(|i| i.zoom_delta());
            if (zoom - 1.0).abs() > f32::EPSILON {
                let at = pointer.map_or(0.5, |p| f64::from((p.x - rect.left()) / rect.width()));
                let anchor = state.first + state.span * at;
                state.span = (state.span / f64::from(zoom)).clamp(MIN_SPAN.min(total), total);
                state.first = (anchor - state.span * at).clamp(0.0, total - state.span);
            }
            crate::mixer::show_readout(ui.ctx(), shown.readout.clone());
        }

        // A press on the waveform: Shift selects; anything else starts a
        // click that auditions, and becomes a pan and zoom if it moves.
        let (pressed, down, shift, origin) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.modifiers.shift,
                i.pointer.press_origin(),
            )
        });
        if pressed
            && !on_handle
            && let Some(origin) = origin.filter(|o| rect.contains(*o))
        {
            let frame = frame_at(state, origin.x);
            state.press = Some(Press {
                origin,
                shift,
                frame,
                first: state.first,
                span: state.span,
                moved: false,
            });
            if !shift {
                state.auditioning = true;
                invoke(
                    events,
                    "device.audition_sample",
                    json!({ "device": device, "layer": shown.layer, "from": frame / total, "down": true }),
                );
            }
        }
        if let (Some(press), Some(now)) = (state.press.as_mut(), pointer)
            && down
        {
            let delta = now - press.origin;
            if press.shift {
                let here = (state.first + f64::from(now.x - rect.left()) / width * state.span)
                    .clamp(0.0, total);
                let (a, b) = (press.frame.min(here), press.frame.max(here));
                if (b - a) * width / state.span >= f64::from(DRAG_THRESHOLD)
                    || state.selection.is_some()
                {
                    state.selection = Some((
                        a.round() as u64,
                        (b.round() as u64).max(a.round() as u64 + 1),
                    ));
                }
            } else {
                if !press.moved && delta.length() > DRAG_THRESHOLD {
                    press.moved = true;
                    if state.auditioning {
                        state.auditioning = false;
                        invoke(
                            events,
                            "device.audition_sample",
                            json!({ "device": device, "layer": shown.layer, "down": false }),
                        );
                    }
                }
                if press.moved {
                    // Dragging down zooms in about where the press began and
                    // up zooms out, as Live's rulers do; sideways pans the
                    // frames under the pointer along with it.
                    let zoom = (f64::from(delta.y) * ZOOM_PER_POINT).exp();
                    let span = (press.span / zoom).clamp(MIN_SPAN.min(total), total);
                    let at = f64::from(press.origin.x - rect.left()) / width;
                    let anchor = press.first + press.span * at;
                    let along = f64::from(delta.x) / width;
                    state.span = span;
                    state.first = (anchor - span * (at + along)).clamp(0.0, total - span);
                }
            }
        }
        if !down {
            state.press = None;
            if state.auditioning {
                state.auditioning = false;
                invoke(
                    events,
                    "device.audition_sample",
                    json!({ "device": device, "layer": shown.layer, "down": false }),
                );
            }
        }
        // Esc clears the selection (R54).
        if state.selection.is_some()
            && (response.hovered() || response.has_focus())
            && ui.input(|i| i.key_pressed(Key::Escape))
        {
            state.selection = None;
        }

        // In Slice mode, double-clicking adds a manual slice (R14, R54).
        if shown.slice_mode
            && response.double_clicked()
            && !on_handle
            && let Some(p) = pointer.filter(|p| rect.contains(*p))
        {
            let frame = frame_at(state, p.x).round() as u64;
            invoke(
                events,
                "device.add_slice",
                json!({ "device": device, "frame": frame }),
            );
            state.selected_slice = Some(frame);
        }

        // Delete or Backspace deletes the selected slice (WP-5.15b).
        if shown.slice_mode
            && (response.hovered() || response.has_focus())
            && ui.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace))
            && let Some(frame) = state.selected_slice
        {
            invoke(
                events,
                "device.delete_slice",
                json!({ "device": device, "frame": frame }),
            );
            state.selected_slice = None;
        }

        // ----- drawing -----------------------------------------------------
        let painter = ui.painter_at(rect);
        let fpp = state.span / width;
        let x_of = |frame: u64| rect.left() + ((frame as f64 - state.first) / fpp) as f32;
        let both = state.channels == Channels::Both && state.audio.channel_count() > 1;
        let lanes = if both { 2 } else { 1 };
        let lane_height = rect.height() / lanes as f32;
        let frames = state.audio.frames();
        for lane in 0..lanes {
            let centre = rect.top() + lane_height * (lane as f32 + 0.5);
            let half = lane_height * 0.45 * state.vertical;
            for px in 0..rect.width() as usize {
                let from = (state.first + px as f64 * fpp) as usize;
                let to = ((state.first + (px as f64 + 1.0) * fpp) as usize).max(from + 1);
                if from >= frames {
                    break;
                }
                let (low, high) = column(state, lane, both, from, to.min(frames));
                let x = rect.left() + px as f32 + 0.5;
                painter.line_segment(
                    [
                        pos2(x, centre - high.clamp(-1.0, 1.0) * half),
                        pos2(x, centre - low.clamp(-1.0, 1.0) * half - 0.5),
                    ],
                    Stroke::new(1.0, c.accent.0),
                );
            }
        }
        if let Some((from, to)) = shown.brace {
            let band = Rect::from_min_max(
                pos2(x_of(from).max(rect.left()), rect.top()),
                pos2(x_of(to).min(rect.right()), rect.top() + 4.0),
            );
            if band.width() > 0.0 {
                painter.rect_filled(band, 0.0, c.focus.0);
            }
        }
        let mut slice_dragged = None;
        for (index, &(frame, manual)) in shown.slices.iter().enumerate() {
            let x = x_of(frame);
            if x < rect.left() - 4.0 || x > rect.right() + 4.0 {
                continue;
            }
            let is_selected = state.selected_slice == Some(frame);
            // Automatic slices in one colour, manual ones in another (R54).
            let colour = if is_selected {
                c.focus.0
            } else if manual {
                c.accent.0
            } else {
                c.text_dim.0
            };
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(if is_selected { 2.0 } else { 1.0 }, colour),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 3.0, rect.top()),
                    pos2(x + 3.0, rect.top()),
                    pos2(x, rect.top() + 5.0),
                ],
                colour,
                Stroke::NONE,
            ));
            let area = slice_handle_rects.get(index).copied().unwrap_or_else(|| {
                Rect::from_min_max(pos2(x - 4.0, rect.top()), pos2(x + 4.0, rect.bottom()))
            });
            let sid = Id::new(("slice-handle", device, index));
            let sr = ui.interact(area, sid, Sense::click_and_drag());
            if sr.clicked() {
                state.selected_slice = Some(frame);
            }
            if sr.double_clicked() {
                invoke(
                    events,
                    "device.delete_slice",
                    json!({ "device": device, "frame": frame }),
                );
                state.selected_slice = None;
            }
            if sr.drag_started() {
                state.dragging_slice = Some((frame, frame));
                state.selected_slice = Some(frame);
                events.push(UiEvent::BeginGesture("Move Slice"));
            }
            if sr.dragged()
                && let Some((_orig, cur)) = state.dragging_slice
                && let Some(p) = pointer
            {
                let new_frame = frame_at(state, p.x).round() as u64;
                slice_dragged = Some((cur, new_frame));
            }
            if sr.drag_stopped() {
                state.dragging_slice = None;
                events.push(UiEvent::EndGesture);
            }
            ui.ctx().accesskit_node_builder(sid, |n| {
                n.set_role(Role::Group);
                n.set_label(format!("Slice {}", index + 2));
                n.set_value(frame.to_string());
                n.set_description(
                    if manual {
                        "A slice made by hand"
                    } else {
                        "An automatic slice"
                    }
                    .to_owned(),
                );
                n.set_bounds(egui::accesskit::Rect {
                    x0: area.min.x.into(),
                    y0: area.min.y.into(),
                    x1: area.max.x.into(),
                    y1: area.max.y.into(),
                });
            });
        }
        if let Some((from, to)) = slice_dragged
            && from != to
        {
            invoke(
                events,
                "device.move_slice",
                json!({ "device": device, "from": from, "to": to }),
            );
            if let Some((orig, _)) = state.dragging_slice {
                state.dragging_slice = Some((orig, to));
            }
            state.selected_slice = Some(to);
        }
        if let Some((from, to)) = state.selection {
            let band = Rect::from_min_max(
                pos2(x_of(from).clamp(rect.left(), rect.right()), rect.top()),
                pos2(x_of(to).clamp(rect.left(), rect.right()), rect.bottom()),
            );
            painter.rect_filled(band, 0.0, Color32::from_white_alpha(40));
            painter.rect_stroke(band, 0.0, Stroke::new(1.0, c.text.0), StrokeKind::Inside);
            let node = Id::new(("selection", device));
            ui.ctx().accesskit_node_builder(node, |n| {
                n.set_role(Role::Group);
                n.set_label("Selection".to_owned());
                n.set_value(format!("{from} to {to}"));
                n.set_bounds(egui::accesskit::Rect {
                    x0: band.min.x.into(),
                    y0: band.min.y.into(),
                    x1: band.max.x.into(),
                    y1: band.max.y.into(),
                });
            });
        }
        ui.ctx().accesskit_node_builder(id, |n| {
            n.set_role(Role::Image);
            n.set_label("Sample Display".to_owned());
            n.set_value(shown.readout.clone());
        });

        // ----- flags and markers -------------------------------------------
        let mut dragged = None;
        for (index, handle) in shown.handles.iter().enumerate() {
            let x = x_of(handle.frame);
            if x < rect.left() - 4.0 || x > rect.right() + 4.0 {
                continue;
            }
            let area = handle_rects[index];
            let hid = Id::new(("sample-handle", device, handle.name));
            let hr = ui.interact(area, hid, Sense::drag());
            let colour = if handle.loop_marker {
                c.focus.0
            } else {
                c.text.0
            };
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(if hr.hovered() { 2.0 } else { 1.0 }, colour),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 4.0, rect.top()),
                    pos2(x + 4.0, rect.top()),
                    pos2(x, rect.top() + 6.0),
                ],
                colour,
                Stroke::NONE,
            ));
            ui.ctx().accesskit_node_builder(hid, |n| {
                n.set_role(Role::Slider);
                n.set_label(handle.name.to_owned());
                n.set_value(handle.frame.to_string());
                n.set_bounds(egui::accesskit::Rect {
                    x0: area.min.x.into(),
                    y0: area.min.y.into(),
                    x1: area.max.x.into(),
                    y1: area.max.y.into(),
                });
            });
            if hr.drag_started() {
                state.dragging = Some(index);
                events.push(UiEvent::BeginGesture(handle.gesture));
            }
            if hr.dragged()
                && state.dragging == Some(index)
                && let Some(p) = pointer
            {
                dragged = Some((index, frame_at(state, p.x).round() as u64));
            }
            if hr.drag_stopped() {
                state.dragging = None;
                events.push(UiEvent::EndGesture);
            }
            if let Some(menu) = &handle.menu {
                hr.context_menu(|ui| menu(ui, events));
            }
        }
        if let Some((index, frame)) = dragged {
            let handle = &shown.handles[index];
            if frame != handle.frame {
                for (command, args) in (handle.drag)(frame) {
                    invoke(events, command, args);
                }
            }
        }

        // ----- the context menu --------------------------------------------
        let selection = state.selection;
        let layer = shown.layer;
        let snapshot = shown.snapshot;
        let media = shown.media;
        response.context_menu(|ui| {
            menu(
                ui,
                shell,
                events,
                (device, layer, media),
                selection,
                snapshot,
            );
        });

        // ----- a sample dropped on the display (R61) -----------------------
        if shown.takes_drops {
            let (dropped, alt) = ui.input(|i| {
                (
                    i.raw
                        .dropped_files
                        .iter()
                        .map(|f| f.path().to_string_lossy().into_owned())
                        .collect::<Vec<_>>(),
                    i.modifiers.alt,
                )
            });
            if !dropped.is_empty()
                && pointer.is_some_and(|p| rect.contains(p))
                && let Some(layer) = shown.layer
            {
                if alt {
                    // With Option held, a new layer after this one, with its
                    // three zones, so the two sound together.
                    invoke(
                        events,
                        bl_shell::multisampler_commands::ADD_LAYERS,
                        json!({ "device": device, "paths": dropped, "after": layer, "like": layer }),
                    );
                } else {
                    invoke(
                        events,
                        bl_shell::multisampler_commands::SET_LAYER_SAMPLE,
                        json!({ "device": device, "layer": layer, "path": dropped[0] }),
                    );
                }
            }
        }
    }
}

/// The lowest and highest sample of one pixel column of `lane`: from the
/// peaks of the edited sample, or from the samples where the view is zoomed
/// in below a peak.
fn column(state: &State, lane: usize, both: bool, from: usize, to: usize) -> (f32, f32) {
    let audio = &state.audio;
    let channels = audio.channel_count();
    let read = |channel: usize| -> Option<&[f32]> { audio.channel(channel) };
    let samples = |from: usize, to: usize, value: &dyn Fn(usize) -> f32| {
        let (mut low, mut high) = (0.0f32, 0.0f32);
        let stride = ((to - from) / 64).max(1);
        for i in (from..to).step_by(stride) {
            let v = value(i);
            low = low.min(v);
            high = high.max(v);
        }
        (low, high)
    };
    // Below a peak's width a column reads the samples.
    let wide = to - from >= 256;
    let peak = |channel: usize| -> Option<(f32, f32)> {
        let [lo, hi] = state
            .peaks
            .as_ref()?
            .min_max(channel, from as u64..to as u64)?;
        Some((f32::from(lo) / 127.0, f32::from(hi) / 127.0))
    };
    match state.channels {
        Channels::Mono if channels > 1 => samples(from, to, &|i| {
            (0..channels).filter_map(read).map(|c| c[i]).sum::<f32>() / channels as f32
        }),
        _ => {
            let channel = match state.channels {
                Channels::Both if both => lane,
                Channels::Right => 1.min(channels - 1),
                _ => 0,
            };
            if wide && let Some(found) = peak(channel) {
                return found;
            }
            let data = read(channel).unwrap_or(&[]);
            samples(from, to, &|i| data.get(i).copied().unwrap_or(0.0))
        }
    }
}

/// The context menu (R55, R58, R59, R60): Live's own items, then the edits.
fn menu(
    ui: &mut Ui,
    shell: &Shell,
    events: &mut Vec<UiEvent>,
    (device, layer, media): (DeviceId, Option<LayerId>, ContentHash),
    selection: Option<(u64, u64)>,
    snapshot: bool,
) {
    let target = json!({ "device": device, "layer": layer });
    let mut run = |ui: &mut Ui, label: &str, id: &str, args: Value, enabled: bool| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            invoke(events, id, args);
            ui.close();
        }
    };
    run(
        ui,
        "Crop",
        bl_shell::multisampler_commands::CROP_SAMPLE,
        target.clone(),
        true,
    );
    run(
        ui,
        "Reverse",
        bl_shell::multisampler_commands::REVERSE_SAMPLE,
        target.clone(),
        true,
    );
    if snapshot {
        run(
            ui,
            "Normalize Volumes",
            bl_shell::multisampler_commands::NORMALIZE_LAYERS,
            json!({ "device": device, "what": "volume" }),
            true,
        );
    }
    if let Some(folder) = shell.layer_folder(media) {
        run(
            ui,
            "Show in Browser",
            "browser.open_folder",
            json!({ "path": folder, "open": true, "keep_others": true }),
            true,
        );
    }
    // Manage Sample opens the File Manager, which isn't built yet.
    ui.add_enabled(false, egui::Button::new("Manage Sample"))
        .on_disabled_hover_text("Not built yet: arrives with WP-2.25 (the File Manager).");
    ui.separator();
    let range = selection.map(|(from, to)| json!({ "from": from, "to": to }));
    let edit = |kind: &str| {
        let mut args = target.clone();
        args["kind"] = json!(kind);
        if let Some(range) = &range {
            args["range"] = range.clone();
        }
        args
    };
    let has = selection.is_some();
    for (label, kind, needs) in [
        ("Trim", "trim", true),
        ("Cut", "cut", true),
        ("Reverse Selection", "reverse", false),
        ("Fade In", "fade_in", false),
        ("Fade Out", "fade_out", false),
        ("Normalize", "normalize", false),
    ] {
        run(
            ui,
            if needs || has || kind != "reverse" {
                label
            } else {
                "Reverse Audio"
            },
            bl_shell::multisampler_commands::EDIT_SAMPLE,
            edit(kind),
            has || !needs,
        );
    }
    let mut revert = target.clone();
    revert["revert"] = json!(true);
    let edited = shell
        .editor()
        .document()
        .device(device)
        .is_some_and(|d| match layer {
            Some(l) => d.layers.get(l).is_some_and(|l| !l.edits.is_empty()),
            None => !d.sample_edits.is_empty(),
        });
    run(
        ui,
        "Revert Edits",
        bl_shell::multisampler_commands::EDIT_SAMPLE,
        revert,
        edited,
    );
}
