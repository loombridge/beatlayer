//! A Beats clip cuts at its file's transients in what the shell renders
//! (the warp modes spec, R14). The app's media store gives the engine no
//! transients, so the shell gives them from its analyses: without them a
//! clip auto-warped one marker a bar was cut a bar at a time, and slowed
//! down each bar filled its stretch from its middle, replaying half a bar
//! every bar (24 October 2026).
//!
//! The file's left channel is a ramp, so what the render plays decodes to
//! the file frame each song frame plays; its right channel clicks every
//! 0.23 seconds, which the analysis finds as transients.

use std::path::{Path, PathBuf};

use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_model::{
    AudioData, ClipData, ClipId, Command, Track, TrackId, TrackKind, WarpMarker, WarpMode,
};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

const RATE: f64 = 48_000.0;
const SCALE: f64 = 2_097_152.0;
const FILE_BPM: f64 = 130.0;
const SONG_BPM: f64 = 120.0;
const BARS: usize = 8;
const CLICK_EVERY: f64 = 0.23;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-beats-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// File frames in one bar at the file's own tempo.
fn file_bar() -> f64 {
    4.0 * 60.0 / FILE_BPM * RATE
}

/// Song frames in one bar at the song's tempo.
fn song_bar() -> usize {
    (4.0 * 60.0 / SONG_BPM * RATE).round() as usize
}

/// The ramp on the left, a click every 0.23 seconds on the right.
fn wav(path: &Path) {
    let frames = (file_bar() * (BARS as f64 + 2.0)) as usize;
    let mut writer = bl_media::AudioWriter::create(
        path,
        bl_media::EncodeSpec {
            file_type: bl_media::FileType::Wav,
            bit_depth: bl_media::BitDepth::Float32,
            dither: bl_media::Dither::None,
            sample_rate: 48_000,
            channels: 2,
        },
    )
    .unwrap();
    let every = (CLICK_EVERY * RATE) as usize;
    for i in 0..frames {
        let ramp = (i as f64 / SCALE) as f32;
        let since = i % every;
        let click = if i >= every && since < 96 {
            0.8 * (1.0 - since as f32 / 96.0) * if since.is_multiple_of(2) { 1.0 } else { -1.0 }
        } else {
            0.0
        };
        writer.write(&[ramp, click]).unwrap();
    }
    writer.finish().unwrap();
}

fn new_shell(scratch: &Scratch) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(scratch.path("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(Track::id)
        .expect("the default set has an audio track")
}

/// The file on the first audio track, its analysis done, warped in Beats
/// mode with a marker at every bar of the file, as auto-warp leaves a long
/// file, in a song at 120 BPM.
fn beats_clip(scratch: &Scratch) -> (Shell, TrackId, ClipId) {
    let mut shell = new_shell(scratch);
    let track = audio_track(&shell);
    let path = scratch.path("ramp.wav");
    wav(&path);
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [path.display().to_string()], "track": track, "at": 0 }),
        )
        .unwrap();
    let clip = shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    let ClipData::Audio(audio) = &shell.editor().document().clip(clip).unwrap().data else {
        unreachable!("an audio clip");
    };
    let media = audio.media;
    let transients = shell.wait_analysis(media).expect("the file is analysed");
    assert!(
        transients.transients.len() > 20,
        "the analysis finds the clicks: {}",
        transients.transients.len()
    );
    let data = AudioData {
        warp: true,
        warp_markers: (0..=BARS + 1)
            .map(|bar| WarpMarker {
                time: bar as f64 * 4.0 * 60.0 / FILE_BPM,
                beat: Ticks(Ticks::WHOLE.get() * bar as i64),
            })
            .collect(),
        tempo: Some(FILE_BPM),
        warp_mode: WarpMode::Beats,
        ..AudioData::new(media)
    };
    shell.edit(|editor| {
        editor
            .apply(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(SONG_BPM).unwrap()),
            })
            .unwrap();
        editor
            .apply(Command::SetClipData {
                clip,
                data: ClipData::Audio(data),
            })
            .unwrap();
    });
    (shell, track, clip)
}

/// The file frame each song frame of the track's export plays.
fn positions(shell: &mut Shell, scratch: &Scratch, track: TrackId) -> Vec<f64> {
    let path = scratch.path("render.wav");
    shell
        .run(
            "export.audio",
            json!({
                "path": path.display().to_string(),
                "bit_depth": 32,
                "sample_rate": 48_000,
                "length": Ticks::WHOLE.get() * BARS as i64,
                "rendered_track": { "track": track },
                "replace": true,
            }),
        )
        .unwrap();
    let file = bl_media::decode_file(&path).unwrap();
    let left = file.channel(0).unwrap();
    // The track's gain and pan law, from a frame inside bar 1's own audio.
    let probe = 20_000_usize;
    let gain = f64::from(left[probe]) * SCALE / probe as f64;
    left.iter().map(|&s| f64::from(s) * SCALE / gain).collect()
}

/// The furthest the clip reads back behind the furthest file frame it has
/// played, in file frames, over bars 2 to BARS - 1.
fn furthest_read_back(positions: &[f64]) -> f64 {
    let bar = song_bar();
    let mut furthest = f64::MIN;
    let mut back = 0.0_f64;
    for &p in &positions[bar..bar * (BARS - 1)] {
        furthest = furthest.max(p);
        back = back.max(furthest - p);
    }
    back
}

// Spec: warp-modes R14. Slowed down, a Beats clip loops within the slice
// between two transients (detected a few milliseconds apart from the
// clicks), never back over half the bar it has played.
#[test]
fn a_beats_clip_slowed_down_cuts_at_its_file_s_transients() {
    let scratch = Scratch::new("export");
    let (mut shell, track, _) = beats_clip(&scratch);
    let back = furthest_read_back(&positions(&mut shell, &scratch, track));
    assert!(
        back < 1.5 * CLICK_EVERY * RATE,
        "the clip read back {back:.0} frames ({:.3} s): it cut at its warp markers alone",
        back / RATE
    );
}
