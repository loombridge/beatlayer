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
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId, DeviceKind, Editor,
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
        let found = &types[0];
        assert!(found.ara.is_some(), "the example plug-in is an ARA plug-in");
        let media = ContentHash([7; 32]);
        let audio = audio(48_000);
        let mut files = Files::default();
        files.0.insert(media, Arc::clone(&audio));
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

    /// Runs the host's main-thread work and the node's blocks until the
    /// plug-in is active with every region the set gives it.
    fn settle(&mut self) {
        for _ in 0..400 {
            self.host.idle();
            self.host.sync_ara(self.editor.document(), self.revision);
            self.block(0, 64, false);
            let status = self.host.status(self.device);
            let ara = self.host.ara_status();
            let regions = ara.first().map_or(0, |a| a.regions.len());
            if status.is_some_and(|s| s.active) && regions > 0 {
                return;
            }
        }
        panic!(
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
