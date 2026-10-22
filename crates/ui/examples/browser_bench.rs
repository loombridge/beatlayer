//! The browser's CPU time per frame (WP-1.10; spec: browser R17; ADR-001,
//! "Consequences": each UI package reports its CPU per frame).
//!
//! It makes a library of 100,000 empty audio files in a temporary folder,
//! gives it to a headless shell as the User Library, and shows the Samples
//! label, so the content pane lists all 100,000. Each frame scrolls the list
//! (a steady wheel, so every frame shows new rows) and moves the selection,
//! as Down does, and the main window is drawn at 1600 × 1000 points and 2
//! pixels per point. It records the main thread's CPU time for egui's pass
//! (the browser, the Arrangement View beside it, and the rest of the window)
//! and tessellation, with the browser shown and then hidden; the difference
//! is the browser's. The GPU's share is egui's own text and rectangles,
//! which the Arrangement benchmark measures with the same renderer.
//!
//! ```sh
//! cargo run --release -p bl-ui --example browser_bench -- [--frames N] [--files N] [--accesskit] [--scene]
//! ```
//!
//! With `--scene`, the Arrangement beside it shows the Arrangement
//! benchmark's scene (200 tracks, 5,000 clips with waveforms) instead of a
//! new set.

use std::path::PathBuf;
use std::time::Duration;

use bl_shell::Shell;
use bl_shell::browser_commands as ids;
use bl_ui::arrangement::benchmark;
use bl_ui::main_window::{MainWindowUi, UiEvent};
use bl_ui::theme::{Theme, fonts};
use egui::{Event, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, vec2};
use serde_json::json;

/// CPU time the calling thread has used.
#[cfg(unix)]
fn thread_cpu_time() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec, and CLOCK_THREAD_CPUTIME_ID
    // is supported on macOS 10.12 and later and on Linux.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Duration::ZERO;
    }
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Wall time stands in where thread CPU time isn't read.
#[cfg(not(unix))]
fn thread_cpu_time() -> Duration {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

/// Runs `frames` frames and returns each one's CPU time in milliseconds,
/// sorted, and the most rows the browser built in one.
fn run(shell: &mut Shell, frames: usize, accesskit: bool, scene: bool) -> (Vec<f64>, usize) {
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts());
    if accesskit {
        ctx.enable_accesskit();
    }
    let theme = Theme::of(&shell);
    theme.apply(&ctx);
    let mut main = MainWindowUi::new();
    if scene {
        // The same seeded scene the shell shows, for its peaks.
        let scene = benchmark::scene(200, 25, 64);
        main.arrangement_mut()
            .set_peak_source(Box::new(scene.peaks), None);
    }
    let size = vec2(1600.0, 1000.0);
    let mut times = Vec::with_capacity(frames);
    let mut most_rows = 0;
    let list_point = Pos2::new(200.0, 400.0);
    for frame in 0..frames + 30 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(frame as f64 / 60.0),
            events: vec![
                Event::PointerMoved(list_point),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: vec2(0.0, -170.0),
                    phase: TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        ctx.set_pixels_per_point(2.0);
        let started = thread_cpu_time();
        let shell_ref = &*shell;
        let mut events = Vec::new();
        let output = ctx.run_ui(input, |ui| {
            events = main.show(ui, shell_ref, &theme, true, None);
        });
        let _ = ctx.tessellate(output.shapes, output.pixels_per_point);
        let took = thread_cpu_time() - started;
        for event in events {
            if let UiEvent::ArrangementViewport { width, height } = event {
                shell.set_arrangement_viewport(width, height);
            }
        }
        // The selection moves on, as Down does.
        if let Some(item) = shell.browser_view().row(frame * 10) {
            let _ = shell.run(ids::SELECT, json!({ "paths": [item.path] }));
        }
        if frame >= 30 {
            times.push(took.as_secs_f64() * 1000.0);
            most_rows = most_rows.max(main.browser().stats().rows_built);
        }
    }
    times.sort_by(f64::total_cmp);
    (times, most_rows)
}

fn main() {
    let mut frames = 600usize;
    let mut files = 100_000usize;
    let mut accesskit = false;
    let mut scene = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next().and_then(|n| n.parse().ok()).expect("a number"),
            "--files" => files = args.next().and_then(|n| n.parse().ok()).expect("a number"),
            "--accesskit" => accesskit = true,
            "--scene" => scene = true,
            other => panic!("unknown option {other}"),
        }
    }
    let root = std::env::temp_dir().join(format!("bl-browser-ui-bench-{}", std::process::id()));
    let library = root.join("User Library");
    let mut folder = PathBuf::new();
    for i in 0..files {
        if i % 200 == 0 {
            folder = library.join(format!("Folder {}", i / 200));
            std::fs::create_dir_all(&folder).expect("made a folder");
        }
        std::fs::File::create(folder.join(format!("Sample {i}.wav"))).expect("made a file");
    }
    let mut shell = if scene {
        benchmark::shell_with(benchmark::scene(200, 25, 64).editor)
    } else {
        Shell::headless()
    };
    shell.set_user_library(Some(library));
    while shell.browser_view().index_state().indexing {
        shell.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    shell.poll();
    shell
        .run(ids::OPEN_PLACE, json!({ "label": "samples" }))
        .expect("opened Samples");
    // No preview while the selection moves.
    shell
        .run(ids::SET_PREVIEW, json!({ "on": false }))
        .expect("switched preview off");
    println!(
        "the content pane lists {} rows{}{}",
        shell.browser_view().row_count(),
        if scene {
            ", beside the benchmark scene (200 tracks, 5,000 clips)"
        } else {
            ""
        },
        if accesskit { ", AccessKit on" } else { "" }
    );
    let (shown, rows) = run(&mut shell, frames, accesskit, scene);
    shell
        .run(bl_shell::commands::VIEW_TOGGLE_BROWSER, json!({}))
        .expect("hid the browser");
    let (hidden, _) = run(&mut shell, frames, accesskit, scene);
    let line = |name: &str, t: &[f64]| {
        println!(
            "{name}: median {:.3} ms, p99 {:.3} ms, max {:.3} ms over {} frames",
            percentile(t, 0.5),
            percentile(t, 0.99),
            t[t.len() - 1],
            t.len()
        );
    };
    line("window with the browser", &shown);
    line("window without the browser", &hidden);
    println!(
        "the browser's share: {:.3} ms median, {:.3} ms p99; at most {rows} rows built a frame",
        percentile(&shown, 0.5) - percentile(&hidden, 0.5),
        percentile(&shown, 0.99) - percentile(&hidden, 0.99),
    );
    let _ = std::fs::remove_dir_all(&root);
}
