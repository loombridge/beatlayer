//! A rack's chains folded out as strips in the mixers (racks spec, R27 to
//! R29): the fold button in a track's title bar, the chain's title bar, and
//! the chain's strip with the chain list's controls for that chain. A
//! change in either place shows in the other at once (R27), and several
//! strips change together where the chain list changes one chain (R28).

use bl_model::{ChainId, DeviceId};
use bl_shell::mixer_controls;
use bl_shell::mixer_view::ChainStrip;
use bl_shell::{Invocation, Shell};
use egui::accesskit::{Action, Role};
use egui::{Align2, Id, Key, Rect, Sense, Shape, Stroke, Ui, WidgetInfo, WidgetType, pos2};
use serde_json::json;

use crate::layout::mixer::ChainStripLayout;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The ID of a rack's fold button.
#[must_use]
pub fn fold_id(rack: DeviceId) -> Id {
    Id::new(("mixer-fold-chains", rack))
}

/// The ID of a chain strip's title bar.
#[must_use]
pub fn title_id(chain: ChainId) -> Id {
    Id::new(("mixer-chain-title", chain))
}

/// The ID of a chain strip's column in the mixer.
#[must_use]
pub fn strip_id(chain: ChainId) -> Id {
    Id::new(("mixer-chain-strip", chain))
}

/// The ID of a chain strip's Volume slider.
#[must_use]
pub fn volume_id(chain: ChainId) -> Id {
    Id::new(("mixer-chain-volume", chain))
}

/// The ID of a chain strip's Pan slider.
#[must_use]
pub fn pan_id(chain: ChainId) -> Id {
    Id::new(("mixer-chain-pan", chain))
}

/// The ID of a chain strip's activator.
#[must_use]
pub fn activator_id(chain: ChainId) -> Id {
    Id::new(("mixer-chain-activator", chain))
}

/// The ID of a chain strip's Solo button.
#[must_use]
pub fn solo_id(chain: ChainId) -> Id {
    Id::new(("mixer-chain-solo", chain))
}

/// The fold button in a title bar (racks spec, R27): a triangle that folds
/// a rack's chains out as strips in the mixers, or folds them back in.
pub fn fold_button(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    rack: DeviceId,
    folded: bool,
    events: &mut Vec<UiEvent>,
) {
    let response = ui.interact(rect, fold_id(rack), Sense::click());
    crate::focus::register(ui.ctx(), response.id, bl_shell::Area::MainView, None);
    let c = &theme.colors;
    let painter = ui.painter_at(rect);
    let ink = if folded { c.accent.0 } else { c.text_dim.0 };
    let centre = rect.center();
    let (w, h) = (rect.width() / 2.0, rect.height() / 2.0);
    // Down when the chains are folded out, up while they are folded in.
    let points = if folded {
        vec![
            pos2(centre.x - w, centre.y - h),
            pos2(centre.x + w, centre.y - h),
            pos2(centre.x, centre.y + h),
        ]
    } else {
        vec![
            pos2(centre.x - w, centre.y + h),
            pos2(centre.x + w, centre.y + h),
            pos2(centre.x, centre.y - h),
        ]
    };
    painter.add(Shape::convex_polygon(points, ink, Stroke::NONE));
    let pressed =
        response.clicked() || (response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
    if pressed {
        events.push(UiEvent::Invoke(Invocation::new(
            "mixer_view.fold_chains",
            json!({ "rack": rack, "shown": !folded }),
        )));
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, "Fold Chains"));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Button);
        node.set_label("Fold Chains".to_owned());
        node.set_value(if folded { "folded out" } else { "folded in" }.to_owned());
    });
}

/// The chain being renamed in a mixer's title bar: the chain, the text, and
/// whether the field has the keyboard.
#[derive(Clone, Debug)]
struct Renaming {
    chain: ChainId,
    text: Option<String>,
    focused: bool,
}

/// Where the chain being renamed is kept.
fn renaming_id() -> Id {
    Id::new("mixer-chain-rename")
}

/// A chain's title bar in a mixer (racks spec, R27, R28): the chain's
/// name, which a click selects, and the fold button of the rack inside the
/// chain. A drag moves the chain among its rack's chains (R29).
pub fn title_bar(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    bar: Rect,
    strip: &ChainStrip,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let painter = ui.painter_at(bar);
    painter.rect_filled(bar, 0.0, c.lane.0);
    let fold = strip.nested.as_ref().map(|nested| {
        let rect = Rect::from_min_max(
            pos2(bar.right() - 12.0, bar.top() + 2.0),
            pos2(bar.right() - 2.0, bar.bottom() - 2.0),
        );
        (nested.rack, nested.folded, rect)
    });
    let name = Rect::from_min_max(
        pos2(bar.left() + 3.0, bar.top()),
        pos2(
            fold.map_or(bar.right(), |(_, _, rect)| rect.left()) - 2.0,
            bar.bottom(),
        ),
    );
    let response = ui.interact(bar, title_id(strip.chain), Sense::click_and_drag());
    if renaming_in(ui.ctx(), strip.chain).is_some() {
        draw_rename(ui, theme, name, strip, events);
    } else {
        painter.text(
            pos2(name.left(), name.center().y),
            Align2::LEFT_CENTER,
            &strip.name,
            theme.small_font(),
            c.clip_text.0,
        );
    }
    if let Some((rack, folded, rect)) = fold {
        fold_button(ui, theme, rect, rack, folded, events);
    }
    if response.clicked() {
        let modifiers = ui.input(|i| i.modifiers);
        events.push(UiEvent::Invoke(Invocation::new(
            "chain.select",
            json!({ "chain": strip.chain, "mode": crate::rack_view::click_mode(modifiers) }),
        )));
    }
    if response.drag_started() {
        // Spec: racks R29. A strip dragged moves the chain among its rack's
        // chains.
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            crate::rack_view::ChainDrag {
                chains: vec![strip.chain],
            },
        );
    }
    if response.double_clicked() {
        start_rename(ui.ctx(), strip.chain, &strip.name);
    }
    response.context_menu(|ui| {
        if ui.button("Rename Chain").clicked() {
            start_rename(ui.ctx(), strip.chain, &strip.name);
            ui.close();
        }
        if ui.button("Group Chains").clicked() {
            let chains: Vec<ChainId> = selected_in_rack(shell, strip.chain);
            events.push(UiEvent::Invoke(Invocation::new(
                "device.group",
                json!({ "chains": chains }),
            )));
            ui.close();
        }
    });
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            false,
            format!("Chain {} of {}", strip.name, strip.rack_name),
        )
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Button);
        node.set_label(format!("Chain {} of {}", strip.name, strip.rack_name));
        node.set_description(
            "Chain strip: click to select the chain, drag to move it among its rack's chains"
                .to_owned(),
        );
    });
}

/// The chains a change on a strip reaches (racks spec, R28): the chain
/// itself and the other selected chains of its own rack.
fn selected_in_rack(shell: &Shell, chain: ChainId) -> Vec<ChainId> {
    let doc = shell.editor().document();
    let rack = doc.chain_rack(chain).map(|r| r.id);
    let mut out = vec![chain];
    for other in shell.devices().selected_chains(doc) {
        let same = doc.chain_rack(other).map(|r| r.id) == rack;
        if other != chain && same && !out.contains(&other) {
            out.push(other);
        }
    }
    out
}

/// Starts renaming a chain in its strip's title bar.
fn start_rename(ctx: &egui::Context, chain: ChainId, name: &str) {
    ctx.data_mut(|d| {
        d.insert_temp(
            renaming_id(),
            Renaming {
                chain,
                text: Some(name.to_owned()),
                focused: false,
            },
        )
    });
}

/// The chain being renamed in a mixer's title bar, if it is this one.
fn renaming_in(ctx: &egui::Context, chain: ChainId) -> Option<Renaming> {
    ctx.data(|d| d.get_temp::<Renaming>(renaming_id()))
        .filter(|r| r.chain == chain)
}

/// The field a chain's name is typed in, and the name it gives.
fn draw_rename(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    strip: &ChainStrip,
    events: &mut Vec<UiEvent>,
) {
    let Some(mut renaming) = renaming_in(ui.ctx(), strip.chain) else {
        return;
    };
    let text = renaming.text.get_or_insert_with(|| strip.name.clone());
    let edit = ui.put(
        rect,
        egui::TextEdit::singleline(text)
            .id(Id::new(("mixer-chain-rename-edit", strip.chain)))
            .font(theme.small_font())
            .margin(egui::vec2(2.0, 0.0)),
    );
    if !renaming.focused {
        edit.request_focus();
        renaming.focused = true;
    }
    let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if edit.lost_focus() || enter || escape {
        if enter || (edit.lost_focus() && !escape) {
            events.push(UiEvent::Invoke(Invocation::new(
                "chain.rename",
                json!({ "chain": strip.chain, "name": text }),
            )));
        }
        ui.ctx().data_mut(|d| d.remove::<Renaming>(renaming_id()));
    } else {
        renaming.text = Some(text.clone());
        ui.ctx()
            .data_mut(|d| d.insert_temp(renaming_id(), renaming));
    }
}

/// A chain's strip in a mixer (racks spec, R27): the chain's name, its
/// Volume and Pan sliders, its activator and its Solo button, the chain
/// list's controls for that chain. With several chains selected, a change
/// reaches each of them, keeping their differences (R28).
pub fn strip(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    layout: &ChainStripLayout,
    strip: &ChainStrip,
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let painter = ui.painter_at(layout.name);
    painter.text(
        pos2(layout.name.left(), layout.name.center().y),
        Align2::LEFT_CENTER,
        &strip.name,
        theme.small_font(),
        c.text.0,
    );
    slider(
        ui,
        shell,
        theme,
        layout.volume,
        volume_id(strip.chain),
        "Volume",
        strip,
        true,
        events,
    );
    slider(
        ui,
        shell,
        theme,
        layout.pan,
        pan_id(strip.chain),
        "Pan",
        strip,
        false,
        events,
    );
    activator(ui, theme, layout.activator, strip, events);
    solo(ui, shell, theme, layout.solo, strip, events);
}

/// One chain's value when a drag began: the chain, its Volume and its Pan.
#[derive(Clone, Copy, Debug)]
struct Start {
    chain: ChainId,
    gain: f64,
    pan: f64,
}

/// A drag on a chain's slider: where each chain started, so the others
/// move by the same amount (racks spec, R28).
#[derive(Clone, Debug, Default)]
struct SliderDrag {
    starts: Vec<Start>,
}

/// A chain's Volume or Pan slider (racks spec, R27): the chain list's
/// control for that chain, so a change in either place shows in the other.
/// With several chains selected, each moves by the same amount, keeping
/// their differences (R28).
#[expect(clippy::too_many_arguments, reason = "the slider's inputs")]
fn slider(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    at: Rect,
    id: Id,
    name: &'static str,
    strip: &ChainStrip,
    volume: bool,
    events: &mut Vec<UiEvent>,
) {
    let chain = strip.chain;
    let value = if volume { strip.volume } else { strip.pan };
    let response = ui.interact(at, id, Sense::click_and_drag());
    let drag_id = id.with("drag");
    if response.drag_started() {
        ui.data_mut(|d| {
            d.insert_temp(
                drag_id,
                SliderDrag {
                    starts: starts(shell, chain),
                },
            )
        });
        events.push(UiEvent::BeginNamedGesture(format!(
            "Change \"Chain {name}\""
        )));
    }
    if response.dragged()
        && !response.double_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let place = f64::from(((pointer.x - at.left()) / at.width()).clamp(0.0, 1.0));
        let drag = ui.data(|d| d.get_temp::<SliderDrag>(drag_id));
        if let Some(drag) = drag {
            events.extend(
                moved(chain, &drag.starts, place, volume)
                    .into_iter()
                    .map(UiEvent::Invoke),
            );
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<SliderDrag>(drag_id));
        events.push(UiEvent::EndGesture);
    } else if response.clicked()
        && !response.double_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let place = f64::from(((pointer.x - at.left()) / at.width()).clamp(0.0, 1.0));
        let starts = starts(shell, chain);
        events.push(UiEvent::BeginNamedGesture(format!(
            "Change \"Chain {name}\""
        )));
        events.extend(
            moved(chain, &starts, place, volume)
                .into_iter()
                .map(UiEvent::Invoke),
        );
        events.push(UiEvent::EndGesture);
    }
    let c = &theme.colors;
    let painter = ui.painter_at(at.expand(1.0));
    painter.rect_filled(at, theme.sizes.corner, c.window.0);
    let (place, text) = if volume {
        (
            bl_model::fader::fader_position(value) as f32,
            crate::rack_view::volume_text(value),
        )
    } else {
        (
            ((value + 1.0) / 2.0) as f32,
            crate::rack_view::pan_text(value),
        )
    };
    let fill = if volume {
        Rect::from_min_max(at.min, pos2(at.left() + at.width() * place, at.bottom()))
    } else {
        let centre = at.center().x;
        let x = at.left() + at.width() * place;
        Rect::from_x_y_ranges(centre.min(x)..=centre.max(x), at.y_range())
    };
    painter.rect_filled(fill, theme.sizes.corner, c.accent.0.gamma_multiply(0.8));
    painter.text(
        at.center(),
        Align2::CENTER_CENTER,
        &text,
        egui::FontId::proportional(8.0),
        c.text.0,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Slider, true, name));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Slider);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
        node.set_label(format!("Chain {name} of {}", strip.name));
        node.set_value(text.clone());
    });
}

/// The chains a change on `chain` reaches, with the values they had when
/// the change began: the chain itself and the other selected chains of its
/// own rack (racks spec, R28).
fn starts(shell: &Shell, chain: ChainId) -> Vec<Start> {
    let doc = shell.editor().document();
    let mut out = Vec::new();
    for other in selected_in_rack(shell, chain) {
        let Some(row) = doc.chain(other) else {
            continue;
        };
        out.push(Start {
            chain: other,
            gain: row.mixer.volume,
            pan: row.mixer.pan,
        });
    }
    out
}

/// The commands a change on a chain's slider runs: the chain itself, and
/// every other selected chain by the same amount, keeping their
/// differences (racks spec, R28).
fn moved(chain: ChainId, starts: &[Start], place: f64, volume: bool) -> Vec<Invocation> {
    let Some(start) = starts.iter().find(|s| s.chain == chain) else {
        return Vec::new();
    };
    // How far the chain moved: the same dB for the volume, the same amount
    // for the pan.
    let (now, shift) = if volume {
        let now = crate::rack_view::volume_at(place);
        let shift = if now <= 0.0 || start.gain <= 0.0 {
            f64::NEG_INFINITY
        } else {
            mixer_controls::gain_db(now) - mixer_controls::gain_db(start.gain)
        };
        (now, shift)
    } else {
        let now = crate::rack_view::pan_at(place);
        (now, now - start.pan)
    };
    starts
        .iter()
        .map(|other| {
            let value = if other.chain == chain {
                now
            } else if volume {
                if shift == f64::NEG_INFINITY {
                    0.0
                } else {
                    mixer_controls::shift_volume(other.gain, shift)
                }
            } else {
                (other.pan + shift).clamp(-1.0, 1.0)
            };
            if volume {
                Invocation::new(
                    "chain.set_gain",
                    json!({ "chain": other.chain, "volume": value }),
                )
            } else {
                Invocation::new(
                    "chain.set_pan",
                    json!({ "chain": other.chain, "pan": value }),
                )
            }
        })
        .collect()
}

/// A chain's Chain Activator in its strip (racks spec, R27): the chain
/// list's button, so a change in either place shows in the other.
fn activator(ui: &mut Ui, theme: &Theme, at: Rect, strip: &ChainStrip, events: &mut Vec<UiEvent>) {
    let id = activator_id(strip.chain);
    let response = ui.interact(at, id, Sense::click());
    if response.clicked() {
        let on = !strip.enabled;
        events.push(UiEvent::Invoke(Invocation::new(
            "chain.set_enabled",
            json!({ "chain": strip.chain, "enabled": on }),
        )));
    }
    let c = &theme.colors;
    let painter = ui.painter_at(at.expand(2.0));
    let r = at.width() / 2.0;
    painter.circle_filled(
        at.center(),
        r,
        if strip.enabled {
            c.accent.0
        } else {
            c.window.0
        },
    );
    painter.circle_stroke(at.center(), r, Stroke::new(1.0, c.separator.0));
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Button, true, strip.enabled, "Chain Activator")
    });
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(format!("Chain Activator of {}", strip.name));
        node.set_value(if strip.enabled { "on" } else { "off" }.to_owned());
    });
}

/// A chain's Solo button in its strip (racks spec, R27, R28): the chain
/// list's button, so a change in either place shows in the other.
fn solo(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    at: Rect,
    strip: &ChainStrip,
    events: &mut Vec<UiEvent>,
) {
    let id = solo_id(strip.chain);
    let response = ui.interact(at, id, Sense::click());
    if response.clicked() {
        let add = ui.input(|i| i.modifiers.command);
        // Spec: racks R23. A click solos it alone; Cmd-click (Ctrl-click)
        // adds or removes it.
        events.push(UiEvent::Invoke(Invocation::new(
            "chain.solo",
            json!({ "chain": strip.chain, "add": add }),
        )));
        // Every other selected chain goes to the clicked one's new state
        // (racks R28).
        let doc = shell.editor().document();
        let soloed = doc
            .device(strip.rack)
            .map(|rack| rack.chains.iter().filter(|c| c.mixer.solo).count())
            .unwrap_or(0);
        let on = !(strip.solo && soloed == 1);
        for other in selected_in_rack(shell, strip.chain) {
            if other != strip.chain {
                events.push(UiEvent::Invoke(Invocation::new(
                    "chain.set_solo",
                    json!({ "chain": other, "solo": on }),
                )));
            }
        }
    }
    let c = &theme.colors;
    let painter = ui.painter_at(at.expand(2.0));
    let face = if strip.solo {
        c.accent.0
    } else if response.hovered() {
        c.control_hover.0
    } else {
        c.window.0
    };
    let ink = if strip.solo {
        c.accent_text.0
    } else {
        c.text_dim.0
    };
    painter.rect_filled(at, theme.sizes.corner, face);
    painter.text(
        at.center(),
        Align2::CENTER_CENTER,
        "S",
        theme.small_font(),
        ink,
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, strip.solo, "Solo"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Button);
        node.set_label(format!("Solo of {}", strip.name));
        node.set_value(if strip.solo { "on" } else { "off" }.to_owned());
    });
}
