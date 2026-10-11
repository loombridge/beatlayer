//! Interrupted saves (WP-0.7 acceptance): a process killed while it saves
//! leaves the previous set readable.
//!
//! The test runs this test binary again as a child that opens a set and
//! saves new revisions of it back to back. The parent watches how long a
//! save takes, kills the child at a random moment late in a save, when it's
//! writing the file, and then checks the set still loads, at a revision no
//! older than the last one the child finished. Twenty rounds with
//! `BL_TEST_DEPTH=full` (pushes to main, manual CI runs, and the nightly
//! run), five with `quick`, the default for local runs and pull requests;
//! each child starts by opening what the last one left.
//!
//! `save::tests::a_save_interrupted_at_any_step_leaves_a_readable_set` in
//! the crate stops a save after each of its steps in turn, including halfway
//! through each write; this test kills real processes.

mod common;

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bl_project::{Project, SetFile, load};
use common::{TestDir, depth, large_song};

/// Set for the child: the set file it saves to.
const CHILD_SET: &str = "BL_PROJECT_TEST_SAVE_UNTIL_KILLED";

/// The child's side. Does nothing unless the parent started it.
#[test]
fn child_saves_until_killed() {
    let Some(path) = std::env::var_os(CHILD_SET) else {
        return;
    };
    let (mut set, project) = SetFile::open(&path).expect("the set loads when a child starts");
    let mut editor = project.editor();
    let track = editor.document().top_level().next().unwrap();
    let mut out = std::io::stdout().lock();
    writeln!(out, "opened {}", set.revision()).unwrap();
    loop {
        // Each revision differs.
        editor
            .apply(bl_model::Command::RenameTrack {
                track,
                name: format!("Revision {}", set.revision() + 1),
            })
            .unwrap();
        let project = Project::from_editor(&editor, project.media.clone());
        set.save(&project).unwrap();
        writeln!(out, "saved {}", set.revision()).unwrap();
        out.flush().unwrap();
    }
}

/// A small, seeded random number generator (xorshift64*), so the kill times
/// are the same on every run on the same machine.
struct Random(u64);

impl Random {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n.max(1)
    }
}

fn leftovers(dir: &Path) -> Vec<OsString> {
    let mut names: Vec<OsString> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().ends_with(".saving"))
        .collect();
    names.sort();
    names
}

#[test]
fn killed_saves_leave_a_readable_set() {
    let dir = TestDir::new("killed");
    let path = dir.path().join("Song.blset");
    let mut set = SetFile::new(&path);
    set.save(&large_song(80, 25)).unwrap();

    let mut random = Random(0x5eed_0007);
    let mut last_saved = 1;
    let (mut mid_write, mut saves) = (0, 0);
    let rounds = depth(20, 5);
    for round in 0..rounds {
        let before = leftovers(dir.path());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "child_saves_until_killed",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_SET, &path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let (lines, received) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send((Instant::now(), line)).is_err() {
                    break;
                }
            }
        });

        // Time two saves, then kill the child late in the one after.
        let mut finished = Vec::new();
        while finished.len() < 3 {
            let (at, line) = received
                .recv_timeout(Duration::from_secs(60))
                .expect("the child reports its saves");
            if let Some(revision) = line.strip_prefix("saved ") {
                let revision: u64 = revision.parse().unwrap();
                assert!(
                    revision > last_saved,
                    "revision {revision} after {last_saved}"
                );
                last_saved = revision;
                finished.push(at);
            }
        }
        // Writing, flushing, and renaming the file come at the end of a
        // save, after encoding, so aim at the last sixth of one.
        let period = finished[2] - finished[1];
        let late = period / 6;
        let before_end =
            Duration::from_micros(random.below(u64::try_from(late.as_micros()).unwrap()));
        let kill_at = finished[2] + period - before_end;
        std::thread::sleep(kill_at.saturating_duration_since(Instant::now()));
        child.kill().unwrap();
        child.wait().unwrap();
        // The pipe is closed now, so once the reader stops, every line the
        // child wrote is in the channel.
        reader.join().unwrap();
        saves += 3;
        while let Ok((_, line)) = received.try_recv() {
            if let Some(revision) = line.strip_prefix("saved ") {
                last_saved = revision.parse().unwrap();
                saves += 1;
            }
        }

        // The set loads, at the last revision the child finished or the one
        // it was writing.
        let (_, info) = load(&path).unwrap_or_else(|error| panic!("round {round}: {error}"));
        assert!(
            info.revision == last_saved || info.revision == last_saved + 1,
            "round {round}: revision {} after the child finished {last_saved}",
            info.revision
        );
        last_saved = info.revision;
        // A new hidden file means the kill landed while the set was being
        // written.
        let after = leftovers(dir.path());
        if after.len() > before.len() {
            mid_write += 1;
        }
        println!(
            "round {round}: a save took {period:?}; killed {before_end:?} before the expected end; the set loads at revision {}{}",
            info.revision,
            if after.len() > before.len() {
                "; killed mid-write"
            } else {
                ""
            }
        );
    }

    // Every backup the children made loads too. A kill between making a
    // backup and pruning the oldest leaves 11, until the next save prunes.
    let backups: Vec<_> = std::fs::read_dir(dir.path().join("Backup"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| bl_project::is_set(path))
        .collect();
    assert!((10..=11).contains(&backups.len()), "{backups:?}");
    for backup in &backups {
        load(backup).unwrap();
    }
    println!(
        "{rounds} children killed after {saves} finished saves; {mid_write} kills landed while the set file was being written; the set loaded every time"
    );
}
