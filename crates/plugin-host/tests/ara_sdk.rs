//! ARA 2 hosting with the ARA SDK's own example plug-in (IMP-20; the ARA
//! hosting spec): the plug-in reads a clip's whole audio, plays it at the
//! clip's place at any block size, takes moves, trims and undo as document
//! edits, and keeps its archive with the set.
//!
//! The example plug-in is built on the test machine from the SDK (the spec
//! says how) and never committed, so these tests run when `BL_ARA_TEST_PLUGIN`
//! names its `.vst3` bundle and say they were skipped otherwise. The tests
//! that need no plug-in are in `ara_fake.rs`.
//!
//! Spec: ara-hosting R1, R2, R4, R5, R7, R8, R10.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bl_core::time::{Frames, SampleRate, Ticks};
use bl_engine::{Node, NodeConfig, NodeFactory, TransportInfo};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId, Editor,
    PluginState, TrackId, TrackKind,
};
use bl_plugin_host::ara::AraMedia;
use bl_plugin_host::vst3::Vst3Host;

use crate::common;

const RATE: SampleRate = SampleRate::HZ_48000;
/// A quarter note, in ticks.
const QUARTER: i64 = 705_600;

/// The ARA SDK's example plug-in, if this machine has built it.
fn plugin() -> Option<PathBuf> {
    std::env::var_os("BL_ARA_TEST_PLUGIN").map(PathBuf::from)
}

/// The plug-in's bundle, or a note in the test output saying the test
/// didn't run.
macro_rules! sdk_plugin {
    () => {
        match plugin() {
            Some(path) => path,
            None => {
                eprintln!("skipped: BL_ARA_TEST_PLUGIN names the ARA SDK's example plug-in");
                return;
            }
        }
    };
}

/// ARA starts and stops in the plug-in's binary for each host, so tests of
/// the same binary take turns.
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

/// Two channels of a file's worth of samples that differ everywhere, so a
/// misplaced sample is found.
fn audio(frames: usize) -> Arc<SampleData> {
    let channel = |seed: usize| -> Vec<f32> {
        (0..frames)
            .map(|at| ((at * 31 + seed * 7) % 1999) as f32 / 1999.0 - 0.5)
            .collect()
    };
    Arc::new(SampleData::new(48_000, vec![channel(0), channel(1)]).unwrap())
}

/// A set with one audio track holding the plug-in and a clip of a file one
/// second long, one beat in.
struct World {
    // The node first: the plug-in must be gone from its renderers before
    // the host destroys its regions.
    node: Box<dyn Node>,
    host: Vst3Host,
    editor: Editor,
    track: TrackId,
    device: DeviceId,
    clip: ClipId,
    media: ContentHash,
    audio: Arc<SampleData>,
    revision: u64,
}

fn node_config() -> NodeConfig {
    NodeConfig {
        round_trip_latency: 0,
        sample_rate: RATE,
        max_frames: 4096,
        channels: 2,
    }
}

impl World {
    fn new(bundle: &PathBuf) -> Self {
        let host = Vst3Host::new();
        let types = host.load(bundle).expect("the example plug-in loads");
        let media = ContentHash([7; 32]);
        let mut files = Files::default();
        files.0.insert(media, audio(48_000));
        host.set_ara_media(Arc::new(files));
        Self::with_archives(host, bundle, &types[0], Vec::new())
    }

    /// A set that holds `archives`, as an opened set does.
    fn with_archives(
        host: Vst3Host,
        bundle: &PathBuf,
        found: &bl_plugin_host::vst3::Vst3Type,
        archives: Vec<bl_model::AraArchive>,
    ) -> Self {
        assert!(found.ara.is_some(), "the example plug-in is an ARA plug-in");
        let media = ContentHash([7; 32]);
        let audio = audio(48_000);
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
        let mut instance = Device::new(device, found.device_type.name.clone(), found.device_type.kind.clone());
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
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    clip,
                    Ticks(QUARTER),
                    Ticks(2 * QUARTER),
                    ClipData::Audio(AudioData::new(media)),
                ),
            })
            .unwrap();
        editor.amend_ara_archives(archives);
        host.know(editor.document());
        let node = host
            .factory()
            .create(editor.document().device(device).unwrap(), &node_config())
            .expect("a node for the plug-in's device");
        let mut world = Self {
            host,
            editor,
            track,
            device,
            clip,
            media,
            audio,
            node,
            revision: 1,
        };
        world.settle();
        world
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
        let input = vec![0.0; frames];
        common::run_at(
            self.node.as_mut(),
            &input,
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

    /// The song frame a tick falls on.
    fn frame(&self, ticks: i64) -> i64 {
        self.editor
            .document()
            .settings()
            .tempo
            .ticks_to_frames(Ticks(ticks), RATE)
            .get()
    }
}

// Spec: ara-hosting R1, R2, R5
#[test]
fn the_plugin_gets_a_document_of_the_set() {
    let bundle = sdk_plugin!();
    let _turn = one_at_a_time();
    let world = World::new(&bundle);
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
    let _ = (&world.media, &world.audio);
}

impl World {
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

    /// The one region the plug-in has.
    fn region(&self) -> bl_plugin_host::ara::AraRegion {
        let status = self.host.ara_status();
        assert_eq!(status[0].regions.len(), 1, "{status:?}");
        status[0].regions[0].clone()
    }
}

// Spec: ara-hosting R7
#[test]
fn the_plugin_reads_the_whole_clip_and_plays_it_at_its_place_at_any_block_size() {
    let bundle = sdk_plugin!();
    let _turn = one_at_a_time();
    let mut world = World::new(&bundle);
    let start = world.frame(QUARTER);
    assert_eq!(start, 24_000);
    for size in [1, 100, 512, 4096] {
        let (from, to) = (start - 300, start + 48_000 + 300);
        let heard = world.render(from, to, size);
        let wanted = world.expected(from, to, start, 0, 48_000);
        assert!(heard == wanted, "block size {size}: the plug-in's rendering differs from the decoded file");
    }
    let status = &world.host.ara_status()[0];
    // It read the file whole, once, on its own thread, and the audio thread
    // read nothing.
    assert_eq!(status.readers_opened, 1);
    assert!(status.frames_read >= 48_000);
    assert_eq!(status.audio_thread_reads, 0);
}

// Spec: ara-hosting R2, R6
#[test]
fn moving_and_trimming_a_clip_and_undoing_reach_the_plugin_as_edits() {
    let bundle = sdk_plugin!();
    let _turn = one_at_a_time();
    let mut world = World::new(&bundle);
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
        (region.start_in_playback, region.duration_in_playback, region.start_in_modification),
        (0.5, 1.0, 0.0)
    );
    let status = &world.host.ara_status()[0];
    // Each was an edit cycle of the same document: no object was made again.
    assert_eq!(status.edit_cycles, first + 6);
    assert_eq!((status.sources, status.modifications, status.readers_opened), (1, 1, 1));
}

// Spec: ara-hosting R6, R8
#[test]
fn a_deleted_clip_comes_back_with_the_plugins_edits_by_undo() {
    let bundle = sdk_plugin!();
    let _turn = one_at_a_time();
    let mut world = World::new(&bundle);
    world.edit(Command::DeleteClip { clip: world.clip });
    let status = &world.host.ara_status()[0];
    assert!(status.regions.is_empty());
    // The track is the plug-in's with nothing on it: it renders silence.
    let heard = world.render(0, 100, 100);
    assert!(heard.iter().all(|channel| channel.iter().all(|s| *s == 0.0)));
    world.editor.undo().unwrap();
    world.changed();
    let status = &world.host.ara_status()[0];
    assert_eq!((status.regions.len(), status.modifications, status.sources), (1, 1, 1));
    let start = world.frame(QUARTER);
    let heard = world.render(start, start + 1000, 100);
    assert!(heard == world.expected(start, start + 1000, start, 0, 48_000));
}

// Spec: ara-hosting R10
#[test]
fn the_plugins_archive_is_saved_with_the_set_and_restored_when_it_opens() {
    let bundle = sdk_plugin!();
    let _turn = one_at_a_time();
    let archives = {
        let world = World::new(&bundle);
        let archives = world.host.ara_archives(&[]);
        assert_eq!(archives.len(), 1, "{:?}", world.host.ara_status());
        assert!(!archives[0].data.is_empty());
        archives
    };
    // Another host opens a set that holds it.
    let host = Vst3Host::new();
    let types = host.load(&bundle).unwrap();
    let mut files = Files::default();
    let media = ContentHash([7; 32]);
    files.0.insert(media, audio(48_000));
    host.set_ara_media(Arc::new(files));
    let mut world = World::with_archives(host, &bundle, &types[0], archives);
    world.settle();
    assert_eq!(world.host.ara_status()[0].restored, Some(Ok(())));
}
