//! The MIDI meter and the meters between devices (WP-6.69; the MIDI meters
//! spec): a column of dots that lights to the velocity of the notes passing
//! it, and the thin level meter a gap between two devices holds where audio
//! passes. The strips and headers draw the MIDI meter in place of their level
//! meter ([`crate::mixer`]); the Device View draws both in its gaps.

use bl_engine::{MeterOwner, MeterSite};
use bl_model::DeviceId;
use bl_shell::Shell;
use bl_shell::mixer_controls::meter_position;
use bl_shell::mixer_view::MeterValue;
use egui::accesskit::Role;
use egui::{Id, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2};

use crate::theme::Theme;

/// How far apart a MIDI meter's dots are, in points.
const DOT_SPACING: f32 = 4.0;

/// How wide a meter in the gap between two devices is, in points.
const GAP_METER_WIDTH: f32 = 3.0;

/// The ID of the meter in front of `before` in the chain `owner`, or after
/// its last device when `before` is `None`.
#[must_use]
pub fn chain_meter_id(owner: MeterOwner, before: Option<DeviceId>) -> Id {
    Id::new(("chain-meter", owner, before))
}

/// How many of a MIDI meter's `dots` a velocity lights from the bottom: any
/// note lights at least the lowest.
#[must_use]
pub fn lit_dots(velocity: f32, dots: usize) -> usize {
    (velocity.clamp(0.0, 1.0) * dots as f32).ceil() as usize
}

/// The velocity a MIDI meter says it shows, from 0 to 127.
#[must_use]
pub fn velocity_text(velocity: f32) -> String {
    format!("{}", (velocity.clamp(0.0, 1.0) * 127.0).round())
}

/// A MIDI meter in `rect` (R3 to R5, R8): the dots lit from the bottom to
/// the velocity's share of the height, the lowest also lit by a channel
/// message, and blue by a per-note expression. It is named MIDI Activity in
/// the accessibility tree and says the velocity it shows.
pub fn midi(ui: &mut Ui, theme: &Theme, id: Id, rect: Rect, value: MeterValue) {
    let MeterValue::Midi {
        velocity,
        channel,
        expression,
    } = value
    else {
        return;
    };
    let c = &theme.colors;
    let painter = ui.painter_at(rect);
    let dots = ((rect.height() - 2.0) / DOT_SPACING).floor().max(0.0) as usize;
    let lit = lit_dots(velocity, dots);
    for dot in 0..dots {
        let colour = if dot == 0 && expression > 0.0 {
            c.meter_expression.0
        } else if dot < lit || (dot == 0 && channel > 0.0) {
            c.meter.0
        } else {
            c.separator.0
        };
        let y = rect.bottom() - 2.0 - DOT_SPACING * dot as f32;
        painter.circle_filled(pos2(rect.center().x, y), 1.0, colour);
    }
    let response = ui.interact(rect, id, Sense::hover());
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "MIDI Activity"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Meter);
        node.set_label("MIDI Activity".to_owned());
        node.set_value(velocity_text(velocity));
        node.set_numeric_value(f64::from((velocity.clamp(0.0, 1.0) * 127.0).round()));
    });
}

/// The meter at `at` in the chain `owner`, drawn in the gap `gap` of the
/// Device View (R2): a MIDI meter or a level meter, whichever the signal
/// there calls for. Nothing while the engine has published none.
pub fn in_gap(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    owner: MeterOwner,
    before: Option<DeviceId>,
    gap: Rect,
) {
    let at = before.map_or(MeterSite::End, MeterSite::Before);
    let Some(value) = shell.chain_meter(owner, at) else {
        return;
    };
    let id = chain_meter_id(owner, before);
    let rect = Rect::from_center_size(gap.center(), egui::vec2(GAP_METER_WIDTH, gap.height()));
    match value {
        MeterValue::Midi { .. } => midi(ui, theme, id, rect, value),
        MeterValue::Level { peak } => {
            let c = &theme.colors;
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, c.window.0);
            let half = rect.width() / 2.0;
            for (side, level) in peak.iter().enumerate() {
                let left = rect.left() + half * side as f32;
                let top = rect.bottom() - rect.height() * meter_position(*level);
                painter.rect_filled(
                    Rect::from_min_max(pos2(left, top), pos2(left + half, rect.bottom())),
                    0.0,
                    c.meter.0,
                );
            }
            let response = ui.interact(rect, id, Sense::hover());
            response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "Level Meter")
            });
            let loudest = peak[0].max(peak[1]);
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Meter);
                node.set_label("Level Meter".to_owned());
                node.set_value(bl_shell::mixer_controls::volume_text(f64::from(loudest)));
                node.set_numeric_value(
                    bl_shell::mixer_controls::gain_db(f64::from(loudest)).max(-1000.0),
                );
            });
        }
    }
}

/// The gaps of a chain of devices, whose frames sit at `frames` in order:
/// the meter before the first, between each two and after the last (R2).
pub fn in_gaps(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    owner: MeterOwner,
    frames: &[(DeviceId, Rect)],
    gap: f32,
) {
    for (id, frame) in frames {
        let at = Rect::from_min_max(
            pos2(frame.left() - gap, frame.top()),
            pos2(frame.left(), frame.bottom()),
        );
        in_gap(ui, shell, theme, owner, Some(*id), at);
    }
    if let Some((_, last)) = frames.last() {
        let at = Rect::from_min_max(
            pos2(last.right(), last.top()),
            pos2(last.right() + gap, last.bottom()),
        );
        in_gap(ui, shell, theme, owner, None, at);
    }
}
