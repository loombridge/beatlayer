//! Record, Fixed Length, New, Duplicate, Quantize and Automate
//! (`docs/specs/push-2-notes.md`, R48 to R63).

use bl_core::time::Ticks;
use bl_model::RecordQuantization;

use super::*;
use crate::recording::LENGTHS;
use crate::state::QuantizeMenu;

/// The gesture key Delete with Automate runs its deletions in.
const AUTOMATE_GESTURE: u32 = 0x5100;

/// What Quantize To steps through: the note value and whether it is a
/// triplet of it (R58).
const GRIDS: [(&str, i64, bool); 6] = [
    ("1/4", 4, false),
    ("1/8", 8, false),
    ("1/8T", 8, true),
    ("1/16", 16, false),
    ("1/16T", 16, true),
    ("1/32", 32, false),
];

fn grid_ticks(division: i64) -> Ticks {
    Ticks(Ticks::WHOLE.get() / division)
}

impl Push {
    /// The Record button (R48, R49): with the Session View in focus it is
    /// Session Record and Shift makes it Arrangement Record; with the
    /// Arrangement in focus, the other way round.
    pub(super) fn record(&mut self, shift: bool, cx: &mut Cx) {
        let Some(model) = &self.model else {
            return;
        };
        if model.session_view != shift {
            Self::run(cx, "session.record", json!({}));
        } else {
            let on = !model.transport.recording;
            Self::run(cx, "transport.set_recording", json!({ "recording": on }));
        }
    }

    /// A tap of a button the surface owns in every mode, which a footswitch
    /// can press too (R29).
    pub(super) fn core_tap(&mut self, button: u8, cx: &mut Cx) {
        match button {
            cc::RECORD => self.record(false, cx),
            cc::NEW => Self::run(cx, "push.new", json!({})),
            _ => {}
        }
    }

    /// What the buttons of this package do when tapped alone (R48 to R63).
    pub(super) fn recording_tap(&mut self, button: u8, cx: &mut Cx) {
        match button {
            cc::RECORD | cc::NEW => self.core_tap(button, cx),
            cc::DUPLICATE => Self::run(cx, "push.duplicate", json!({})),
            cc::FIXED_LENGTH => {
                let on = !self.shared.fixed_length().on;
                Self::run(cx, "push.set_fixed_length", json!({ "enabled": on }));
            }
            cc::QUANTIZE if self.menu == Some(Menu::Quantize) => self.menu = None,
            cc::QUANTIZE => self.quantize(None, cx),
            cc::AUTOMATE => Self::run(cx, "arrangement.automation_arm", json!({})),
            _ => {}
        }
    }

    /// What a core button of this package does with a modifier held (R49,
    /// R53, R63).
    pub(super) fn recording_combination(
        &mut self,
        button: u8,
        modifiers: &Modifiers,
        cx: &mut Cx,
    ) {
        match button {
            cc::RECORD if modifiers.shift() => self.record(true, cx),
            cc::DUPLICATE if modifiers.shift() => {
                Self::run(cx, "push.duplicate", json!({ "scene": true }));
            }
            cc::AUTOMATE if modifiers.shift() => Self::run(cx, "automation.re_enable", json!({})),
            cc::AUTOMATE if modifiers.has(cc::DELETE) => self.delete_clip_automation(cx),
            _ => {}
        }
    }

    /// Delete with Automate: every Automation envelope of the clip the
    /// steps edit (R63).
    fn delete_clip_automation(&mut self, cx: &mut Cx) {
        let Some(clip) = self.edited_slot_clip() else {
            return;
        };
        let Some(targets) = self
            .model
            .as_ref()
            .and_then(|model| model.document.clip(clip))
            .map(|clip| {
                clip.envelopes
                    .iter()
                    .filter(|e| e.kind == bl_model::EnvelopeKind::Automation)
                    .map(|e| json!({ "clip": clip.id, "target": e.target, "kind": "automation" }))
                    .collect::<Vec<_>>()
            })
        else {
            return;
        };
        // One gesture, so one undo step.
        let last = targets.len().saturating_sub(1);
        for (n, args) in targets.into_iter().enumerate() {
            if n == last {
                cx.run_released(AUTOMATE_GESTURE, "clip.delete_envelope", args);
            } else {
                cx.run_held(AUTOMATE_GESTURE, "clip.delete_envelope", args);
            }
        }
    }

    /// The clip the steps edit: the selected slot's, else Note Mode's.
    pub(super) fn edited_slot_clip(&self) -> Option<bl_model::ClipId> {
        let model = self.model.as_ref()?;
        model
            .selection
            .slots
            .first()
            .and_then(|&(track, scene)| model.document.slot(track, scene))
            .map(|clip| clip.id)
            .or_else(|| self.contents.pads[0].note_state()?.edited_clip)
    }

    /// Quantize's tap, and a drum pad's with it held (`pitch`): the selected
    /// notes, or all of them, of the clip the steps edit, or an audio clip's
    /// transients (R57).
    pub(super) fn quantize(&mut self, pitch: Option<u8>, cx: &mut Cx) {
        let Some(clip) = self.edited_slot_clip() else {
            return;
        };
        let audio = self
            .model
            .as_ref()
            .and_then(|model| model.document.clip(clip))
            .is_some_and(|clip| matches!(clip.data, bl_model::ClipData::Audio(_)));
        if audio {
            Self::run(cx, "clip.quantize_audio", json!({ "clip": clip }));
            return;
        }
        let swing = (self.shared.swing() * 100.0).round() as u8;
        let mut args = json!({ "clip": clip, "swing": swing, "or_all": true });
        if let Some(pitch) = pitch {
            args["pitch"] = json!(pitch);
        }
        Self::run(cx, "note.quantize", args);
    }

    /// Holding Fixed Length or Quantize shows its menu (R50, R58).
    pub(super) fn open_hold_menu(&mut self, button: u8) {
        if self.menu.is_some() {
            return;
        }
        match button {
            cc::FIXED_LENGTH => self.menu = Some(Menu::FixedLength),
            cc::QUANTIZE => {
                self.menu = Some(Menu::Quantize);
                self.quantize_changed = false;
            }
            _ => {}
        }
    }

    /// Letting go of the button that held a menu open closes it, except
    /// Quantize's after a value changed (R58).
    pub(super) fn close_hold_menu(&mut self, button: u8) {
        match button {
            cc::FIXED_LENGTH if self.menu == Some(Menu::FixedLength) => self.menu = None,
            cc::QUANTIZE if self.menu == Some(Menu::Quantize) && !self.quantize_changed => {
                self.menu = None;
            }
            _ => {}
        }
    }

    /// A display button pressed with a menu of this package open (R50,
    /// R58).
    pub(super) fn recording_menu_button(&mut self, button: u8, cx: &mut Cx) {
        match self.menu {
            Some(Menu::FixedLength) => {
                self.buttons.mark_used(cc::FIXED_LENGTH);
                if button == controls::upper(0) {
                    let on = !self.shared.fixed_length().phrase_sync;
                    Self::run(cx, "push.set_fixed_length", json!({ "phrase_sync": on }));
                } else if let Some(column) = controls::lower_column(button) {
                    let length = bl_shell_length_name(column);
                    Self::run(cx, "push.set_fixed_length", json!({ "length": length }));
                }
            }
            Some(Menu::Quantize) if button == controls::upper(4) => {
                let Some(model) = &self.model else {
                    return;
                };
                let now = model.document.settings().record_quantization;
                let next = if now == RecordQuantization::None {
                    self.record_quantize_last
                } else {
                    self.record_quantize_last = now;
                    RecordQuantization::None
                };
                self.quantize_changed = true;
                Self::set_record_quantization(next, cx);
            }
            _ => {}
        }
    }

    fn set_record_quantization(value: RecordQuantization, cx: &mut Cx) {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        Self::run(
            cx,
            "transport.set_record_quantization",
            json!({ "quantization": value }),
        );
    }

    /// The Quantize menu's encoders (R58): 1 Swing Amount, 2 Quantize To,
    /// 3 Quantize Amount, 5 Record Quantize.
    pub(super) fn quantize_turn(&mut self, column: u8, steps: i32, shift: bool, cx: &mut Cx) {
        let Some(model) = &self.model else {
            return;
        };
        let counts = &mut self.setup_counts[usize::from(column)];
        let per_percent = if shift { 20 } else { 2 };
        match column {
            0 => {
                let swing = (self.shared.swing() * 100.0).round() as i32;
                let swing = (swing + counts.take(steps, per_percent)).clamp(0, 100);
                self.shared.set_swing(f64::from(swing) / 100.0);
            }
            1 => {
                let at = GRIDS
                    .iter()
                    .position(|&(_, n, t)| {
                        grid_ticks(n) == model.quantize.grid && t == model.quantize.triplets
                    })
                    .unwrap_or(3);
                let by = counts.take(steps, LIST_STEPS);
                if by == 0 {
                    return;
                }
                let next = (at as i32 + by).clamp(0, GRIDS.len() as i32 - 1) as usize;
                let (_, division, triplets) = GRIDS[next];
                Self::run(
                    cx,
                    "clip_view.quantize_settings",
                    json!({ "grid": grid_ticks(division), "triplets": triplets }),
                );
            }
            2 => {
                let amount = i32::from(model.quantize.amount) + counts.take(steps, per_percent);
                Self::run(
                    cx,
                    "clip_view.quantize_settings",
                    json!({ "amount": amount.clamp(0, 100) }),
                );
            }
            4 => {
                let all = RecordQuantization::ALL;
                let now = model.document.settings().record_quantization;
                let at = all.iter().position(|&v| v == now).unwrap_or(0) as i32;
                let by = counts.take(steps, LIST_STEPS);
                if by == 0 {
                    return;
                }
                let next = (at + by).clamp(0, all.len() as i32 - 1) as usize;
                Self::set_record_quantization(all[next], cx);
            }
            _ => return,
        }
        self.quantize_changed = true;
    }

    /// The Quantize menu's values, for the display and `push.state`.
    pub(super) fn quantize_values(&self) -> Option<QuantizeMenu> {
        let model = self.model.as_ref()?;
        let grid = GRIDS
            .iter()
            .find(|&&(_, n, t)| grid_ticks(n) == model.quantize.grid && t == model.quantize.triplets)
            .map_or_else(|| format!("{} ticks", model.quantize.grid.get()), |g| g.0.to_owned());
        Some(QuantizeMenu {
            swing: (self.shared.swing() * 100.0).round() as u8,
            quantize_to: grid,
            amount: model.quantize.amount,
            record_quantize: serde_json::to_value(model.document.settings().record_quantization)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            stays: self.quantize_changed,
        })
    }

    pub(super) fn paint_quantize(&mut self) {
        let Some(values) = self.quantize_values() else {
            return;
        };
        let percent = |name: &str, value: u8| Column {
            name: name.to_owned(),
            value: format!("{value} %"),
            graphic: Graphic::Arc(f32::from(value) / 100.0),
            ..Column::default()
        };
        self.screen.columns = Default::default();
        self.screen.title = None;
        self.screen.columns[0] = percent("Swing Amount", values.swing);
        self.screen.columns[1] = Column {
            name: "Quantize To".to_owned(),
            value: values.quantize_to.clone(),
            graphic: Graphic::Choice(values.quantize_to),
            ..Column::default()
        };
        self.screen.columns[2] = percent("Quantize Amount", values.amount);
        self.screen.columns[4] = Column {
            name: "Record Quantize".to_owned(),
            value: values.record_quantize.clone(),
            graphic: Graphic::Choice(values.record_quantize),
            ..Column::default()
        };
        let on = self
            .model
            .as_ref()
            .is_some_and(|m| m.document.settings().record_quantization != RecordQuantization::None);
        self.screen.upper[4] = Label::new("Record Quantize", [255, 255, 255], on);
    }

    pub(super) fn paint_fixed_length(&mut self) {
        let fixed = self.shared.fixed_length();
        self.screen.columns = Default::default();
        self.screen.title = None;
        self.screen.upper[0] = Label::new("Phrase Sync", [255, 255, 255], fixed.phrase_sync);
        for (column, length) in LENGTHS.iter().enumerate() {
            self.screen.lower[column] =
                Label::new(length.label(), [255, 255, 255], usize::from(fixed.length) == column);
        }
    }
}

/// The name `push.set_fixed_length` takes for the length on lower display
/// button `column`.
fn bl_shell_length_name(column: u8) -> String {
    LENGTHS[usize::from(column)].label()
}
