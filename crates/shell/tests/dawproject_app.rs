//! DAWproject in the app, headless (WP-6.37b): what a person's import and
//! export ask and show, behind the Open dialog, the browser, a dropped file
//! and File ▸ Export DAWproject... The behaviour is in the DAWproject spec,
//! R83 to R87.

use std::path::{Path, PathBuf};

use bl_shell::browser_commands as browser_ids;
use bl_shell::dawproject_commands::{
    CLOSE_REPORT, EXPORT_DAWPROJECT, IMPORT_DAWPROJECT, ImportReport, ReportWindowState,
};
use bl_shell::testing::rig;
use bl_shell::{Effect, Invocation, Shell, commands as ids};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dawproject")
}

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-dawproject-app-{name}"));
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

fn path_args(path: &Path) -> Value {
    json!({ "path": path.to_string_lossy() })
}

/// The fixture's report, as its file beside it says.
fn fixture_report(name: &str) -> ImportReport {
    let text = std::fs::read_to_string(fixtures().join(format!("{name}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Makes the open set differ from its file.
fn edit(shell: &mut Shell) {
    let id: bl_model::TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": id, "kind": "midi", "name": "Added", "index": 0 }),
        )
        .expect("a track is made");
    assert!(shell.unsaved_changes());
}

/// The first effect that asks about unsaved changes.
fn confirm(shell: &mut Shell) -> Invocation {
    shell
        .take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::ConfirmDiscard { then } => Some(then),
            _ => None,
        })
        .expect("the person is asked about the unsaved changes")
}

/// Spec: dawproject R83, R85. The Open dialog's choice and a dropped file run
/// the import for a `.dawproject` file and `set.open` for anything else.
#[test]
fn a_chosen_dawproject_runs_the_import_and_a_set_runs_the_open() {
    let song = Invocation::new(ids::SET_OPEN, path_args(Path::new("/a/Song.blset")));
    assert_eq!(Shell::open_invocation(Path::new("/a/Song.blset")), song);
    for name in ["/a/Song.dawproject", "/a/Song.DAWPROJECT"] {
        let open = Shell::open_invocation(Path::new(name));
        assert_eq!(open.id, IMPORT_DAWPROJECT);
        assert_eq!(open.args, path_args(Path::new(name)));
    }
}

/// Spec: dawproject R83. A person's import asks whether to save first, and
/// imports once the answer is Don't Save.
#[test]
fn an_import_asks_about_unsaved_changes_first() {
    let (mut shell, _control) = rig();
    edit(&mut shell);
    let path = fixtures().join("minimal.dawproject");
    shell.invoke(IMPORT_DAWPROJECT, path_args(&path)).unwrap();
    // Nothing has changed yet.
    assert_ne!(shell.set_name(), "minimal");
    let then = confirm(&mut shell);
    assert_eq!(then.id, IMPORT_DAWPROJECT);
    shell.discard_and_invoke(&then).unwrap();
    assert_eq!(shell.set_name(), "minimal");
    assert!(!shell.unsaved_changes());
}

/// Spec: dawproject R87. A report with entries opens the window, listing
/// them; Close closes it.
#[test]
fn a_report_with_entries_opens_the_window_and_close_shuts_it() {
    let (mut shell, _control) = rig();
    let path = fixtures().join("bitwig-shaped.dawproject");
    shell.invoke(IMPORT_DAWPROJECT, path_args(&path)).unwrap();
    let window = shell.dawproject_report_window().expect("the window opens");
    assert_eq!(window.title(), "DAWproject Import");
    assert_eq!(window.report, fixture_report("bitwig-shaped"));
    let text = window.report.text();
    assert!(text.contains("Made by Fixture DAW 1.0"), "{text}");
    assert!(text.contains("\nLeft Out\n"), "{text}");
    assert!(text.contains("\nChanged\n"), "{text}");
    assert!(
        text.contains("the clip \"Kick\" at 1.1.1 on the track \"Kick\""),
        "{text}"
    );
    let state = shell.run(CLOSE_REPORT, json!({})).unwrap();
    assert_eq!(
        serde_json::from_value::<ReportWindowState>(state).unwrap(),
        ReportWindowState { open: false }
    );
    assert!(shell.dawproject_report_window().is_none());
}

/// Spec: dawproject R87. An empty report opens no window; the Status Bar
/// says the file was imported. An agent's call opens none either.
#[test]
fn an_empty_report_opens_no_window_and_the_status_bar_says_so() {
    let (mut shell, _control) = rig();
    let path = fixtures().join("minimal.dawproject");
    shell.invoke(IMPORT_DAWPROJECT, path_args(&path)).unwrap();
    assert!(shell.dawproject_report_window().is_none());
    assert_eq!(shell.message().unwrap().text, "Imported minimal");
    let (mut agent, _control) = rig();
    let path = fixtures().join("bitwig-shaped.dawproject");
    agent.run(IMPORT_DAWPROJECT, path_args(&path)).unwrap();
    assert!(agent.dawproject_report_window().is_none());
}

/// Spec: dawproject R86. Export DAWproject... asks where, suggesting the
/// set's name with `.dawproject`; given the path it writes the file, and the
/// report opens the window when it has entries.
#[test]
fn export_dawproject_asks_where_then_writes_the_file() {
    let scratch = Scratch::new("export");
    let (mut shell, _control) = rig();
    let path = fixtures().join("devices.dawproject");
    shell.invoke(IMPORT_DAWPROJECT, path_args(&path)).unwrap();
    shell.invoke(EXPORT_DAWPROJECT, json!({})).unwrap();
    let asked: Vec<String> = shell
        .take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::PickDawProjectPath { file_name, .. } => Some(file_name),
            _ => None,
        })
        .collect();
    assert_eq!(asked, ["devices.dawproject"]);
    let target = scratch.0.join("devices.dawproject");
    let result = shell
        .invoke(
            EXPORT_DAWPROJECT,
            json!({ "path": target.to_string_lossy(), "replace": false }),
        )
        .unwrap();
    assert!(target.is_file());
    let report: ImportReport = serde_json::from_value(result["report"].clone()).unwrap();
    assert!(
        !report.entries.is_empty(),
        "the fixture's devices don't all export"
    );
    let window = shell.dawproject_report_window().expect("the window opens");
    assert_eq!(window.title(), "DAWproject Export");
    assert_eq!(window.report, report);
    // Asked again with the file there, the dialog's yes is `replace`.
    shell.run(CLOSE_REPORT, json!({})).unwrap();
    let again = json!({ "path": target.to_string_lossy(), "replace": true });
    shell.invoke(EXPORT_DAWPROJECT, again).unwrap();
}

/// Spec: dawproject R84. Enter on a DAWproject file in the browser imports
/// it, and on a set opens it, each after the unsaved-changes question.
#[test]
fn enter_in_the_browser_imports_a_dawproject_and_opens_a_set() {
    let scratch = Scratch::new("browser");
    let (mut shell, _control) = rig();
    let library = scratch.0.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(
        fixtures().join("minimal.dawproject"),
        library.join("Plain.dawproject"),
    )
    .unwrap();
    shell
        .run(ids::SET_SAVE_AS, path_args(&library.join("Mine.blset")))
        .unwrap();
    let set = shell.set_path().expect("the set is saved").to_owned();
    shell.set_user_library(Some(library.clone()));
    let state = shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    let kinds: Vec<(String, String)> = state["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["name"].to_string(), row["kind"].to_string()))
        .collect();
    assert!(
        kinds.contains(&(
            "\"Plain.dawproject\"".to_owned(),
            "\"daw_project\"".to_owned()
        )),
        "{kinds:?}"
    );
    shell.set_focused_area(bl_shell::Area::Browser);
    let select = |shell: &mut Shell, path: &Path| {
        shell
            .run(
                browser_ids::SELECT,
                json!({ "paths": [path.to_string_lossy()] }),
            )
            .unwrap();
    };

    select(&mut shell, &library.join("Plain.dawproject"));
    assert!(shell.can_invoke(browser_ids::LOAD_SELECTED, &json!({})));
    edit(&mut shell);
    shell.invoke(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    let then = confirm(&mut shell);
    assert_eq!(
        (then.id.as_str(), &then.args),
        (
            IMPORT_DAWPROJECT,
            &path_args(&library.join("Plain.dawproject"))
        )
    );
    shell.discard_and_invoke(&then).unwrap();
    assert_eq!(shell.set_name(), "Plain");

    select(&mut shell, &set);
    edit(&mut shell);
    shell.invoke(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    let then = confirm(&mut shell);
    assert_eq!(then.id, ids::SET_OPEN);
    shell.discard_and_invoke(&then).unwrap();
    assert_eq!(shell.set_path(), Some(set.as_path()));
}
