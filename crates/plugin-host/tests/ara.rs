//! ARA 2 hosting (IMP-20; the ARA hosting spec): a plug-in that implements
//! Audio Random Access is bound to a document controller, given the set's
//! tracks, files, clips and tempo as edit cycles, reads each clip's whole
//! audio through readers that never run on the audio thread, plays the clips
//! at their place at any block size, takes moves, trims, deletes and undo as
//! document edits, and keeps its archive with the set.
//!
//! Two plug-ins run the same tests. `bl-test-ara` is built from source and
//! runs everywhere. The ARA SDK's own example plug-in is built on the test
//! machine from the SDK and never committed; `BL_ARA_TEST_PLUGIN` names its
//! `.vst3` bundle, and without it those tests say they were skipped. The
//! tests that read what the plug-in kept (`bl-test-ara`'s report) and those
//! that need time stretching run on `bl-test-ara` alone.
//!
//! Spec: ara-hosting R1 to R12.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bl_core::time::{Frames, SampleRate, Ticks};
use bl_engine::{Node, NodeConfig, NodeFactory, TransportInfo};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId, Editor,
    PluginState, TrackId, TrackKind, WarpMarker,
};
use bl_plugin_host::ara::{AraMedia, PlaybackRequest};
use bl_plugin_host::vst3::{Vst3Host, plugin_state};

use crate::common;

const RATE: SampleRate = SampleRate::HZ_48000;
/// A quarter note, in ticks.
const QUARTER: i64 = 705_600;
/// Where the clip starts: one beat, half a second at 120 BPM, in frames.
const START: i64 = 24_000;
const FILE: ContentHash = ContentHash([7; 32]);

/// ARA starts and stops in a plug-in's binary for each host, so the tests of
/// one binary take turns.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The set's files, in memory.
#[derive(Default)]
struct Files(BTreeMap<ContentHash, Arc<SampleData>>);

impl AraMedia for Files {
    fn audio(&self, media: ContentHash) -> Option<Arc<SampleData>> {
        self.0.get(&media).cloned()
    }
}

/// Two channels of `frames` samples that differ everywhere, so a misplaced
/// sample is found.
fn audio(frames: usize) -> Arc<SampleData> {
    let channel = |seed: usize| -> Vec<f32> {
        (0..frames)
            .map(|at| ((at * 31 + seed * 7) % 1999) as f32 / 1999.0 - 0.5)
            .collect()
    };
    Arc::new(SampleData::new(48_000, vec![channel(0), channel(1)]).unwrap())
}

fn node_config() -> NodeConfig {
    NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames: 4096,
        channels: 2,
    }
}

/// A set with one audio track holding the plug-in and a clip of a file one
/// beat in.
struct World {
    // The node first: the plug-in must be gone from its renderers before the
    // host destroys their regions.
    node: Box<dyn Node>,
    host: Vst3Host,
    editor: Editor,
    track: TrackId,
    device: DeviceId,
    clip: ClipId,
    audio: Arc<SampleData>,
    revision: u64,
    /// Whether the plug-in is `bl-test-ara`, which reports what it kept.
    reports: bool,
}

impl World {
    /// A set whose clip plays `frames` frames of a file, with `shape` setting
    /// the clip's audio and `length` its length in ticks.
    fn build(
        bundle: &Path,
        frames: usize,
        length: i64,
        shape: impl FnOnce(&mut AudioData),
        archives: Vec<bl_model::AraArchive>,
    ) -> Self {
        let host = Vst3Host::new();
        let types = host.load(bundle).expect("the plug-in loads");
        let found = &types[0];
        assert!(found.ara.is_some(), "an ARA plug-in");
        let audio = audio(frames);
        let mut files = Files::default();
        files.0.insert(FILE, Arc::clone(&audio));
        host.set_ara_media(Arc::new(files));

        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Audio,
                name: "Vocal".to_owned(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let device: DeviceId = editor.new_id();
        let mut instance = Device::new(
            device,
            found.device_type.name.clone(),
            found.device_type.kind.clone(),
        );
        instance.plugin = Some(PluginState {
            path: bundle.to_string_lossy().into_owned(),
            data: Vec::new(),
        });
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: instance,
            })
            .unwrap();
        let clip: ClipId = editor.new_id();
        let mut data = AudioData::new(FILE);
        shape(&mut data);
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(clip, Ticks(QUARTER), Ticks(length), ClipData::Audio(data)),
            })
            .unwrap();
        editor.amend_ara_archives(archives);
        host.know(editor.document());
        let node = host
            .factory()
            .create(editor.document().device(device).unwrap(), &node_config())
            .expect("a node for the plug-in's device");
        let mut world = Self {
            node,
            host,
            editor,
            track,
            device,
            clip,
            audio,
            revision: 1,
            reports: bundle == bl_test_ara::bundle(),
        };
        world.settle();
        world
    }

    /// A one-second file played whole.
    fn new(bundle: &Path) -> Self {
        Self::build(bundle, 48_000, 2 * QUARTER, |_| {}, Vec::new())
    }

    /// Edits the set and lets the host follow.
    fn edit(&mut self, command: Command) {
        self.editor.apply(command).unwrap();
        self.changed();
    }

    fn changed(&mut self) {
        self.revision += 1;
        self.settle();
    }

    /// Runs the host's main-thread work and the node's blocks for a few
    /// rounds, which is enough for the plug-in to be handed back, edited and
    /// activated again.
    fn settle(&mut self) {
        for _ in 0..20 {
            self.host.idle();
            self.host.sync_ara(self.editor.document(), self.revision);
            self.block(0, 64, false);
        }
        assert!(
            self.host.status(self.device).is_some_and(|s| s.active),
            "the plug-in never became active: {:?} {:?}",
            self.host.status(self.device),
            self.host.ara_status()
        );
    }

    /// Renders `frames` frames from song frame `at`, playing or not.
    fn block(&mut self, at: i64, frames: usize, playing: bool) -> Vec<Vec<f32>> {
        common::run_at(
            self.node.as_mut(),
            &vec![0.0; frames],
            &[],
            &[],
            TransportInfo {
                playing,
                position: Frames(at),
            },
        )
        .channels
    }

    /// Renders song frames `from..to` in blocks of `size`.
    fn render(&mut self, from: i64, to: i64, size: usize) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::new(), Vec::new()];
        let mut at = from;
        while at < to {
            let frames = size.min(usize::try_from(to - at).unwrap());
            for (channel, block) in self.block(at, frames, true).into_iter().enumerate() {
                out[channel].extend(block);
            }
            at += i64::try_from(frames).unwrap();
        }
        out
    }

    /// What the file plays at song frames `from..to` when it starts at song
    /// frame `start` with `skip` frames of it left out: silence outside.
    fn expected(&self, from: i64, to: i64, start: i64, skip: i64, length: i64) -> Vec<Vec<f32>> {
        (0..2)
            .map(|channel| {
                let file = self.audio.channel(channel).unwrap();
                (from..to)
                    .map(|frame| {
                        let at = frame - start;
                        if (0..length).contains(&at) {
                            file[usize::try_from(at + skip).unwrap()]
                        } else {
                            0.0
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// The song frame a tick falls on.
    fn frame(&self, ticks: i64) -> i64 {
        self.editor
            .document()
            .settings()
            .tempo
            .ticks_to_frames(Ticks(ticks), RATE)
            .get()
    }

    /// The one region the plug-in has.
    fn region(&self) -> bl_plugin_host::ara::AraRegion {
        let status = self.host.ara_status();
        assert_eq!(status[0].regions.len(), 1, "{status:?}");
        status[0].regions[0].clone()
    }

    /// What `bl-test-ara` kept, from its saved state.
    fn report(&self) -> bl_test_ara::Report {
        assert!(self.reports, "only bl-test-ara reports");
        let state = self.host.save_state(self.device).unwrap();
        bl_test_ara::report_of(&state.data)
    }
}

// Spec: ara-hosting R1, R2, R5
fn the_plugin_gets_a_document_of_the_set(bundle: &Path) {
    let world = World::new(bundle);
    let status = world.host.ara_status();
    assert_eq!(status.len(), 1);
    let ara = &status[0];
    assert_eq!((ara.sequences, ara.sources, ara.modifications), (1, 1, 1));
    assert_eq!(ara.devices, vec![(world.device, Some(world.track))]);
    assert_eq!(ara.regions.len(), 1);
    let region = &ara.regions[0];
    assert_eq!(region.clip, world.clip);
    // One beat in at 120 BPM is half a second; the file plays a second.
    assert_eq!(region.start_in_playback, 0.5);
    assert_eq!(region.duration_in_playback, 1.0);
    assert_eq!(region.start_in_modification, 0.0);
    assert_eq!(region.duration_in_modification, 1.0);
    assert_eq!(ara.missing, vec![]);
}

// Spec: ara-hosting R7
fn the_plugin_reads_the_whole_clip_and_plays_it_at_its_place_at_any_block_size(bundle: &Path) {
    let mut world = World::new(bundle);
    assert_eq!(world.frame(QUARTER), START);
    for size in [1, 100, 512, 4096] {
        let (from, to) = (START - 300, START + 48_000 + 300);
        let heard = world.render(from, to, size);
        let wanted = world.expected(from, to, START, 0, 48_000);
        assert!(
            heard == wanted,
            "block size {size}: the plug-in's rendering differs from the decoded file"
        );
    }
    let status = &world.host.ara_status()[0];
    // It read the file whole, once, on its own thread, and the audio thread
    // read nothing.
    assert_eq!(status.readers_opened, 1);
    assert!(status.frames_read >= 48_000);
    assert_eq!(status.audio_thread_reads, 0);
}

// Spec: ara-hosting R2, R6
fn moving_and_trimming_a_clip_and_undoing_reach_the_plugin_as_edits(bundle: &Path) {
    let mut world = World::new(bundle);
    let first = world.host.ara_status()[0].edit_cycles;

    // Moved a beat later.
    world.edit(Command::MoveClip {
        clip: world.clip,
        track: world.track,
        start: Ticks(2 * QUARTER),
    });
    let region = world.region();
    assert_eq!((region.start_in_playback, region.duration_in_playback), (1.0, 1.0));
    let start = world.frame(2 * QUARTER);
    let heard = world.render(start - 10, start + 48_010, 512);
    assert!(heard == world.expected(start - 10, start + 48_010, start, 0, 48_000));

    // Trimmed at the start by half a beat (a quarter of a second): the Start
    // marker follows.
    world.edit(Command::TrimClipStart {
        clip: world.clip,
        start: Ticks(2 * QUARTER + QUARTER / 2),
    });
    let region = world.region();
    assert_eq!(region.start_in_modification, 0.25);
    assert_eq!(region.start_in_playback, 1.25);
    assert_eq!(region.duration_in_playback, 0.75);
    let start = world.frame(2 * QUARTER + QUARTER / 2);
    let heard = world.render(start - 10, start + 36_010, 100);
    assert!(heard == world.expected(start - 10, start + 36_010, start, 12_000, 36_000));

    // Shortened to a beat.
    world.edit(Command::SetClipLength {
        clip: world.clip,
        length: Ticks(QUARTER),
    });
    assert_eq!(world.region().duration_in_playback, 0.5);

    // Three edits, three steps back.
    for _ in 0..3 {
        world.editor.undo().unwrap();
        world.changed();
    }
    let region = world.region();
    assert_eq!(
        (
            region.start_in_playback,
            region.duration_in_playback,
            region.start_in_modification
        ),
        (0.5, 1.0, 0.0)
    );
    let status = &world.host.ara_status()[0];
    // Each was an edit cycle of the same document: no object was made again.
    assert_eq!(status.edit_cycles, first + 6);
    assert_eq!(
        (status.sources, status.modifications, status.readers_opened),
        (1, 1, 1)
    );
}

// Spec: ara-hosting R6, R8
fn a_deleted_clip_comes_back_with_the_plugins_edits_by_undo(bundle: &Path) {
    let mut world = World::new(bundle);
    world.edit(Command::DeleteClip { clip: world.clip });
    let status = &world.host.ara_status()[0];
    assert!(status.regions.is_empty());
    // The track is the plug-in's with nothing on it: it renders silence.
    let heard = world.render(0, 100, 100);
    assert!(heard.iter().all(|channel| channel.iter().all(|s| *s == 0.0)));
    world.editor.undo().unwrap();
    world.changed();
    let status = &world.host.ara_status()[0];
    assert_eq!(
        (status.regions.len(), status.modifications, status.sources),
        (1, 1, 1)
    );
    let heard = world.render(START, START + 1000, 100);
    assert!(heard == world.expected(START, START + 1000, START, 0, 48_000));
}

// Spec: ara-hosting R10
fn the_plugins_archive_is_saved_with_the_set_and_restored_when_it_opens(bundle: &Path) {
    let archives = {
        let world = World::new(bundle);
        let archives = world.host.ara_archives(&[]);
        assert_eq!(archives.len(), 1, "{:?}", world.host.ara_status());
        assert!(!archives[0].data.is_empty());
        archives
    };
    // Another host opens a set that holds it.
    let world = World::build(bundle, 48_000, 2 * QUARTER, |_| {}, archives);
    assert_eq!(world.host.ara_status()[0].restored, Some(Ok(())));
}

// Spec: ara-hosting R9
fn an_offline_render_borrows_the_plugin_and_renders_its_clips(bundle: &Path) {
    let World {
        node,
        host,
        editor,
        device,
        audio,
        ..
    } = World::new(bundle);
    // The engine's node runs on a thread of its own, as on an audio device.
    let engine = common::Engine::start(node);
    {
        let loan = host.lend().unwrap();
        // The render has its own block size, at the file's rate.
        let config = NodeConfig {
            round_trip_latency: 0,
            sample_rate: RATE,
            max_frames: 32,
            channels: 2,
        };
        let mut render = loan
            .factory()
            .create(editor.document().device(device).unwrap(), &config)
            .expect("a node for the render");
        assert!(loan.problems().is_empty(), "{:?}", loan.problems());
        assert!(host.status(device).unwrap().active);
        // Played from the clip's start, it is the file.
        for at in (START..START + 1_600).step_by(32) {
            let heard = common::run_at(
                render.as_mut(),
                &[0.0; 32],
                &[],
                &[],
                TransportInfo {
                    playing: true,
                    position: Frames(at),
                },
            );
            for (channel, block) in heard.channels.iter().enumerate() {
                let file = audio.channel(channel).unwrap();
                let from = usize::try_from(at - START).unwrap();
                assert!(block == &file[from..from + 32], "frame {at}, channel {channel}");
            }
        }
        drop(render);
    }
    // Back in the engine, still bound and active.
    drop(engine);
    host.idle();
    assert!(host.status(device).unwrap().failure.is_none());
}

/// One of the tests above as a `#[test]` against `$bundle`, which is `None`
/// where this machine lacks the plug-in.
macro_rules! run {
    ($bundle:expr, $test:ident) => {
        #[test]
        fn $test() {
            let Some(bundle) = $bundle else {
                eprintln!("skipped: BL_ARA_TEST_PLUGIN names the ARA SDK's example plug-in");
                return;
            };
            let _turn = one_at_a_time();
            super::$test(&bundle);
        }
    };
}

/// The same tests against each plug-in: `bl-test-ara` always, the SDK's
/// example when this machine has built it.
macro_rules! suite {
    ($module:ident, $bundle:expr) => {
        mod $module {
            use super::*;

            run!($bundle, the_plugin_gets_a_document_of_the_set);
            run!($bundle, the_plugin_reads_the_whole_clip_and_plays_it_at_its_place_at_any_block_size);
            run!($bundle, moving_and_trimming_a_clip_and_undoing_reach_the_plugin_as_edits);
            run!($bundle, a_deleted_clip_comes_back_with_the_plugins_edits_by_undo);
            run!($bundle, the_plugins_archive_is_saved_with_the_set_and_restored_when_it_opens);
            run!($bundle, an_offline_render_borrows_the_plugin_and_renders_its_clips);
        }
    };
}

suite!(test_plugin, Some(bl_test_ara::bundle().to_path_buf()));
suite!(
    sdk_plugin,
    std::env::var_os("BL_ARA_TEST_PLUGIN").map(PathBuf::from)
);

// --- What only bl-test-ara can tell ---------------------------------------

// Spec: ara-hosting R8
#[test]
fn the_host_never_changes_a_running_plugins_regions_or_destroys_what_is_in_use() {
    let _turn = one_at_a_time();
    let mut world = World::new(bl_test_ara::bundle());
    // Every kind of membership change, with the plug-in running.
    let second: ClipId = world.editor.new_id();
    world.edit(Command::CreateClip {
        track: world.track,
        clip: Clip::new(
            second,
            Ticks(8 * QUARTER),
            Ticks(QUARTER),
            ClipData::Audio(AudioData::new(FILE)),
        ),
    });
    world.edit(Command::DeleteClip { clip: world.clip });
    world.editor.undo().unwrap();
    world.changed();
    world.edit(Command::SetClipMute {
        clip: second,
        mute: true,
    });
    world.edit(Command::SetClipMute {
        clip: second,
        mute: false,
    });
    let report = world.report();
    assert_eq!(report.violations, 0);
    assert_eq!(world.host.ara_status()[0].regions.len(), 2);
}

// Spec: ara-hosting R6
#[test]
fn a_modification_that_left_the_set_comes_back_with_what_the_plugin_kept() {
    let _turn = one_at_a_time();
    let mut world = World::new(bl_test_ara::bundle());
    // A move updates the clip's properties in the plug-in: its own edit
    // count goes up.
    world.edit(Command::RenameClip {
        clip: world.clip,
        name: "Take 2".to_owned(),
    });
    let id = format!("clip-{}", u64::from(world.clip.0));
    let kept = world.report().modifications;
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].id.as_str(), kept[0].active), (id.as_str(), true));
    let updates = kept[0].updates;
    assert!(updates >= 1);

    // Deleted, it stays in the plug-in, deactivated.
    world.edit(Command::DeleteClip { clip: world.clip });
    let report = world.report();
    assert_eq!(report.modifications.len(), 1);
    assert_eq!(
        (report.modifications[0].updates, report.modifications[0].active),
        (updates, false)
    );

    // Undone, it is the same modification, active again.
    world.editor.undo().unwrap();
    world.changed();
    let report = world.report();
    assert_eq!(report.modifications.len(), 1);
    assert_eq!(
        (report.modifications[0].updates, report.modifications[0].active),
        (updates, true)
    );
}

// Spec: ara-hosting R10
#[test]
fn what_the_plugin_kept_is_in_its_archive() {
    let _turn = one_at_a_time();
    let archives = {
        let mut world = World::new(bl_test_ara::bundle());
        world.edit(Command::RenameClip {
            clip: world.clip,
            name: "Take 2".to_owned(),
        });
        let updates = world.report().modifications[0].updates;
        assert!(updates >= 1);
        (world.host.ara_archives(&[]), updates)
    };
    let (archives, updates) = archives;
    assert_eq!(archives[0].archive_id, "io.loombridge.beatlayer.test-ara.archive.1");
    // Opened again: the set's clip is the one the archive names (the set
    // is built the same way, so it has the same ID), and the plug-in has its
    // edits back.
    let world = World::build(bl_test_ara::bundle(), 48_000, 2 * QUARTER, |_| {}, archives);
    let report = world.report();
    assert_eq!(report.archive_id, "io.loombridge.beatlayer.test-ara.archive.1");
    assert_eq!(report.modifications.len(), 1);
    assert_eq!(
        (report.modifications[0].updates, report.modifications[0].restored),
        (updates, Some(updates))
    );
}

// Spec: ara-hosting R5
#[test]
fn a_warped_clip_is_played_stretched_to_its_warp_map() {
    let _turn = one_at_a_time();
    // A file of two seconds: its first second is squeezed into a beat (half
    // a second at 120 BPM), and after that marker it plays at the clip's
    // tempo of 120.
    let world_shape = |audio: &mut AudioData| {
        audio.warp = true;
        audio.warp_markers = vec![
            WarpMarker {
                time: 0.0,
                beat: Ticks(0),
            },
            WarpMarker {
                time: 1.0,
                beat: Ticks(QUARTER),
            },
        ];
        audio.tempo = Some(120.0);
        audio.start = 0.0;
        audio.end = None;
    };
    let mut world = World::build(
        bl_test_ara::bundle(),
        96_000,
        8 * QUARTER,
        world_shape,
        Vec::new(),
    );
    let status = &world.host.ara_status()[0];
    assert!(status.timestretch);
    let regions: Vec<_> = status
        .regions
        .iter()
        .map(|r| (r.piece, r.start_in_playback, r.duration_in_playback))
        .collect();
    assert_eq!(regions, vec![(0, 0.5, 0.5), (1, 1.0, 1.0)]);
    // The clip player leaves it to the plug-in: the first piece plays the
    // file's samples two to a frame, the second one to a frame.
    let (from, to) = (START - 10, START + 72_010);
    let heard = world.render(from, to, 512);
    for channel in 0..2 {
        let file = world.audio.channel(channel).unwrap();
        for frame in from..to {
            let want = if (START..48_000).contains(&frame) {
                file[usize::try_from(2 * (frame - START)).unwrap()]
            } else if (48_000..96_000).contains(&frame) {
                file[usize::try_from(frame).unwrap()]
            } else {
                0.0
            };
            assert_eq!(
                heard[channel][usize::try_from(frame - from).unwrap()],
                want,
                "frame {frame}, channel {channel}"
            );
        }
    }
}

// Spec: ara-hosting R12
#[test]
fn the_editor_view_is_told_what_the_arrangement_has_selected() {
    let _turn = one_at_a_time();
    let world = World::new(bl_test_ara::bundle());
    world.host.ara_select(&[world.clip], &[world.track]);
    world.host.ara_select(&[], &[world.track]);
    // A clip on another track is nobody's here.
    world.host.ara_select(&[], &[]);
    assert_eq!(world.report().selections, vec![(1, 1), (0, 1), (0, 0)]);
}

// Spec: ara-hosting R11
#[test]
fn the_transport_requests_a_plugin_makes_reach_the_host() {
    let _turn = one_at_a_time();
    let world = World::new(bl_test_ara::bundle());
    world
        .host
        .load_state(world.device, &plugin_state(bl_test_ara::REQUEST, &[]))
        .unwrap();
    assert_eq!(
        world.host.take_ara_requests(),
        vec![
            PlaybackRequest::Start,
            PlaybackRequest::Locate(2.5),
            PlaybackRequest::CycleRange {
                start: 1.0,
                duration: 2.0
            },
            PlaybackRequest::Cycle(true),
            PlaybackRequest::Stop,
        ]
    );
    assert!(world.host.take_ara_requests().is_empty());
}

// Spec: ara-hosting R4
#[test]
fn clips_the_plugin_cant_render_stay_with_the_clip_player() {
    let _turn = one_at_a_time();
    // A clip at half gain is the player's, not the plug-in's.
    let world = World::build(
        bl_test_ara::bundle(),
        48_000,
        2 * QUARTER,
        |audio| audio.gain = 0.5,
        Vec::new(),
    );
    let status = &world.host.ara_status()[0];
    assert!(status.regions.is_empty());
    assert_eq!(status.modifications, 0);
}
