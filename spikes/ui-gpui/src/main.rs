//! WP-0.8a: the UI benchmark scene in GPUI.
//!
//! Run `cargo run --release` for the interactive scene, `-- --bench` for the
//! scripted 30-second scenario, and `-- --idle` for the idle measurement.
//! the UI spike reports describes every mode and records the results.

mod bench;
mod browser;
mod data;
mod headers;
mod platform;
mod selftest;
mod session;
mod timeline;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, Role, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    px, rgb, size,
};
use gpui_component::Root;
use raw_window_handle::HasWindowHandle;

use crate::bench::{Recorder, SETTLE_SECONDS, scenario};
use crate::browser::{BROWSER_W, Browser};
use crate::data::{Project, SEED};
use crate::headers::{HEADER_W, METER_W, MeterColumn, Meters, TrackHeaders};
use crate::session::Session;
use crate::timeline::{RULER_H, Timeline};

actions!(
    spike,
    [ZoomIn, ZoomOut, ToggleMeters, OpenPluginWindow, Quit]
);

const WINDOW_W: f32 = 1600.0;
const WINDOW_H: f32 = 1000.0;
const TOOLBAR_H: f32 = 32.0;
const PLUGIN_STRIP_H: f32 = 32.0;

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

#[derive(Debug, Default)]
struct Args {
    bench: bool,
    bench_seconds: f64,
    bench_out: Option<PathBuf>,
    idle: bool,
    plugin_window: bool,
    a11y_dump: Option<PathBuf>,
    a11y_active: bool,
    no_cache: bool,
    selftest: bool,
    screenshot: Option<PathBuf>,
    screenshot_delay: f64,
    quit_after: Option<f64>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = Args {
            bench_seconds: 30.0,
            plugin_window: true,
            screenshot_delay: 2.0,
            ..Default::default()
        };
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            let mut value = || it.next().ok_or(format!("{a} needs a value"));
            match a.as_str() {
                "--bench" => args.bench = true,
                "--bench-seconds" => {
                    args.bench_seconds = value()?.parse().map_err(|e| format!("{e}"))?
                }
                "--bench-out" => args.bench_out = Some(value()?.into()),
                "--idle" => args.idle = true,
                "--no-plugin-window" => args.plugin_window = false,
                "--a11y-dump" => args.a11y_dump = Some(value()?.into()),
                "--a11y-active" => args.a11y_active = true,
                "--no-cache" => args.no_cache = true,
                "--no-layers" => {
                    timeline::USE_LAYERS.store(false, std::sync::atomic::Ordering::Relaxed)
                }
                "--selftest" => args.selftest = true,
                "--screenshot" => args.screenshot = Some(value()?.into()),
                "--screenshot-delay" => {
                    args.screenshot_delay = value()?.parse().map_err(|e| format!("{e}"))?
                }
                "--quit-after" => {
                    args.quit_after = Some(value()?.parse().map_err(|e| format!("{e}"))?)
                }
                "-h" | "--help" => {
                    println!(
                        "bl-spike-ui-gpui [--bench [--bench-seconds N] [--bench-out PREFIX]] [--idle]\n\
                         \t[--no-plugin-window] [--a11y-dump PREFIX] [--a11y-active] [--no-cache] [--no-layers]\n\
                         \t[--quit-after SECONDS] [--selftest]\n\
                         \t[--screenshot PREFIX [--screenshot-delay SECONDS]] (needs --features screenshot)"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(args)
    }
}

struct BenchRun {
    launched: Instant,
    start: Option<Instant>,
    recorder: Recorder,
}

struct MainView {
    args: Rc<Args>,
    session: Entity<Session>,
    meters: Entity<Meters>,
    headers: Entity<TrackHeaders>,
    meter_column: Entity<MeterColumn>,
    timeline: Entity<Timeline>,
    browser: Entity<Browser>,
    focus: FocusHandle,
    meters_running: bool,
    frame_pending: bool,
    first_frame: bool,
    /// Frame callbacks run so far.
    frames: u64,
    launched: Instant,
    bench: Option<BenchRun>,
}

impl MainView {
    fn new(
        args: Rc<Args>,
        project: Project,
        launched: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = cx.new(|_| Session::new(project));
        let meters = cx.new(|_| Meters::new());
        let headers = cx.new(|cx| TrackHeaders::new(session.clone(), cx));
        let meter_column = cx.new(|cx| MeterColumn::new(session.clone(), meters.clone(), cx));
        let timeline = cx.new(|cx| Timeline::new(session.clone(), cx));
        let browser = cx.new(|_| Browser::new());
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Never leave the pointer locked when the window loses focus mid-drag.
        cx.observe_window_activation(window, |_, window, _| {
            if !window.is_window_active() {
                platform::unlock_pointer();
            }
        })
        .detach();
        if let Some(prefix) = args.a11y_dump.clone() {
            Self::dump_a11y(prefix, window, cx);
        }
        if args.selftest {
            cx.spawn_in(window, async move |this, cx| selftest::run(this, cx).await)
                .detach();
        }
        #[cfg(feature = "screenshot")]
        if let Some(prefix) = args.screenshot.clone() {
            Self::screenshot(prefix, args.screenshot_delay, window, cx);
        }
        #[cfg(not(feature = "screenshot"))]
        if args.screenshot.is_some() {
            eprintln!("screenshot: rebuild with --features screenshot");
        }
        let bench = args.bench.then(|| BenchRun {
            launched,
            start: None,
            recorder: Recorder::default(),
        });
        Self {
            meters_running: !args.idle,
            args,
            session,
            meters,
            headers,
            meter_column,
            timeline,
            browser,
            focus,
            frame_pending: false,
            first_frame: true,
            frames: 0,
            launched,
            bench,
        }
    }

    /// Dumps the window's accessibility tree to `<prefix>.txt` (the platform
    /// tree, walked in-process) and `<prefix>.json` (GPUI's own record).
    ///
    /// AccessKit builds the tree only when something asks for it, so the
    /// first walk activates it; GPUI then sends the full tree with the next
    /// frame, and the second walk reads it.
    fn dump_a11y(prefix: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update_in(cx, |_, window, _| {
                let first = window
                    .window_handle()
                    .map(|h| platform::dump_accessibility(h.as_raw(), 50))
                    .unwrap_or_default();
                eprintln!(
                    "a11y: activation walk saw {} lines; active now: {}",
                    first.lines().count(),
                    window.is_a11y_active()
                );
                window.refresh();
            })
            .ok();
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update_in(cx, |_, window, _| {
                let tree = window
                    .window_handle()
                    .map(|h| platform::dump_accessibility(h.as_raw(), 5000))
                    .unwrap_or_default();
                let txt = prefix.with_extension("txt");
                let ok = std::fs::write(&txt, &tree).is_ok();
                eprintln!("a11y: wrote the platform tree to {} ({ok})", txt.display());
                if let Some(json) = window.debug_a11y_tree_json() {
                    let path = prefix.with_extension("json");
                    let ok = std::fs::write(&path, json).is_ok();
                    eprintln!("a11y: wrote GPUI's tree to {} ({ok})", path.display());
                }
            })
            .ok();
        })
        .detach();
    }

    /// Saves the main window's last frame as `<prefix>.png`, rendered
    /// off-screen by GPUI (no screen recording involved). Only in builds with
    /// the `screenshot` feature, which turns on GPUI's test support.
    #[cfg(feature = "screenshot")]
    fn screenshot(prefix: PathBuf, delay: f64, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f64(delay))
                .await;
            this.update_in(cx, |_, window, _| {
                let path = prefix.with_extension("png");
                match window.render_to_image().map(|img| img.save(&path)) {
                    Ok(Ok(())) => eprintln!("screenshot: wrote {}", path.display()),
                    Ok(Err(e)) => eprintln!("screenshot: save failed: {e}"),
                    Err(e) => eprintln!("screenshot: render failed: {e}"),
                }
            })
            .ok();
        })
        .detach();
    }

    fn schedule_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frame_pending {
            return;
        }
        self.frame_pending = true;
        let view = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            view.update(cx, |this, cx| {
                this.frame_pending = false;
                this.on_frame(window, cx);
            })
            .ok();
        });
    }

    /// Runs once per display frame while anything animates, just before GPUI
    /// draws.
    fn on_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.frames += 1;
        if self.first_frame {
            self.first_frame = false;
            let window_id = window
                .window_handle()
                .ok()
                .and_then(|h| platform::window_number(h.as_raw()));
            eprintln!(
                "ready: pid {}, window {:?}, first frame {:.0} ms after launch, scale factor {}",
                std::process::id(),
                window_id,
                now.duration_since(self.launched).as_secs_f64() * 1e3,
                window.scale_factor()
            );
            if self.args.a11y_active {
                // Ask for the accessibility tree as a screen reader would, so
                // AccessKit activates and GPUI builds the tree every frame.
                if let Ok(h) = window.window_handle() {
                    platform::dump_accessibility(h.as_raw(), 1);
                }
                window.refresh();
            }
            if let Some(secs) = self.args.quit_after {
                cx.spawn(async move |_, cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs_f64(secs))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        }

        let duration = self.args.bench_seconds;
        let mut finished = false;
        if let Some(run) = &mut self.bench {
            let lanes_width = self.session.read(cx).lanes_width.get();
            let t = if now.duration_since(run.launched).as_secs_f64() < SETTLE_SECONDS {
                0.0
            } else {
                let start = *run.start.get_or_insert(now);
                run.recorder
                    .frame(now, self.timeline.read(cx).stats.get().quads);
                now.duration_since(start).as_secs_f64()
            };
            if t >= duration {
                finished = true;
            } else {
                let (x, zoom) = scenario(t, duration, lanes_width);
                self.session
                    .update(cx, |s, cx| s.set_horizontal(x, zoom, cx));
            }
        }
        if finished {
            self.finish_bench(window, cx);
            return;
        }

        if self.meters_running {
            self.meters.update(cx, |m, cx| m.tick(now, cx));
        }
        if self.meters_running || self.bench.is_some() {
            self.schedule_frame(window, cx);
        }
    }

    fn finish_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.bench.take() else { return };
        let s = self.session.read(cx);
        let report = run.recorder.report(
            self.args.bench_seconds,
            window.scale_factor(),
            (s.lanes_width.get(), s.lanes_height.get()),
        );
        println!("{report}");
        if let Some(prefix) = &self.args.bench_out {
            let txt = prefix.with_extension("txt");
            let csv = prefix.with_extension("csv");
            let ok = std::fs::write(&txt, &report).is_ok()
                && std::fs::write(&csv, run.recorder.csv()).is_ok();
            eprintln!(
                "bench: wrote {} and {} ({ok})",
                txt.display(),
                csv.display()
            );
        }
        cx.quit();
    }

    fn toggle_meters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.meters_running = !self.meters_running;
        if self.meters_running {
            self.schedule_frame(window, cx);
        }
        cx.notify();
    }

    fn zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            let w = s.lanes_width.get();
            s.zoom_at(factor, w / 2.0, cx);
        });
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let button = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .tab_stop(true)
                .focusable()
                .px(px(10.0))
                .h(px(22.0))
                .flex()
                .items_center()
                .rounded(px(3.0))
                .bg(hex(0x3A3E46))
                .cursor_pointer()
                .child(label)
        };
        div()
            .h(px(TOOLBAR_H))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(10.0))
            .bg(hex(0x2F3238))
            .border_b_1()
            .border_color(hex(0x15171A))
            .child(
                div()
                    .text_color(hex(0x8A9099))
                    .child(SharedString::new_static("Beatlayer, GPUI spike")),
            )
            .child(
                button(
                    "meters",
                    if self.meters_running {
                        "Stop meters"
                    } else {
                        "Start meters"
                    }
                    .into(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_meters(window, cx))),
            )
            .child(
                button("plugin", "Open plug-in window".into())
                    .on_click(|_, _, cx| open_plugin_window(cx)),
            )
            .child(
                button("zoom-out", "Zoom out".into())
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(0.8, cx))),
            )
            .child(
                button("zoom-in", "Zoom in".into())
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.25, cx))),
            )
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.first_frame {
            self.schedule_frame(window, cx);
        }
        let cache = !window.is_a11y_active() && !self.args.no_cache;
        div()
            .id("spike-root")
            .role(Role::Application)
            .aria_label("Beatlayer GPUI spike")
            .key_context("Spike")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(1.25, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(0.8, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleMeters, window, cx| this.toggle_meters(window, cx)),
            )
            .on_action(|_: &OpenPluginWindow, _, cx| open_plugin_window(cx))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(0x1B1D21))
            .text_color(hex(0xDDE1E6))
            .text_size(px(12.0))
            .child(self.toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(HEADER_W))
                            .h_full()
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(hex(0x15171A))
                            .child(
                                div()
                                    .h(px(RULER_H))
                                    .px(px(10.0))
                                    .flex()
                                    .items_center()
                                    .bg(hex(0x2A2D33))
                                    .text_color(hex(0x8A9099))
                                    .child(SharedString::new_static("200 tracks")),
                            )
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(view_slot(
                                        &self.headers,
                                        StyleRefinement::default().absolute().size_full(),
                                        cache,
                                    ))
                                    .child(view_slot(
                                        &self.meter_column,
                                        StyleRefinement::default()
                                            .absolute()
                                            .top_0()
                                            .right(px(6.0))
                                            .w(px(METER_W))
                                            .h_full(),
                                        cache,
                                    )),
                            ),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(view_slot(
                        &self.timeline,
                        StyleRefinement::default().size_full(),
                        cache,
                    )))
                    .child(div().w(px(BROWSER_W)).h_full().child(view_slot(
                        &self.browser,
                        StyleRefinement::default().size_full(),
                        cache,
                    ))),
            )
    }
}

/// Embeds a child view, cached unless an assistive client is connected.
///
/// In gpui-pre 0.3.8 a cached view that's reused for a frame (not notified)
/// contributes no accessibility nodes to that frame's tree, because
/// `Window::reuse_prepaint` copies hitboxes, listeners, and focus but not
/// AccessKit nodes. Screen readers would see the view vanish, so caching is
/// turned off while the accessibility tree is active.
fn view_slot<V: Render>(
    entity: &Entity<V>,
    style: StyleRefinement,
    cache: bool,
) -> gpui::AnyElement {
    if cache {
        entity.clone().cached(style).into_any_element()
    } else {
        let mut slot = div();
        *slot.style() = style;
        slot.child(entity.clone()).into_any_element()
    }
}

/// The second top-level window: a GPUI window whose native view gets a
/// child NSView, as a plug-in editor would add one.
struct PluginHost {
    status: SharedString,
}

impl Render for PluginHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("plugin-host")
            .role(Role::Group)
            .aria_label("Plug-in editor")
            .size_full()
            .bg(hex(0x2A2D33))
            .text_color(hex(0xDDE1E6))
            .text_size(px(12.0))
            .child(
                div()
                    .h(px(PLUGIN_STRIP_H))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .child(self.status.clone()),
            )
    }
}

fn open_plugin_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(480.0), px(360.0)), cx);
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Plug-in editor (stand-in)".into()),
                ..Default::default()
            }),
            focus: false,
            ..Default::default()
        },
        |window, cx| {
            // The window and its NSView exist by the time GPUI builds the root
            // view, and the event loop is running, so the child view goes in here.
            let status = match window.window_handle() {
                Ok(handle) => {
                    let id = platform::window_number(handle.as_raw());
                    platform::embed_child_view(handle.as_raw(), PLUGIN_STRIP_H as f64)
                        .map(|msg| format!("window {id:?}: {msg}"))
                }
                Err(e) => Err(format!("no raw window handle: {e}")),
            };
            match &status {
                Ok(msg) => eprintln!("plug-in window: {msg}"),
                Err(msg) => eprintln!("plug-in window: embedding failed: {msg}"),
            }
            let status = match status {
                Ok(_) => {
                    "Native child view embedded below this strip (click it: clicks go to stderr)"
                        .into()
                }
                Err(e) => SharedString::from(format!("Embedding failed: {e}")),
            };
            cx.new(|_| PluginHost { status })
        },
    );
    if let Err(e) = result {
        eprintln!("plug-in window: open_window failed: {e}");
    }
}

fn main() {
    let launched = Instant::now();
    let args = match Args::parse() {
        Ok(args) => Rc::new(args),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let t = Instant::now();
    let project = Project::generate(SEED);
    let peak_bytes: usize = project
        .peaks
        .iter()
        .flat_map(|p| &p.levels)
        .map(|l| l.len() * 4)
        .sum();
    eprintln!(
        "data: {} tracks, {} clips, {:.1} MB of peaks, generated in {:.0} ms",
        project.tracks.len(),
        project.clips.len(),
        peak_bytes as f64 / 1e6,
        t.elapsed().as_secs_f64() * 1e3
    );

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-=", ZoomIn, None),
            KeyBinding::new("cmd--", ZoomOut, None),
            KeyBinding::new("cmd-shift-m", ToggleMeters, None),
            KeyBinding::new("cmd-shift-p", OpenPluginWindow, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        let bounds = Bounds::centered(None, size(px(WINDOW_W), px(WINDOW_H)), cx);
        let bench = args.bench;
        let main_args = args.clone();
        let main = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Beatlayer GPUI spike".into()),
                    ..Default::default()
                }),
                // The bench measures the toolkit, not GPUI's 30 fps throttle for
                // inactive windows.
                inactive_frame_interval: if bench {
                    None
                } else {
                    Some(Duration::from_micros(33_333))
                },
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| MainView::new(main_args, project, launched, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        );
        let main_id = main.map(|w| w.window_id()).ok();
        cx.on_window_closed(move |cx, id| {
            if Some(id) == main_id {
                platform::unlock_pointer();
                cx.quit();
            }
        })
        .detach();
        if args.plugin_window && !args.bench {
            // Open the plug-in window a second after launch, from a task on the
            // running event loop, as a host opens an editor on demand.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                cx.update(open_plugin_window);
            })
            .detach();
        }
        cx.activate(true);
    });
}
