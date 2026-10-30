//! The display (`docs/specs/push-2.md`, R17 to R20): golden frames of the
//! bands, a parameter column with each dot, a notification, the count-in bar
//! and the progress bars; the first frame at the sink; a claim that fails.
//!
//! The golden frames are PNGs in `tests/golden`. `BL_UPDATE_GOLDEN=1 cargo
//! test` writes them again; a test never does it by itself.

use std::path::PathBuf;

use bl_push::frame::{HEIGHT, RGB_BYTES, WIDTH};
use bl_push::paint;
use bl_push::screen::{Column, Dot, Graphic, Label, Screen};

use crate::rig::Rig;

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.png"))
}

/// Compares `screen`'s image with the golden frame `name`: within a few
/// levels on a handful of pixels, since the edges of letters and arcs are
/// drawn with floating-point maths that systems round differently.
fn golden(name: &str, screen: &Screen) {
    let rgb = paint::render(screen);
    assert_eq!(rgb.len(), RGB_BYTES);
    let path = golden_path(name);
    if std::env::var_os("BL_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, paint::png(&rgb).unwrap()).unwrap();
        return;
    }
    let file = std::fs::File::open(&path).unwrap_or_else(|_| {
        panic!("no golden frame {path:?}: BL_UPDATE_GOLDEN=1 cargo test makes it")
    });
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .unwrap();
    let mut expected = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut expected).unwrap();
    assert_eq!((info.width as usize, info.height as usize), (WIDTH, HEIGHT));
    let differing = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .zip(expected.as_chunks::<3>().0)
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 3))
        .count();
    assert!(
        differing <= WIDTH * HEIGHT / 200,
        "{name}: {differing} pixels differ from {path:?}"
    );
}

fn label(text: &str, colour: [u8; 3], selected: bool) -> Label {
    Label::new(text, colour, selected)
}

fn column(name: &str, value: &str, graphic: Graphic, dot: Dot) -> Column {
    Column {
        name: name.to_owned(),
        value: value.to_owned(),
        graphic,
        dot,
        touched: false,
    }
}

const RED: [u8; 3] = [230, 70, 70];
const GREEN: [u8; 3] = [90, 200, 110];
const BLUE: [u8; 3] = [80, 130, 235];
const AMBER: [u8; 3] = [240, 180, 60];

fn tracks() -> Screen {
    let mut screen = Screen::default();
    for (n, (text, colour)) in [
        ("Drums", RED),
        ("Bass", AMBER),
        ("Pad", GREEN),
        ("A very long track name", BLUE),
    ]
    .into_iter()
    .enumerate()
    {
        screen.lower[n] = label(text, colour, n == 1);
    }
    for (n, text) in ["Volume", "Pan", "Send A", "Send B"]
        .into_iter()
        .enumerate()
    {
        screen.upper[n] = label(text, [200, 200, 200], n == 0);
    }
    screen
}

// Spec: push-2 R18. Eight columns of 120 pixels between an upper and a lower
// band of 24: labels in the colour of what they stand for, the selected one
// highlighted in that colour, long text cut to the column.
#[test]
fn the_bands_hold_the_labels() {
    golden("bands", &tracks());
}

// Spec: push-2 R20. A column shows the parameter's name, its value, an arc for a
// continuous parameter or the chosen entry of a list, a white dot beside the
// name while its automation plays and a grey one while it is overridden, and
// is highlighted while its encoder is touched.
#[test]
fn a_parameter_column_shows_its_value_and_each_dot() {
    let mut screen = tracks();
    screen.columns[0] = column("Cutoff", "1.20 kHz", Graphic::Arc(0.4), Dot::None);
    screen.columns[1] = column("Resonance", "34 %", Graphic::Arc(0.34), Dot::Automated);
    screen.columns[2] = column("Drive", "-6.0 dB", Graphic::Arc(0.9), Dot::Overridden);
    screen.columns[3] = column(
        "Filter Type",
        "Low-pass",
        Graphic::Choice("Low-pass".to_owned()),
        Dot::None,
    );
    screen.columns[4] = Column {
        touched: true,
        ..column("Attack", "10 ms", Graphic::Arc(0.1), Dot::None)
    };
    screen.columns[5] = column("Release", "0.0 ms", Graphic::Arc(0.0), Dot::None);
    screen.columns[6] = column("Amount", "100 %", Graphic::Arc(1.0), Dot::None);
    golden("parameters", &screen);
    assert!(
        screen.columns[7].is_empty(),
        "a column whose encoder controls nothing is empty"
    );
}

// Spec: push-2 R19. A notification is a box in the middle of the content band.
#[test]
fn a_notification_is_a_box_over_the_content() {
    let mut screen = tracks();
    screen.columns[0] = column("Cutoff", "1.20 kHz", Graphic::Arc(0.4), Dot::None);
    screen.notification = Some("Tempo 123.0 BPM".to_owned());
    golden("notification", &screen);
}

// Spec: push-2 R19. The count-in is a bar across the top of the upper band,
// flashing on each beat.
#[test]
fn the_count_in_is_a_bar_across_the_top() {
    let mut lit = tracks();
    lit.count_in = Some(true);
    golden("count_in", &lit);
    let mut dark = tracks();
    dark.count_in = Some(false);
    assert_ne!(paint::render(&lit), paint::render(&dark), "it flashes");
}

// Spec: push-2 R19. A progress bar under the label of each lower display
// button whose track has a clip playing.
#[test]
fn progress_bars_sit_under_the_lower_labels() {
    let mut screen = tracks();
    screen.progress = [
        Some(0.0),
        Some(0.5),
        Some(1.0),
        None,
        None,
        None,
        None,
        None,
    ];
    golden("progress", &screen);
}

// Spec: push-2 R31, R33. The Setup menu's curve over columns 4 and 5, and the
// User Mode screen.
#[test]
fn the_setup_curve_and_the_user_mode_screen() {
    let mut screen = Screen::default();
    screen.columns[0] = column("Pad Sensitivity", "5", Graphic::Arc(0.5), Dot::None);
    screen.curve = Some(bl_push::curve::curve(5, 5, 5).to_vec());
    golden("setup", &screen);
    let user = Screen {
        title: Some("Push 2 is in User Mode".to_owned()),
        ..Screen::default()
    };
    golden("user_mode", &user);
}

// Spec: push-2 R34. The display's content as text, by band and column.
#[test]
fn the_screen_reads_back_as_text() {
    let mut screen = tracks();
    screen.columns[1] = column("Resonance", "34 %", Graphic::Arc(0.34), Dot::Automated);
    screen.progress[1] = Some(0.5);
    screen.notification = Some("Swing 10 %".to_owned());
    let text = screen.text();
    for line in [
        "upper 1: Volume (selected)",
        "column 2: Resonance = 34 % [white dot]",
        "lower 2: Bass (selected)",
        "progress 2: 50%",
        "notification: Swing 10 %",
    ] {
        assert!(text.contains(&line.to_owned()), "{line}: {text:?}");
    }
}

/// Waits for the display's thread, which works on the clock of the machine:
/// polls without sleeping, for at most a few seconds.
fn until(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..5_000_000 {
        if done() {
            return true;
        }
        std::thread::yield_now();
    }
    done()
}

// Spec: push-2 R17. The display's thread claims the display and sends the first
// frame, whole, as the device decodes it.
#[test]
fn the_first_frame_reaches_the_display() {
    let mut rig = Rig::new();
    rig.start();
    rig.tick();
    let frames = rig.sim.frames().clone();
    assert!(until(|| frames.count() > 0), "no frame came");
    assert_eq!(frames.bad(), 0, "every frame decodes");
    assert_eq!(frames.latest().unwrap().len(), RGB_BYTES);
}

// Spec: push-2 R17. A display that can't be claimed leaves the pads and the
// LEDs working.
#[test]
fn a_failed_claim_leaves_the_pads_and_leds_working() {
    let mut rig = Rig::new();
    rig.sim.frames().fail_claims(1_000);
    rig.start();
    rig.tick();
    let frames = rig.sim.frames().clone();
    assert!(until(|| frames.claims() > 0), "the display was tried");
    rig.model.transport.playing = true;
    rig.tick();
    assert_eq!(rig.sim.led("play").unwrap().colour, 126, "the LEDs follow");
    rig.tap("undo");
    assert_eq!(rig.commands(), ["edit.undo {}"], "the buttons work");
    assert!(rig.sim.press_pad(0, 0, 100));
    rig.advance(1);
    assert!(
        rig.heard
            .all()
            .contains(&"Note: pad 0,0 On(100)".to_owned()),
        "and the pads"
    );
    assert_eq!(frames.count(), 0);
}
