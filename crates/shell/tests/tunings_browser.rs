//! Tunings in the browser (WP-6.10; the tunings spec, R16, R25 to R28, R31
//! and R32): the Tunings label over Beatlayer's own tunings and the files in
//! the places, its search and Info View text, loading from the browser, the
//! Tuning section's commands, the scales a tuning hides, and the shipped
//! tunings against the script that writes them. Driven through `Shell::run`
//! and `Shell::invoke`, as agents, menus and keys drive them.

use std::path::{Path, PathBuf};
use std::process::Command;

use bl_audio_io::NullControl;
use bl_shell::browser::places::create_user_library;
use bl_shell::browser::tunings::{BUNDLED, bundled_path};
use bl_shell::browser_commands::{self as ids, BrowserState, ItemKind, Label};
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_shell::testing;
use bl_shell::{Effect, Shell};
use bl_shell_spec::tunings::TuningState;
use serde_json::{Value, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-tunings-browser-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell on the null device paced by hand, with a User Library whose
/// Tunings folder holds a generated `.scl` file, a generated `.ascl` file,
/// and a file the browser leaves out.
fn rig_with_library(scratch: &Scratch) -> (Shell, NullControl, PathBuf) {
    let (mut shell, control) = testing::rig();
    let root = scratch.0.join("User Library");
    create_user_library(&root).unwrap();
    let fixtures = repo().join("tests/fixtures/tunings");
    let tunings = root.join("Tunings");
    std::fs::copy(
        fixtures.join("equal-19.scl"),
        tunings.join("Slendro Plus.scl"),
    )
    .unwrap();
    std::fs::copy(fixtures.join("just-12.scl"), tunings.join("Ptolemy.ascl")).unwrap();
    std::fs::write(tunings.join("notes.txt"), b"left out").unwrap();
    shell.set_user_library(Some(root.clone()));
    shell.start_audio();
    (shell, control, root)
}

fn wait_for_index(control: &NullControl, shell: &mut Shell) {
    for _ in 0..2_000 {
        shell.poll();
        if !shell.browser_view().index_state().indexing {
            return;
        }
        testing::advance(control, shell, testing::STEP_MS);
    }
    panic!("timed out waiting for the index");
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn tunings(shell: &mut Shell, search: &str) -> BrowserState {
    shell
        .run(ids::OPEN_PLACE, json!({ "label": "tunings" }))
        .unwrap();
    shell
        .run(ids::SEARCH, json!({ "query": search, "label": "tunings" }))
        .unwrap();
    run(shell, ids::STATE, json!({ "limit": 1000 }))
}

fn names(state: &BrowserState) -> Vec<&str> {
    state.rows.iter().map(|row| row.name.as_str()).collect()
}

fn tuning_state(shell: &mut Shell) -> TuningState {
    run(shell, "tuning.state", json!({}))
}

// Spec: tunings R25. The Tunings label lists Beatlayer's own tunings, then the
// `.scl` and `.ascl` files in the places, by name without extension.
#[test]
fn the_tunings_label_lists_the_bundled_tunings_then_the_files_in_the_places() {
    let scratch = Scratch::new("label");
    let (mut shell, control, _) = rig_with_library(&scratch);
    wait_for_index(&control, &mut shell);
    let state = tunings(&mut shell, "");
    let shown = names(&state);
    let own: Vec<&str> = BUNDLED.iter().map(|(name, _)| *name).collect();
    assert_eq!(shown[..own.len()], own[..], "Beatlayer's own come first");
    let mut files: Vec<&str> = shown[own.len()..].to_vec();
    files.sort_unstable();
    assert_eq!(files, ["Ptolemy", "Slendro Plus"], "no extensions, no .txt");
    assert!(state.rows.iter().all(|row| row.kind == ItemKind::Tuning));
    assert!(state.note.is_none());
    assert!(own.contains(&"19 Equal Steps") && own.contains(&"Bohlen-Pierce"));
}

// Spec: tunings R25, browser R9. A search under the label matches those names.
#[test]
fn a_search_under_the_label_finds_the_bundled_tunings_and_the_files() {
    let scratch = Scratch::new("search");
    let (mut shell, control, _) = rig_with_library(&scratch);
    wait_for_index(&control, &mut shell);
    assert_eq!(names(&tunings(&mut shell, "pythag")), ["Pythagorean"]);
    assert_eq!(names(&tunings(&mut shell, "SLENDRO")), ["Slendro Plus"]);
    assert_eq!(
        names(&tunings(&mut shell, "equal 19")),
        ["19 Equal Steps"],
        "every word must match"
    );
    let comma = tunings(&mut shell, "comma");
    assert_eq!(names(&comma), ["Quarter-Comma Meantone"]);
    let none = tunings(&mut shell, "zzz");
    assert!(none.rows.is_empty());
    assert!(none.note.unwrap().contains("Nothing matches"));
}

// Spec: tunings R27, R28, R32. Choosing a tuning and loading the selection
// makes it the set's, as `tuning.load` does, shows the Tuning section, and
// reads back through `tuning.state`; a file in the places loads too.
#[test]
fn loading_the_selection_loads_the_tuning_and_shows_the_section() {
    let scratch = Scratch::new("load");
    let (mut shell, control, root) = rig_with_library(&scratch);
    wait_for_index(&control, &mut shell);
    assert!(!shell.tuning_section_shown(), "hidden in a new set");
    tunings(&mut shell, "");
    let steps = shell.editor().undo_history().len();
    shell
        .run(
            ids::SELECT,
            json!({ "paths": [bundled_path("19 Equal Steps")] }),
        )
        .unwrap();
    shell.invoke(ids::LOAD_SELECTED, json!({})).unwrap();
    let loaded = tuning_state(&mut shell).tuning.expect("a tuning");
    assert_eq!(
        (loaded.name.as_str(), loaded.notes_per_octave),
        ("19 Equal Steps", 19)
    );
    assert!(shell.tuning_section_shown());
    assert_eq!(
        shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    // A file in the User Library loads by its path.
    let file = root.join("Tunings").join("Ptolemy.ascl");
    shell
        .run(ids::SELECT, json!({ "paths": [file.to_string_lossy()] }))
        .unwrap();
    shell.invoke(ids::LOAD_SELECTED, json!({})).unwrap();
    let loaded = tuning_state(&mut shell).tuning.unwrap();
    assert_eq!(
        (loaded.name.as_str(), loaded.notes_per_octave),
        ("Ptolemy", 12)
    );
    // With Retune Set On Loading Tuning Systems on, a person is asked first.
    shell
        .invoke("settings.set_retune_on_tuning_load", json!({}))
        .unwrap();
    shell
        .run(ids::SELECT, json!({ "paths": [bundled_path("Vallotti")] }))
        .unwrap();
    shell.invoke(ids::LOAD_SELECTED, json!({})).unwrap();
    assert!(matches!(
        &shell.take_effects()[..],
        [Effect::ConfirmRetune { .. }]
    ));
    assert_eq!(tuning_state(&mut shell).tuning.unwrap().name, "Ptolemy");
}

// Spec: tunings R26. The Info View's lines for a tuning: its description,
// its notes to the pseudo-octave and the pseudo-octave's size; a file R3
// refuses shows the error.
#[test]
fn the_info_view_text_names_the_notes_and_the_pseudo_octave() {
    use bl_shell::browser::tunings::info;
    let (title, body) = info(&bundled_path("Bohlen-Pierce"));
    assert_eq!(title, "Bohlen-Pierce");
    assert_eq!(
        body,
        "13 equal steps of 3/1\n13 notes to the pseudo-octave\nPseudo-octave: 1901.96 cents"
    );
    let bad = repo().join("tests/fixtures/tunings/no-pitches.scl");
    let (title, body) = info(&bad.to_string_lossy());
    assert_eq!(title, "no-pitches");
    assert!(body.contains("no pitches"), "{body}");
}

fn view(shell: &mut Shell) -> Value {
    shell.run("view.state", json!({})).unwrap()["window"].clone()
}

// Spec: tunings R28, R32. `view.tuning_section` shows and hides the section,
// `focus.tuning` shows it, with the browser, and gives it the focus; both
// read back in `view.state` and change nothing in the set.
#[test]
fn the_section_shows_hides_and_takes_the_focus() {
    let mut shell = Shell::headless();
    assert_eq!(view(&mut shell)["tuning_section"], json!(false));
    assert_eq!(shell.switch_state("view.tuning_section"), Some(false));
    let steps = shell.editor().undo_history().len();
    shell.run("view.tuning_section", json!({})).unwrap();
    assert_eq!(view(&mut shell)["tuning_section"], json!(true));
    assert_eq!(shell.switch_state("view.tuning_section"), Some(true));
    shell.run("view.tuning_section", json!({})).unwrap();
    assert_eq!(view(&mut shell)["tuning_section"], json!(false));
    // Navigate ▸ Tuning with the browser hidden shows both.
    shell.run("view.toggle_browser", json!({})).unwrap();
    assert_eq!(view(&mut shell)["browser"], json!(false));
    shell.take_effects();
    shell.run("focus.tuning", json!({})).unwrap();
    assert_eq!(view(&mut shell)["browser"], json!(true));
    assert_eq!(view(&mut shell)["tuning_section"], json!(true));
    assert!(
        shell
            .take_effects()
            .iter()
            .any(|effect| matches!(effect, Effect::FocusTuning))
    );
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
}

// Spec: tunings R28 (M-VIEW-023, M-NAV-009). The View and Navigate menus'
// Tuning items run the two commands.
#[test]
fn the_menu_items_run_the_commands() {
    for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
        let menus = menu_bar(platform);
        let items: Vec<_> = menus
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .filter(|item| item.row == "M-VIEW-023" || item.row == "M-NAV-009")
            .collect();
        let commands: Vec<(&str, &str)> = items
            .iter()
            .map(|item| match item.action {
                Action::Command(id) => (item.row, id),
                ref other => panic!("{}: {other:?}", item.row),
            })
            .collect();
        assert_eq!(
            commands,
            [
                ("M-VIEW-023", "view.tuning_section"),
                ("M-NAV-009", "focus.tuning")
            ]
        );
        assert!(items.iter().all(|item| item.label == "Tuning"));
    }
    let mut shell = Shell::headless();
    shell.invoke("view.tuning_section", json!({})).unwrap();
    assert!(shell.tuning_section_shown());
    shell.invoke("focus.tuning", json!({})).unwrap();
    assert!(shell.tuning_section_shown());
}

// Spec: tunings R16. A tuning of other than twelve notes hides the scales; one
// of twelve changes nothing.
#[test]
fn a_tuning_of_other_than_twelve_notes_hides_the_scales() {
    let mut shell = Shell::headless();
    assert!(!shell.scales_hidden());
    let fixtures = repo().join("tests/fixtures/tunings");
    for (file, hidden) in [
        ("equal-19.scl", true),
        ("equal-12.scl", false),
        ("bohlen-pierce.scl", true),
        ("just-12.scl", false),
    ] {
        let path = fixtures.join(file);
        shell
            .run("tuning.load", json!({ "path": path.to_string_lossy() }))
            .unwrap();
        assert_eq!(shell.scales_hidden(), hidden, "{file}");
    }
    shell.run("tuning.remove", json!({})).unwrap();
    assert!(!shell.scales_hidden(), "back with the tuning gone");
}

fn python() -> Command {
    for name in ["python3", "python"] {
        if Command::new(name).arg("--version").output().is_ok() {
            return Command::new(name);
        }
    }
    panic!("a Python 3 interpreter is needed to run the generator");
}

/// The pitches of a tuning Beatlayer ships, from its definition alone: the
/// cents of degrees 1 to n.
fn definition(name: &str) -> Vec<f64> {
    const FIFTH: f64 = 701.955_000_865_387_4;
    let ratios = |list: &[(f64, f64)]| -> Vec<f64> {
        list.iter()
            .map(|(top, bottom)| 1200.0 * (top / bottom).log2())
            .collect()
    };
    // A circle of twelve fifths from C, in the order C G D A E B F# C# G# D#
    // A# F: `None` is the one fifth left to close it (the wolf).
    let circle = |fifths: [Option<f64>; 12]| -> Vec<f64> {
        let wolf = fifths.iter().position(Option::is_none).unwrap_or(11);
        let mut place = [0.0f64; 13];
        for k in 1..=wolf {
            place[k] = place[k - 1] + fifths[k - 1].unwrap();
        }
        for k in (wolf + 1..12).rev() {
            place[k] = place[k + 1] - fifths[k].unwrap();
        }
        let mut classes: Vec<f64> = place[1..12].iter().map(|p| p.rem_euclid(1200.0)).collect();
        classes.sort_by(f64::total_cmp);
        classes.push(1200.0);
        classes
    };
    let pythagorean_comma = 12.0 * FIFTH - 8400.0;
    let syntonic_comma = 4.0 * FIFTH - 2400.0 - 1200.0 * 1.25f64.log2();
    let narrowed = |by: f64, which: &[usize]| -> [Option<f64>; 12] {
        std::array::from_fn(|k| Some(FIFTH - if which.contains(&k) { by } else { 0.0 }))
    };
    if let Some(steps) = name
        .strip_suffix(" Equal Steps")
        .and_then(|n| n.parse::<f64>().ok())
    {
        return (1..=steps as usize)
            .map(|i| 1200.0 * i as f64 / steps)
            .collect();
    }
    match name {
        "Just Intonation 5-Limit" => ratios(&[
            (16., 15.),
            (9., 8.),
            (6., 5.),
            (5., 4.),
            (4., 3.),
            (45., 32.),
            (3., 2.),
            (8., 5.),
            (5., 3.),
            (9., 5.),
            (15., 8.),
            (2., 1.),
        ]),
        "Just Intonation 7-Limit" => ratios(&[
            (15., 14.),
            (8., 7.),
            (6., 5.),
            (5., 4.),
            (4., 3.),
            (7., 5.),
            (3., 2.),
            (8., 5.),
            (5., 3.),
            (7., 4.),
            (15., 8.),
            (2., 1.),
        ]),
        "Harmonics 8 to 16" => (9..=16)
            .map(|n| 1200.0 * (f64::from(n) / 8.0).log2())
            .collect(),
        "Bohlen-Pierce" => (1..=13)
            .map(|i| 1200.0 * 3f64.log2() * f64::from(i) / 13.0)
            .collect(),
        "Pythagorean" => {
            let mut fifths = narrowed(0.0, &[]);
            fifths[8] = None;
            circle(fifths)
        }
        "Quarter-Comma Meantone" => {
            let mut fifths = [Some(1200.0 * 5f64.log2() / 4.0); 12];
            fifths[8] = None;
            circle(fifths)
        }
        "Werckmeister III" => circle(narrowed(pythagorean_comma / 4.0, &[0, 1, 2, 5])),
        "Kirnberger III" => {
            let mut fifths = narrowed(syntonic_comma / 4.0, &[0, 1, 2, 3]);
            fifths[6] = None;
            circle(fifths)
        }
        "Vallotti" => circle(narrowed(pythagorean_comma / 6.0, &[11, 0, 1, 2, 3, 4])),
        "Young" => circle(narrowed(pythagorean_comma / 6.0, &[0, 1, 2, 3, 4, 5])),
        other => panic!("no definition for {other}"),
    }
}

// Spec: tunings R31. Each shipped file is what the script writes, and reads
// back to the pitches its mathematical definition gives, within 0.001 cent.
#[test]
fn the_shipped_tunings_equal_the_scripts_output_and_their_definitions() {
    let scratch = Scratch::new("generator");
    let out = scratch.0.join("generated");
    let status = python()
        .arg(repo().join("tests/fixtures/gen/gen_bundled_tunings.py"))
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "the generator ran");
    let shipped = repo().join("crates/shell/tunings");
    let mut generated: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    generated.sort();
    let mut kept: Vec<String> = BUNDLED
        .iter()
        .map(|(name, _)| format!("{name}.ascl"))
        .collect();
    kept.sort();
    assert_eq!(
        generated, kept,
        "the script writes exactly the tunings shipped"
    );
    for (name, text) in BUNDLED {
        let file = format!("{name}.ascl");
        let written = std::fs::read(out.join(&file)).unwrap();
        assert_eq!(
            written,
            std::fs::read(Path::new(&shipped).join(&file)).unwrap(),
            "{name}: the file on disk is the script's"
        );
        assert_eq!(written, text.as_bytes(), "{name}: the app ships that file");
        let tuning = bl_shell::browser::tunings::read(&bundled_path(name)).unwrap();
        let want = definition(name);
        assert_eq!(tuning.cents.len(), want.len(), "{name}");
        for (step, (got, want)) in tuning.cents.iter().zip(&want).enumerate() {
            assert!(
                (got - want).abs() < 0.001,
                "{name}, step {}: {got} against {want}",
                step + 1
            );
        }
    }
    // At least one tuning whose pseudo-octave isn't an octave.
    assert!(BUNDLED.iter().any(|(name, _)| {
        let tuning = bl_shell::browser::tunings::read(&bundled_path(name)).unwrap();
        (tuning.pseudo_octave() - 1200.0).abs() > 1.0
    }));
}

// Spec: tunings R25. The Tunings label is no longer waiting for a package.
#[test]
fn the_label_is_not_pending() {
    assert!(Label::Tunings.pending().is_none());
}
