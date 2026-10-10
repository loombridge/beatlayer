//! `--selftest`: drives every interaction in the brief through GPUI's real
//! event dispatch on the real window (`Window::dispatch_event`), checks the
//! result, prints one line per check, and exits non-zero if any failed.
//!
//! What this covers and what it doesn't: events enter GPUI where the platform
//! layer would hand them over, so hit testing, listeners, focus, the text
//! input handler, and view updates are all real. The macOS NSEvent-to-GPUI
//! conversion is skipped, except for pointer lock: there the drag deltas go
//! through AppKit's event queue (`NSApp.postEvent`) so the lock's event
//! monitor sees them as it would see a mouse.

use std::time::Duration;

use gpui::{
    AsyncWindowContext, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollStrategy, ScrollWheelEvent,
    TouchPhase, WeakEntity, Window, point, px,
};

use raw_window_handle::HasWindowHandle;

use crate::MainView;
use crate::data::BARS;
use crate::platform;
use crate::session::TRACK_H;

struct Report {
    lines: Vec<String>,
    failed: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        if !ok {
            self.failed += 1;
        }
        let line = format!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        eprintln!("selftest: {line}");
        self.lines.push(line);
    }
}

fn mouse_move(
    window: &mut Window,
    cx: &mut gpui::App,
    at: Point<Pixels>,
    pressed: Option<MouseButton>,
) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: pressed,
            modifiers: Modifiers::default(),
        }),
        cx,
    );
}

fn mouse_down(window: &mut Window, cx: &mut gpui::App, at: Point<Pixels>, clicks: usize) {
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: clicks,
            first_mouse: false,
        }),
        cx,
    );
}

fn mouse_up(window: &mut Window, cx: &mut gpui::App, at: Point<Pixels>, clicks: usize) {
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: clicks,
        }),
        cx,
    );
}

fn scroll(
    window: &mut Window,
    cx: &mut gpui::App,
    at: Point<Pixels>,
    dx: f32,
    dy: f32,
    platform_key: bool,
) {
    window.dispatch_event(
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Modifiers {
                platform: platform_key,
                ..Modifiers::default()
            },
            touch_phase: TouchPhase::Moved,
        }),
        cx,
    );
}

async fn pause(cx: &mut AsyncWindowContext, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Waits until GPUI has drawn two more frames, so listeners registered at
/// paint time (drags) exist and layout reflects the last step.
async fn settle(this: &WeakEntity<MainView>, cx: &mut AsyncWindowContext) {
    let start = this.update_in(cx, |this, _, _| this.frames).unwrap_or(0);
    for _ in 0..400 {
        pause(cx, 5).await;
        let now = this
            .update_in(cx, |this, _, _| this.frames)
            .unwrap_or(u64::MAX);
        if now >= start + 2 {
            return;
        }
    }
    eprintln!("selftest: no frames for 2 s; is the window hidden?");
}

pub async fn run(this: WeakEntity<MainView>, cx: &mut AsyncWindowContext) {
    let mut r = Report {
        lines: Vec::new(),
        failed: 0,
    };
    pause(cx, 2000).await;
    // Real mouse input would race the scripted events, so the window ignores
    // it for the duration (the process exits at the end).
    let _ = cx.update(|window, _| {
        if let Ok(h) = window.window_handle() {
            platform::set_ignores_mouse_events(h.as_raw(), true);
        }
    });

    // Vertical scroll over the arrangement.
    let lanes = this
        .update_in(cx, |this, _, cx| this.timeline.read(cx).lanes())
        .expect("main view");
    let _ = cx.update(|window, cx| {
        mouse_move(window, cx, lanes.center(), None);
        scroll(window, cx, lanes.center(), 0.0, -220.0, false);
    });
    settle(&this, cx).await;
    let _ = this.update_in(cx, |this, _, cx| {
        let s = this.session.read(cx);
        r.check(
            "vertical scroll",
            (s.scroll_y - 220.0).abs() < 0.01,
            format!("scroll_y = {}", s.scroll_y),
        );
    });

    // Horizontal scroll: 240 px at 24 px per bar is 10 bars.
    let _ = cx.update(|window, cx| scroll(window, cx, lanes.center(), -240.0, 0.0, false));
    settle(&this, cx).await;
    let _ = this.update_in(cx, |this, _, cx| {
        let s = this.session.read(cx);
        r.check(
            "horizontal scroll",
            (s.scroll_x - 10.0).abs() < 1e-6,
            format!("scroll_x = {} bars", s.scroll_x),
        );
    });

    // Zoom with the platform key held: exp(0.01 * 69.3147) = 2x, anchored at
    // the pointer.
    let anchor = lanes.center();
    let before = this
        .update_in(cx, |this, _, cx| {
            let s = this.session.read(cx);
            (s.px_per_bar, s.bar_at(f32::from(anchor.x - lanes.origin.x)))
        })
        .expect("main view");
    let _ = cx.update(|window, cx| scroll(window, cx, anchor, 0.0, 69.3147, true));
    settle(&this, cx).await;
    let _ = this.update_in(cx, |this, _, cx| {
        let s = this.session.read(cx);
        let bar_after = s.bar_at(f32::from(anchor.x - lanes.origin.x));
        r.check(
            "zoom",
            (s.px_per_bar - before.0 * 2.0).abs() < 0.05 && (bar_after - before.1).abs() < 1e-3,
            format!(
                "{:.1} -> {:.1} px per bar, bar under pointer {:.3} -> {:.3}",
                before.0, s.px_per_bar, before.1, bar_after
            ),
        );
    });

    // Clip drag: one bar right and one track down, into free space.
    let plan = this
        .update_in(cx, |this, _, cx| {
            let s = this.session.read(cx);
            let lanes = this.timeline.read(cx).lanes();
            let first = s.track_at(0.0);
            let last_bar = s.scroll_x + s.visible_bars() - 2.0;
            for t in first + 1..first + 8 {
                for c in s.project.clips_in(t, s.scroll_x + 0.5..last_bar) {
                    let clip = &s.project.clips[c];
                    if clip.start < s.scroll_x || clip.end() + 1.0 > last_bar.min(BARS as f64) {
                        continue;
                    }
                    let (to_start, to_track) = (clip.start + 1.0, t + 1);
                    let free = s
                        .project
                        .clips_in(to_track, to_start..to_start + clip.bars as f64)
                        .next()
                        .is_none();
                    if free {
                        let x = lanes.origin.x
                            + px(((clip.start + 0.25 - s.scroll_x) as f32) * s.px_per_bar);
                        let y =
                            lanes.origin.y + px(t as f32 * TRACK_H - s.scroll_y + TRACK_H / 2.0);
                        return Some((c, t, clip.start, point(x, y), px(s.px_per_bar)));
                    }
                }
            }
            None
        })
        .ok()
        .flatten();
    match plan {
        Some((c, t, start, at, ppb)) => {
            let to = point(at.x + ppb, at.y + px(TRACK_H));
            let _ = cx.update(|window, cx| {
                mouse_move(window, cx, at, None);
            });
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| mouse_down(window, cx, at, 1));
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| mouse_move(window, cx, to, Some(MouseButton::Left)));
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| mouse_up(window, cx, to, 1));
            settle(&this, cx).await;
            let _ = this.update_in(cx, |this, _, cx| {
                let clip = &this.session.read(cx).project.clips[c];
                r.check(
                    "clip drag",
                    clip.track == t + 1 && (clip.start - (start + 1.0)).abs() < 1e-9,
                    format!(
                        "clip {c} moved from track {}, bar {} to track {}, bar {}",
                        t + 1,
                        start + 1.0,
                        clip.track + 1,
                        clip.start + 1.0
                    ),
                );
            });
        }
        None => r.check("clip drag", false, "no clip with free space found".into()),
    }

    // Knob drag with pointer lock. Pick the first visible track's knob.
    let knob = this
        .update_in(cx, |this, _, cx| {
            let s = this.session.read(cx);
            let t = s.track_at(0.0) + 1;
            let b = this.headers.read(cx).knob_bounds.borrow().get(&t).copied();
            b.map(|b| (t, b.center(), s.project.tracks[t].volume))
        })
        .ok()
        .flatten();
    match knob {
        Some((t, at, v0)) => {
            let _ = cx.update(|window, cx| {
                mouse_move(window, cx, at, None);
            });
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| mouse_down(window, cx, at, 1));
            settle(&this, cx).await;
            let locked = platform::pointer_locked();
            // Four drags of 10 points up, through AppKit's queue.
            let posted = platform::post_drag_deltas(&[(0.0, -10.0); 4]);
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| mouse_move(window, cx, at, Some(MouseButton::Left)));
            settle(&this, cx).await;
            let v1 = this
                .update_in(cx, |this, _, cx| {
                    this.session.read(cx).project.tracks[t].volume
                })
                .unwrap_or(f32::NAN);
            let _ = cx.update(|window, cx| mouse_up(window, cx, at, 1));
            settle(&this, cx).await;
            let unlocked = !platform::pointer_locked();
            let expected = (v0 + 40.0 / 200.0).min(1.0);
            r.check(
                "knob drag with pointer lock",
                locked && unlocked && posted == 4 && (v1 - expected).abs() < 0.011,
                format!(
                    "locked on press: {locked}, {posted} drag events of -10 pt posted, volume {v0:.3} -> {v1:.3} (expected {expected:.3}), unlocked on release: {unlocked}"
                ),
            );
        }
        None => r.check(
            "knob drag with pointer lock",
            false,
            "no knob bounds recorded".into(),
        ),
    }

    // Rename: double-click the track name, type, press Return.
    let name_at = this
        .update_in(cx, |this, _, cx| {
            let s = this.session.read(cx);
            let t = s.track_at(0.0) + 2;
            let b = this.headers.read(cx).knob_bounds.borrow().get(&t).copied();
            // The name sits 40 pt in from the header's left edge, level with the knob.
            b.map(|b| (t, point(px(40.0), b.center().y)))
        })
        .ok()
        .flatten();
    match name_at {
        Some((t, at)) => {
            let _ = cx.update(|window, cx| mouse_move(window, cx, at, None));
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| {
                mouse_down(window, cx, at, 1);
                mouse_up(window, cx, at, 1);
                mouse_down(window, cx, at, 2);
                mouse_up(window, cx, at, 2);
            });
            settle(&this, cx).await;
            let editing = this
                .update_in(cx, |this, _, cx| this.headers.read(cx).renaming())
                .ok()
                .flatten();
            let _ = cx.update(|window, cx| {
                for key in ["r", "e", "n", "a", "m", "e", "d", "space", "4", "2"] {
                    if let Ok(k) = Keystroke::parse(key) {
                        window.dispatch_keystroke(k, cx);
                    }
                }
            });
            settle(&this, cx).await;
            let _ = cx.update(|window, cx| {
                if let Ok(k) = Keystroke::parse("enter") {
                    window.dispatch_keystroke(k, cx);
                }
            });
            settle(&this, cx).await;
            let _ = this.update_in(cx, |this, _, cx| {
                let name = this.session.read(cx).project.tracks[t].name.clone();
                let done = this.headers.read(cx).renaming().is_none();
                r.check(
                    "rename by double-click",
                    editing == Some(t) && &*name == "renamed 42" && done,
                    format!(
                        "field opened for track {:?}, name now {name:?}, field closed: {done}",
                        editing.map(|t| t + 1)
                    ),
                );
            });
        }
        None => r.check(
            "rename by double-click",
            false,
            "no header bounds recorded".into(),
        ),
    }

    // Browser: wheel scroll, then jump near the end of the 100,000 names.
    let _ = cx.update(|window, cx| {
        let at = point(window.viewport_size().width - px(120.0), px(500.0));
        mouse_move(window, cx, at, None);
        scroll(window, cx, at, 0.0, -2000.0, false);
    });
    settle(&this, cx).await;
    let top = this
        .update_in(cx, |this, _, cx| browser_top_row(this, cx))
        .unwrap_or(0);
    let _ = this.update_in(cx, |this, _, cx| {
        this.browser.update(cx, |b, cx| {
            b.scroll.scroll_to_item(99_990, ScrollStrategy::Top);
            cx.notify();
        });
    });
    settle(&this, cx).await;
    let end = this
        .update_in(cx, |this, _, cx| browser_top_row(this, cx))
        .unwrap_or(0);
    r.check(
        "browser list",
        top == 100 && end >= 99_950,
        format!("2,000 pt of wheel scroll shows row {top}; jumping to 99,990 shows row {end}"),
    );

    // The plug-in window opened a second after launch. Click the middle of
    // its child view (100 pt up from the bottom of a 360 pt window, well
    // below GPUI's 32 pt strip) through AppKit's hit testing.
    let draws = platform::child_view_draws();
    let plugin = platform::PLUGIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    let clicks_before = platform::child_view_clicks();
    let posted = plugin != 0 && platform::post_click(plugin, 240.0, 100.0);
    pause(cx, 200).await;
    let clicks = platform::child_view_clicks() - clicks_before;
    r.check(
        "plug-in child view",
        draws > 0 && posted && clicks == 1,
        format!(
            "the embedded NSView in window {plugin} has drawn {draws} times; a posted click reached its mouseDown: {clicks} times"
        ),
    );

    println!("selftest: {} checks, {} failed", r.lines.len(), r.failed);
    for line in &r.lines {
        println!("  {line}");
    }
    platform::unlock_pointer();
    std::process::exit(if r.failed == 0 { 0 } else { 1 });
}

/// The browser row at the top of its viewport, from the list's scroll offset
/// (rows are 20 pt high).
fn browser_top_row(this: &MainView, cx: &gpui::App) -> usize {
    let offset = this.browser.read(cx).scroll.0.borrow().base_handle.offset();
    (-f32::from(offset.y) / 20.0).round().max(0.0) as usize
}
