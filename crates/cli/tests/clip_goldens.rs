//! WP-1.4's acceptance: in the clip fixtures' golden renders, every sample
//! is the frame of its file the spec says. A model of clip playback, written
//! here from the audio clips spec apart from the engine's code, works
//! out for each frame of the song which clip covers it, which frame of the
//! clip's file plays there (from the Start and End markers and the loop),
//! and at what level (the gain and the fades); each frame of the render must
//! be that sample at that level, to the bit, or silence. So clips start and
//! end on the frames the tempo map gives, loops wrap on their exact frames,
//! and fades are straight lines from the clips' edges.
//!
//! The null tests against the stored references are in `golden.rs`.

bl_rt::install_trap!();

use bl_cli::golden::{BLOCK, Render, SAMPLE_RATE, Scene, fixture};
use bl_cli::render::{self, Settings};
use bl_engine::MediaMap;
use bl_model::{ClipData, Command, Editor, TrackId};

/// A time in a file, in seconds, as a frame at 48 kHz.
fn frames(seconds: f64) -> u64 {
    (seconds * SAMPLE_RATE.as_f64()).round() as u64
}

/// What the spec says each frame of the song plays: the sum, over the clips
/// that cover it, of their file's sample at the frame their markers and
/// loop give, times their gain and fades. Left and right, interleaved. Only
/// for songs whose files are at 48 kHz and whose mixers are at rest.
fn model(scene: &Scene) -> Vec<f32> {
    let document = scene.document();
    let tempo = &document.settings().tempo;
    let mut out = vec![0.0f32; scene.frames as usize * 2];
    for track in document.tracks() {
        assert_eq!(*track.mixer(), bl_model::Mixer::default(), "at rest");
        for clip in track.clips() {
            let ClipData::Audio(audio) = &clip.data else {
                continue;
            };
            let file = scene.media.get(audio.media).expect("the file is there");
            assert_eq!(file.sample_rate(), SAMPLE_RATE.hz());
            let length = file.frames() as u64;
            // R1: from the frame the start falls on to the frame the end
            // does.
            let start = tempo.ticks_to_frames(clip.start, SAMPLE_RATE).get();
            let end = tempo.ticks_to_frames(clip.end(), SAMPLE_RATE).get();
            let (fade_in, fade_out) = (frames(audio.fade_in), frames(audio.fade_out));
            for song in start..end {
                let elapsed = (song - start) as u64;
                // R3, R4: the frame of the file.
                let linear = frames(audio.start) + elapsed;
                let position = if audio.looping {
                    let (from, to) = (
                        frames(audio.loop_start),
                        audio.loop_end.map_or(length, frames),
                    );
                    if linear < to {
                        linear
                    } else {
                        from + (linear - to) % (to - from)
                    }
                } else if linear < audio.end.map_or(length, frames) {
                    linear
                } else {
                    continue;
                };
                // R5, R6: the gain, and straight-line fades from the edges.
                let mut fade = 1.0f64;
                if elapsed < fade_in {
                    fade = elapsed as f64 / fade_in as f64;
                }
                let left = (end - song) as u64;
                if left < fade_out {
                    fade = fade.min(left as f64 / fade_out as f64);
                }
                for c in 0..2 {
                    let channel = file.channel(c.min(file.channel_count() - 1)).unwrap();
                    let sample = channel.get(position as usize).copied().unwrap_or(0.0);
                    let at = song as usize * 2 + c;
                    if at < out.len() {
                        out[at] += (f64::from(sample) * (audio.gain * fade)) as f32;
                    }
                }
            }
        }
    }
    out
}

/// Checks a fixture's render against the model, to the bit, and returns
/// the render.
fn matches_the_model(name: &str) -> (Scene, Render) {
    let fixture = fixture(name).expect("the fixture exists");
    let scene = fixture.scene();
    let render = fixture.render();
    assert!(render.problems().is_empty(), "{:?}", render.problems());
    let expected = model(&scene);
    assert_eq!(render.samples.len(), expected.len());
    for (i, (&got, &want)) in render.samples.iter().zip(&expected).enumerate() {
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "{name}: frame {}, channel {}: {got} where the spec gives {want}",
            i / 2,
            i % 2
        );
    }
    (scene, render)
}

fn left(render: &Render, frame: usize) -> f32 {
    render.samples[frame * 2]
}

/// The chirp file's left channel.
fn chirp(scene: &Scene) -> &[f32] {
    let file = scene.files.iter().find(|f| f.name == "chirp.wav").unwrap();
    scene.media.get(file.hash).unwrap().channel(0).unwrap()
}

// Spec: audio-clips R1, R3
#[test]
fn clips_starting_mid_bar_play_their_markers_from_their_first_frame() {
    let (scene, render) = matches_the_model("clip-mid-bar");
    let chirp = chirp(&scene);
    // 1.2.3 is frame 36,000, and the clip plays the file from 0.5 seconds
    // (frame 24,000) to 1.75 (84,000).
    assert_eq!(left(&render, 35_999), 0.0);
    assert_eq!(left(&render, 36_000), chirp[24_000]);
    assert_ne!(chirp[24_000], 0.0, "mid-waveform");
    assert_eq!(left(&render, 95_999), chirp[83_999]);
    assert_eq!(left(&render, 96_000), 0.0);
    // 2.3.2 is frame 150,000, from two seconds in, for a beat.
    assert_eq!(left(&render, 150_000), chirp[96_000]);
    assert_eq!(left(&render, 173_999), chirp[119_999]);
    assert_eq!(left(&render, 174_000), 0.0);
}

// Spec: audio-clips R4, R5
#[test]
fn a_looped_clip_wraps_on_its_exact_frames() {
    let (scene, render) = matches_the_model("looped-clip");
    let chirp = chirp(&scene);
    let half = |sample: f32| (f64::from(sample) * 0.5) as f32;
    // From 1.1.3 (frame 12,000): the Start marker at 0.25 seconds, into the
    // brace at 0.5 (frame 24,000 of the file) a quarter of a second later,
    // and back to it from 0.8 (38,400) every 14,400 frames.
    assert_eq!(left(&render, 11_999), 0.0);
    assert_eq!(left(&render, 12_000), half(chirp[12_000]));
    assert_eq!(left(&render, 24_000), half(chirp[24_000]));
    assert_eq!(left(&render, 38_399), half(chirp[38_399]));
    for wrap in 0..9 {
        let at = 38_400 + 14_400 * wrap;
        assert_eq!(left(&render, at), half(chirp[24_000]), "wrap {wrap}");
    }
    // It ends three seconds after it starts.
    assert_ne!(left(&render, 155_999), 0.0);
    assert_eq!(left(&render, 156_000), 0.0);
}

// Spec: audio-clips R5, R6, R7
#[test]
fn fades_off_cut_and_fades_on_rise_from_silence() {
    let (scene, render) = matches_the_model("clip-fades");
    let chirp = chirp(&scene);
    // Fades off: the first frame (1.1.2, frame 6,000) is the file's, mid-
    // waveform, and so is the last.
    assert_eq!(left(&render, 6_000), chirp[14_400]);
    assert_ne!(chirp[14_400], 0.0);
    assert_eq!(left(&render, 29_999), chirp[38_399]);
    // Create Fades on Clip Edges' 4 ms: 192 frames from silence (1.3.1,
    // frame 48,000), and back to it.
    assert_eq!(left(&render, 48_000), 0.0);
    let ramp = (f64::from(chirp[48_000 + 96]) * 0.5) as f32;
    assert_eq!(left(&render, 48_096), ramp, "half way in");
    assert_eq!(left(&render, 48_192), chirp[48_192]);
    // Long fades on a clip at +3.5 dB (a gain of 1.5).
    assert_eq!(left(&render, 96_000), 0.0);
    let full = (f64::from(chirp[72_000 + 12_000]) * 1.5) as f32;
    assert_eq!(left(&render, 96_000 + 12_000), full, "after the fade-in");
}

// Spec: audio-clips R2, R4
#[test]
fn across_a_tempo_ramp_the_audio_runs_on_and_the_ends_follow_the_map() {
    let (scene, render) = matches_the_model("clip-tempo-change");
    let chirp = chirp(&scene);
    let tempo = &scene.document().settings().tempo;
    let frame = |beats: i64| {
        let ticks = bl_core::time::Ticks::QUARTER * beats;
        tempo.ticks_to_frames(ticks, SAMPLE_RATE).get() as usize
    };
    // The first clip runs from beat 1 (frame 24,000) through the ramp to
    // beat 4, its file unbroken: its last frame is the file's frame that
    // many frames in, though the clip is three beats long.
    let (start, end) = (frame(1), frame(4));
    assert_eq!(start, 24_000);
    assert!(end - start > 72_000, "slower than 120 BPM: {}", end - start);
    assert_eq!(left(&render, end - 1), chirp[end - 1 - start]);
    // The looped clip starts on beat 4 with a 4 ms fade, under 90 BPM.
    assert_eq!(left(&render, end), 0.0);
    assert_eq!(frame(6) - frame(4), 64_000, "two beats at 90 BPM");
}

/// Renders `editor`'s song as `scene` plays, with `scene`'s media.
fn render_song(editor: &Editor, media: &MediaMap, frames: u64) -> Vec<f32> {
    let settings = Settings {
        sample_rate: SAMPLE_RATE,
        block: BLOCK,
        frames,
    };
    let mut samples = Vec::new();
    render::render_with(
        editor.document(),
        std::sync::Arc::new(media.clone()),
        &[],
        &[render::Cue::PLAY],
        &settings,
        |output| render::interleave(output, &mut samples),
    );
    samples
}

// Spec: audio-clips R2, R8, R9
#[test]
fn overlapping_clips_on_two_tracks_add_to_the_bit() {
    let fixture = fixture("overlapping-clips").expect("the fixture exists");
    let scene = fixture.scene();
    let both = fixture.render();
    assert!(both.problems().is_empty(), "{:?}", both.problems());
    let tracks: Vec<TrackId> = scene.document().top_level().collect();
    assert_eq!(tracks.len(), 2);
    let alone = |keep: TrackId| {
        let mut editor = fixture.scene().editor;
        for &track in &tracks {
            if track != keep {
                editor.apply(Command::DeleteTrack { track }).unwrap();
            }
        }
        render_song(&editor, &scene.media, scene.frames)
    };
    let (first, second) = (alone(tracks[0]), alone(tracks[1]));
    let mut overlapping = 0;
    for (i, &sample) in both.samples.iter().enumerate() {
        assert_eq!(
            sample.to_bits(),
            (first[i] + second[i]).to_bits(),
            "sample {i}"
        );
        if first[i] != 0.0 && second[i] != 0.0 {
            overlapping += 1;
        }
    }
    assert!(overlapping > 48_000 * 2, "{overlapping}");
    // The drone, on the second track, is mono and panned right: both
    // channels carry it, the right louder.
    let peak = |channel: usize| {
        second
            .iter()
            .skip(channel)
            .step_by(2)
            .fold(0.0f32, |peak, s| peak.max(s.abs()))
    };
    assert!(
        peak(0) > 0.0 && peak(1) > peak(0),
        "{} {}",
        peak(0),
        peak(1)
    );
}
