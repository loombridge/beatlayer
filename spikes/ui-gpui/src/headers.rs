//! The track-header column (name, mute, solo, volume knob) and the meter
//! column beside it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AccessibleAction, App, AppContext, Bounds, ClickEvent, Context, Entity, FocusHandle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Toggled, Window, canvas, div, fill, point,
    prelude::FluentBuilder, px, rgb, size, text,
};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::data::{TRACKS, meter_level};
use crate::platform;
use crate::session::{Session, SessionEvent, TRACK_H};

pub const HEADER_W: f32 = 240.0;
pub const METER_W: f32 = 8.0;
const KNOB: f32 = 26.0;

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

struct KnobDrag {
    track: usize,
    last: Point<Pixels>,
    locked: bool,
}

struct Rename {
    track: usize,
    state: Entity<InputState>,
    _subscription: Subscription,
}

pub struct TrackHeaders {
    session: Entity<Session>,
    knob_drag: Option<KnobDrag>,
    rename: Option<Rename>,
    knob_focus: Vec<FocusHandle>,
    /// Where each visible track's knob was last painted, for `--selftest`.
    pub knob_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    _subscription: Subscription,
}

struct RowData {
    track: usize,
    name: SharedString,
    color: u32,
    mute: bool,
    solo: bool,
    volume: f32,
}

impl TrackHeaders {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event, cx| match event {
            SessionEvent::Vertical | SessionEvent::Tracks => cx.notify(),
            SessionEvent::Horizontal | SessionEvent::Clips => {}
        });
        let knob_focus = (0..TRACKS).map(|_| cx.focus_handle()).collect();
        Self {
            session,
            knob_drag: None,
            rename: None,
            knob_focus,
            knob_bounds: Rc::default(),
            _subscription: subscription,
        }
    }

    /// The track whose name is being edited, if any.
    pub fn renaming(&self) -> Option<usize> {
        self.rename.as_ref().map(|r| r.track)
    }

    pub fn start_rename(&mut self, track: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_rename(cx);
        let name = self.session.read(cx).project.tracks[track].name.to_string();
        let state = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let subscription = cx.subscribe_in(&state, window, |this, _, event, _, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(cx),
            InputEvent::Change | InputEvent::Focus => {}
        });
        state.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.rename = Some(Rename {
            track,
            state,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let value = rename.state.read(cx).value();
        let name = value.trim();
        if !name.is_empty() {
            let name: std::sync::Arc<str> = name.into();
            self.session.update(cx, |s, cx| {
                s.project.tracks[rename.track].name = name;
                cx.emit(SessionEvent::Tracks);
            });
        }
        cx.notify();
    }

    fn set_volume(&mut self, track: usize, volume: f32, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.project.tracks[track].volume = volume.clamp(0.0, 1.0);
            cx.emit(SessionEvent::Tracks);
        });
    }

    fn nudge_volume(&mut self, track: usize, by: f32, cx: &mut Context<Self>) {
        let v = self.session.read(cx).project.tracks[track].volume;
        self.set_volume(track, v + by, cx);
    }

    fn on_knob_down(
        &mut self,
        track: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.knob_focus[track], cx);
        let locked = platform::lock_pointer();
        self.knob_drag = Some(KnobDrag {
            track,
            last: event.position,
            locked,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn on_knob_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.knob_drag else {
            return;
        };
        let dy = if drag.locked {
            let d = platform::take_delta();
            if std::env::var_os("BL_SPIKE_DEBUG").is_some() {
                eprintln!("knob: locked delta {d:?}");
            }
            d.1 as f32
        } else {
            let d = event.position.y - drag.last.y;
            drag.last = event.position;
            f32::from(d)
        };
        if dy == 0.0 {
            return;
        }
        // 200 points of travel cover the whole range; shift for fine control.
        let fine = if event.modifiers.shift { 0.1 } else { 1.0 };
        let track = drag.track;
        self.nudge_volume(track, -dy * fine / 200.0, cx);
    }

    fn on_knob_up(&mut self, cx: &mut Context<Self>) {
        if self.knob_drag.take().is_some() {
            platform::unlock_pointer();
            cx.notify();
        }
    }

    fn toggle(&mut self, track: usize, solo: bool, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            let t = &mut s.project.tracks[track];
            if solo {
                t.solo = !t.solo;
            } else {
                t.mute = !t.mute;
            }
            cx.emit(SessionEvent::Tracks);
        });
    }

    fn render_row(
        &self,
        row: RowData,
        top: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = row.track;
        let name: gpui::AnyElement = match &self.rename {
            Some(r) if r.track == t => Input::new(&r.state)
                .aria_label(format!("Rename track {}", t + 1))
                .w(px(118.0))
                .h(px(22.0))
                .into_any_element(),
            _ => div()
                .id("name")
                .w(px(118.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .cursor_pointer()
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if e.click_count() >= 2 {
                        this.start_rename(t, window, cx);
                    }
                }))
                .child(text!(row.name.clone()))
                .into_any_element(),
        };

        div()
            .id(("track", t))
            .role(Role::Group)
            .aria_label(format!("Track {}", t + 1))
            .absolute()
            .top(px(top))
            .left_0()
            .w_full()
            .h(px(TRACK_H))
            .flex()
            .items_center()
            .gap(px(6.0))
            .pr(px(METER_W + 10.0))
            .bg(if t.is_multiple_of(2) {
                hex(0x2A2D33)
            } else {
                hex(0x272A2F)
            })
            .border_b_1()
            .border_color(hex(0x15171A))
            .child(div().w(px(4.0)).h_full().bg(hex(row.color)))
            .child(name)
            .child(self.toggle_button(t, false, row.mute, cx))
            .child(self.toggle_button(t, true, row.solo, cx))
            .child(self.knob(t, row.volume, cx))
    }

    fn toggle_button(
        &self,
        t: usize,
        solo: bool,
        on: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (id, label, glyph, on_color) = if solo {
            ("solo", "Solo", "S", 0x4F8FE8)
        } else {
            ("mute", "Mute", "M", 0xE8A23F)
        };
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .tab_stop(true)
            .focusable()
            .size(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.0))
            .text_size(px(11.0))
            .cursor_pointer()
            .when(on, |d| d.bg(hex(on_color)).text_color(hex(0x101114)))
            .when(!on, |d| d.bg(hex(0x3A3E46)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(t, solo, cx)))
            .child(SharedString::new_static(glyph))
    }

    fn knob(&self, t: usize, volume: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let focus = self.knob_focus[t].clone();
        div()
            .id("volume")
            .role(Role::Slider)
            .aria_label("Volume")
            .aria_numeric_value((volume * 100.0).round() as f64)
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(100.0)
            .aria_numeric_value_step(1.0)
            .track_focus(&focus)
            .tab_stop(true)
            .size(px(KNOB))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e, window, cx| this.on_knob_down(t, e, window, cx)),
            )
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "up" | "right" => this.nudge_volume(t, 0.01, cx),
                    "down" | "left" => this.nudge_volume(t, -0.01, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .on_a11y_action(AccessibleAction::Increment, {
                let view = cx.entity().downgrade();
                move |_, _, cx| {
                    view.update(cx, |this, cx| this.nudge_volume(t, 0.01, cx))
                        .ok();
                }
            })
            .on_a11y_action(AccessibleAction::Decrement, {
                let view = cx.entity().downgrade();
                move |_, _, cx| {
                    view.update(cx, |this, cx| this.nudge_volume(t, -0.01, cx))
                        .ok();
                }
            })
            .child({
                let knob_bounds = self.knob_bounds.clone();
                canvas(
                    |_, _, _| (),
                    move |b, _, window, _| {
                        knob_bounds.borrow_mut().insert(t, b);
                        paint_knob(b, volume, window);
                    },
                )
                .size_full()
            })
    }
}

fn paint_knob(b: Bounds<Pixels>, volume: f32, window: &mut Window) {
    let r = f32::from(b.size.width) / 2.0;
    window.paint_quad(
        fill(b, hex(0x1B1D21))
            .corner_radii(px(r))
            .border_widths(px(2.0))
            .border_color(hex(0x5A606B)),
    );
    // A dot at the value's angle: -135 degrees (minimum) to +135 (maximum).
    let angle = (-135.0 + 270.0 * volume).to_radians();
    let c = b.center();
    let d = r - 6.0;
    let dot = point(
        c.x + px(d * angle.sin()) - px(2.5),
        c.y - px(d * angle.cos()) - px(2.5),
    );
    window.paint_quad(
        fill(Bounds::new(dot, size(px(5.0), px(5.0))), hex(0xF0F2F5)).corner_radii(px(2.5)),
    );
}

impl Render for TrackHeaders {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (scroll_y, rows): (f32, Vec<RowData>) = {
            let s = self.session.read(cx);
            let rows = s
                .visible_tracks()
                .map(|t| {
                    let tr = &s.project.tracks[t];
                    RowData {
                        track: t,
                        name: SharedString::from(tr.name.clone()),
                        color: tr.color,
                        mute: tr.mute,
                        solo: tr.solo,
                        volume: tr.volume,
                    }
                })
                .collect();
            (s.scroll_y, rows)
        };
        let elements: Vec<_> = rows
            .into_iter()
            .map(|row| {
                let top = row.track as f32 * TRACK_H - scroll_y;
                self.render_row(row, top, cx)
            })
            .collect();
        let view = cx.entity().downgrade();
        let dragging = self.knob_drag.is_some();
        div()
            .id("track-headers")
            .role(Role::List)
            .aria_label("Tracks")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(hex(0x1B1D21))
            .text_size(px(12.0))
            .on_scroll_wheel(cx.listener(|this, e: &gpui::ScrollWheelEvent, _, cx| {
                let dy = f32::from(e.delta.pixel_delta(px(20.0)).y);
                this.session
                    .update(cx, |s, cx| s.set_scroll_y(s.scroll_y - dy, cx));
            }))
            .children(elements)
            .when(dragging, |d| {
                // While a knob is dragged, follow the pointer anywhere in the window.
                d.child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            let v = view.clone();
                            window.on_mouse_event(
                                move |e: &MouseMoveEvent, phase, _, cx: &mut App| {
                                    if phase.bubble() {
                                        v.update(cx, |this, cx| this.on_knob_move(e, cx)).ok();
                                    }
                                },
                            );
                            let v = view.clone();
                            window.on_mouse_event(
                                move |e: &MouseUpEvent, phase, _, cx: &mut App| {
                                    if phase.bubble() && e.button == MouseButton::Left {
                                        v.update(cx, |this, cx| this.on_knob_up(cx)).ok();
                                    }
                                },
                            );
                        },
                    )
                    .absolute()
                    .size_0(),
                )
            })
    }
}

/// Synthetic meter levels for every track.
pub struct Meters {
    pub levels: Vec<f32>,
    epoch: Instant,
}

impl Meters {
    pub fn new() -> Self {
        Self {
            levels: vec![0.0; TRACKS],
            epoch: Instant::now(),
        }
    }

    /// Advances all 200 meters to `now`.
    pub fn tick(&mut self, now: Instant, cx: &mut Context<Self>) {
        let t = now.duration_since(self.epoch).as_secs_f64();
        for (i, level) in self.levels.iter_mut().enumerate() {
            *level = meter_level(i, t);
        }
        cx.notify();
    }
}

/// Draws the visible tracks' meters in one canvas, so a meter frame doesn't
/// re-render the headers.
pub struct MeterColumn {
    session: Entity<Session>,
    meters: Entity<Meters>,
    _subscriptions: [Subscription; 2],
}

impl MeterColumn {
    pub fn new(session: Entity<Session>, meters: Entity<Meters>, cx: &mut Context<Self>) -> Self {
        let a = cx.observe(&meters, |_, _, cx| cx.notify());
        let b = cx.subscribe(&session, |_, _, event, cx| {
            if *event == SessionEvent::Vertical {
                cx.notify();
            }
        });
        Self {
            session,
            meters,
            _subscriptions: [a, b],
        }
    }
}

impl Render for MeterColumn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.clone();
        let meters = self.meters.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let s = session.read(cx);
                let levels = &meters.read(cx).levels;
                let top = f32::from(bounds.origin.y);
                for t in s.visible_tracks() {
                    let y = top + t as f32 * TRACK_H - s.scroll_y + 6.0;
                    let h = TRACK_H - 12.0;
                    let well = Bounds::new(point(bounds.origin.x, px(y)), size(px(METER_W), px(h)));
                    window.paint_quad(fill(well, hex(0x121417)));
                    let level = levels[t];
                    let lh = h * level;
                    let color = if level > 0.9 {
                        0xE5_4B_4B
                    } else if level > 0.7 {
                        0xE8_C1_4F
                    } else {
                        0x5C_C9_6B
                    };
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.origin.x, px(y + h - lh)),
                            size(px(METER_W), px(lh)),
                        ),
                        hex(color),
                    ));
                }
            },
        )
        .size_full()
    }
}
