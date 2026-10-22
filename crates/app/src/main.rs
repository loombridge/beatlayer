//! The Beatlayer application: the main window and the Settings window,
//! native menus, file dialogs, and preferences, wired to the headless shell
//! (`bl-shell`) and drawn by `bl-ui` (WP-1.7).

mod capture;
mod host;
#[cfg(any(target_os = "macos", windows))]
mod native_menu;
mod screenshot;
#[cfg(target_os = "macos")]
mod title_bar;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
Usage: beatlayer [options] [set file]

Options:
  -V, --version          Print the version and exit
  -h, --help             Print this message and exit
      --check            Load the keymap, the menus, the themes, and the command
                         registry without opening a window, report, and exit
      --check-audio      Open the default audio output for a second without
                         opening a window, report, and exit (3 if no device
                         could start)
      --config-dir DIR   Keep preferences in DIR instead of the platform's place
      --null-audio       Play through the null device instead of the sound card
      --window-size WxH  Open the main window at W x H points, title bar included
      --settings         Open the Settings window at start-up
      --drawn-menus      Draw the menu bar in the window, as on Linux
      --screenshot FILE  Draw the main window (with --settings, the Settings
                         window) offscreen, save it as a PNG, and quit; no
                         window opens
      --benchmark-scene  Show the Arrangement benchmark scene (200 tracks,
                         5,000 clips with waveforms) instead of a new set
      --user-library DIR Use DIR as the browser's User Library (made if it's
                         missing) instead of the preference or the default
      --command ID[=JSON]
                         Run a command at start-up, such as
                         arrangement.optimize_height or
                         arrangement.zoom='{\"factor\":4}'; repeatable
";

/// What the command line asked for.
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub config_dir: Option<PathBuf>,
    pub null_audio: bool,
    pub window_size: Option<(f32, f32)>,
    pub open_settings: bool,
    pub drawn_menus: bool,
    pub screenshot: Option<PathBuf>,
    pub set: Option<PathBuf>,
    pub benchmark_scene: bool,
    pub commands: Vec<(String, serde_json::Value)>,
    pub user_library: Option<PathBuf>,
}

enum Parsed {
    Run(Args),
    Exit(ExitCode),
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut parsed = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("{}", version_line());
                return Ok(Parsed::Exit(ExitCode::SUCCESS));
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(Parsed::Exit(ExitCode::SUCCESS));
            }
            "--check" => return Ok(Parsed::Exit(check())),
            "--check-audio" => return Ok(Parsed::Exit(check_audio())),
            "--config-dir" => {
                parsed.config_dir = Some(it.next().ok_or("--config-dir takes a directory")?.into());
            }
            "--null-audio" => parsed.null_audio = true,
            "--window-size" => {
                let size = it
                    .next()
                    .ok_or("--window-size takes WxH, such as 1600x1000")?;
                let (w, h) = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| format!("--window-size takes WxH, not {size}"))?;
                parsed.window_size = Some((w, h));
            }
            "--settings" => parsed.open_settings = true,
            "--drawn-menus" => parsed.drawn_menus = true,
            "--screenshot" => {
                parsed.screenshot = Some(it.next().ok_or("--screenshot takes a file")?.into());
            }
            "--benchmark-scene" => parsed.benchmark_scene = true,
            "--user-library" => {
                parsed.user_library =
                    Some(it.next().ok_or("--user-library takes a folder")?.into());
            }
            "--command" => {
                let text = it.next().ok_or("--command takes a command ID")?;
                let (id, args) = match text.split_once('=') {
                    Some((id, json)) => (
                        id.to_owned(),
                        serde_json::from_str(json).map_err(|e| {
                            format!("--command {id}: the arguments aren't JSON: {e}")
                        })?,
                    ),
                    None => (text.clone(), serde_json::json!({})),
                };
                parsed.commands.push((id, args));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n\n{USAGE}"));
            }
            // A set to open, such as one passed by a file association.
            path => parsed.set = Some(path.into()),
        }
    }
    Ok(Parsed::Run(parsed))
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)) {
        Ok(Parsed::Exit(code)) => code,
        Ok(Parsed::Run(args)) => match match &args.screenshot {
            Some(path) => screenshot::run(&args, path),
            None => host::run(args),
        } {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("beatlayer: {message}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("beatlayer: {message}");
            ExitCode::from(2)
        }
    }
}

/// What `--version` prints: the binary's name and the package version.
/// `cargo xtask package` checks every packaged binary against this.
fn version_line() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

/// What `--check-audio` exits with when no device could start, as on a
/// machine with no sound card. A crash shows as a signal instead.
const NO_AUDIO_DEVICE: u8 = 3;

/// `--check-audio`: opens the default output as the app does at start-up,
/// runs it for a second, and closes it, so a packaged binary shows its
/// audio path starts in a release build.
fn check_audio() -> ExitCode {
    let mut audio = bl_shell::audio::AudioHost::new(bl_shell::audio::AudioSettings::default());
    if let Err(error) = audio.start() {
        println!("{}: no audio device started: {error}", version_line());
        return ExitCode::from(NO_AUDIO_DEVICE);
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let status = audio.status();
    audio.stop();
    let device = status
        .output_device
        .map_or_else(|| "no output".to_owned(), |device| device.name);
    let ms = |value: Option<f64>| value.map_or_else(|| "?".to_owned(), |ms| format!("{ms:.1} ms"));
    println!(
        "{}: {} on {device} ran for a second at {} Hz; output latency {}",
        version_line(),
        status.driver,
        status
            .sample_rate
            .map_or_else(|| "?".to_owned(), |rate| rate.to_string()),
        ms(status.output_latency_ms),
    );
    ExitCode::SUCCESS
}

/// `--check`: builds everything the window needs except the window, so a
/// packaged binary can show its embedded data loads.
fn check() -> ExitCode {
    let shell = bl_shell::Shell::headless();
    let problems = shell.registry().problems();
    let keymap = bl_ui::keymap::Keymap::live12();
    let bound = keymap
        .entries()
        .iter()
        .filter(|e| e.status == bl_ui::keymap::Status::Bound)
        .count();
    let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let (items, separators) = menus.iter().flat_map(bl_shell::menus::Menu::walk).fold(
        (0usize, 0usize),
        |(items, separators), item| {
            if item.is_separator() {
                (items, separators + 1)
            } else {
                (items + 1, separators)
            }
        },
    );
    for dark in [true, false] {
        let _ = bl_ui::theme::Theme::bundled(dark);
    }
    println!(
        "{}: {} commands, {} problems; keymap of {} shortcuts, {bound} bound; {} menus, {items} \
         items, {separators} separators; themes and typeface loaded",
        version_line(),
        shell.registry().len(),
        problems.len(),
        keymap.entries().len(),
        menus.len(),
    );
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
