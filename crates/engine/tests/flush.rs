//! A flush on a running engine (the audio clips spec, R17): clips' sources
//! open on the loader thread, and a clip without its source is silent, so
//! what tests wait on before they play has to wait for the loader too.

use std::sync::Arc;
use std::time::Duration;

use bl_core::time::{SampleRate, Ticks};
use bl_engine::{Engine, EngineConfig, MediaLibrary, MediaMap, OpenedSource, Registry};
use bl_media::SampleData;
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackKind};

use crate::common::{RATE, track};

/// A library whose files take a while to open, as a loaded machine's do.
struct Slow(MediaMap);

impl MediaLibrary for Slow {
    fn open(&self, media: ContentHash, rate: SampleRate, start: u64) -> Option<OpenedSource> {
        std::thread::sleep(Duration::from_millis(300));
        self.0.open(media, rate, start)
    }
}

// Spec: audio-clips R17. A clip whose source the loader is still opening is
// silent, so a flush doesn't return until the source is open: the first
// block played after it sounds.
#[test]
fn a_flush_waits_for_the_sources_the_loader_is_opening() {
    let hash = ContentHash([1; 32]);
    let mut media = MediaMap::new();
    media.insert(
        hash,
        Arc::new(SampleData::new(48_000, vec![vec![0.5; 48_000]; 2]).unwrap()),
    );
    let mut editor = Editor::new();
    let a = track(&mut editor, TrackKind::Audio, None);
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: a,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Audio(AudioData::new(hash)),
            ),
        })
        .unwrap();
    let registry = Registry::new().with_media(Arc::new(Slow(media)));
    let (engine, mut processor) = Engine::start(EngineConfig::new(RATE, 256), registry).unwrap();
    engine.sync(editor.document());
    engine.play();
    assert!(engine.flush(Duration::from_secs(10)));
    assert!(
        processor.process(256).channel(0).iter().any(|&s| s != 0.0),
        "the clip sounds in the first block after the flush"
    );
}
