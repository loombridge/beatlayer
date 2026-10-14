//! Crash recovery and Save Continuously (WP-1.11 acceptance): killing the
//! process mid-session and relaunching offers the recovered project, and
//! recovering it restores every edit.
//!
//! The tests run this test binary again as a child that opens a set and
//! edits it without pause, journalling each edit as the app does, and
//! reports each edit and, every few edits, that everything so far is on
//! disk. The parent kills it at a random moment, then plays the part of the
//! relaunched app: it starts the autosave service on the same recovery
//! folder, checks the set is offered, recovers it through the registered
//! commands, and checks the recovered song is exactly the song after one of
//! the child's edits, no earlier than the last one the child knew was on
//! disk and no later than the one it was making. The parent replays the
//! child's edits itself to know each state. Then it saves, closes, and
//! starts the next child from what it saved.
//!
//! `BL_TEST_DEPTH=full` (pushes to main, manual CI runs, and the nightly run)
//! kills twelve children and makes 400 edits in one process; `quick`, the
//! default for local runs and pull requests, kills four and makes 100.

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, mpsc};
use std::time::{Duration, Instant};

use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Editor, MidiData, Note, TrackId, TrackKind};
use bl_project::autosave::Autosave;
use bl_project::{Project, SetFile, journal, registry};
use common::{TestDir, depth, full_depth, large_song};
use serde_json::json;

/// Set for the child: the recovery folder.
const CHILD_DIR: &str = "BL_PROJECT_TEST_RECOVERY_DIR";
/// Set for the child: the set it edits.
const CHILD_SET: &str = "BL_PROJECT_TEST_RECOVERY_SET";
/// Set for the child: the number of its first edit.
const CHILD_FIRST: &str = "BL_PROJECT_TEST_RECOVERY_FIRST";
/// Set for the child: turn Save Continuously on.
const CHILD_CONTINUOUS: &str = "BL_PROJECT_TEST_RECOVERY_CONTINUOUS";

/// The child reports that every edit so far is on disk after every this
/// many edits.
const FLUSH_EVERY: u64 = 5;

/// The timing test runs alone. The other tests here start children that
/// edit and write to disk without pause, so beside them it would time the
/// machine's load rather than the journal (on a two-core CI runner, an edit
/// took 1.2 s at most beside them). They share this lock; it takes it whole.
/// That covers `cargo test`, which runs a binary's tests as threads of one
/// process; under cargo-nextest, which runs each test in a process of its
/// own, `.config/nextest.toml` gives the timing test the machine instead.
static ALONE: RwLock<()> = RwLock::new(());

/// Lets the other tests run together, but not beside the timing test.
fn beside_others() -> RwLockReadGuard<'static, ()> {
    ALONE.read().unwrap_or_else(PoisonError::into_inner)
}

/// Applies edit number `n`: one of a fixed cycle of edits, chosen and
/// aimed by `n` and the song as it stands, so the parent can replay
/// exactly what the child did. Returns the edit's undo label.
fn edit(editor: &mut Editor, n: u64) -> String {
    let doc = editor.document();
    let midi: Vec<TrackId> = doc
        .top_level()
        .filter(|track| doc.track(*track).unwrap().kind() == TrackKind::Midi)
        .collect();
    let clips: Vec<(ClipId, TrackId)> = midi
        .iter()
        .flat_map(|track| {
            doc.track(*track)
                .unwrap()
                .clips()
                .map(move |clip| (clip.id, *track))
        })
        .collect();
    let pick = |len: usize| usize::try_from(n).unwrap() % len;
    let bar = |k: u64| Ticks::WHOLE * i64::try_from(k).unwrap();
    let main = doc.main_id();
    let command = match n % 8 {
        0 => {
            let id = editor.new_id();
            bl_model::Command::CreateTrack {
                id,
                kind: TrackKind::Midi,
                name: format!("Track {n}"),
                parent: None,
                index: 0,
            }
        }
        1 if !midi.is_empty() => {
            let notes = (0..4).map(|i| {
                Note::new(
                    u8::try_from(48 + (n + i) % 24).unwrap(),
                    Ticks::QUARTER * i64::try_from(i).unwrap(),
                    Ticks::QUARTER,
                    100,
                )
            });
            let clip = Clip::new(
                editor.new_id(),
                bar(n % 64),
                Ticks::WHOLE,
                ClipData::Midi(MidiData::new(notes)),
            );
            bl_model::Command::CreateClip {
                track: midi[pick(midi.len())],
                clip,
            }
        }
        2 if !clips.is_empty() => {
            // A drag: one transaction of several moves, then a mute.
            let (clip, track) = clips[pick(clips.len())];
            editor
                .transact("Drag Clip", |editor| {
                    for step in 1..=3 {
                        editor.apply(bl_model::Command::MoveClip {
                            clip,
                            track,
                            start: bar(n % 32 + step),
                        })?;
                    }
                    editor.apply(bl_model::Command::SetClipMute {
                        clip,
                        mute: n.is_multiple_of(3),
                    })
                })
                .unwrap();
            return "Drag Clip".to_owned();
        }
        4 if editor.can_undo() => {
            editor.undo().unwrap();
            return "Undo".to_owned();
        }
        5 if editor.can_redo() => {
            editor.redo().unwrap();
            return "Redo".to_owned();
        }
        6 if !clips.is_empty() => bl_model::Command::DeleteClip {
            clip: clips[pick(clips.len())].0,
        },
        7 if !midi.is_empty() => bl_model::Command::RenameTrack {
            track: midi[pick(midi.len())],
            name: format!("Renamed {n}"),
        },
        _ => bl_model::Command::SetVolume {
            track: main,
            volume: f64::from(u32::try_from(n % 97).unwrap()) / 97.0,
        },
    };
    let label = command.label().to_owned();
    editor.apply(command).unwrap();
    label
}

/// The child's side. Does nothing unless the parent started it.
#[test]
fn child_edits_until_killed() {
    let (Some(dir), Some(set), Some(first)) = (
        std::env::var_os(CHILD_DIR),
        std::env::var_os(CHILD_SET),
        std::env::var(CHILD_FIRST).ok(),
    ) else {
        return;
    };
    let first: u64 = first.parse().unwrap();
    let mut autosave = Autosave::start(dir).unwrap();
    println!("offers {}", autosave.offers().len());
    if std::env::var_os(CHILD_CONTINUOUS).is_some() {
        autosave.set_continuous(true);
    }
    let (set, project) = SetFile::open(&set).unwrap();
    autosave.begin(Some(&set)).unwrap();
    let mut editor = project.editor();
    println!("opened");
    for n in first..u64::MAX {
        let label = edit(&mut editor, n);
        autosave.edited(&Project::from_editor(&editor, project.media.clone()), label);
        println!("edit {n}");
        if n.is_multiple_of(FLUSH_EVERY) {
            autosave.flush().unwrap();
            println!("durable {n}");
        }
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

/// A running child and what it has reported.
struct Run {
    child: Child,
    lines: mpsc::Receiver<String>,
    reader: std::thread::JoinHandle<()>,
    offers: Option<usize>,
    edited: Option<u64>,
    durable: Option<u64>,
}

impl Run {
    fn start(dir: &Path, set: &Path, first: u64, continuous: bool) -> Self {
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
            .env(CHILD_FIRST, first.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if continuous {
            command.env(CHILD_CONTINUOUS, "1");
        }
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
            offers: None,
            edited: None,
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
        if let Some(n) = line.strip_prefix("offers ") {
            self.offers = Some(n.parse().unwrap());
        } else if let Some(n) = line.strip_prefix("edit ") {
            self.edited = Some(n.parse().unwrap());
        } else if let Some(n) = line.strip_prefix("durable ") {
            self.durable = Some(n.parse().unwrap());
        }
    }

    /// Kills the child, and reads everything it reported.
    fn kill(mut self) -> Self {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        // The pipe is closed now, so once the reader stops, every line the
        // child wrote is in the channel.
        let reader = std::mem::replace(&mut self.reader, std::thread::spawn(|| {}));
        reader.join().unwrap();
        while let Ok(line) = self.lines.try_recv() {
            self.note(&line);
        }
        self
    }
}

/// The songs after each of the edits `first..=last` from `base`: index 0 is
/// `base` itself.
fn replay(base: &Project, first: u64, last: u64) -> Vec<Project> {
    let mut editor = base.editor();
    let mut states = vec![base.clone()];
    for n in first..=last {
        edit(&mut editor, n);
        states.push(Project::from_editor(&editor, base.media.clone()));
    }
    states
}

/// The newest edit no earlier than `durable` after which the song is
/// `song`, given the `states` from [`replay`] from edit `first`. Undo and
/// redo can bring a song back to an earlier state, so more than one edit
/// can match.
fn matching_edit(states: &[Project], first: u64, durable: u64, song: &Project) -> Option<u64> {
    (0..states.len())
        .rev()
        .map(|i| (first - 1 + u64::try_from(i).unwrap(), &states[i]))
        .find(|(k, state)| *k >= durable && *state == song)
        .map(|(k, _)| k)
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "Discarded")
        .collect();
    names.sort();
    names
}

struct Setup {
    _dir: TestDir,
    recovery: PathBuf,
    set: PathBuf,
}

fn setup(label: &str) -> Setup {
    let dir = TestDir::new(label);
    let recovery = dir.path().join("Recovery");
    let set = dir.path().join("Song Project").join("Song.blset");
    SetFile::new(&set).save(&large_song(24, 6)).unwrap();
    Setup {
        _dir: dir,
        recovery,
        set,
    }
}

#[test]
fn a_killed_session_is_offered_and_every_edit_recovered() {
    // Spec: crash-recovery R2, R3, R5
    let _shared = beside_others();
    let rounds: u64 = depth(12, 4);
    let setup = setup("killed");
    let mut random = Random(0x5eed_0011);
    let mut first = 1;
    let mut offered_rounds = 0;
    for round in 0..rounds {
        let (_, base) = SetFile::open(&setup.set).unwrap();
        let run = Run::start(&setup.recovery, &setup.set, first, false);
        // The last round closed cleanly, so the child was offered nothing.
        assert_eq!(run.offers, Some(0), "round {round}");
        if round % 3 == 0 {
            // While the child runs, another instance offers nothing: the
            // child's session is running, not crashed.
            let other = Autosave::start(&setup.recovery).unwrap();
            assert!(other.offers().is_empty(), "round {round}");
        }
        std::thread::sleep(Duration::from_millis(30 + random.below(470)));
        let run = run.kill();
        let edited = run.edited.unwrap_or(first - 1);
        let durable = run.durable.unwrap_or(first - 1);

        // Relaunch: the set is offered, through the registered commands.
        let mut autosave = Autosave::start(&setup.recovery).unwrap();
        let listed = registry::run(&mut autosave, registry::LIST_RECOVERABLE, json!({})).unwrap();
        let sets = listed.result["sets"].as_array().unwrap().clone();
        // The child's edits from `first`, plus the one it may have been
        // making when it was killed.
        let states = replay(&base, first, edited + 1);
        let (recovered, journalled) = if sets.is_empty() {
            // Nothing reached the disk: only possible if the child never
            // knew an edit was there.
            assert_eq!(durable, first - 1, "round {round}: nothing offered");
            (base.clone(), 0)
        } else {
            offered_rounds += 1;
            assert_eq!(sets.len(), 1, "round {round}: {sets:?}");
            assert_eq!(
                sets[0]["path"].as_str(),
                Some(setup.set.to_str().unwrap()),
                "round {round}"
            );
            assert_eq!(sets[0]["name"], "Song");
            assert_eq!(sets[0]["saved_since"], false);
            let ran = registry::run(&mut autosave, registry::RECOVER, json!({})).unwrap();
            let recovered = ran.recovered.unwrap();
            let edits = recovered.edits;
            let set = recovered.set.unwrap();
            assert_eq!(set.path(), setup.set);
            (recovered.project, edits)
        };
        let k = matching_edit(&states, first, durable, &recovered).unwrap_or_else(|| {
            panic!(
                "round {round}: the recovered song isn't the song after any edit from {durable} \
                 (on disk) to {}",
                edited + 1
            )
        });
        println!(
            "round {round}: the child made edits {first} to {edited}, knew {durable} was on disk; \
             recovered the song after edit {k} from {journalled} journal entries",
        );

        // Save what was recovered and close: the next child starts there,
        // and nothing is left to offer.
        let (mut set, _) = SetFile::open(&setup.set).unwrap();
        if journalled > 0 {
            set.save(&recovered).unwrap();
            autosave.saved(&set);
            autosave.flush().unwrap();
            autosave.close_set().unwrap();
        }
        assert_eq!(files_in(&setup.recovery), Vec::<String>::new());
        assert!(journal::journals_beside(&setup.set).unwrap().is_empty());
        assert_eq!(SetFile::open(&setup.set).unwrap().1, recovered);
        first = edited + 2;
    }
    assert!(
        offered_rounds >= rounds / 2,
        "only {offered_rounds} of {rounds} rounds journalled an edit before the kill"
    );
    println!(
        "{rounds} children killed; {offered_rounds} had journalled edits, each offered and \
         recovered to the song after an edit no earlier than the last one on disk"
    );
}

#[test]
fn with_save_continuously_a_killed_session_reopens_at_its_last_edit() {
    // Spec: crash-recovery R8
    let _shared = beside_others();
    const ROUNDS: u64 = 4;
    let setup = setup("continuous");
    let mut random = Random(0x5eed_0012);
    let mut first = 1;
    for round in 0..ROUNDS {
        let (opened, base) = SetFile::open(&setup.set).unwrap();
        let revision = opened.revision();
        let run = Run::start(&setup.recovery, &setup.set, first, true);
        std::thread::sleep(Duration::from_millis(100 + random.below(300)));
        let run = run.kill();
        let edited = run.edited.unwrap_or(first - 1);
        let durable = run.durable.unwrap_or(first - 1);

        // Nothing to recover: every edit was saved.
        let mut autosave = Autosave::start(&setup.recovery).unwrap();
        assert!(autosave.offers().is_empty(), "round {round}");
        // Opening the set gives the song after one of the child's edits,
        // no earlier than the last one on disk, with the set file itself
        // untouched.
        let (mut set, project) = SetFile::open(&setup.set).unwrap();
        assert_eq!(set.revision(), revision);
        let states = replay(&base, first, edited + 1);
        let k = matching_edit(&states, first, durable, &project).unwrap_or_else(|| {
            panic!(
                "round {round}: the set opens at the song after no edit from {durable} (saved) \
                 to {}",
                edited + 1
            )
        });
        let points = set.save_points();
        println!(
            "round {round}: the child made edits {first} to {edited}, knew {durable} was saved; \
             the set opens at edit {k}, with {} save points since revision {revision}",
            points.len()
        );
        assert!(!points.is_empty() || k == first - 1);

        // A normal save writes the next revision and retires the journal.
        autosave.begin(Some(&set)).unwrap();
        set.save(&project).unwrap();
        autosave.saved(&set);
        autosave.flush().unwrap();
        autosave.close_set().unwrap();
        assert!(journal::journals_beside(&setup.set).unwrap().is_empty());
        assert_eq!(files_in(&setup.recovery), Vec::<String>::new());
        first = edited + 2;
    }
}

/// Every finished edit is on disk within a second, on a song the size of
/// the UI benchmark (200 tracks, 5,000 clips), and reporting one costs the
/// caller next to nothing.
#[test]
fn edits_are_journalled_within_a_second() {
    // Spec: crash-recovery R1
    let _alone = ALONE.write().unwrap_or_else(PoisonError::into_inner);
    let dir = TestDir::new("latency");
    let set_path = dir.path().join("Large.blset");
    let mut set = SetFile::new(&set_path);
    let song = large_song(200, 25);
    set.save(&song).unwrap();
    let mut autosave = Autosave::start(dir.path().join("Recovery")).unwrap();
    autosave.set_continuous(true);
    autosave.begin(Some(&set)).unwrap();
    let mut editor = song.editor();
    let mut waits = Vec::new();
    let mut calls = Vec::new();
    for n in 0..20 {
        edit(&mut editor, n);
        let project = Project::from_editor(&editor, song.media.clone());
        let started = Instant::now();
        autosave.edited(&project, "Edit");
        calls.push(started.elapsed());
        let sent = autosave.status().edits;
        while autosave.status().journalled < sent {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "edit {n} never landed"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        waits.push(started.elapsed());
        assert_eq!(autosave.status().error, None);
    }
    waits.sort();
    calls.sort();
    let journal = journal::journals_beside(&set_path).unwrap();
    let size = std::fs::metadata(&journal[0]).unwrap().len();
    println!(
        "{} tracks, {} clips: an edit is on disk in {:?} at the median and {:?} at most; \
         reporting it takes {:?} at most; 20 save points take {size} bytes ({} build)",
        song.document.track_count(),
        song.document.clip_count(),
        waits[waits.len() / 2],
        waits[waits.len() - 1],
        calls[calls.len() - 1],
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    // The second is wall-clock time, which other processes on a busy
    // machine can take: it's judged at full depth (pushes to main and the
    // nightly run); every run checks each edit lands.
    if full_depth() {
        assert!(waits[waits.len() - 1] < Duration::from_secs(1));
    }
    // Reopening gives the last edit.
    let final_song = Project::from_editor(&editor, song.media.clone());
    assert_eq!(SetFile::open(&set_path).unwrap().1, final_song);
    autosave.close_set().unwrap();
}

/// In one process, with Save Continuously on: once the service says an edit
/// is on disk, opening the set gives the song after it.
#[test]
fn with_save_continuously_every_journalled_edit_is_saved() {
    // Spec: crash-recovery R8
    let _shared = beside_others();
    let setup = setup("every-edit");
    let (set, base) = SetFile::open(&setup.set).unwrap();
    let mut autosave = Autosave::start(&setup.recovery).unwrap();
    autosave.set_continuous(true);
    autosave.begin(Some(&set)).unwrap();
    let mut editor = base.editor();
    let edits: u64 = depth(400, 100);
    let states = replay(&base, 1, edits);
    for n in 1..=edits {
        let label = edit(&mut editor, n);
        let project = Project::from_editor(&editor, base.media.clone());
        let index = usize::try_from(n).unwrap();
        assert_eq!(project, states[index], "edit {n} replays differently");
        autosave.edited(&project, label);
        if n % 5 == 0 {
            autosave.flush().unwrap();
            let opened = SetFile::open(&setup.set).unwrap().1;
            assert_eq!(opened, states[index], "after edit {n}");
        }
    }
    autosave.close_set().unwrap();
    assert_eq!(
        SetFile::open(&setup.set).unwrap().1,
        states[usize::try_from(edits).unwrap()]
    );
}
