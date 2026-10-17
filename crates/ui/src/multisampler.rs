//! The multisampler's views (WP-5.13; the multisampler spec, R1, R7, R10,
//! R11, R14, R20, R29): the six tabs in its title bar, the zone editor
//! above the Device View with its layer list and its Key, Vel and Sel
//! editors, and the Sample tab's waveform.
//!
//! Every change runs a registered command (`device.set_layer_zone`,
//! `device.set_layer`, `device.select_layers`, ...), which an agent runs
//! too. A drag on a zone or a marker is one undo step however far it goes.

use bl_model::{
    EditorSize, Layer, LayerId, LayerSettings, ReleaseMode, SamplerTab, SustainMode, Zone,
    ZoneKind,
};
use bl_shell::device_commands::{DeviceFrame, DevicePanel};
use bl_shell::multisampler_commands::MultisamplerState;
use bl_shell::{Invocation, Shell};
use egui::accesskit::Role;
use egui::{
    Align2, Id, Key, Modifiers, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    UiBuilder, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The zone editor's header: Hot-Swap Sample, Auto, Lin/Pow, RR and the
/// Key/Vel/Sel chooser, in points at 100% (R10).
pub const HEADER: f32 = 22.0;
/// The layer list's width at 100%.
pub const LIST_WIDTH: f32 = 180.0;
/// The editor's scale, above its bars.
pub const SCALE_HEIGHT: f32 = 14.0;
/// How wide a zone's edge handle is, and how big its fade handle.
const EDGE: f32 = 4.0;
const CORNER: f32 = 7.0;

/// The undo step of a drag on zones (the command's own name).
pub const ZONES_STEP: &str = "Change Zones";
/// The undo step of a drag on a marker.
pub const MARKERS_STEP: &str = "Change Sample Markers";

/// One row of the layer list and the editors: its height by the editor's
/// size.
#[must_use]
pub fn row_height(size: EditorSize) -> f32 {
    match size {
        EditorSize::Small => 10.0,
        EditorSize::Medium => 16.0,
        EditorSize::Large => 26.0,
    }
}

/// How tall the zone editor is, by size.
#[must_use]
pub fn editor_height(size: EditorSize) -> f32 {
    match size {
        EditorSize::Small => 150.0,
        EditorSize::Medium => 230.0,
        EditorSize::Large => 330.0,
    }
}

/// Where the zone editor sits when the detail area is at `detail`: above
/// the Device View, over the foot of the main view (L-DEV-007).
///
/// shortcut: it overlays the main view rather than making the detail area
/// taller; upgrade when the layout can grow the detail area.
#[must_use]
pub fn zone_editor_rect(detail: Rect, size: EditorSize) -> Rect {
    let bottom = detail.top() - 4.0;
    Rect::from_min_max(
        pos2(detail.left(), bottom - editor_height(size)),
        pos2(detail.right(), bottom),
    )
}

/// The parts of the zone editor in `rect`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Areas {
    /// Hot-Swap Sample, Auto, Lin/Pow, RR, Key/Vel/Sel.
    pub header: Rect,
    /// The rows' names.
    pub list: Rect,
    /// The editor's scale, with the selector's indicator above Sel.
    pub scale: Rect,
    /// The rows' bars.
    pub bars: Rect,
}

/// The zone editor's parts.
#[must_use]
pub fn areas(rect: Rect) -> Areas {
    let header = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + HEADER));
    let list = Rect::from_min_max(
        pos2(rect.left(), header.bottom()),
        pos2(rect.left() + LIST_WIDTH, rect.bottom()),
    );
    let scale = Rect::from_min_max(
        pos2(list.right(), header.bottom()),
        pos2(rect.right(), header.bottom() + SCALE_HEIGHT),
    );
    let bars = Rect::from_min_max(pos2(list.right(), scale.bottom()), rect.max);
    Areas {
        header,
        list,
        scale,
        bars,
    }
}

/// How many values `kind`'s scale holds.
fn values(kind: ZoneKind) -> f32 {
    let (low, high) = kind.range();
    f32::from(high - low) + 1.0
}

/// The x of the left edge of `value` on `kind`'s scale across `bars`.
#[must_use]
pub fn x_of(bars: Rect, kind: ZoneKind, value: f32) -> f32 {
    bars.left() + (value - f32::from(kind.range().0)) / values(kind) * bars.width()
}

/// The whole value at x on `kind`'s scale, kept on the scale.
#[must_use]
pub fn value_at(bars: Rect, kind: ZoneKind, x: f32) -> i32 {
    let (low, high) = kind.range();
    let raw = (x - bars.left()) / bars.width() * values(kind);
    (raw.floor() as i32 + i32::from(low)).clamp(i32::from(low), i32::from(high))
}

/// What a drag on a zone moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// The lowest value.
    Low,
    /// The highest value.
    High,
    /// The whole zone.
    Body,
    /// The low end's fade.
    FadeLow,
    /// The high end's fade.
    FadeHigh,
}

/// `zone` after its `handle` moves by `delta` values (R10): a zone keeps at
/// least one value and stays on the scale, and its fades stay inside it.
#[must_use]
pub fn drag_zone(zone: Zone, handle: Handle, delta: i32, kind: ZoneKind) -> Zone {
    let (min, max) = (i32::from(kind.range().0), i32::from(kind.range().1));
    let (low, high) = (i32::from(zone.low), i32::from(zone.high));
    let (mut fade_low, mut fade_high) = (i32::from(zone.fade_low), i32::from(zone.fade_high));
    let (mut new_low, mut new_high) = (low, high);
    match handle {
        Handle::Low => new_low = (low + delta).clamp(min, high),
        Handle::High => new_high = (high + delta).clamp(low, max),
        Handle::Body => {
            let shift = delta.clamp(min - low, max - high);
            new_low += shift;
            new_high += shift;
        }
        Handle::FadeLow => fade_low = (fade_low + delta).clamp(0, high - low - fade_high),
        Handle::FadeHigh => fade_high = (fade_high - delta).clamp(0, high - low - fade_low),
    }
    // A shorter zone takes its fades in: the low one first.
    let room = new_high - new_low;
    if fade_low + fade_high > room {
        let over = fade_low + fade_high - room;
        let from_low = over.min(fade_low);
        fade_low -= from_low;
        fade_high -= over - from_low;
    }
    let byte = |v: i32| u8::try_from(v).unwrap_or(u8::MAX);
    Zone {
        low: byte(new_low),
        high: byte(new_high),
        fade_low: byte(fade_low),
        fade_high: byte(fade_high),
    }
}

/// A key's name as the zone editor's scale reads it: C-2 for key 0, C3 for
/// key 60.
#[must_use]
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[usize::from(key % 12)], i32::from(key / 12) - 2)
}

/// Where the parts of one zone's bar are, in its row of `bars`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarParts {
    /// The zone's body, under its fade handles.
    pub body: Rect,
    /// The bar's left and right edge handles.
    pub low: Rect,
    /// See `low`.
    pub high: Rect,
    /// The fade handles at the top corners.
    pub fade_low: Rect,
    /// See `fade_low`.
    pub fade_high: Rect,
}

/// The parts of `zone`'s bar in `row`, the row's rectangle across the bars.
#[must_use]
pub fn bar_parts(bars: Rect, row: Rect, kind: ZoneKind, zone: Zone) -> BarParts {
    let left = x_of(bars, kind, f32::from(zone.low));
    let right = x_of(bars, kind, f32::from(zone.high) + 1.0).max(left + 1.0);
    let split = row.top() + row.height() * 0.4;
    let lower = Rect::from_min_max(pos2(left, split), pos2(right, row.bottom() - 1.0));
    let fade_x = |value: f32| x_of(bars, kind, value);
    let handle = |x: f32| {
        Rect::from_min_max(
            pos2(x - CORNER / 2.0, row.top()),
            pos2(x + CORNER / 2.0, split),
        )
    };
    BarParts {
        body: lower,
        low: Rect::from_min_max(lower.min, pos2((left + EDGE).min(right), lower.bottom())),
        high: Rect::from_min_max(pos2((right - EDGE).max(left), lower.top()), lower.max),
        fade_low: handle(fade_x(f32::from(zone.low) + f32::from(zone.fade_low))),
        fade_high: handle(fade_x(
            f32::from(zone.high) + 1.0 - f32::from(zone.fade_high),
        )),
    }
}

/// The ID of a zone bar's part.
#[must_use]
pub fn bar_id(device: bl_model::DeviceId, layer: LayerId, part: &str) -> Id {
    Id::new(("zone-bar", device, layer, part))
}

/// The ID of a layer's row in the list.
#[must_use]
pub fn row_id(device: bl_model::DeviceId, layer: LayerId) -> Id {
    Id::new(("layer-row", device, layer))
}

/// The ID of a zone editor header control.
#[must_use]
pub fn header_id(device: bl_model::DeviceId, name: &str) -> Id {
    Id::new(("zone-editor", device, name))
}

/// The ID of the selector's indicator.
#[must_use]
pub fn selector_id(device: bl_model::DeviceId) -> Id {
    Id::new(("zone-selector", device))
}

/// A drag on zones under way.
#[derive(Clone, Debug)]
struct ZoneDrag {
    kind: ZoneKind,
    handle: Handle,
    start_x: f32,
    layers: Vec<(LayerId, Zone)>,
    delta: i32,
}

/// A drag on one of the waveform's markers under way.
#[derive(Clone, Debug)]
struct MarkerDrag {
    layer: LayerId,
    marker: Marker,
}

/// The waveform's markers (R14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// Sample Start.
    Start,
    /// Sample End.
    End,
    /// Loop Start.
    LoopStart,
    /// Loop End.
    LoopEnd,
    /// The Release Loop point.
    ReleaseLoop,
}

impl Marker {
    const ALL: [Self; 5] = [
        Self::Start,
        Self::End,
        Self::LoopStart,
        Self::LoopEnd,
        Self::ReleaseLoop,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Start => "Sample Start",
            Self::End => "Sample End",
            Self::LoopStart => "Loop Start",
            Self::LoopEnd => "Loop End",
            Self::ReleaseLoop => "Release Loop",
        }
    }

    fn frame(self, layer: &Layer) -> u64 {
        match self {
            Self::Start => layer.start,
            Self::End => layer.end,
            Self::LoopStart => layer.loop_start,
            Self::LoopEnd => layer.loop_end,
            Self::ReleaseLoop => layer.release_loop,
        }
    }

    fn settings(self, frame: u64) -> LayerSettings {
        let mut settings = LayerSettings::default();
        match self {
            Self::Start => settings.start = Some(frame),
            Self::End => settings.end = Some(frame),
            Self::LoopStart => settings.loop_start = Some(frame),
            Self::LoopEnd => settings.loop_end = Some(frame),
            Self::ReleaseLoop => settings.release_loop = Some(frame),
        }
        settings
    }
}

/// Which channels the waveform draws (R14): B, M, L and R.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Channels {
    #[default]
    Both,
    Mono,
    Left,
    Right,
}

/// The audio the Sample tab draws, and how it's zoomed.
struct Waveform {
    media: bl_model::ContentHash,
    audio: std::sync::Arc<bl_media::SampleData>,
    /// The first frame shown, and how many frames the view spans.
    first: f64,
    span: f64,
    /// A drawing-only vertical zoom (R14).
    vertical: f32,
    channels: Channels,
}

/// The multisampler's UI state between frames.
#[derive(Default)]
pub struct MultisamplerUi {
    drag: Option<ZoneDrag>,
    marker: Option<MarkerDrag>,
    /// The layer being renamed in the list, and its text.
    renaming: Option<(LayerId, String, bool)>,
    /// The layer clicked last, where Shift-click extends from.
    anchor: Option<LayerId>,
    waveform: Option<Waveform>,
}

impl std::fmt::Debug for MultisamplerUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultisamplerUi").finish_non_exhaustive()
    }
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// A small text button: lit while `on`, in the Device View's Tab order.
/// `name` is what assistive technology reads. Returns whether it was
/// clicked, or Enter was pressed with the keyboard on it, and its response.
#[expect(clippy::too_many_arguments, reason = "a button's parts")]
fn button(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    (face_text, name): (&str, &str),
    on: bool,
    description: &str,
) -> (bool, egui::Response) {
    let response = ui.interact(rect, id, Sense::click());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    let pressed = response.clicked()
        || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
    let c = &theme.colors;
    let face = if on {
        c.accent.0
    } else if response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, theme.sizes.corner, face);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        face_text,
        theme.small_font(),
        if on { c.accent_text.0 } else { c.text.0 },
    );
    if response.has_focus() && crate::focus::outlined(ui.ctx()) {
        painter.rect_stroke(
            rect.expand(1.5),
            theme.sizes.corner,
            Stroke::new(1.0, c.focus.0),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(name.to_owned());
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
        node.set_toggled(if on {
            egui::accesskit::Toggled::True
        } else {
            egui::accesskit::Toggled::False
        });
        node.set_description(description.to_owned());
    });
    (pressed, response)
}

/// The value of a device's parameter, from its panel.
fn param_value(panel: &DevicePanel, id: bl_model::ParamId) -> f64 {
    panel
        .description
        .params
        .iter()
        .position(|p| p.id == id)
        .map_or(0.0, |i| panel.value(i))
}

impl MultisamplerUi {
    /// The six tabs in the title bar (R1): Zone, which shows and hides the
    /// zone editor, then Sample, Pitch/Osc, Filter/Global, Modulation and
    /// MIDI.
    pub fn tabs(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        title: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        const WIDTHS: [f32; 6] = [34.0, 46.0, 56.0, 76.0, 66.0, 34.0];
        let total: f32 = WIDTHS.iter().sum::<f32>() + 2.0 * 5.0;
        let mut x = (title.right() - total).max(title.left() + 70.0);
        let names = [
            ("Zone", None),
            ("Sample", Some(SamplerTab::Sample)),
            ("Pitch/Osc", Some(SamplerTab::PitchOsc)),
            ("Filter/Global", Some(SamplerTab::FilterGlobal)),
            ("Modulation", Some(SamplerTab::Modulation)),
            ("MIDI", Some(SamplerTab::Midi)),
        ];
        for ((label, tab), width) in names.into_iter().zip(WIDTHS) {
            let rect = Rect::from_min_size(pos2(x, title.top() + 1.0), vec2(width, title.height() - 2.0));
            x += width + 2.0;
            let on = match tab {
                None => state.view.zone_editor,
                Some(tab) => state.view.tab == tab,
            };
            let (pressed, _) = button(
                ui,
                theme,
                header_id(frame.id, label),
                rect,
                (label, label),
                on,
                match tab {
                    None => "Shows or hides the zone editor above the Device View",
                    Some(_) => "Shows this tab's controls in the device",
                },
            );
            if pressed {
                let args = match tab {
                    None => json!({ "device": frame.id, "zone_editor": !state.view.zone_editor }),
                    Some(tab) => json!({ "device": frame.id, "tab": tab }),
                };
                invoke(events, bl_shell::multisampler_commands::SET_VIEW, args);
            }
        }
    }

    /// The body of the device: the selected tab's controls.
    #[expect(clippy::too_many_arguments, reason = "the body's inputs")]
    pub fn body(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        panel: &DevicePanel,
        body: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        match state.view.tab {
            SamplerTab::Sample => self.sample_tab(ui, shell, theme, frame, state, panel, body, events),
            SamplerTab::FilterGlobal => {
                // The envelope and output bands of the layout.
                let mut shown = panel.clone();
                if let bl_model::device_layout::LayoutNode::Row(nodes) =
                    &panel.description.layout.root
                {
                    let keep: Vec<_> = nodes.iter().skip(2).cloned().collect();
                    shown.description.layout = bl_model::DeviceLayout::new(
                        bl_model::device_layout::LayoutNode::Row(keep),
                    );
                }
                crate::device_panel::show(ui, theme, frame, &shown, body, events);
            }
            SamplerTab::PitchOsc | SamplerTab::Modulation | SamplerTab::Midi => {
                let name = match state.view.tab {
                    SamplerTab::PitchOsc => "Pitch/Osc",
                    SamplerTab::Modulation => "Modulation",
                    _ => "MIDI",
                };
                ui.painter().text(
                    body.center(),
                    Align2::CENTER_CENTER,
                    format!("The {name} tab's controls arrive with WP-5.14."),
                    theme.small_font(),
                    theme.colors.text_dim.0,
                );
            }
        }
    }

    // ----- the Sample tab ------------------------------------------------

    #[expect(clippy::too_many_arguments, reason = "the tab's inputs")]
    fn sample_tab(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        panel: &DevicePanel,
        body: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let Some(device) = shell.multisampler_device(frame.id) else {
            return;
        };
        // The selected layer: the first selected, or the first.
        let layer = state
            .selection
            .first()
            .and_then(|id| device.layers.get(*id))
            .or_else(|| device.layers.list.first());
        let display = Rect::from_min_max(
            pos2(body.left() + 4.0, body.top() + 4.0),
            pos2(body.right() - 4.0, body.bottom() - 50.0),
        );
        let painter = ui.painter_at(body);
        painter.rect_filled(display, 0.0, c.display.0);
        painter.rect_stroke(
            display,
            0.0,
            Stroke::new(1.0, c.separator.0),
            StrokeKind::Inside,
        );
        let cell = Id::new(("device-cell", frame.id, 0usize));
        let _ = ui.interact(display, cell, Sense::hover());
        ui.ctx().accesskit_node_builder(cell, |node| {
            node.set_role(Role::Image);
            node.set_label("Sample".to_owned());
        });
        let Some(layer) = layer else {
            painter.text(
                display.center(),
                Align2::CENTER_CENTER,
                "Drop samples on the zone editor's layer list",
                theme.small_font(),
                c.text_dim.0,
            );
            return;
        };
        // The sample chooser and what the waveform shows, in a strip along
        // the display's top; the settings along the bottom.
        let strip = Rect::from_min_size(
            display.min + vec2(4.0, 2.0),
            vec2(display.width() - 8.0, 16.0),
        );
        self.waveform_strip(ui, theme, frame, device, layer, strip, events);
        let wave = Rect::from_min_max(
            pos2(display.left() + 2.0, strip.bottom() + 2.0),
            pos2(display.right() - 2.0, display.bottom() - 2.0),
        );
        self.waveform(ui, shell, theme, frame, layer, wave, events);
        self.settings_row(
            ui,
            theme,
            frame,
            panel,
            layer,
            &device.layers,
            Rect::from_min_max(pos2(body.left() + 4.0, display.bottom() + 4.0), body.max),
            events,
        );
    }

    /// The Sample chooser, B/M/L/R and the vertical zoom (R14).
    #[expect(clippy::too_many_arguments, reason = "the strip's inputs")]
    fn waveform_strip(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        device: &bl_model::Device,
        layer: &Layer,
        strip: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        // The Sample chooser: the selected layer's name, opening a menu of
        // every layer.
        let chooser = Rect::from_min_size(strip.min, vec2(150.0, strip.height()));
        let id = header_id(frame.id, "sample-chooser");
        let response = ui.interact(chooser, id, Sense::click());
        let painter = ui.painter_at(chooser.expand(1.0));
        painter.rect_filled(
            chooser,
            theme.sizes.corner,
            if response.hovered() {
                theme.colors.control_hover.0
            } else {
                theme.colors.control.0
            },
        );
        painter.text(
            pos2(chooser.left() + 4.0, chooser.center().y),
            Align2::LEFT_CENTER,
            format!("{} ▾", layer.name),
            theme.small_font(),
            theme.colors.text.0,
        );
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Sample Chooser"));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label("Sample Chooser".to_owned());
            node.set_value(layer.name.clone());
            node.set_description("Chooses which layer the tab shows".to_owned());
        });
        egui::Popup::menu(&response).show(|ui| {
            for other in &device.layers.list {
                if ui.selectable_label(other.id == layer.id, &other.name).clicked() {
                    invoke(
                        events,
                        bl_shell::multisampler_commands::SELECT_LAYERS,
                        json!({ "device": frame.id, "layers": [other.id] }),
                    );
                    ui.close();
                }
            }
        });
        // B, M, L and R.
        let mut x = strip.right() - 4.0 * 20.0;
        for (label, channels, hint) in [
            ("B", Channels::Both, "Draws both channels"),
            ("M", Channels::Mono, "Draws the channels' sum"),
            ("L", Channels::Left, "Draws the left channel"),
            ("R", Channels::Right, "Draws the right channel"),
        ] {
            let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(18.0, strip.height()));
            x += 20.0;
            let current = self.waveform.as_ref().map_or(Channels::Both, |w| w.channels);
            if button(
                ui,
                theme,
                header_id(frame.id, label),
                rect,
                (label, label),
                current == channels,
                hint,
            )
            .0
                && let Some(waveform) = &mut self.waveform
            {
                waveform.channels = channels;
            }
        }
    }

    /// The waveform with its markers (R14), zoomed by Cmd or Ctrl and the
    /// wheel.
    #[expect(clippy::too_many_arguments, reason = "the waveform's inputs")]
    fn waveform(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        layer: &Layer,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        if self.waveform.as_ref().is_none_or(|w| w.media != layer.media) {
            self.waveform = shell.layer_audio(layer.media).map(|audio| Waveform {
                media: layer.media,
                first: 0.0,
                span: audio.frames().max(1) as f64,
                audio,
                vertical: 1.0,
                channels: Channels::Both,
            });
        }
        let Some(wave) = &mut self.waveform else {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "The sample's file is missing",
                theme.small_font(),
                c.text_dim.0,
            );
            return;
        };
        let id = Id::new(("waveform", frame.id));
        let response = ui.interact(rect, id, Sense::hover());
        // Cmd or Ctrl with the wheel zooms the time, about the pointer.
        if response.hovered() {
            // egui turns Cmd or Ctrl with the wheel (and a pinch) into a zoom.
            let (zoom, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.latest_pos()));
            if (zoom - 1.0).abs() > f32::EPSILON {
                let total = wave.audio.frames().max(1) as f64;
                let at = pointer.map_or(0.5, |p| f64::from((p.x - rect.left()) / rect.width()));
                let anchor = wave.first + wave.span * at;
                wave.span = (wave.span / f64::from(zoom)).clamp(64.0, total);
                wave.first = (anchor - wave.span * at).clamp(0.0, total - wave.span);
            }
            crate::mixer::show_readout(
                ui.ctx(),
                format!(
                    "{}: {} frames, {} Hz, {} channel{}",
                    layer.name,
                    layer.length,
                    layer.sample_rate,
                    layer.channels,
                    if layer.channels == 1 { "" } else { "s" }
                ),
            );
        }
        let painter = ui.painter_at(rect);
        let frames_per_point = wave.span / f64::from(rect.width().max(1.0));
        let conv = wave.audio.frames() as f64 / layer.length.max(1) as f64;
        let channel_data: Vec<&[f32]> = match wave.channels {
            Channels::Both | Channels::Mono => {
                (0..wave.audio.channel_count()).filter_map(|i| wave.audio.channel(i)).collect()
            }
            Channels::Left => wave.audio.channel(0).into_iter().collect(),
            Channels::Right => wave
                .audio
                .channel(1)
                .or_else(|| wave.audio.channel(0))
                .into_iter()
                .collect(),
        };
        let both = wave.channels == Channels::Both && channel_data.len() > 1;
        let lanes = if both { 2 } else { 1 };
        let lane_height = rect.height() / lanes as f32;
        for lane in 0..lanes {
            let centre = rect.top() + lane_height * (lane as f32 + 0.5);
            let half = lane_height * 0.45 * wave.vertical;
            for px in 0..rect.width() as usize {
                let from = (wave.first + px as f64 * frames_per_point) as usize;
                let to = ((wave.first + (px as f64 + 1.0) * frames_per_point) as usize).max(from + 1);
                let (mut low, mut high) = (0.0f32, 0.0f32);
                let sum = wave.channels == Channels::Mono;
                for frame_index in from..to.min(wave.audio.frames()) {
                    let value = if sum {
                        channel_data.iter().map(|d| d[frame_index]).sum::<f32>()
                            / channel_data.len() as f32
                    } else if both {
                        channel_data[lane][frame_index]
                    } else {
                        channel_data[0][frame_index]
                    };
                    low = low.min(value);
                    high = high.max(value);
                }
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
        // The markers, each a handle dragged as one undo step.
        let x_of_frame = |frame_in_file: u64| {
            rect.left() + ((frame_in_file as f64 * conv - wave.first) / frames_per_point) as f32
        };
        for marker in Marker::ALL {
            if matches!(marker, Marker::LoopStart | Marker::LoopEnd) && layer.sustain == SustainMode::Off {
                continue;
            }
            if marker == Marker::ReleaseLoop
                && !(layer.sustain != SustainMode::Off
                    && matches!(layer.release, ReleaseMode::Loop | ReleaseMode::BackAndForth))
            {
                continue;
            }
            let x = x_of_frame(marker.frame(layer));
            if x < rect.left() - 4.0 || x > rect.right() + 4.0 {
                continue;
            }
            let handle = Rect::from_min_max(pos2(x - 4.0, rect.top()), pos2(x + 4.0, rect.bottom()));
            let id = Id::new(("marker", frame.id, marker.name()));
            let response = ui.interact(handle, id, Sense::drag());
            let colour = match marker {
                Marker::Start | Marker::End => c.text.0,
                _ => c.focus.0,
            };
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(if response.hovered() { 2.0 } else { 1.0 }, colour),
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
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Slider);
                node.set_label(marker.name().to_owned());
                node.set_value(marker.frame(layer).to_string());
                node.set_bounds(egui::accesskit::Rect {
                    x0: handle.min.x.into(),
                    y0: handle.min.y.into(),
                    x1: handle.max.x.into(),
                    y1: handle.max.y.into(),
                });
            });
            if response.drag_started() {
                self.marker = Some(MarkerDrag {
                    layer: layer.id,
                    marker,
                });
                events.push(UiEvent::BeginGesture(MARKERS_STEP));
            }
            if response.dragged()
                && let Some(drag) = self.marker.as_ref().filter(|d| d.marker == marker)
                && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
            {
                let in_audio = wave.first + f64::from(pointer.x - rect.left()) * frames_per_point;
                let frame_in_file = (in_audio / conv).round().clamp(0.0, layer.length as f64) as u64;
                if frame_in_file != marker.frame(layer) {
                    invoke(
                        events,
                        bl_shell::multisampler_commands::SET_LAYER,
                        json!({ "device": frame.id, "layers": [drag.layer],
                                "settings": marker.settings(frame_in_file) }),
                    );
                }
            }
            if response.drag_stopped() {
                self.marker = None;
                events.push(UiEvent::EndGesture);
            }
            // A loop's marker menu snaps that loop's markers once (R20).
            if matches!(
                marker,
                Marker::LoopStart | Marker::LoopEnd | Marker::ReleaseLoop
            ) {
                response.context_menu(|ui| {
                    if ui.button("Snap Markers").clicked() {
                        let which = if marker == Marker::ReleaseLoop {
                            "release"
                        } else {
                            "sustain"
                        };
                        invoke(
                            events,
                            bl_shell::multisampler_commands::SNAP_LAYER_MARKERS,
                            json!({ "device": frame.id, "layers": [layer.id], "loop": which }),
                        );
                        ui.close();
                    }
                });
            }
        }
    }

    /// The settings along the Sample tab's foot: Root Key, Detune, Volume,
    /// Pan, the markers' values and the loop modes (R15 to R19).
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn settings_row(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        panel: &DevicePanel,
        layer: &Layer,
        options: &bl_model::Layers,
        area: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        use crate::widgets::{Choice, chooser, number_box};
        let device = frame.id;
        let set = |events: &mut Vec<UiEvent>, settings: LayerSettings| {
            invoke(
                events,
                bl_shell::multisampler_commands::SET_LAYER,
                json!({ "device": device, "layers": [layer.id], "settings": settings }),
            );
        };
        let mut x = area.left();
        let top = area.top();
        let step = |label: &str,
                    ui: &mut Ui,
                    x: &mut f32,
                    row: f32,
                    width: f32,
                    value: f64,
                    range: (f64, f64),
                    stepping: &dyn Fn(f64, i32) -> f64,
                    text: String|
         -> Option<f64> {
            let rect = Rect::from_min_size(pos2(*x, top + row), vec2(width, 18.0));
            *x += width + 4.0;
            // In the Device View's Tab order, so the keyboard reaches it.
            crate::focus::register(
                ui.ctx(),
                Id::new(("number-box", format!("{device:?}-{label}"))),
                bl_shell::Area::Detail,
                None,
            );
            number_box(
                ui,
                theme,
                &format!("{device:?}-{label}"),
                label,
                rect,
                value,
                range,
                stepping,
                &format!("{label} {text}"),
            )
        };
        let db = |gain: f64| {
            if gain <= 0.0 {
                "-inf dB".to_owned()
            } else {
                format!("{:.1} dB", 20.0 * gain.log10())
            }
        };
        if let Some(root) = step(
            "Root",
            ui,
            &mut x,
            0.0,
            62.0,
            f64::from(layer.root_key),
            (0.0, 127.0),
            &|v, n| v + f64::from(n),
            note_name(layer.root_key),
        ) {
            set(events, LayerSettings { root_key: Some(root as u8), ..Default::default() });
        }
        if let Some(detune) = step(
            "Detune",
            ui,
            &mut x,
            0.0,
            78.0,
            layer.detune,
            (-50.0, 50.0),
            &|v, n| v + f64::from(n),
            format!("{:.0} ct", layer.detune),
        ) {
            set(events, LayerSettings { detune: Some(detune), ..Default::default() });
        }
        if let Some(volume) = step(
            "Volume",
            ui,
            &mut x,
            0.0,
            92.0,
            layer.volume,
            (0.0, bl_model::LAYER_GAIN_MAX),
            &|v, n| {
                let db = if v <= 0.0 { -60.0 } else { 20.0 * v.log10() };
                let next = db + f64::from(n) * 0.5;
                if next <= -60.0 { 0.0 } else { 10f64.powf(next / 20.0) }
            },
            db(layer.volume),
        ) {
            set(events, LayerSettings { volume: Some(volume), ..Default::default() });
        }
        if let Some(pan) = step(
            "Pan",
            ui,
            &mut x,
            0.0,
            62.0,
            layer.pan,
            (-50.0, 50.0),
            &|v, n| v + f64::from(n),
            format!("{:.0}", layer.pan),
        ) {
            set(events, LayerSettings { pan: Some(pan), ..Default::default() });
        }
        // Reverse and Interpolation are the device's parameters.
        let reverse = param_value(panel, bl_devices::multisampler::REVERSE) >= 0.5;
        let rect = Rect::from_min_size(pos2(x, top), vec2(54.0, 18.0));
        x += 58.0;
        if button(
            ui,
            theme,
            header_id(device, "reverse"),
            rect,
            ("Reverse", "Reverse"),
            reverse,
            "Plays every layer backwards, from Sample End to Sample Start",
        )
        .0
        {
            invoke(
                events,
                "device.set_param",
                json!({ "device": device, "param": bl_devices::multisampler::REVERSE.0,
                        "value": f64::from(u8::from(!reverse)) }),
            );
        }
        let interpolation = param_value(panel, bl_devices::multisampler::INTERPOLATION) as usize;
        let choices: Vec<Choice<usize>> = bl_devices::multisampler::INTERPOLATIONS
            .iter()
            .enumerate()
            .map(|(i, name)| Choice { label: (*name).to_owned(), value: i })
            .collect();
        let rect = Rect::from_min_size(pos2(x, top), vec2(74.0, 18.0));
        x += 78.0;
        if let Some(chosen) = chooser(
            ui,
            theme,
            &format!("{device:?}-interpolation"),
            "Interpolation",
            rect,
            &choices,
            Some(&interpolation),
            true,
        ) {
            invoke(
                events,
                "device.set_param",
                json!({ "device": device, "param": bl_devices::multisampler::INTERPOLATION.0, "value": chosen }),
            );
        }
        // Snap and RAM Mode are options of the layer list, not parameters.
        for (label, on, key, hint) in [
            ("Snap", options.snap, "snap", "Markers set land on zero crossings of the left channel"),
            ("RAM", options.ram_mode, "ram_mode", "Holds every layer whole in memory"),
        ] {
            let rect = Rect::from_min_size(pos2(x, top), vec2(36.0, 18.0));
            x += 40.0;
            if button(ui, theme, header_id(device, label), rect, (label, label), on, hint).0 {
                invoke(
                    events,
                    "device.set_layer_options",
                    json!({ "device": device, key: !on }),
                );
            }
        }
        let _ = x;
        // The second row: the markers and the loops.
        let mut x = area.left();
        let row = 22.0;
        let frames_text = |frames: u64| format!("{frames}");
        for (label, value, marker) in [
            ("Start", layer.start, Marker::Start),
            ("End", layer.end, Marker::End),
            ("Loop Start", layer.loop_start, Marker::LoopStart),
            ("Loop End", layer.loop_end, Marker::LoopEnd),
        ] {
            let length = layer.length as f64;
            if let Some(next) = step(
                label,
                ui,
                &mut x,
                row,
                96.0,
                value as f64,
                (0.0, length),
                &|v, n| v + f64::from(n) * (length / 1000.0).max(1.0),
                frames_text(value),
            ) {
                set(events, marker.settings(next.round() as u64));
            }
        }
        let rect = Rect::from_min_size(pos2(x, top + row), vec2(38.0, 18.0));
        x += 42.0;
        if button(
            ui,
            theme,
            header_id(device, "link"),
            rect,
            ("Link", "Link"),
            layer.link,
            "Starts the voice at Loop Start instead of Sample Start",
        )
        .0
        {
            set(events, LayerSettings { link: Some(!layer.link), ..Default::default() });
        }
        let sustains = [
            ("No Sustain Loop", SustainMode::Off),
            ("Sustain Loop", SustainMode::Loop),
            ("Back-and-Forth", SustainMode::BackAndForth),
        ];
        let choices: Vec<Choice<SustainMode>> = sustains
            .iter()
            .map(|(label, value)| Choice { label: (*label).to_owned(), value: *value })
            .collect();
        let rect = Rect::from_min_size(pos2(x, top + row), vec2(110.0, 18.0));
        x += 114.0;
        if let Some(mode) = chooser(
            ui,
            theme,
            &format!("{device:?}-sustain"),
            "Sustain Mode",
            rect,
            &choices,
            Some(&layer.sustain),
            true,
        ) {
            set(events, LayerSettings { sustain: Some(mode), ..Default::default() });
        }
        let releases = [
            ("Release Off", ReleaseMode::Off),
            ("Release", ReleaseMode::Release),
            ("Release Loop", ReleaseMode::Loop),
            ("Back-and-Forth Release", ReleaseMode::BackAndForth),
        ];
        let choices: Vec<Choice<ReleaseMode>> = releases
            .iter()
            .map(|(label, value)| Choice { label: (*label).to_owned(), value: *value })
            .collect();
        let rect = Rect::from_min_size(pos2(x, top + row), vec2(140.0, 18.0));
        if layer.sustain != SustainMode::Off
            && let Some(mode) = chooser(
                ui,
                theme,
                &format!("{device:?}-release"),
                "Release Mode",
                rect,
                &choices,
                Some(&layer.release),
                true,
            )
        {
            set(events, LayerSettings { release: Some(mode), ..Default::default() });
        }
    }

    // ----- the zone editor -----------------------------------------------

    /// Draws the zone editor in `rect` for the multisampler `frame` and
    /// turns what the person does into commands (R7, R10, R11).
    pub fn zone_editor(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(device) = shell.multisampler_device(frame.id) else {
            return;
        };
        let Some(panel) = frame.panel.as_ref() else {
            return;
        };
        let c = &theme.colors;
        let parts = areas(rect);
        let region = Id::new(("zone-editor-region", frame.id));
        let _ = ui.interact(rect, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Zone Editor".to_owned());
            node.set_bounds(egui::accesskit::Rect {
                x0: rect.min.x.into(),
                y0: rect.min.y.into(),
                x1: rect.max.x.into(),
                y1: rect.max.y.into(),
            });
        });
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, c.panel.0);
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, c.separator.0), StrokeKind::Inside);
        self.header(ui, shell, theme, frame, state, panel, parts.header, events);

        let kind = state.view.editor;
        // The scale, with the selector's indicator above Sel.
        self.scale(ui, theme, frame, panel, kind, parts, events);

        // The rows: the list's names and the bars, scrolling together.
        let row_h = row_height(state.view.size);
        let body = Rect::from_min_max(pos2(parts.list.left(), parts.bars.top()), rect.max);
        let mut inner = ui.new_child(UiBuilder::new().max_rect(body).accessibility_parent(region));
        inner.set_clip_rect(body);
        let layers = &device.layers.list;
        egui::ScrollArea::vertical()
            .id_salt(("zone-rows", frame.id))
            .auto_shrink([false, false])
            .show_rows(&mut inner, row_h, layers.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in range {
                    let layer = &layers[index];
                    let (row, _) = ui.allocate_exact_size(vec2(body.width(), row_h), Sense::hover());
                    let list_row = Rect::from_min_size(row.min, vec2(LIST_WIDTH, row_h));
                    let bars_row = Rect::from_min_max(pos2(parts.bars.left(), row.top()), row.max);
                    self.list_row(ui, shell, theme, frame, state, device, layer, index, list_row, events);
                    self.bar_row(ui, theme, frame, state, device, layer, parts.bars, bars_row, events);
                }
            });
        if layers.is_empty() {
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                "Drop samples here to add them as layers",
                theme.small_font(),
                c.text_dim.0,
            );
        }
        self.update_drag(ui, frame, parts.bars, events);
        self.drop_files(ui, frame, layers.len(), parts, row_h, rect, events);
    }

    /// Carries a drag on zones on: the zones follow the pointer, each as the
    /// drag's handle moves it, and the release ends the one undo step.
    fn update_drag(
        &mut self,
        ui: &Ui,
        frame: &DeviceFrame,
        bars: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let (pointer, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        if let Some(pointer) = pointer {
            let per_value = bars.width() / values(drag.kind);
            let delta = ((pointer.x - drag.start_x) / per_value).round() as i32;
            if delta != drag.delta {
                drag.delta = delta;
                let zones: Vec<_> = drag
                    .layers
                    .iter()
                    .map(|(layer, zone)| {
                        json!({ "layer": layer, "zone": drag_zone(*zone, drag.handle, delta, drag.kind) })
                    })
                    .collect();
                invoke(
                    events,
                    "device.set_layer_zone",
                    json!({ "device": frame.id, "kind": drag.kind, "zones": zones }),
                );
            }
        }
        if !down {
            self.drag = None;
            events.push(UiEvent::EndGesture);
        }
    }

    /// Files dropped on the layer list or the editor add layers at the drop
    /// point (R3).
    #[expect(clippy::too_many_arguments, reason = "the drop's inputs")]
    fn drop_files(
        &mut self,
        ui: &Ui,
        frame: &DeviceFrame,
        count: usize,
        parts: Areas,
        row_h: f32,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let (dropped, pointer) = ui.input(|i| {
            (
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                i.pointer.latest_pos(),
            )
        });
        let Some(pointer) = pointer.filter(|p| rect.contains(*p)) else {
            return;
        };
        if dropped.is_empty() {
            return;
        }
        // The row the pointer is over, counted in rows from the top of the
        // list as scrolled (the first row sits at the list's top).
        let index = (((pointer.y - parts.bars.top()) / row_h).max(0.0) as usize).min(count);
        invoke(
            events,
            bl_shell::multisampler_commands::ADD_LAYERS,
            json!({ "device": frame.id, "paths": dropped, "index": index }),
        );
    }

    /// Hot-Swap Sample, Auto, Lin/Pow, RR with its modes, and the
    /// Key/Vel/Sel chooser.
    #[expect(clippy::too_many_arguments, reason = "the header's inputs")]
    fn header(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        panel: &DevicePanel,
        header: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        use bl_devices::multisampler::{FADE_MODE, RESET_INTERVAL, ROUND_ROBIN, RR_MODE};
        let mut x = header.left() + 4.0;
        let top = header.top() + 2.0;
        let h = header.height() - 4.0;
        fn put(x: &mut f32, top: f32, h: f32, width: f32) -> Rect {
            let rect = Rect::from_min_size(pos2(*x, top), vec2(width, h));
            *x += width + 3.0;
            rect
        }
        let device = frame.id;
        // Hot-Swap Sample: always shown; opens the selected layer's folder
        // in the browser, where the sample chosen replaces the layer's (R10).
        let hot = put(&mut x, top, h, 26.0);
        let layer = state.selection.first().copied();
        if button(
            ui,
            theme,
            header_id(device, "hot-swap"),
            hot,
            ("⟳", "Hot-Swap Sample"),
            false,
            "Opens the selected layer's folder in the browser; the sample chosen there \
             replaces the layer's sample",
        )
        .0
            && let Some(layer) = layer
        {
            invoke(
                events,
                bl_shell::multisampler_commands::HOT_SWAP_LAYER,
                json!({ "device": device, "layer": layer }),
            );
            if let Some(folder) = shell
                .multisampler_device(device)
                .and_then(|d| d.layers.get(layer))
                .and_then(|l| shell.layer_folder(l.media))
            {
                invoke(
                    events,
                    "browser.open_folder",
                    json!({ "path": folder, "open": true, "keep_others": true }),
                );
            }
        }
        x += 8.0;
        let auto = put(&mut x, top, h, 34.0);
        if button(
            ui,
            theme,
            header_id(device, "auto"),
            auto,
            ("Auto", "Auto"),
            state.view.auto_select,
            "Auto Select: each incoming note selects the layers it plays",
        )
        .0
        {
            invoke(
                events,
                bl_shell::multisampler_commands::SET_VIEW,
                json!({ "device": device, "auto_select": !state.view.auto_select }),
            );
        }
        let fade = param_value(panel, FADE_MODE) as usize;
        for (label, value) in [("Lin", 0usize), ("Pow", 1)] {
            let rect = put(&mut x, top, h, 30.0);
            if button(
                ui,
                theme,
                header_id(device, label),
                rect,
                (label, label),
                fade == value,
                "Every zone's fades: linear or constant-power",
            )
            .0
            {
                invoke(
                    events,
                    "device.set_param",
                    json!({ "device": device, "param": FADE_MODE.0, "value": value }),
                );
            }
        }
        let rr_on = param_value(panel, ROUND_ROBIN) >= 0.5;
        let rr = put(&mut x, top, h, 30.0);
        let (pressed, response) = button(
            ui,
            theme,
            header_id(device, "rr"),
            rr,
            ("RR", "RR"),
            rr_on,
            "Round Robin: layers that share a key zone take turns",
        );
        if pressed {
            invoke(
                events,
                "device.set_param",
                json!({ "device": device, "param": ROUND_ROBIN.0, "value": f64::from(u8::from(!rr_on)) }),
            );
        }
        // Right-click for the mode and the reset interval.
        response.context_menu(|ui| {
            let mode = param_value(panel, RR_MODE) as usize;
            for (i, name) in bl_devices::multisampler::RR_MODES.iter().enumerate() {
                if ui.selectable_label(mode == i, *name).clicked() {
                    invoke(
                        events,
                        "device.set_param",
                        json!({ "device": device, "param": RR_MODE.0, "value": i }),
                    );
                    ui.close();
                }
            }
            ui.separator();
            let reset = param_value(panel, RESET_INTERVAL) as usize;
            for (i, name) in bl_devices::multisampler::RESET_INTERVALS.iter().enumerate() {
                let label = if i == 0 { "Reset Interval: None" } else { name };
                if ui.selectable_label(reset == i, label).clicked() {
                    invoke(
                        events,
                        "device.set_param",
                        json!({ "device": device, "param": RESET_INTERVAL.0, "value": i }),
                    );
                    ui.close();
                }
            }
        });
        x += 8.0;
        for (label, kind) in [
            ("Key", ZoneKind::Key),
            ("Vel", ZoneKind::Velocity),
            ("Sel", ZoneKind::Select),
        ] {
            let rect = put(&mut x, top, h, 32.0);
            if button(
                ui,
                theme,
                header_id(device, label),
                rect,
                (label, label),
                state.view.editor == kind,
                "Shows this zone editor",
            )
            .0
            {
                invoke(
                    events,
                    bl_shell::multisampler_commands::SET_VIEW,
                    json!({ "device": device, "editor": kind }),
                );
            }
        }
    }

    /// The editor's scale and, over Sel, the selector's indicator (R6).
    #[expect(clippy::too_many_arguments, reason = "the scale's inputs")]
    fn scale(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        panel: &DevicePanel,
        kind: ZoneKind,
        parts: Areas,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let painter = ui.painter_at(parts.scale.expand2(vec2(0.0, 1.0)));
        let (low, high) = kind.range();
        let step = match kind {
            ZoneKind::Key => 12,
            _ => 16,
        };
        for value in (low..=high).filter(|v| {
            if kind == ZoneKind::Velocity {
                (*v == 1) || v % step == 0
            } else {
                v % step == 0
            }
        }) {
            let x = x_of(parts.bars, kind, f32::from(value));
            painter.line_segment(
                [pos2(x, parts.scale.bottom() - 4.0), pos2(x, parts.scale.bottom())],
                Stroke::new(1.0, c.text_dim.0),
            );
            let text = if kind == ZoneKind::Key {
                note_name(value)
            } else {
                value.to_string()
            };
            painter.text(
                pos2(x + 2.0, parts.scale.center().y),
                Align2::LEFT_CENTER,
                text,
                theme.small_font(),
                c.text_dim.0,
            );
        }
        if kind == ZoneKind::Select {
            let selector = param_value(panel, bl_devices::multisampler::SELECTOR);
            let x = x_of(parts.bars, kind, selector as f32 + 0.5);
            let handle = Rect::from_center_size(
                pos2(x, parts.scale.center().y),
                vec2(10.0, parts.scale.height()),
            );
            let id = selector_id(frame.id);
            let response = ui.interact(handle, id, Sense::drag());
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 5.0, parts.scale.top()),
                    pos2(x + 5.0, parts.scale.top()),
                    pos2(x, parts.scale.bottom()),
                ],
                c.focus.0,
                Stroke::NONE,
            ));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Slider);
                node.set_label("Sample Selector".to_owned());
                node.set_value(format!("{selector:.0}"));
            });
            if response.drag_started() {
                events.push(UiEvent::BeginNamedGesture("Change \"Selector\"".to_owned()));
            }
            if response.dragged()
                && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
            {
                let value = value_at(parts.bars, kind, pointer.x);
                if f64::from(value) != selector {
                    invoke(
                        events,
                        "device.set_param",
                        json!({ "device": frame.id, "param": bl_devices::multisampler::SELECTOR.0, "value": value }),
                    );
                }
            }
            if response.drag_stopped() {
                events.push(UiEvent::EndGesture);
            }
        }
    }

    /// One layer's name in the list: clicks select, a double-click renames,
    /// the menu holds the list's items (R11).
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn list_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        device: &bl_model::Device,
        layer: &Layer,
        index: usize,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let selected = state.selection.contains(&layer.id);
        let id = row_id(frame.id, layer.id);
        if let Some((renaming, text, focused)) = self.renaming.as_mut()
            && *renaming == layer.id
        {
            let edit = ui.put(
                rect,
                egui::TextEdit::singleline(text).font(theme.small_font()).margin(vec2(2.0, 0.0)),
            );
            if !*focused {
                edit.request_focus();
                *focused = true;
            }
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if edit.lost_focus() || enter || escape {
                if enter || (edit.lost_focus() && !escape) {
                    invoke(
                        events,
                        "device.rename_layer",
                        json!({ "device": frame.id, "layer": layer.id, "name": text }),
                    );
                }
                self.renaming = None;
            }
            return;
        }
        let response = ui.interact(rect, id, Sense::click());
        crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
        // The keyboard on a row: Enter selects it.
        if response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            invoke(
                events,
                bl_shell::multisampler_commands::SELECT_LAYERS,
                json!({ "device": frame.id, "layers": [layer.id] }),
            );
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            0.0,
            if selected {
                c.accent.0
            } else if index % 2 == 0 {
                c.lane.0
            } else {
                c.panel.0
            },
        );
        painter.text(
            pos2(rect.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            &layer.name,
            theme.small_font(),
            if selected { c.accent_text.0 } else { c.text.0 },
        );
        if response.hovered() {
            let zone = layer.zone(state.view.editor);
            crate::mixer::show_readout(
                ui.ctx(),
                format!(
                    "{}: {}, {}",
                    layer.name,
                    sample_name(shell, layer),
                    zone_text(state.view.editor, zone)
                ),
            );
        }
        if response.has_focus() && crate::focus::outlined(ui.ctx()) {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, c.focus.0),
                StrokeKind::Inside,
            );
        }
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::ListItem);
            node.set_label(layer.name.clone());
            node.set_selected(selected);
        });
        if response.clicked() {
            let modifiers = ui.input(|i| i.modifiers);
            let (layers, mode) = clicked(device, state, self.anchor, layer.id, modifiers);
            if !modifiers.shift {
                self.anchor = Some(layer.id);
            }
            invoke(
                events,
                bl_shell::multisampler_commands::SELECT_LAYERS,
                json!({ "device": frame.id, "layers": layers, "mode": mode }),
            );
        }
        if response.double_clicked() {
            self.renaming = Some((layer.id, layer.name.clone(), false));
        }
        response.context_menu(|ui| {
            self.list_menu(ui, shell, frame, state, layer, events);
        });
    }

    /// The layer list's context menu, in the order of the manual (R11).
    fn list_menu(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        layer: &Layer,
        events: &mut Vec<UiEvent>,
    ) {
        let device = frame.id;
        // The menu acts on the selection, which the click that opened it
        // made when it wasn't one of the selected layers.
        let layers: Vec<LayerId> = if state.selection.contains(&layer.id) {
            state.selection.clone()
        } else {
            vec![layer.id]
        };
        fn run(
            ui: &mut Ui,
            events: &mut Vec<UiEvent>,
            label: &str,
            id: &str,
            args: serde_json::Value,
        ) {
            if ui.button(label).clicked() {
                invoke(events, id, args);
                ui.close();
            }
        }
        run(ui, events, "Delete", "device.delete_layers", json!({ "device": device, "layers": layers }));
        run(ui, events, "Duplicate", "device.duplicate_layers", json!({ "device": device, "layers": layers }));
        if ui.button("Rename").clicked() {
            self.renaming = Some((layer.id, layer.name.clone(), false));
            ui.close();
        }
        ui.separator();
        let editor = state.view.editor;
        run(
            ui,
            events,
            "Distribute Ranges Equally",
            "device.distribute_layers",
            json!({ "device": device, "layers": layers, "how": "equally", "kind": editor }),
        );
        run(
            ui,
            events,
            "Distribute Ranges Around Root Key",
            "device.distribute_layers",
            json!({ "device": device, "layers": layers, "how": "around_root_key", "kind": "key" }),
        );
        ui.separator();
        for (label, size) in [
            ("Small", EditorSize::Small),
            ("Medium", EditorSize::Medium),
            ("Large", EditorSize::Large),
        ] {
            run(
                ui,
                events,
                label,
                bl_shell::multisampler_commands::SET_VIEW,
                json!({ "device": device, "size": size }),
            );
        }
        ui.separator();
        if let Some(folder) = shell.layer_folder(layer.media)
            && ui.button("Show in Browser").clicked()
        {
            invoke(
                events,
                "browser.open_folder",
                json!({ "path": folder, "open": true, "keep_others": true }),
            );
            ui.close();
        }
        // Manage Sample opens the File Manager, which isn't built yet.
        ui.add_enabled(false, egui::Button::new("Manage Sample"))
            .on_disabled_hover_text("Not built yet: arrives with WP-2.25 (the File Manager).");
        run(
            ui,
            events,
            "Normalize Volume",
            bl_shell::multisampler_commands::NORMALIZE_LAYERS,
            json!({ "device": device, "layers": layers, "what": "volume" }),
        );
        run(
            ui,
            events,
            "Normalize Pan",
            bl_shell::multisampler_commands::NORMALIZE_LAYERS,
            json!({ "device": device, "layers": layers, "what": "pan" }),
        );
        run(
            ui,
            events,
            "Select All With Same Range",
            bl_shell::multisampler_commands::SELECT_LAYERS,
            json!({ "device": device, "layers": [layer.id], "mode": "same_range" }),
        );
        ui.separator();
        for (label, by, descending) in [
            ("Sort Alphabetically, Ascending", "name", false),
            ("Sort Alphabetically, Descending", "name", true),
            ("Sort by Key, Ascending", "key", false),
            ("Sort by Key, Descending", "key", true),
            ("Sort by Velocity, Ascending", "velocity", false),
            ("Sort by Velocity, Descending", "velocity", true),
            ("Sort by Selector, Ascending", "selector", false),
            ("Sort by Selector, Descending", "selector", true),
        ] {
            run(
                ui,
                events,
                label,
                "device.sort_layers",
                json!({ "device": device, "by": by, "descending": descending }),
            );
        }
    }

    /// One layer's bar in the editor in view, with its fades and handles.
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn bar_row(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        frame: &DeviceFrame,
        state: &MultisamplerState,
        device: &bl_model::Device,
        layer: &Layer,
        bars: Rect,
        row: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let c = &theme.colors;
        let kind = state.view.editor;
        let zone = layer.zone(kind);
        let selected = state.selection.contains(&layer.id);
        let parts = bar_parts(bars, row, kind, zone);
        let painter = ui.painter_at(row);
        // The zone's shape: its fades slope up from the silent edges.
        let (left, right) = (parts.body.left(), parts.body.right());
        let fade_left = x_of(bars, kind, f32::from(zone.low) + f32::from(zone.fade_low));
        let fade_right = x_of(bars, kind, f32::from(zone.high) + 1.0 - f32::from(zone.fade_high));
        let (top, bottom) = (row.top() + 2.0, row.bottom() - 1.0);
        let colour = if selected { c.accent.0 } else { c.control_hover.0 };
        painter.add(Shape::convex_polygon(
            vec![
                pos2(left, bottom),
                pos2(fade_left.max(left), top),
                pos2(fade_right.min(right).max(fade_left), top),
                pos2(right, bottom),
            ],
            colour,
            Stroke::new(1.0, if selected { c.accent_text.0 } else { c.separator.0 }),
        ));
        let name = format!("{} {}", layer.name, zone_text(kind, zone));
        // Later widgets sit above earlier ones where they overlap: the body
        // first, then the edges over it, then the fade handles.
        for (part, rect, handle) in [
            ("body", parts.body, Handle::Body),
            ("low", parts.low, Handle::Low),
            ("high", parts.high, Handle::High),
            ("fade-low", parts.fade_low, Handle::FadeLow),
            ("fade-high", parts.fade_high, Handle::FadeHigh),
        ] {
            let id = bar_id(frame.id, layer.id, part);
            let response = ui.interact(rect, id, Sense::click_and_drag());
            if matches!(handle, Handle::Low | Handle::High) {
                let _ = response.clone().on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            }
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Slider);
                node.set_label(format!("{name}, {part}"));
            });
            if response.clicked() && !state.selection.contains(&layer.id) {
                invoke(
                    events,
                    bl_shell::multisampler_commands::SELECT_LAYERS,
                    json!({ "device": frame.id, "layers": [layer.id] }),
                );
            }
            if response.drag_started()
                && let Some(pointer) = ui.input(|i| i.pointer.press_origin())
            {
                // A drag acts on every selected layer alike; a layer that
                // isn't selected is the drag's own (and becomes the
                // selection).
                let picked: Vec<LayerId> = if state.selection.contains(&layer.id) {
                    state.selection.clone()
                } else {
                    invoke(
                        events,
                        bl_shell::multisampler_commands::SELECT_LAYERS,
                        json!({ "device": frame.id, "layers": [layer.id] }),
                    );
                    vec![layer.id]
                };
                self.drag = Some(ZoneDrag {
                    kind,
                    handle,
                    start_x: pointer.x,
                    layers: picked
                        .iter()
                        .filter_map(|id| device.layers.get(*id).map(|l| (*id, l.zone(kind))))
                        .collect(),
                    delta: 0,
                });
                events.push(UiEvent::BeginGesture(ZONES_STEP));
            }
        }
    }
}

/// What a click on `layer` selects, and how, with `anchor` the layer
/// clicked before (R11): a plain click replaces the selection, Shift
/// extends it from the anchor, Cmd or Ctrl toggles.
fn clicked(
    device: &bl_model::Device,
    state: &MultisamplerState,
    anchor: Option<LayerId>,
    layer: LayerId,
    modifiers: Modifiers,
) -> (Vec<LayerId>, &'static str) {
    if modifiers.command {
        return (vec![layer], "toggle");
    }
    if modifiers.shift {
        let list = &device.layers.list;
        let from = anchor
            .or_else(|| state.selection.first().copied())
            .and_then(|a| list.iter().position(|l| l.id == a));
        let to = list.iter().position(|l| l.id == layer);
        if let (Some(from), Some(to)) = (from, to) {
            let (a, b) = (from.min(to), from.max(to));
            return (list[a..=b].iter().map(|l| l.id).collect(), "replace");
        }
    }
    (vec![layer], "replace")
}

/// A layer's sample as the Status Bar names it.
fn sample_name(shell: &Shell, layer: &Layer) -> String {
    shell
        .layer_folder(layer.media)
        .map_or_else(|| format!("missing ({})", layer.name), |_| layer.name.clone())
}

/// A zone as the Status Bar reads it: `C1 to C3` for keys, `1 to 127`
/// otherwise, with the fades in values.
#[must_use]
pub fn zone_text(kind: ZoneKind, zone: Zone) -> String {
    let value = |v: u8| {
        if kind == ZoneKind::Key {
            note_name(v)
        } else {
            v.to_string()
        }
    };
    let mut text = format!("{} to {}", value(zone.low), value(zone.high));
    if zone.fade_low > 0 || zone.fade_high > 0 {
        text.push_str(&format!(", fades {} and {}", zone.fade_low, zone.fade_high));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_and_values_agree() {
        let bars = Rect::from_min_max(pos2(100.0, 0.0), pos2(100.0 + 1280.0, 100.0));
        for key in [0u8, 1, 60, 127] {
            let x = x_of(bars, ZoneKind::Key, f32::from(key)) + 0.5;
            assert_eq!(value_at(bars, ZoneKind::Key, x), i32::from(key));
        }
        assert_eq!(value_at(bars, ZoneKind::Velocity, 100.0), 1);
        assert_eq!(value_at(bars, ZoneKind::Key, 5000.0), 127, "kept on the scale");
        assert_eq!(note_name(0), "C-2");
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(127), "G8");
    }

    #[test]
    fn dragging_keeps_a_value_and_the_fades_inside() {
        let zone = Zone {
            low: 20,
            high: 40,
            fade_low: 5,
            fade_high: 5,
        };
        let key = ZoneKind::Key;
        // The low edge past the high end stops at it, and the fades fit.
        let shrunk = drag_zone(zone, Handle::Low, 100, key);
        assert_eq!((shrunk.low, shrunk.high, shrunk.fade_low, shrunk.fade_high), (40, 40, 0, 0));
        let moved = drag_zone(zone, Handle::Body, 200, key);
        assert_eq!((moved.low, moved.high), (107, 127), "the body stops at the scale's end");
        assert_eq!((moved.fade_low, moved.fade_high), (5, 5));
        let faded = drag_zone(zone, Handle::FadeLow, 50, key);
        assert_eq!((faded.fade_low, faded.fade_high), (15, 5), "inside the zone, beside the other fade");
        let high = drag_zone(zone, Handle::FadeHigh, -50, key);
        assert_eq!((high.fade_low, high.fade_high), (5, 15));
        for handle in [Handle::Low, Handle::High, Handle::Body, Handle::FadeLow, Handle::FadeHigh] {
            for delta in [-300, -7, 0, 7, 300] {
                assert!(drag_zone(zone, handle, delta, key).is_valid(key), "{handle:?} {delta}");
                assert!(drag_zone(zone, handle, delta, ZoneKind::Velocity).low >= 1);
            }
        }
    }

    #[test]
    fn a_row_has_handles_for_its_edges_its_body_and_its_fades() {
        let bars = Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 100.0));
        let row = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 16.0));
        let zone = Zone {
            low: 10,
            high: 49,
            fade_low: 10,
            fade_high: 0,
        };
        let parts = bar_parts(bars, row, ZoneKind::Key, zone);
        assert_eq!(parts.body.left(), 100.0);
        assert_eq!(parts.body.right(), 500.0);
        assert_eq!(parts.fade_low.center().x, 200.0, "the fade's inner corner");
        assert!(parts.low.right() <= parts.high.left());
    }
}
