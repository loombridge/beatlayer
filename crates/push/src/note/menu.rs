//! The Scale menu (`docs/specs/push-2-notes.md`, R35 to R37): the keys on the
//! display buttons, the scale list on encoders 2 to 7, Layout and Direction on
//! encoders 1 and 8, In Key and Chromatic on the first lower button and Fixed
//! on the last.

use bl_control::ReadModel;
use bl_model::{PadDirection, PadSpacing, ScaleName};
use serde_json::{Value, json};

use super::Note;
use crate::content::{Ctx, Paint};
use crate::controls::{self, lower, upper};
use crate::encoder::LIST_STEPS;
use crate::leds::{Led, fixed};
use crate::screen::{Column, Graphic, Label, List};

/// The keys of upper display buttons 2 to 7, then of the lower ones, as
/// pitch classes with their names (R35).
const UPPER_KEYS: [(u8, &str); 6] = [(0, "C"), (7, "G"), (2, "D"), (9, "A"), (4, "E"), (11, "B")];
const LOWER_KEYS: [(u8, &str); 6] = [
    (5, "F"),
    (10, "B♭"),
    (3, "E♭"),
    (8, "A♭"),
    (1, "D♭"),
    (6, "G♭"),
];

/// The Scale menu's lower button for In Key and Chromatic, and for Fixed.
const IN_KEY: u8 = 0;
const FIXED: u8 = 7;

/// How many scale names a column of the list holds.
const PER_COLUMN: usize = 3;

impl Note {
    /// The clips the keys and scales go to: the selected Session slots'
    /// clips, or none, where the set's current scale is set instead (R36).
    fn scale_clips(model: &ReadModel) -> Vec<bl_model::ClipId> {
        model
            .selection
            .slots
            .iter()
            .filter_map(|(track, scene)| model.document.slot(*track, *scene).map(|c| c.id))
            .collect()
    }

    /// Sets the key or the scale as the Control Bar's choosers do, leaving
    /// Scale Mode as it is (R36).
    fn set_scale(model: &ReadModel, args: &Value, column: Option<u8>, ctx: &mut Ctx<'_>) {
        let clips = Self::scale_clips(model);
        let (id, mut args) = if clips.is_empty() {
            ("song.set_scale", args.clone())
        } else {
            ("clip.set_scale", args.clone())
        };
        if !clips.is_empty()
            && let Value::Object(map) = &mut args
        {
            map.insert("clips".to_owned(), json!(clips));
        }
        match column {
            Some(column) => ctx.run_turn(column, id, args),
            None => ctx.cx.run(id, args),
        }
    }

    pub(super) fn scale_press(&mut self, button: u8, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let (upper_at, lower_at) = (
            controls::upper_column(button),
            controls::lower_column(button),
        );
        let root = match (upper_at, lower_at) {
            (Some(column @ 1..=6), _) => Some(UPPER_KEYS[usize::from(column) - 1].0),
            (_, Some(column @ 1..=6)) => Some(LOWER_KEYS[usize::from(column) - 1].0),
            _ => None,
        };
        if let Some(root) = root {
            Self::set_scale(model, &json!({ "root": root }), None, ctx);
            return;
        }
        let pads = model.document.settings().pad_layout;
        match lower_at {
            Some(IN_KEY) => ctx
                .cx
                .run("song.set_pad_layout", json!({ "in_key": !pads.in_key })),
            Some(FIXED) => ctx
                .cx
                .run("song.set_pad_layout", json!({ "fixed": !pads.fixed })),
            _ => {}
        }
    }

    pub(super) fn scale_steps(&mut self, column: u8, steps: i32, ctx: &mut Ctx<'_>) {
        let Some(model) = ctx.model else {
            return;
        };
        let by = self.menu_counts[usize::from(column)].take(steps, LIST_STEPS);
        if by == 0 {
            return;
        }
        let settings = model.document.settings();
        let pads = settings.pad_layout;
        match column {
            0 => {
                let at = PadSpacing::ALL.iter().position(|s| *s == pads.spacing);
                let next = moved(at, by, PadSpacing::ALL.len());
                ctx.cx.run(
                    "song.set_pad_layout",
                    json!({ "spacing": PadSpacing::ALL[next] }),
                );
            }
            7 => {
                let at = PadDirection::ALL.iter().position(|d| *d == pads.direction);
                let next = moved(at, by, PadDirection::ALL.len());
                ctx.cx.run(
                    "song.set_pad_layout",
                    json!({ "direction": PadDirection::ALL[next] }),
                );
            }
            _ => {
                let at = ScaleName::ALL
                    .iter()
                    .position(|n| *n == settings.scale.name);
                let next = moved(at, by, ScaleName::ALL.len());
                Self::set_scale(
                    model,
                    &json!({ "name": ScaleName::ALL[next].id() }),
                    Some(column),
                    ctx,
                );
            }
        }
    }

    pub(super) fn paint_scale_menu(&mut self, paint: &mut Paint<'_>) {
        let settings = paint.model.document.settings();
        let (scale, pads) = (settings.scale, settings.pad_layout);
        paint.screen.columns = Default::default();
        paint.screen.lists = Default::default();
        paint.screen.upper = Default::default();
        paint.screen.lower = Default::default();
        paint.screen.title = None;
        let white = [255, 255, 255];
        let choice = |name: &str, value: &str| Column {
            name: name.to_owned(),
            value: value.to_owned(),
            graphic: Graphic::Choice(value.to_owned()),
            ..Column::default()
        };
        paint.screen.columns[0] = choice("Layout", pads.spacing.name());
        paint.screen.columns[7] = choice("Direction", pads.direction.name());
        for (n, names) in ScaleName::ALL.chunks(PER_COLUMN).take(6).enumerate() {
            let selected = names.iter().position(|name| *name == scale.name);
            paint.screen.lists[n + 1] = Some(List {
                rows: names
                    .iter()
                    .map(|name| (name.label().to_owned(), false))
                    .collect(),
                selected,
                in_use: selected.is_some(),
            });
        }
        let keys = [(&UPPER_KEYS, true), (&LOWER_KEYS, false)];
        for (keys, top) in keys {
            for (n, (root, name)) in keys.iter().enumerate() {
                let column = n as u8 + 1;
                let chosen = *root == scale.root;
                let (labels, button) = if top {
                    (&mut paint.screen.upper, upper(column))
                } else {
                    (&mut paint.screen.lower, lower(column))
                };
                labels[usize::from(column)] = Label::new(*name, white, chosen);
                let colour = if chosen {
                    fixed::WHITE
                } else {
                    fixed::DARK_GREY
                };
                paint.lights.button(button, Led::steady(colour));
            }
        }
        let (in_key, fixed_on) = (pads.in_key, pads.fixed);
        paint.screen.lower[usize::from(IN_KEY)] =
            Label::new(if in_key { "In Key" } else { "Chromatic" }, white, true);
        paint.screen.lower[usize::from(FIXED)] = Label::new("Fixed", white, fixed_on);
        paint
            .lights
            .button(lower(IN_KEY), Led::steady(fixed::WHITE));
        paint.lights.button(
            lower(FIXED),
            Led::steady(if fixed_on {
                fixed::WHITE
            } else {
                fixed::DARK_GREY
            }),
        );
    }
}

/// The place `by` places from `at` in a list of `len`, held to its ends.
fn moved(at: Option<usize>, by: i32, len: usize) -> usize {
    let at = at.unwrap_or(0) as i64;
    (at + i64::from(by)).clamp(0, len as i64 - 1) as usize
}
