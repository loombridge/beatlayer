//! Crash recovery in the app's shell (WP-6.62; the crash-recovery spec,
//! R21 to R28): a session that stops without closing its set is offered at
//! the next launch, recovering it gives the song as it was after its last
//! journalled edit, and a clean quit leaves nothing to offer.
//!
//! The test runs this test binary again as a child that opens a set in a
//! shell with a recovery folder and edits it without pause, and the parent
//! kills it at a random moment, as a crash would.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use bl_project::Project;
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// Set for the child: the recovery folder.
const CHILD_DIR: &str = "BL_SHELL_TEST_RECOVERY_DIR";
/// Set for the child: the set it edits.
const CHILD_SET: &str = "BL_SHELL_TEST_RECOVERY_SET";

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-recovery-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell with a recovery folder, as the app has at start-up.
fn shell(recovery: &Path) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.use_recovery(recovery.to_owned()).unwrap();
    shell
}

/// The set the tests edit: a saved one, so the journal has a set to follow.
fn saved_set(dir: &Path) -> PathBuf {
    let path = dir.join("Song Project").join("Song.blset");
    let mut set = bl_project::SetFile::new(&path);
    set.save(&Project::new()).unwrap();
    path
}

/// The child's side: it edits the set without pause, and the parent kills
/// it. Does nothing unless the parent started it.
#[test]
fn child_edits_until_killed() {
    let (Some(dir), Some(set)) = (std::env::var_os(CHILD_DIR), std::env::var_os(CHILD_SET)) else {
        return;
    };
    let mut shell = shell(Path::new(&dir));
    shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    println!("opened");
    for n in 0..u64::MAX {
        // An edit by command, a gesture's transaction, an undo and a redo,
        // in turn, as a person and an agent make them.
        match n % 4 {
            0 => {
                shell
                    .invoke(
                        "track.create",
                        json!({ "kind": "midi", "name": format!("Track {n}") }),
                    )
                    .unwrap();
            }
            1 => {
                shell.begin_gesture("Move Clip");
                shell
                    .run("arrangement.move_clips", json!({ "clips": [], "start": 0 }))
                    .ok();
                shell.end_gesture();
            }
            2 => {
                shell.run("edit.undo", json!({})).ok();
            }
            _ => {
                shell.run("edit.redo", json!({})).ok();
            }
        }
        println!("edit {n}");
        // The edit is on disk before the next one: the parent kills the
        // child at a moment when the journal holds an edit.
        shell.flush_recovery().unwrap();
        println!("durable {n}");
    }
}

/// A running child and what it has reported.
struct Run {
    child: Child,
    lines: mpsc::Receiver<String>,
    reader: std::thread::JoinHandle<()>,
    /// The last edit the child reported on disk.
    durable: Option<u64>,
}

impl Run {
    fn start(dir: &Path, set: &Path) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "child_edits_until_killed",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_DIR, dir)
            .env(CHILD_SET, set)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut run = Self {
            child,
            lines,
            reader,
            durable: None,
        };
        // Wait until it has the set open.
        loop {
            let line = run
                .lines
                .recv_timeout(Duration::from_secs(60))
                .expect("the child opens the set");
            let opened = line.ends_with("opened");
            run.note(&line);
            if opened {
                break;
            }
        }
        run
    }

    fn note(&mut self, line: &str) {
        // The test harness starts the first line with the test's name.
        let line = line.rsplit(" ... ").next().unwrap_or(line);
        if let Some(n) = line.strip_prefix("durable ") {
            self.durable = Some(n.parse().unwrap());
        }
    }

    /// Kills the child, and reads everything it reported.
    fn kill(mut self) -> Self {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        let reader = std::mem::replace(&mut self.reader, std::thread::spawn(|| {}));
        reader.join().unwrap();
        while let Ok(line) = self.lines.try_recv() {
            self.note(&line);
        }
        self
    }
}

/// The tracks the child's edits have made after its edit `n`: every fourth
/// edit makes one, the one two after it undoes it, and the next redoes it.
fn tracks_after(n: u64) -> usize {
    let made = usize::try_from(n / 4 + 1).unwrap();
    if n % 4 == 2 { made - 1 } else { made }
}

#[test]
fn a_killed_session_is_offered_and_recovered() {
    // Spec: crash-recovery R2, R3, R21, R28.
    let scratch = Scratch::new("killed");
    let recovery = scratch.path("Recovery");
    let set = saved_set(&scratch.path(""));
    let tracks = bl_project::SetFile::open(&set)
        .unwrap()
        .1
        .document
        .tracks()
        .count();
    let run = Run::start(&recovery, &set);
    // Let it make a number of edits that differs from run to run, wait
    // until they are on disk, then kill it, as a crash would.
    let stop = 3 + std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        % 16;
    let run = {
        let mut run = run;
        loop {
            let line = run.lines.recv_timeout(Duration::from_secs(60)).unwrap();
            run.note(&line);
            if run.durable >= Some(u64::from(stop)) {
                break;
            }
        }
        run.kill()
    };
    // Every edit it reported on disk is recovered; the one after it may be
    // on disk too, killed before it said so.
    let durable = run.durable.expect("the child's edits are on disk");

    // The next launch offers the set.
    let mut shell = shell(&recovery);
    let offers = shell.recovery_offers();
    assert_eq!(offers.len(), 1, "the set is offered");
    let offer = &offers[0];
    assert_eq!(offer.name(), "Song");
    assert!(offer.edits >= 1, "its edits are counted");
    assert!(!offer.saved_since, "nothing else saved the set");

    // Recovering it opens the song as it was after its last journalled
    // edit, with an empty undo history.
    let recovered = shell
        .run("set.recover", json!({ "id": offer.id }))
        .expect("the set is recovered");
    assert_eq!(recovered["name"], "Song");
    assert_eq!(recovered["path"], set.to_string_lossy().as_ref());
    assert!(
        !shell.editor().can_undo(),
        "the recovered set has no history"
    );
    assert!(shell.is_dirty(), "the recovered set is unsaved");
    let recovered = shell.editor().document().tracks().count() - tracks;
    assert!(
        recovered == tracks_after(durable) || recovered == tracks_after(durable + 1),
        "the song is as it was after edit {durable}: {recovered} tracks made"
    );

    // Discard Changes goes back to the set file.
    shell.run("set.discard_changes", json!({})).unwrap();
    assert_eq!(shell.editor().document().tracks().count(), tracks);
    assert!(!shell.is_dirty());
}

#[test]
fn a_clean_quit_leaves_nothing_to_offer() {
    // Spec: crash-recovery R6, R21.
    let scratch = Scratch::new("quit");
    let recovery = scratch.path("Recovery");
    let set = saved_set(&scratch.path(""));
    let mut app = shell(&recovery);
    app.run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    app.invoke("track.create", json!({ "kind": "midi", "name": "Track" }))
        .unwrap();
    assert!(app.is_dirty());
    // The person answers Don't Save, and the app quits.
    app.run("set.discard_changes", json!({})).unwrap();
    app.run("app.quit", json!({})).unwrap();
    drop(app);

    // The next launch offers nothing.
    let next = shell(&recovery);
    assert!(next.recovery_offers().is_empty());
}
#[test]
fn discarding_a_recovery_leaves_nothing_to_offer() {
    // Spec: crash-recovery R4, R26.
    let scratch = Scratch::new("discard");
    let recovery = scratch.path("Recovery");
    let set = saved_set(&scratch.path(""));
    // A session that stopped without closing its set.
    {
        let mut shell = shell(&recovery);
        shell
            .run("set.open", json!({ "path": set.to_string_lossy() }))
            .unwrap();
        shell
            .invoke("track.create", json!({ "kind": "midi", "name": "Track" }))
            .unwrap();
    }
    let mut shell = shell(&recovery);
    let offers = shell.recovery_offers();
    assert_eq!(offers.len(), 1, "the set is offered");
    let discarded = shell
        .run("set.discard_recovery", json!({ "id": offers[0].id }))
        .expect("the set is declined");
    assert_eq!(discarded["id"], offers[0].id);
    assert!(shell.recovery_offers().is_empty(), "nothing is offered");
    // A copy of the journal is kept for seven days.
    assert!(!discarded["kept_at"].as_str().unwrap().is_empty());
}

#[test]
fn later_closes_the_prompt_and_keeps_the_sets_for_the_next_launch() {
    // Spec: crash-recovery R22.
    let scratch = Scratch::new("later");
    let recovery = scratch.path("Recovery");
    let set = saved_set(&scratch.path(""));
    {
        let mut shell = shell(&recovery);
        shell
            .run("set.open", json!({ "path": set.to_string_lossy() }))
            .unwrap();
        shell
            .invoke("track.create", json!({ "kind": "midi", "name": "Track" }))
            .unwrap();
    }
    let mut app = shell(&recovery);
    assert!(app.recovery_prompt_open(), "the prompt is open");
    let prompt = app.run("set.later_recovery", json!({})).unwrap();
    assert_eq!(prompt["open"], false);
    assert!(!app.recovery_prompt_open(), "the prompt closed");
    // Every set is still offered, for the next launch.
    assert_eq!(app.recovery_offers().len(), 1);
    drop(app);
    let next = shell(&recovery);
    assert_eq!(next.recovery_offers().len(), 1);
    assert!(next.recovery_prompt_open());
}

#[test]
fn save_continuously_leaves_a_set_with_a_file_clean() {
    // Spec: crash-recovery R25.
    let scratch = Scratch::new("continuous");
    let recovery = scratch.path("Recovery");
    let set = saved_set(&scratch.path(""));
    let mut app = shell(&recovery);
    app.run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    // A set that has never been saved still has unsaved changes.
    app.invoke("track.create", json!({ "kind": "midi", "name": "Track" }))
        .unwrap();
    assert!(app.unsaved_changes());
    // With Save Continuously on, a set that has a file has none.
    app.run("settings.set_continuous_save", json!({ "on": true }))
        .unwrap();
    assert!(!app.unsaved_changes(), "the set is saved within a second");
    // Quitting doesn't ask about it.
    app.invoke("app.quit", json!({})).unwrap();
    assert!(
        app.take_effects()
            .iter()
            .all(|effect| !matches!(effect, bl_shell::Effect::ConfirmDiscard { .. })),
        "quitting didn't ask"
    );
    // A set that has never been saved still asks.
    let mut app = shell(&recovery);
    app.invoke("track.create", json!({ "kind": "midi", "name": "Track" }))
        .unwrap();
    app.invoke("app.quit", json!({})).unwrap();
    assert!(
        app.take_effects()
            .iter()
            .any(|effect| matches!(effect, bl_shell::Effect::ConfirmDiscard { .. })),
        "quitting asked"
    );
}

// Windows has no read-only folder to make: its read-only attribute doesn't
// stop files being made in one.
#[cfg(unix)]
#[test]
fn a_read_only_recovery_folder_says_so_once_and_the_set_still_edits() {
    use std::os::unix::fs::PermissionsExt;

    // Spec: crash-recovery R27.
    let scratch = Scratch::new("read-only");
    let recovery = scratch.path("Recovery");
    std::fs::create_dir_all(&recovery).unwrap();
    let mut shell = shell(&recovery);
    // Make the folder unwritable, as a disk that is full or a folder that
    // has gone would be.
    let mut permissions = std::fs::metadata(&recovery).unwrap().permissions();
    permissions.set_mode(0o500);
    std::fs::set_permissions(&recovery, permissions).unwrap();
    // The session can't start, so the app says so.
    shell.use_recovery(recovery.clone()).unwrap();
    shell
        .run("set.new", json!({}))
        .expect("the set still opens");
    let message = shell.message().expect("the Status Bar says why");
    assert!(
        message.text.contains("aren't being kept for recovery"),
        "{}",
        message.text
    );
    assert!(message.error, "the message reports a failure");
    // The app goes on working, and says it once.
    let before = shell.editor().document().tracks().count();
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Track" }))
        .unwrap();
    assert_eq!(shell.editor().document().tracks().count(), before + 1);
    // The message is still there after the edit.
    let message = shell.message().expect("the Status Bar still says why");
    assert!(
        message.text.contains("aren't being kept for recovery"),
        "{}",
        message.text
    );
    let mut permissions = std::fs::metadata(&recovery).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&recovery, permissions).unwrap();
}
