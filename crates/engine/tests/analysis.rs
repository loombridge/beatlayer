//! Analysis beside playback (the warping spec, R28): a file is analysed on
//! the analysis cache's own threads, which allocate freely, while the engine
//! renders blocks under the allocation trap. Nothing on either side waits on
//! the other.

use crate::common;
use std::sync::Arc;

use bl_analysis::cache::{AnalysisCache, AnalysisOptions, AnalysisStatus};
use bl_core::time::Ticks;
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::{MediaStore, SampleData};
use bl_model::{AudioData, Clip, ClipData, Command, ContentHash, Editor, TrackKind};
use common::{RATE, track};

/// A mono WAV file of `seconds` of clicks at 48 kHz, written as bytes.
fn clicks(seconds: usize) -> Vec<u8> {
    let frames = seconds * 48_000;
    let mut bytes = Vec::with_capacity(44 + 2 * frames);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + 2 * frames as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&[1, 0, 1, 0]);
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 16, 0]);
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(2 * frames as u32).to_le_bytes());
    for n in 0..frames {
        let click = if n % 12_000 < 200 { 16_000i16 } else { 0 };
        bytes.extend_from_slice(&click.to_le_bytes());
    }
    bytes
}

// Spec: warping R28
#[test]
fn blocks_stay_heap_free_while_a_file_is_analysed() {
    let folder = std::env::temp_dir().join(format!("bl-engine-analysis-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("clicks.wav");
    std::fs::write(&path, clicks(60)).unwrap();
    let store = Arc::new(MediaStore::new());
    let hash = store.import(&path).unwrap();
    let cache = AnalysisCache::new(Arc::clone(&store), AnalysisOptions::memory_only()).unwrap();

    // The clip plays a ramp of its own; the analysis is of the file beside it.
    let media = ContentHash([1; 32]);
    let mut map = MediaMap::new();
    map.insert(
        media,
        Arc::new(SampleData::new(48_000, vec![vec![0.25; 96_000]]).unwrap()),
    );
    let mut editor = Editor::new();
    let audio_track = track(&mut editor, TrackKind::Audio, None);
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: audio_track,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks::WHOLE * 8,
                ClipData::Audio(AudioData::new(media)),
            ),
        })
        .unwrap();

    for block in [1, 61, 512, 4_096] {
        let registry = Registry::new().with_media(Arc::new(map.clone()));
        let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
        offline.sync(editor.document());
        offline.sender().transport(TransportCommand::Play);
        // Block-by-block while the analysis is under way.
        let mut rendered = 0;
        while matches!(cache.request(hash), AnalysisStatus::Pending) && rendered < 200 {
            offline.render(block.max(256) as u64, |_| {});
            assert_eq!(offline.heap_operations(), 0, "block size {block}");
            rendered += 1;
        }
        if block == 1 {
            assert!(rendered > 0, "the analysis ran beside the blocks");
        }
        // The next block size analyses the file again.
        cache.forget(hash);
    }
    let analysis = cache.wait(hash).unwrap();
    assert_eq!(analysis.transients.len(), 240);
    let _ = std::fs::remove_dir_all(&folder);
}
