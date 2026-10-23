//! The In/Out section's controls (WP-2.19; the routing spec), in the
//! Session and Arrangement mixers and the Arrangement's track headers:
//! the input and output choosers and the Monitor switch.
//!
//! What each chooser lists, and the command each entry runs, comes from
//! `bl_shell::routing`, so the window, its tests, and an agent share them.
//! A click opens the list; choosing an entry runs its command. Entries
//! another package brings are listed, disabled, and say which.

use bl_model::{Monitor, TrackId};
use bl_shell::Invocation;
use bl_shell::routing::{Chooser, Entry, InOut, MonitorSwitch, TuningBypass};
use egui::accesskit::Role;
use egui::{Align2, Id, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::focus::{Slot, TrackControl};
use crate::main_window::UiEvent;
use crate::mixer::Place;
use crate::theme::Theme;

/// Where a track's In/Out controls go and whose they are.
pub struct InOutParts<'a> {
    /// The theme.
    pub theme: &'a Theme,
    /// A mixer or the headers.
    pub place: Place,
    /// The track.
    pub track: TrackId,
}

/// The ID of one of a track's In/Out controls.
#[must_use]
pub fn control_id(place: Place, track: TrackId, slot: Slot, index: usize) -> Id {
    Id::new(("in-out", place, track, slot, index))
}

impl InOutParts<'_> {
    /// The section's rows, top to bottom, each with the rect `row(i)`
    /// gives, or `None` where there's no room: labels, choosers, and the
    /// Monitor switch, in Live's order (L-MIX-001, L-ARR-008). `labels`
    /// says whether the label rows (From, To) are drawn, as the mixer
    /// draws them and the headers don't.
    pub fn draw(
        &self,
        ui: &mut Ui,
        section: &InOut,
        labels: bool,
        row: impl Fn(usize) -> Option<Rect>,
        events: &mut Vec<UiEvent>,
    ) {
        let mut rows: Vec<Row<'_>> = Vec::new();
        match section {
            InOut::Clips {
                input,
                input_channel,
                monitor,
                output,
                output_channel,
                tuning_bypass,
            } => {
                if labels {
                    rows.push(Row::Label("From"));
                }
                rows.push(Row::Chooser(input, Slot::InputType));
                rows.push(Row::Chooser(input_channel, Slot::InputChannel));
                rows.push(Row::Monitor(monitor));
                if labels {
                    rows.push(Row::Label("To"));
                }
                rows.push(Row::Chooser(output, Slot::OutputType));
                rows.push(Row::Chooser(output_channel, Slot::OutputChannel));
                if let Some(switch) = tuning_bypass {
                    rows.push(Row::TuningBypass(switch));
                }
            }
            InOut::Output {
                output,
                output_channel,
            } => {
                if labels {
                    rows.push(Row::Label("Audio To"));
                }
                rows.push(Row::Chooser(output, Slot::OutputType));
                rows.push(Row::Chooser(output_channel, Slot::OutputChannel));
            }
            InOut::Main { cue, main } => {
                if labels {
                    rows.push(Row::Label("Cue Out"));
                }
                rows.push(Row::Chooser(cue, Slot::InputType));
                if labels {
                    rows.push(Row::Label("Main Out"));
                }
                rows.push(Row::Chooser(main, Slot::OutputType));
            }
        }
        for (i, line) in rows.into_iter().enumerate() {
            let Some(rect) = row(i) else {
                break;
            };
            match line {
                Row::Label(text) => {
                    ui.painter_at(rect).text(
                        rect.left_center(),
                        Align2::LEFT_CENTER,
                        text,
                        self.theme.small_font(),
                        self.theme.colors.text_dim.0,
                    );
                }
                Row::Chooser(chooser, slot) => {
                    self.chooser(ui, rect, chooser, slot, events);
                }
                Row::Monitor(switch) => self.monitor(ui, rect, switch, events),
                Row::TuningBypass(switch) => self.tuning_bypass(ui, rect, switch, events),
            }
        }
    }

    fn register(&self, ui: &Ui, id: Id, slot: Slot) {
        crate::focus::register(
            ui.ctx(),
            id,
            bl_shell::Area::MainView,
            Some(TrackControl {
                place: self.place,
                track: self.track,
                slot,
            }),
        );
    }

    /// A chooser: a box showing the choice, and a list a click opens.
    fn chooser(
        &self,
        ui: &mut Ui,
        rect: Rect,
        chooser: &Chooser,
        slot: Slot,
        events: &mut Vec<UiEvent>,
    ) {
        let id = control_id(self.place, self.track, slot, 0);
        let enabled = chooser.enabled();
        // A disabled chooser still takes the keyboard focus in its place,
        // as Live's do (observed: Tab stops on Input Channel with no audio
        // device).
        let response = ui.interact(
            rect,
            id,
            if enabled {
                Sense::click()
            } else {
                Sense::focusable_noninteractive()
            },
        );
        self.register(ui, id, slot);
        if response.clicked() {
            response.request_focus();
        }
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let face = if response.hovered() && enabled {
            c.control_hover.0
        } else {
            c.control.0
        };
        painter.rect(
            rect,
            self.theme.sizes.corner,
            if enabled {
                face
            } else {
                face.gamma_multiply(0.6)
            },
            Stroke::new(1.0, c.separator.0),
            StrokeKind::Inside,
        );
        let ink = if enabled { c.text.0 } else { c.pending.0 };
        let text_area = Rect::from_min_max(rect.min, pos2(rect.right() - 9.0, rect.bottom()));
        ui.painter_at(text_area).text(
            pos2(rect.left() + 3.0, rect.center().y),
            Align2::LEFT_CENTER,
            &chooser.text,
            self.theme.small_font(),
            ink,
        );
        if enabled {
            let a = pos2(rect.right() - 5.0, rect.center().y);
            painter.add(Shape::convex_polygon(
                vec![
                    a + vec2(-2.5, -1.25),
                    a + vec2(2.5, -1.25),
                    a + vec2(0.0, 1.75),
                ],
                ink,
                Stroke::NONE,
            ));
        }
        focus_ring(&painter, self.theme, rect, &response);
        describe(
            ui,
            &response,
            Role::ComboBox,
            chooser.name,
            &chooser.text,
            chooser.pending,
        );
        if !enabled {
            return;
        }
        // With the focus on it, Up and Down choose the entry before or
        // after the chosen one, as the Settings window's choosers step
        // (S-SET-003); Configure… is an action, not a choice, and is
        // stepped over.
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            let step = ui.input(|i| {
                i32::from(i.key_pressed(egui::Key::ArrowDown))
                    - i32::from(i.key_pressed(egui::Key::ArrowUp))
            });
            if let Some(run) = stepped(chooser, step) {
                events.push(UiEvent::Invoke(run.clone()));
            }
        }
        egui::Popup::menu(&response)
            .width(rect.width().max(120.0))
            .show(|ui| {
                for entry in &chooser.entries {
                    match entry {
                        Entry::Separator => {
                            ui.separator();
                        }
                        Entry::Choice {
                            label,
                            run,
                            chosen,
                            pending,
                        } => {
                            let item = ui.add_enabled(
                                run.is_some(),
                                egui::Button::selectable(*chosen, label.as_str()),
                            );
                            let item = match pending {
                                Some(package) => {
                                    item.on_disabled_hover_text(format!("Arrives with {package}."))
                                }
                                None => item,
                            };
                            if item.clicked()
                                && let Some(run) = run
                            {
                                events.push(UiEvent::Invoke(run.clone()));
                            }
                        }
                    }
                }
            });
    }

    /// Bypass Tuning (the tunings spec, R14): one button, lit while the
    /// track plays twelve equal steps. On a track that holds a Drum Rack it
    /// is lit and can't be changed.
    fn tuning_bypass(
        &self,
        ui: &mut Ui,
        rect: Rect,
        switch: &TuningBypass,
        events: &mut Vec<UiEvent>,
    ) {
        let id = control_id(self.place, self.track, Slot::TuningBypass, 0);
        let response = ui.interact(
            rect,
            id,
            if switch.locked {
                Sense::focusable_noninteractive()
            } else {
                Sense::click()
            },
        );
        self.register(ui, id, Slot::TuningBypass);
        if response.clicked() {
            response.request_focus();
            events.push(UiEvent::Invoke(switch.run.clone()));
        }
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        let face = if switch.on { c.accent.0 } else { c.control.0 };
        painter.rect_filled(
            rect,
            self.theme.sizes.corner,
            if switch.locked {
                face.gamma_multiply(0.6)
            } else {
                face
            },
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Bypass Tuning",
            self.theme.small_font(),
            if switch.on { c.accent_text.0 } else { c.text.0 },
        );
        focus_ring(&painter, self.theme, rect, &response);
        describe(
            ui,
            &response,
            Role::CheckBox,
            "Bypass Tuning",
            if switch.on { "on" } else { "off" },
            None,
        );
    }

    /// The Monitor switch: In, Auto, and Off side by side, the chosen one
    /// lit. It's one stop in the keyboard's order, as Live's Monitoring is
    /// (observed); with the focus on it, Left and Right choose the state
    /// beside the chosen one.
    fn monitor(&self, ui: &mut Ui, rect: Rect, switch: &MonitorSwitch, events: &mut Vec<UiEvent>) {
        let third = rect.width() / 3.0;
        let cell = |i: usize| {
            Rect::from_min_max(
                pos2(rect.left() + third * i as f32, rect.top()),
                pos2(rect.left() + third * (i as f32 + 1.0) - 1.0, rect.bottom()),
            )
        };
        let id = control_id(self.place, self.track, Slot::Monitoring, 0);
        let response = ui.interact(rect, id, Sense::click());
        self.register(ui, id, Slot::Monitoring);
        let chosen = switch
            .runs
            .iter()
            .position(|(state, _)| *state == switch.chosen)
            .unwrap_or(1);
        let mut pick = None;
        if response.clicked() {
            response.request_focus();
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            pick = (0..3).find(|&i| cell(i).contains(pointer));
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        ..Default::default()
                    },
                );
            });
            let step = ui.input(|i| {
                i32::from(i.key_pressed(egui::Key::ArrowRight))
                    - i32::from(i.key_pressed(egui::Key::ArrowLeft))
            });
            if step != 0 {
                pick = Some((chosen as i32 + step).clamp(0, 2) as usize);
            }
        }
        if let Some(i) = pick.filter(|&i| i != chosen) {
            events.push(UiEvent::Invoke(Invocation::clone(&switch.runs[i].1)));
        }
        let c = &self.theme.colors;
        let painter = ui.painter_at(rect.expand(2.0));
        for (i, (state, _)) in switch.runs.iter().enumerate() {
            let on = i == chosen;
            painter.rect_filled(
                cell(i),
                self.theme.sizes.corner,
                if on { c.accent.0 } else { c.control.0 },
            );
            painter.text(
                cell(i).center(),
                Align2::CENTER_CENTER,
                monitor_label(*state),
                self.theme.small_font(),
                if on { c.accent_text.0 } else { c.text.0 },
            );
        }
        focus_ring(&painter, self.theme, rect, &response);
        describe(
            ui,
            &response,
            Role::RadioGroup,
            "Monitoring",
            monitor_label(switch.chosen),
            None,
        );
    }
}

/// The command of the live choice `step` places from the chosen one
/// (Configure… aside), if there is one.
fn stepped(chooser: &Chooser, step: i32) -> Option<&Invocation> {
    if step == 0 {
        return None;
    }
    let choices: Vec<(&Invocation, bool)> = chooser
        .entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Choice {
                label,
                run: Some(run),
                chosen,
                ..
            } if label != "Configure…" => Some((run, *chosen)),
            _ => None,
        })
        .collect();
    let at = choices.iter().position(|(_, chosen)| *chosen)? as i32;
    let next = at + step;
    if next < 0 || next >= choices.len() as i32 || next == at {
        return None;
    }
    Some(choices[next as usize].0)
}

/// One row of the section.
enum Row<'a> {
    Label(&'static str),
    Chooser(&'a Chooser, Slot),
    Monitor(&'a MonitorSwitch),
    TuningBypass(&'a TuningBypass),
}

/// What a Monitor button reads.
#[must_use]
pub const fn monitor_label(state: Monitor) -> &'static str {
    match state {
        Monitor::In => "In",
        Monitor::Auto => "Auto",
        Monitor::Off => "Off",
    }
}

/// The keyboard focus's outline, while Outline View in Focus is on (the
/// accessibility spec, R7).
pub(crate) fn focus_ring(painter: &egui::Painter, theme: &Theme, rect: Rect, response: &Response) {
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, theme.colors.focus.0),
            StrokeKind::Outside,
        );
    }
}

/// Tells assistive technology a control's role, name, and value, and, if
/// another package brings it, that it's disabled and which.
pub(crate) fn describe(
    ui: &Ui,
    response: &Response,
    role: Role,
    name: &str,
    value: &str,
    pending: Option<&str>,
) {
    let enabled = pending.is_none() && response.sense.senses_click();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, enabled, name));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(role);
        node.set_label(name.to_owned());
        if !value.is_empty() {
            node.set_value(value.to_owned());
        }
        if !enabled {
            node.set_disabled();
        }
        if let Some(package) = pending {
            node.set_description(format!("Not built yet: arrives with {package}."));
        }
    });
}
