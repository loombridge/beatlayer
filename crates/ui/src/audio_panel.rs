//! The Clip View's Audio Utilities panel (WP-4.10b; L-CLIP-003, L-CLIP-008):
//! the right panel group's panel on an audio clip, with the clip's sound
//! settings.
//!
//! Each control runs the command agents run: the Warp switch, the Warp Mode
//! chooser and the chosen mode's own controls, the BPM field with ×2 and ÷2,
//! the Lead/Follow switch, Reverse, Edit, RAM, HiQ and Gain, and Transpose
//! with Detune. The control whose command hasn't landed yet — the Session
//! View's Clip Fade switch — is drawn pending, naming the package that
//! brings it.
//!
//! With several clips selected the panel shows what they share and, on a
//! control whose values differ, their range.

use std::cell::Cell;

use bl_model::{
    AudioData, ClipControl, ClipData, ClipId, DETUNE_MAX, DETUNE_MIN, Document, Preserve,
    TRANSPOSE_MAX, TRANSPOSE_MIN, TransientLoopMode, WarpMode,
};
use bl_shell::{Area, Invocation, Shell};
use egui::{Align2, Id, Pos2, Rect, Sense, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::{Value, json};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use crate::widgets::{self, Choice, NO_LEDS};

/// How wide the right panel group is (L-CLIP-003: x 424 to 588).
pub const WIDTH: f32 = 164.0;
/// The panel selector row's height (L-CLIP-003).
const SELECTOR: f32 = 16.0;
/// A control's size, and the step between rows.
const CONTROL: Vec2 = vec2(84.0, 12.0);
const ROW: f32 = 13.0;
/// Where the controls start, from the panel's left edge.
const CONTROLS_AT: f32 = 58.0;
/// How wide a switch is.
const SWITCH: f32 = 54.0;
/// How wide a small button is, the BPM row's ×2 and ÷2.
const SMALL: f32 = 26.0;
/// The gain's lowest reading in decibels: the point at which the gain is
/// silence, which is −∞ dB (rule G1).
const GAIN_FLOOR: f64 = -96.0;
/// The gain's highest reading in decibels (the warp modes spec, R28).
const GAIN_CEILING: f64 = 24.0;

/// What a control shows for the selected clips: the value they share, or the
/// range they span when they differ.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shared<T> {
    /// They all have this value.
    One(T),
    /// They differ: the lowest and the highest.
    Range(T, T),
}

/// What the clips share: the value they all have, or that they differ.
fn shared<T: Copy + PartialEq>(values: &[T]) -> Shared<T> {
    let first = values[0];
    if values.iter().all(|value| *value == first) {
        Shared::One(first)
    } else {
        Shared::Range(first, first)
    }
}

/// What the clips share of a number: the value they all have, or the range
/// they span.
fn shared_number(values: &[f64]) -> Shared<f64> {
    let mut low = values[0];
    let mut high = values[0];
    for &value in values {
        low = low.min(value);
        high = high.max(value);
    }
    if low == high {
        Shared::One(low)
    } else {
        Shared::Range(low, high)
    }
}

/// The ID of the panel's group.
#[must_use]
pub fn audio_panel_id() -> Id {
    Id::new("audio-utilities")
}

/// The right panel group's place: beside the left group, whose right edge is
/// at `left`.
#[must_use]
pub fn panel_rect(view: Rect, left: f32) -> Rect {
    Rect::from_min_max(
        pos2(left, view.top()),
        pos2((left + WIDTH).min(view.right()), view.bottom()),
    )
}

/// A control drawn in place that does nothing yet, announced as disabled
/// with the reason.
fn pending(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    name: &str,
    rect: Rect,
    text: &str,
    reason: &str,
) {
    let id = Id::new(("audio-pending", salt));
    let response = ui.interact(rect, id, Sense::hover());
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, c.control.0.gamma_multiply(0.55));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        c.pending.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_description(reason.to_owned());
        node.set_value(text.to_owned());
    });
}

/// A switch in the panel: a box that reads On, Off, or Mixed when the
/// selected clips differ. Returns the new state when it is clicked. A
/// switch that can't be used now is drawn quiet and announced as disabled.
fn switch(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    name: &str,
    rect: Rect,
    state: Shared<bool>,
    enabled: bool,
) -> Option<bool> {
    let id = Id::new(("switch", salt));
    let response = ui.interact(
        rect,
        id,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let c = &theme.colors;
    let (text, on) = match state {
        Shared::One(on) => (if on { "On" } else { "Off" }, on),
        Shared::Range(_, _) => ("Mixed", false),
    };
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if !enabled {
            c.control.0.gamma_multiply(0.55)
        } else if on {
            c.accent.0
        } else {
            c.control.0
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        if !enabled {
            c.pending.0
        } else if on {
            c.accent_text.0
        } else {
            c.text.0
        },
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, on, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_value(text.to_owned());
        if !enabled {
            node.set_description("the clip isn't warped: switch Warp on first".to_owned());
        } else if let Shared::Range(low, high) = state {
            node.set_description(format!(
                "the selected clips differ: {} to {}",
                if low { "on" } else { "off" },
                if high { "on" } else { "off" }
            ));
        }
    });
    (enabled && response.clicked()).then_some(!on)
}

/// A button in the panel: a box with a name on it, which runs a command when
/// it is clicked.
fn button(ui: &mut Ui, theme: &Theme, salt: &str, name: &str, rect: Rect) -> bool {
    let id = Id::new(("button", salt));
    let response = ui.interact(rect, id, Sense::click());
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(3.0));
    painter.rect_filled(
        rect,
        theme.sizes.corner,
        if response.hovered() {
            c.control_hover.0
        } else {
            c.control.0
        },
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        name,
        theme.small_font(),
        c.text.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    response.clicked()
}

/// Draws "Mixed" over a control whose clips differ.
fn mixed_text(ui: &mut Ui, theme: &Theme, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.text(
        pos2(rect.left() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Mixed",
        theme.font(),
        theme.colors.text.0,
    );
}

/// A control that can't be used now, drawn quiet and announced as disabled,
/// with the reason why.
fn disabled(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    name: &str,
    rect: Rect,
    text: &str,
    reason: &str,
) {
    let id = Id::new(("disabled", salt));
    let response = ui.interact(rect, id, Sense::hover());
    let c = &theme.colors;
    ui.painter()
        .rect_filled(rect, theme.sizes.corner, c.control.0.gamma_multiply(0.55));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        theme.small_font(),
        c.pending.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_description(reason.to_owned());
        node.set_value(text.to_owned());
    });
}

/// The clips the panel acts on: the Arrangement's selected audio clips, or
/// the Session clip the Clip View shows.
#[must_use]
pub fn targets(shell: &Shell, _doc: &Document, shown: ClipId) -> Vec<ClipId> {
    let clips = shell.audio_panel_clips();
    if clips.is_empty() { vec![shown] } else { clips }
}

/// The tempo the BPM field shows for `audio`: the segment that starts at the
/// warp marker `marker`, or the first segment's (warping R9).
fn segment_bpm(audio: &AudioData, marker: Option<u32>) -> Option<f64> {
    let beat = match marker {
        Some(place) => audio
            .warp_markers
            .get(usize::try_from(place).unwrap_or(usize::MAX))
            .map(|m| m.beat.get() as f64),
        None => audio.warp_markers.first().map(|m| m.beat.get() as f64),
    };
    audio
        .segment_bpm_at(beat.unwrap_or(0.0))
        .map(|bpm| bpm.get())
}

/// A gain in decibels, from a linear gain: 0 is −∞ dB.
fn gain_db(gain: f64) -> f64 {
    if gain <= 0.0 {
        GAIN_FLOOR
    } else {
        (20.0 * gain.log10()).clamp(GAIN_FLOOR, GAIN_CEILING)
    }
}

/// A linear gain, from a reading in decibels: the floor is silence.
fn gain_linear(db: f64) -> f64 {
    if db <= GAIN_FLOOR {
        0.0
    } else {
        10.0_f64.powf(db / 20.0)
    }
}

/// Draws the Audio Utilities panel for the audio clip `clip` in `rect`.
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    clip: ClipId,
    events: &mut Vec<UiEvent>,
) {
    let doc = shell.editor().document();
    let Some(ClipData::Audio(audio)) = doc.clip(clip).map(|c| &c.data) else {
        return;
    };
    let audio = audio.clone();
    let clips = targets(shell, doc, clip);
    let data: Vec<AudioData> = clips
        .iter()
        .filter_map(|id| match doc.clip(*id).map(|c| &c.data) {
            Some(ClipData::Audio(audio)) => Some(audio.clone()),
            _ => None,
        })
        .collect();
    if data.is_empty() {
        return;
    }
    let c = &theme.colors;
    ui.painter().rect_filled(rect, 0.0, c.panel.0);

    // The panel selector row (L-CLIP-003): Audio Utilities and Transform.
    // The Transform panel holds Quantize alone (the MIDI transformations
    // spec, R22).
    let shown = shell.clip_view().midi_tools().panel();
    let shown = if shown == bl_shell::midi_tool_commands::Panel::Transform {
        shown
    } else {
        bl_shell::midi_tool_commands::Panel::Utilities
    };
    crate::tool_panel::selector(
        ui,
        theme,
        Rect::from_min_size(rect.min, vec2(rect.width(), SELECTOR)),
        true,
        shown,
        events,
    );
    if shown == bl_shell::midi_tool_commands::Panel::Transform {
        crate::tool_panel::show_audio_transform(ui, shell, theme, rect, clip, events);
        return;
    }
    let group = ui.interact(rect, audio_panel_id(), Sense::hover());
    ui.ctx().accesskit_node_builder(group.id, |node| {
        node.set_role(egui::accesskit::Role::TabPanel);
        node.set_label("Audio Utilities".to_owned());
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });

    let painter = ui.painter_at(rect);
    let y = Cell::new(rect.top() + SELECTOR + 4.0);
    let row = |painter: &egui::Painter, label: &str| {
        painter.text(
            pos2(rect.left() + 6.0, y.get() + CONTROL.y / 2.0),
            Align2::LEFT_CENTER,
            label,
            theme.small_font(),
            c.text_dim.0,
        );
        let at = pos2(rect.left() + CONTROLS_AT, y.get());
        y.set(y.get() + ROW);
        at
    };

    // Every control runs its command on each of the clips the panel acts on.
    // With several clips the edit is one undo step (audio-clips R15), unless a
    // drag already holds a gesture open.
    let ctx = ui.ctx().clone();
    let mut each = |events: &mut Vec<UiEvent>, id: &str, args: Value| {
        let wrap = clips.len() > 1 && ctx.dragged_id().is_none();
        if wrap {
            events.push(UiEvent::BeginNamedGesture("Edit Audio Clips".to_owned()));
        }
        for target in &clips {
            let mut args = args.clone();
            args["clip"] = json!(target);
            events.push(UiEvent::Invoke(Invocation::new(id, args)));
        }
        if wrap {
            events.push(UiEvent::EndGesture);
        }
    };

    // Warp.
    let warp = shared(&data.iter().map(|d| d.warp).collect::<Vec<_>>());
    let at = row(&painter, "Warp");
    crate::focus::register(
        ui.ctx(),
        Id::new(("switch", "audio-warp")),
        Area::Detail,
        None,
    );
    if let Some(on) = switch(
        ui,
        theme,
        "audio-warp",
        "Warp",
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
        warp,
        true,
    ) {
        each(events, "clip.set_warp", json!({ "warp": on }));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
        Id::new(("switch", "audio-warp")),
        ClipControl::Warp,
        events,
    );

    // Warp Mode, and the chosen mode's own controls. With Warp off they are
    // drawn disabled (R15).
    let mode = shared(&data.iter().map(|d| d.warp_mode).collect::<Vec<_>>());
    let at = row(&painter, "Mode");
    let modes: Vec<Choice<WarpMode>> = [
        WarpMode::Beats,
        WarpMode::Tones,
        WarpMode::Texture,
        WarpMode::RePitch,
        WarpMode::Complex,
        WarpMode::ComplexPro,
    ]
    .into_iter()
    .map(|mode| Choice {
        label: mode_label(mode).to_owned(),
        value: mode,
        leds: NO_LEDS,
    })
    .collect();
    let current = match mode {
        Shared::One(mode) => Some(mode),
        Shared::Range(_, _) => None,
    };
    crate::focus::register(
        ui.ctx(),
        Id::new(("chooser", "audio-warp-mode")),
        Area::Detail,
        None,
    );
    let chooser = Rect::from_min_size(at, CONTROL);
    if let Some(chosen) = widgets::chooser(
        ui,
        theme,
        "audio-warp-mode",
        "Warp Mode",
        chooser,
        &modes,
        current.as_ref(),
        audio.warp,
    ) {
        each(
            events,
            "clip.set_warp_mode",
            json!({ "mode": mode_id(chosen) }),
        );
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        chooser,
        Id::new(("chooser", "audio-warp-mode")),
        ClipControl::WarpMode,
        events,
    );
    if let Shared::Range(_, _) = mode {
        mixed_text(ui, theme, chooser);
    }
    if audio.warp {
        mode_controls(ui, theme, rect, &y, &data, events, &mut each);
    }

    // The BPM field, with ×2 and ÷2 (R15). With Warp off they are disabled.
    let marker = shell.clip_view().sample().markers().first().copied();
    let bpms: Vec<f64> = data.iter().filter_map(|d| segment_bpm(d, marker)).collect();
    let bpm = if bpms.is_empty() {
        Shared::One(0.0)
    } else {
        shared_number(&bpms)
    };
    let at = row(&painter, "BPM");
    let value = match bpm {
        Shared::One(bpm) => bpm,
        Shared::Range(low, high) => (low + high) / 2.0,
    };
    let fine = ui.input(|i| i.modifiers.shift);
    let step = move |value: f64, steps: i32| {
        let step = if fine { 0.1 } else { 1.0 };
        value + f64::from(steps) * step
    };
    let text = match bpm {
        Shared::Range(low, high) => format!("{low:.1} – {high:.1}"),
        _ => format!("{value:.1}"),
    };
    crate::focus::register(
        ui.ctx(),
        Id::new(("number-box", "audio-bpm")),
        Area::Detail,
        None,
    );
    let field = Rect::from_min_size(at, vec2(48.0, CONTROL.y));
    let twice = Rect::from_min_size(at + vec2(52.0, 0.0), vec2(SMALL, CONTROL.y));
    let half = Rect::from_min_size(at + vec2(80.0, 0.0), vec2(SMALL, CONTROL.y));
    if !audio.warp {
        // R15: with Warp off the tempo controls are disabled.
        let reason = "the clip isn't warped: switch Warp on first";
        disabled(ui, theme, "audio-bpm", "BPM", field, &text, reason);
        disabled(ui, theme, "audio-bpm-twice", "×2", twice, "×2", reason);
        disabled(ui, theme, "audio-bpm-half", "÷2", half, "÷2", reason);
    } else {
        widgets::number_box_gesture(ui, "audio-bpm", "Change \"BPM\"", events);
        if let Some(bpm) = widgets::number_box(
            ui,
            theme,
            "audio-bpm",
            "BPM",
            field,
            value,
            (1.0, 1000.0),
            &step,
            &text,
        ) {
            let mut args = json!({ "bpm": bpm });
            if let Some(marker) = marker {
                args["marker"] = json!(marker);
            }
            each(events, "clip.set_segment_bpm", args);
        }
        if button(ui, theme, "audio-bpm-twice", "×2", twice) {
            each(events, "clip.scale_tempo", json!({ "factor": 2.0 }));
        }
        if button(ui, theme, "audio-bpm-half", "÷2", half) {
            each(events, "clip.scale_tempo", json!({ "factor": 0.5 }));
        }
    }

    // Lead/Follow, on an Arrangement clip.
    let in_session = shell
        .clip_view()
        .shown_session_clip(doc, shell.arrangement())
        .is_some();
    if !in_session {
        let leader = shared(&data.iter().map(|d| d.tempo_leader).collect::<Vec<_>>());
        let at = row(&painter, "Lead/Follow");
        crate::focus::register(
            ui.ctx(),
            Id::new(("switch", "audio-tempo-leader")),
            Area::Detail,
            None,
        );
        if let Some(on) = switch(
            ui,
            theme,
            "audio-tempo-leader",
            "Lead/Follow",
            Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
            leader,
            // Spec: warping R46, R55. With EXT on every clip follows: the
            // switch keeps showing what the clip is and can't be changed.
            audio.warp && !shell.external_sync(),
        ) {
            each(events, "clip.set_tempo_leader", json!({ "leader": on }));
        }
        if shell.external_sync() {
            ui.ctx()
                .accesskit_node_builder(Id::new(("switch", "audio-tempo-leader")), |node| {
                    node.set_description(
                        "External Sync is on: every clip follows the incoming clock".to_owned(),
                    );
                });
        }
    }

    // Reverse (the audio clip editing spec, R7).
    let at = row(&painter, "Reverse");
    if button(
        ui,
        theme,
        "audio-reverse",
        "Reverse",
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
    ) {
        each(events, "clip.reverse", json!({}));
    }

    // Edit.
    let at = row(&painter, "Edit");
    if button(
        ui,
        theme,
        "audio-edit",
        "Edit",
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
    ) {
        each(events, "clip.edit_sample", json!({}));
    }

    // Clip Fade, on a Session clip.
    if in_session {
        let at = row(&painter, "Clip Fade");
        pending(
            ui,
            theme,
            "audio-clip-fade",
            "Clip Fade",
            Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
            "Clip Fade",
            "Not built yet: arrives with the Session View's Clip Fade switch (Phase 3).",
        );
    }

    // RAM and HiQ.
    let ram = shared(&data.iter().map(|d| d.ram).collect::<Vec<_>>());
    let at = row(&painter, "RAM");
    crate::focus::register(
        ui.ctx(),
        Id::new(("switch", "audio-ram")),
        Area::Detail,
        None,
    );
    if let Some(on) = switch(
        ui,
        theme,
        "audio-ram",
        "RAM",
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
        ram,
        true,
    ) {
        each(events, "clip.set_ram_mode", json!({ "ram": on }));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
        Id::new(("switch", "audio-ram")),
        ClipControl::Ram,
        events,
    );
    let hq = shared(&data.iter().map(|d| d.high_quality).collect::<Vec<_>>());
    let at = row(&painter, "HiQ");
    crate::focus::register(
        ui.ctx(),
        Id::new(("switch", "audio-hiq")),
        Area::Detail,
        None,
    );
    if let Some(on) = switch(
        ui,
        theme,
        "audio-hiq",
        "HiQ",
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
        hq,
        true,
    ) {
        each(
            events,
            "clip.set_high_quality",
            json!({ "high_quality": on }),
        );
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, vec2(SWITCH, CONTROL.y)),
        Id::new(("switch", "audio-hiq")),
        ClipControl::HighQuality,
        events,
    );

    // Gain, in decibels, down to −∞ dB (rule G1).
    let gains: Vec<f64> = data.iter().map(|d| gain_db(d.gain)).collect();
    let gain = shared_number(&gains);
    let at = row(&painter, "Gain");
    let value = match gain {
        Shared::One(db) => db,
        Shared::Range(low, high) => (low + high) / 2.0,
    };
    let text = match gain {
        Shared::Range(low, high) => format!("{low:.1} – {high:.1}"),
        Shared::One(db) if db <= GAIN_FLOOR => "−∞ dB".to_owned(),
        Shared::One(db) => format!("{db:.1} dB"),
    };
    crate::focus::register(
        ui.ctx(),
        Id::new(("number-box", "audio-gain")),
        Area::Detail,
        None,
    );
    widgets::number_box_gesture(ui, "audio-gain", "Change \"Gain\"", events);
    if let Some(db) = widgets::number_box(
        ui,
        theme,
        "audio-gain",
        "Gain",
        Rect::from_min_size(at, CONTROL),
        value,
        (GAIN_FLOOR, GAIN_CEILING),
        &|value, steps| value + f64::from(steps) * 0.5,
        &text,
    ) {
        each(events, "clip.set_gain", json!({ "gain": gain_linear(db) }));
    }
    crate::mapping::clip_overlay(
        ui,
        theme,
        Rect::from_min_size(at, CONTROL),
        Id::new(("number-box", "audio-gain")),
        ClipControl::Gain,
        events,
    );

    // Transpose, with Detune beside it (the warp modes spec, R2 and R25).
    let transposes: Vec<f64> = data.iter().map(|d| f64::from(d.transpose)).collect();
    let detunes: Vec<f64> = data.iter().map(|d| d.detune).collect();
    let at = row(&painter, "Transpose");
    let (shown, text) = match shared_number(&transposes) {
        Shared::One(value) => (value, format!("{value:.0} st")),
        Shared::Range(low, high) => (low, format!("{low:.0} – {high:.0} st")),
    };
    let (detune, detune_text) = match shared_number(&detunes) {
        Shared::One(value) => (value, format!("{value:.0} ct")),
        Shared::Range(low, high) => (low, format!("{low:.0} – {high:.0} ct")),
    };
    let transpose_rect = Rect::from_min_size(at, vec2(48.0, CONTROL.y));
    let detune_rect = Rect::from_min_size(at + vec2(52.0, 0.0), vec2(48.0, CONTROL.y));
    // Spec: warp-modes R10. In Re-Pitch the Transpose and the Detune don't
    // act, and the panel draws them switched off.
    if data
        .iter()
        .all(|d| d.warp && d.warp_mode == WarpMode::RePitch)
    {
        let reason = "the clip is in Re-Pitch: Transpose and Detune don't act there";
        disabled(
            ui,
            theme,
            "audio-transpose",
            "Transpose",
            transpose_rect,
            &text,
            reason,
        );
        disabled(
            ui,
            theme,
            "audio-detune",
            "Detune",
            detune_rect,
            &detune_text,
            reason,
        );
    } else {
        crate::focus::register(
            ui.ctx(),
            Id::new(("number-box", "audio-transpose")),
            Area::Detail,
            None,
        );
        widgets::number_box_gesture(ui, "audio-transpose", "Change \"Transpose\"", events);
        if let Some(value) = widgets::number_box(
            ui,
            theme,
            "audio-transpose",
            "Transpose",
            transpose_rect,
            shown,
            (TRANSPOSE_MIN as f64, TRANSPOSE_MAX as f64),
            &|value, steps| value + f64::from(steps),
            &text,
        ) {
            each(
                events,
                "clip.set_pitch",
                json!({ "transpose": value.round() as i32 }),
            );
        }
        crate::focus::register(
            ui.ctx(),
            Id::new(("number-box", "audio-detune")),
            Area::Detail,
            None,
        );
        widgets::number_box_gesture(ui, "audio-detune", "Change \"Detune\"", events);
        if let Some(value) = widgets::number_box(
            ui,
            theme,
            "audio-detune",
            "Detune",
            detune_rect,
            detune,
            (DETUNE_MIN, DETUNE_MAX),
            &|value, steps| value + f64::from(steps),
            &detune_text,
        ) {
            each(events, "clip.set_pitch", json!({ "detune": value }));
        }
    }
    // Spec: map-modes R11, R17. Transpose and Detune are mappable even in
    // Re-Pitch, where they are drawn off: the clips keep the values.
    crate::mapping::clip_overlay(
        ui,
        theme,
        transpose_rect,
        Id::new(("number-box", "audio-transpose")),
        ClipControl::Transpose,
        events,
    );
    crate::mapping::clip_overlay(
        ui,
        theme,
        detune_rect,
        Id::new(("number-box", "audio-detune")),
        ClipControl::Detune,
        events,
    );
}

/// The mode's own controls, under the Warp Mode chooser (the warp modes
/// spec, R2).
fn mode_controls(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    y: &Cell<f32>,
    data: &[AudioData],
    events: &mut Vec<UiEvent>,
    each: &mut impl FnMut(&mut Vec<UiEvent>, &str, Value),
) {
    let painter = ui.painter_at(rect);
    let row = |painter: &egui::Painter, label: &str| {
        painter.text(
            pos2(rect.left() + 6.0, y.get() + CONTROL.y / 2.0),
            Align2::LEFT_CENTER,
            label,
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        let at = pos2(rect.left() + CONTROLS_AT, y.get());
        y.set(y.get() + ROW);
        at
    };
    match data[0].warp_mode {
        WarpMode::Beats => {
            let preserve = shared(
                &data
                    .iter()
                    .map(|d| d.warp_settings.beats.preserve)
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Preserve");
            let choices: Vec<Choice<Preserve>> = [
                Preserve::Transients,
                Preserve::Bar,
                Preserve::Half,
                Preserve::Quarter,
                Preserve::Eighth,
                Preserve::Sixteenth,
                Preserve::ThirtySecond,
            ]
            .into_iter()
            .map(|value| Choice {
                label: preserve_label(value).to_owned(),
                value,
                leds: NO_LEDS,
            })
            .collect();
            let current = match preserve {
                Shared::One(value) => Some(value),
                Shared::Range(_, _) => None,
            };
            crate::focus::register(
                ui.ctx(),
                Id::new(("chooser", "audio-preserve")),
                Area::Detail,
                None,
            );
            let chooser = Rect::from_min_size(at, CONTROL);
            if let Some(value) = widgets::chooser(
                ui,
                theme,
                "audio-preserve",
                "Preserve",
                chooser,
                &choices,
                current.as_ref(),
                true,
            ) {
                each(
                    events,
                    "clip.set_warp_settings",
                    json!({ "beats": { "preserve": preserve_id(value) } }),
                );
            }
            if let Shared::Range(_, _) = preserve {
                mixed_text(ui, theme, chooser);
            }
            let loop_mode = shared(
                &data
                    .iter()
                    .map(|d| d.warp_settings.beats.transient_loop_mode)
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Loop Mode");
            let choices: Vec<Choice<TransientLoopMode>> = [
                TransientLoopMode::LoopOff,
                TransientLoopMode::LoopForward,
                TransientLoopMode::LoopBackAndForth,
            ]
            .into_iter()
            .map(|value| Choice {
                label: loop_mode_label(value).to_owned(),
                value,
                leds: NO_LEDS,
            })
            .collect();
            let current = match loop_mode {
                Shared::One(value) => Some(value),
                Shared::Range(_, _) => None,
            };
            crate::focus::register(
                ui.ctx(),
                Id::new(("chooser", "audio-loop-mode")),
                Area::Detail,
                None,
            );
            let chooser = Rect::from_min_size(at, CONTROL);
            if let Some(value) = widgets::chooser(
                ui,
                theme,
                "audio-loop-mode",
                "Transient Loop Mode",
                chooser,
                &choices,
                current.as_ref(),
                true,
            ) {
                each(
                    events,
                    "clip.set_warp_settings",
                    json!({ "beats": { "transient_loop_mode": loop_mode_id(value) } }),
                );
            }
            if let Shared::Range(_, _) = loop_mode {
                mixed_text(ui, theme, chooser);
            }
            let envelope = shared_number(
                &data
                    .iter()
                    .map(|d| f64::from(d.warp_settings.beats.transient_envelope))
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Envelope");
            number(
                ui,
                theme,
                "audio-transient-envelope",
                "Transient Envelope",
                at,
                envelope,
                0.0,
                100.0,
                1.0,
                "%",
                events,
                each,
                &|value| json!({ "beats": { "transient_envelope": value } }),
            );
        }
        WarpMode::Tones => {
            let grain = shared_number(
                &data
                    .iter()
                    .map(|d| d.warp_settings.tones.grain_size)
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Grain Size");
            number(
                ui,
                theme,
                "audio-tones-grain",
                "Grain Size",
                at,
                grain,
                10.0,
                100.0,
                1.0,
                "ms",
                events,
                each,
                &|value| json!({ "tones": { "grain_size": value } }),
            );
        }
        WarpMode::Texture => {
            let grain = shared_number(
                &data
                    .iter()
                    .map(|d| d.warp_settings.texture.grain_size)
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Grain Size");
            number(
                ui,
                theme,
                "audio-texture-grain",
                "Grain Size",
                at,
                grain,
                10.0,
                500.0,
                1.0,
                "ms",
                events,
                each,
                &|value| json!({ "texture": { "grain_size": value } }),
            );
            let flux = shared_number(
                &data
                    .iter()
                    .map(|d| d.warp_settings.texture.flux)
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Flux");
            number(
                ui,
                theme,
                "audio-texture-flux",
                "Flux",
                at,
                flux,
                0.0,
                100.0,
                1.0,
                "%",
                events,
                each,
                &|value| json!({ "texture": { "flux": value } }),
            );
        }
        WarpMode::ComplexPro => {
            let formants = shared_number(
                &data
                    .iter()
                    .map(|d| d.warp_settings.complex_pro.formants)
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Formants");
            number(
                ui,
                theme,
                "audio-formants",
                "Formants",
                at,
                formants,
                0.0,
                100.0,
                1.0,
                "%",
                events,
                each,
                &|value| json!({ "complex_pro": { "formants": value } }),
            );
            let envelope = shared_number(
                &data
                    .iter()
                    .map(|d| f64::from(d.warp_settings.complex_pro.envelope))
                    .collect::<Vec<_>>(),
            );
            let at = row(&painter, "Envelope");
            number(
                ui,
                theme,
                "audio-complex-envelope",
                "Envelope",
                at,
                envelope,
                8.0,
                512.0,
                1.0,
                "",
                events,
                each,
                &|value| json!({ "complex_pro": { "envelope": value } }),
            );
        }
        WarpMode::RePitch | WarpMode::Complex => {}
    }
}

/// A number box in the panel, showing what the clips share or the range they
/// span.
#[expect(
    clippy::too_many_arguments,
    reason = "a number box has this many parts"
)]
fn number(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    name: &str,
    at: Pos2,
    value: Shared<f64>,
    low: f64,
    high: f64,
    step: f64,
    unit: &str,
    events: &mut Vec<UiEvent>,
    each: &mut impl FnMut(&mut Vec<UiEvent>, &str, Value),
    args: &dyn Fn(f64) -> Value,
) {
    let (shown, text) = match value {
        Shared::One(value) => (value, format!("{value:.0}{unit}")),
        Shared::Range(low, high) => (low, format!("{low:.0} – {high:.0}{unit}")),
    };
    crate::focus::register(ui.ctx(), Id::new(("number-box", salt)), Area::Detail, None);
    widgets::number_box_gesture(ui, salt, &format!("Change \"{name}\""), events);
    if let Some(value) = widgets::number_box(
        ui,
        theme,
        salt,
        name,
        Rect::from_min_size(at, CONTROL),
        shown,
        (low, high),
        &move |value, steps| value + f64::from(steps) * step,
        &text,
    ) {
        each(events, "clip.set_warp_settings", args(value));
    }
}

/// A mode's name, as Live writes it.
fn mode_label(mode: WarpMode) -> &'static str {
    match mode {
        WarpMode::Beats => "Beats",
        WarpMode::Tones => "Tones",
        WarpMode::Texture => "Texture",
        WarpMode::RePitch => "Re-Pitch",
        WarpMode::Complex => "Complex",
        WarpMode::ComplexPro => "Complex Pro",
    }
}

/// A mode's name, as the command takes it.
fn mode_id(mode: WarpMode) -> &'static str {
    match mode {
        WarpMode::Beats => "beats",
        WarpMode::Tones => "tones",
        WarpMode::Texture => "texture",
        WarpMode::RePitch => "re_pitch",
        WarpMode::Complex => "complex",
        WarpMode::ComplexPro => "complex_pro",
    }
}

/// What Beats preserves, as Live writes it.
fn preserve_label(value: Preserve) -> &'static str {
    match value {
        Preserve::Transients => "Transients",
        Preserve::Bar => "1 Bar",
        Preserve::Half => "1/2",
        Preserve::Quarter => "1/4",
        Preserve::Eighth => "1/8",
        Preserve::Sixteenth => "1/16",
        Preserve::ThirtySecond => "1/32",
    }
}

/// What Beats preserves, as the command takes it.
fn preserve_id(value: Preserve) -> &'static str {
    match value {
        Preserve::Transients => "transients",
        Preserve::Bar => "1_bar",
        Preserve::Half => "half",
        Preserve::Quarter => "quarter",
        Preserve::Eighth => "eighth",
        Preserve::Sixteenth => "sixteenth",
        Preserve::ThirtySecond => "thirty_second",
    }
}

/// How Beats fills the time a segment's audio doesn't, as Live writes it.
fn loop_mode_label(value: TransientLoopMode) -> &'static str {
    match value {
        TransientLoopMode::LoopOff => "Loop Off",
        TransientLoopMode::LoopForward => "Loop Forward",
        TransientLoopMode::LoopBackAndForth => "Loop Back-and-Forth",
    }
}

/// How Beats fills the time a segment's audio doesn't, as the command takes
/// it.
fn loop_mode_id(value: TransientLoopMode) -> &'static str {
    match value {
        TransientLoopMode::LoopOff => "loop_off",
        TransientLoopMode::LoopForward => "loop_forward",
        TransientLoopMode::LoopBackAndForth => "loop_back_and_forth",
    }
}
