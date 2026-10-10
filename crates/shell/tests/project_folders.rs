//! Where Save Set and Save Set As put a set: in a project folder, as Live
//! does (app-shell R17 to R22). Driven through `Shell::run`, as an agent
//! drives it (D17), on real folders and bl-project's real set files.

use std::path::{Path, PathBuf};

use bl_model::ContentHash;
use bl_project::{MediaPath, MediaRef, Project, ProjectFolder, SetFile};
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-projects-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    /// A path for a command's arguments.
    fn arg(&self, relative: &str) -> String {
        self.join(relative).to_string_lossy().into_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The names in a folder, sorted.
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Where a command's result says the set is.
fn saved_path(result: &Value) -> PathBuf {
    PathBuf::from(result["path"].as_str().unwrap())
}

fn save_as(shell: &mut Shell, path: &str) -> PathBuf {
    saved_path(&shell.run("set.save_as", json!({ "path": path })).unwrap())
}

fn add_track(shell: &mut Shell, name: &str) {
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": name }))
        .unwrap();
}

fn revision(set: &Path) -> u64 {
    bl_project::load(set).unwrap().1.revision
}

#[test]
fn a_new_sets_first_save_makes_its_project_folder() {
    // Spec: app-shell R17, R19.
    let dir = TestDir::new("first-save");
    let mut shell = Shell::headless();
    let saved = shell
        .run("set.save_as", json!({ "path": dir.arg("Tango.blset") }))
        .unwrap();
    let project = dir.join("Tango Project");
    let tango = project.join("Tango.blset");
    assert_eq!(
        saved,
        json!({ "path": tango.to_string_lossy(), "name": "Tango" })
    );
    assert_eq!(shell.set_path(), Some(tango.as_path()));
    assert_eq!(names(&dir.0), ["Tango Project"], "nothing beside it");
    assert_eq!(names(&project), ["Tango.blset"], "it holds just the set");
    assert_eq!(revision(&tango), 1);

    // The next save replaces the set in place, and its backup stays in the
    // project folder.
    add_track(&mut shell, "Bass");
    shell.run("set.save", json!({})).unwrap();
    assert_eq!(revision(&tango), 2);
    assert_eq!(names(&dir.0), ["Tango Project"]);
    assert_eq!(names(&project), ["Backup", "Tango.blset"]);
    assert_eq!(names(&project.join("Backup")).len(), 1);
}

#[test]
fn save_as_outside_any_project_makes_a_new_one() {
    // Spec: app-shell R17.
    let dir = TestDir::new("outside");
    let mut shell = Shell::headless();
    let tango = save_as(&mut shell, &dir.arg("Tango.blset"));
    add_track(&mut shell, "Bass");
    shell.run("set.save", json!({})).unwrap();

    // Beside the project rather than in it: a project of its own, and the
    // revision count carries on.
    let samba = save_as(&mut shell, &dir.arg("Samba.blset"));
    assert_eq!(samba, dir.join("Samba Project/Samba.blset"));
    assert_eq!(names(&dir.0), ["Samba Project", "Tango Project"]);
    assert_eq!(names(&dir.join("Samba Project")), ["Samba.blset"]);
    assert_eq!(revision(&samba), 3);
    assert_eq!(revision(&tango), 2, "the first project is untouched");
    assert_eq!(shell.set_name(), "Samba");

    // A folder that isn't there yet is made, and one with other files but
    // no set isn't a project.
    let rumba = save_as(&mut shell, &dir.arg("New/Folder/Rumba.blset"));
    assert_eq!(rumba, dir.join("New/Folder/Rumba Project/Rumba.blset"));
    std::fs::create_dir(dir.join("Notes")).unwrap();
    std::fs::write(dir.join("Notes/Lyrics.txt"), "la la").unwrap();
    let waltz = save_as(&mut shell, &dir.arg("Notes/Waltz.blset"));
    assert_eq!(waltz, dir.join("Notes/Waltz Project/Waltz.blset"));
    assert!(waltz.is_file());
}

#[test]
fn save_as_into_a_project_folder_joins_it() {
    // Spec: app-shell R18.
    let dir = TestDir::new("join");
    let mut shell = Shell::headless();
    let tango = save_as(&mut shell, &dir.arg("Tango.blset"));
    add_track(&mut shell, "Bass");

    // A new version, saved in the set's project folder, joins it.
    let second = save_as(&mut shell, &dir.arg("Tango Project/Tango 2.blset"));
    assert_eq!(second, dir.join("Tango Project/Tango 2.blset"));
    assert_eq!(names(&dir.0), ["Tango Project"], "no new project folder");
    assert_eq!(
        names(&dir.join("Tango Project")),
        ["Tango 2.blset", "Tango.blset"]
    );
    assert_eq!(revision(&tango), 1, "the first version is untouched");
    assert_eq!(revision(&second), 2);

    // So does a new set saved there.
    shell.run("set.new", json!({})).unwrap();
    let milonga = save_as(&mut shell, &dir.arg("Tango Project/Milonga.blset"));
    assert_eq!(milonga, dir.join("Tango Project/Milonga.blset"));
    assert_eq!(names(&dir.0), ["Tango Project"]);

    // A folder holding a loose set is a project folder too.
    let loose = dir.join("Sets/Old.blset");
    SetFile::new(&loose).save(&Project::new()).unwrap();
    let new = save_as(&mut shell, &dir.arg("Sets/New.blset"));
    assert_eq!(new, dir.join("Sets/New.blset"));
    assert_eq!(names(&dir.join("Sets")), ["New.blset", "Old.blset"]);
}

#[test]
fn save_as_over_a_projects_set_keeps_its_folder() {
    // Spec: app-shell R18, R19.
    let dir = TestDir::new("over");
    let mut shell = Shell::headless();
    let tango = save_as(&mut shell, &dir.arg("Tango.blset"));

    // Another set, saved over the project's set, replaces it there, and
    // the replaced version is kept in that project's Backup folder.
    shell.run("set.new", json!({})).unwrap();
    add_track(&mut shell, "Replacement");
    let saved = save_as(&mut shell, &tango.to_string_lossy());
    assert_eq!(saved, tango);
    assert_eq!(names(&dir.0), ["Tango Project"]);
    assert_eq!(names(&dir.join("Tango Project")), ["Backup", "Tango.blset"]);
    assert_eq!(names(&dir.join("Tango Project/Backup")).len(), 1);
    let (project, _) = bl_project::load(&tango).unwrap();
    assert_eq!(project.document, *shell.editor().document());
}

#[test]
fn a_taken_project_folder_name_gets_the_next_number() {
    // Spec: app-shell R20.
    let dir = TestDir::new("taken");
    let mut shell = Shell::headless();
    let tango = save_as(&mut shell, &dir.arg("Tango.blset"));
    let before = std::fs::read(&tango).unwrap();

    // A new set called Tango, saved in the same place, never touches the
    // other Tango's project.
    shell.run("set.new", json!({})).unwrap();
    add_track(&mut shell, "Another");
    let other = save_as(&mut shell, &dir.arg("Tango.blset"));
    assert_eq!(other, dir.join("Tango Project 2/Tango.blset"));
    assert_eq!(std::fs::read(&tango).unwrap(), before);
    assert_eq!(names(&dir.join("Tango Project")), ["Tango.blset"]);

    // Anything with the name counts as taken: an empty folder, or a file.
    std::fs::create_dir(dir.join("Samba Project")).unwrap();
    std::fs::write(dir.join("Samba Project 2"), "not a folder").unwrap();
    let samba = save_as(&mut shell, &dir.arg("Samba.blset"));
    assert_eq!(samba, dir.join("Samba Project 3/Samba.blset"));
    assert!(names(&dir.join("Samba Project")).is_empty());
}

#[test]
fn a_failed_save_leaves_no_project_folder_behind() {
    // Spec: app-shell R20. Names are at most 255 bytes. This one fits in a
    // folder's name and a set's, but not in the hidden name a save writes
    // first, so the save fails after the project folder is made (or, where
    // whole paths are limited too, before).
    let dir = TestDir::new("failed");
    let mut shell = Shell::headless();
    let name = "x".repeat(240);
    let error = shell
        .run(
            "set.save_as",
            json!({ "path": dir.arg(&format!("{name}.blset")) }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
    assert!(names(&dir.0).is_empty(), "{:?}", names(&dir.0));
    assert_eq!(shell.set_path(), None);
}

#[test]
fn save_as_to_another_project_keeps_using_the_same_media() {
    // Spec: app-shell R21. A set made outside the shell, with one file in
    // its project and one beside it.
    let dir = TestDir::new("media");
    let tango = ProjectFolder::named(&dir.0, "Tango");
    let mut project = Project::new();
    project.media.insert(MediaRef {
        hash: ContentHash([1; 32]),
        path: MediaPath::new("Samples/Imported/Kick.wav").unwrap(),
        size: 3,
    });
    project.media.insert(MediaRef {
        hash: ContentHash([2; 32]),
        path: MediaPath::new("../Library/Pad.wav").unwrap(),
        size: 4,
    });
    SetFile::new(tango.set_path("Tango"))
        .save(&project)
        .unwrap();
    let media = |set: &Path| -> Vec<String> {
        bl_project::load(set)
            .unwrap()
            .0
            .media
            .iter()
            .map(|file| file.path.to_string())
            .collect()
    };

    let mut shell = Shell::headless();
    shell
        .run("set.open", json!({ "path": tango.set_path("Tango") }))
        .unwrap();
    let samba = save_as(&mut shell, &dir.arg("Elsewhere/Samba.blset"));
    assert_eq!(samba, dir.join("Elsewhere/Samba Project/Samba.blset"));
    assert_eq!(
        media(&samba),
        [
            "../../Tango Project/Samples/Imported/Kick.wav",
            "../../Library/Pad.wav"
        ]
    );
    assert_eq!(shell.message(), None, "nothing to warn about");

    // Back in the first project, the paths are as they were.
    let back = save_as(&mut shell, &tango.set_path("Tango 2").to_string_lossy());
    assert_eq!(back, tango.set_path("Tango 2"));
    assert_eq!(
        media(&back),
        ["Samples/Imported/Kick.wav", "../Library/Pad.wav"]
    );
}

#[test]
fn the_save_dialog_opens_in_the_sets_project_folder() {
    // Spec: app-shell R22.
    let dir = TestDir::new("dialog");
    let mut shell = Shell::headless();
    assert_eq!(shell.save_dialog_folder(), None, "where the system left it");
    let tango = save_as(&mut shell, &dir.arg("Tango.blset"));
    let folder = shell.save_dialog_folder().unwrap();
    assert_eq!(folder, dir.join("Tango Project"));

    // Keeping the suggestion, the folder and the set's name, saves in
    // place; a new name there joins the project.
    let suggested = folder.join(format!("{}.blset", shell.set_name()));
    assert_eq!(save_as(&mut shell, &suggested.to_string_lossy()), tango);
    let second = folder.join("Tango 2.blset");
    assert_eq!(save_as(&mut shell, &second.to_string_lossy()), second);
    assert_eq!(names(&dir.0), ["Tango Project"]);

    shell.run("set.new", json!({})).unwrap();
    assert_eq!(shell.save_dialog_folder(), None);
}

#[test]
fn a_name_without_the_extension_gets_it() {
    // Spec: app-shell R17.
    let dir = TestDir::new("extension");
    let mut shell = Shell::headless();
    let tango = save_as(&mut shell, &dir.arg("Tango"));
    assert_eq!(tango, dir.join("Tango Project/Tango.blset"));
    let final_mix = save_as(&mut shell, &dir.arg("Tango Project/Tango.final"));
    assert_eq!(final_mix, dir.join("Tango Project/Tango.final.blset"));
    assert_eq!(shell.set_name(), "Tango.final");
}
