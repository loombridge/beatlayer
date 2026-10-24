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
            // R24: a crossfade has the clip play on past its edges, over the
            // frames its fades reach.
            let (before, after) = (frames(audio.fade_in_before), frames(audio.fade_out_after));
            let (in_power, out_power) = (
                4f64.powf(-audio.fade_in_curve),
                4f64.powf(-audio.fade_out_curve),
            );
            for song in (start - before as i64)..(end + after as i64) {
                let elapsed = song - start;
                // R3, R4: the frame of the file.
                let Ok(linear) = u64::try_from(frames(audio.start) as i64 + elapsed) else {
                    continue;
                };
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
                // R5, R6, R23: the gain, and fades from the edges, straight
                // lines unless the curve says otherwise.
                let mut fade = 1.0f64;
                let (rising, falling) = (fade_in + before, fade_out + after);
                let into = elapsed + before as i64;
                if rising > 0 && into < rising as i64 {
                    let x = into.max(0) as f64 / rising as f64;
                    fade = if in_power == 1.0 { x } else { x.powf(in_power) };
                }
                let left = end + after as i64 - song;
                if falling > 0 && left < falling as i64 {
                    let x = left.max(0) as f64 / falling as f64;
                    fade = fade.min(if out_power == 1.0 {
                        x
                    } else {
                        x.powf(out_power)
                    });
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

// Spec: comping R17, R18. A comp copied from two takes renders as the
// stretches of the takes it was cut from: the main lane's parts play what the
// model says, and the take lanes, auditioned or not here, add nothing.
#[test]
fn a_comp_copied_from_two_takes_renders_their_stretches() {
    let (scene, render) = matches_the_model("comp-from-two-takes");
    let chirp = chirp(&scene);
    // The first half of the bar is the first take from its start, the second
    // half the other take from where the cut left it: a beat is 24,000 frames.
    assert_eq!(left(&render, 0), chirp[0]);
    assert_eq!(left(&render, 47_999), chirp[47_999]);
    // The second take started 0.75 seconds (36,000 frames) later in the file.
    assert_eq!(left(&render, 48_000), chirp[36_000 + 48_000]);
    assert_ne!(chirp[48_000], chirp[84_000], "mid-waveform");
    let takes = scene
        .document()
        .tracks()
        .map(|track| track.take_lanes().count())
        .sum::<usize>();
    assert_eq!(takes, 2);
}

/// The crossfades of the main lane of the scene's one track with take lanes,
/// as `(boundary frame, seconds before, seconds after)`.
fn comp_crossfades(scene: &Scene) -> Vec<(usize, f64, f64)> {
    let document = scene.document();
    let tempo = &document.settings().tempo;
    let track = document
        .tracks()
        .find(|track| track.take_lanes().count() > 0)
        .expect("a track with takes");
    let mut found: Vec<_> = bl_model::crossfades(track.clips())
        .iter()
        .map(|c| {
            (
                tempo.ticks_to_frames(c.boundary, SAMPLE_RATE).get() as usize,
                c.before,
                c.after,
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

// Spec: comping R22, audio-clips R24, R26. A comp of three parts with Create
// Fades on Clip Edges on is crossfaded at both of its boundaries, 2 ms each
// side: before the span only the earlier part plays, after it only the later
// one, and across it both do, to the bit as the audio clips spec gives.
#[test]
fn a_three_part_comp_is_crossfaded_at_both_edges() {
    let (scene, render) = matches_the_model("comp-three-parts");
    let chirp = chirp(&scene);
    let half = bl_model::EDGE_FADE_SECONDS / 2.0;
    assert_eq!(
        comp_crossfades(&scene),
        [(48_000, half, half), (96_000, half, half)]
    );
    // The takes' start markers are a second apart, so the part from take n
    // plays the chirp n seconds (48,000 n frames) further on.
    for (boundary, earlier, later) in [(48_000usize, 0usize, 48_000usize), (96_000, 48_000, 96_000)]
    {
        // 2 ms is 96 frames.
        assert_eq!(left(&render, boundary - 97), chirp[boundary - 97 + earlier]);
        assert_eq!(left(&render, boundary + 96), chirp[boundary + 96 + later]);
        assert_ne!(left(&render, boundary), chirp[boundary + earlier]);
        assert_ne!(left(&render, boundary), chirp[boundary + later]);
    }
}

// Spec: comping R21, R22. After a comp boundary is moved, the crossfade is
// where the boundary is: the earlier part plays on from its take to the new
// place, the later part starts there, and the span is as long as it was.
#[test]
fn a_moved_comp_edge_takes_its_crossfade_with_it() {
    let (scene, render) = matches_the_model("comp-edge-moved");
    let chirp = chirp(&scene);
    let half = bl_model::EDGE_FADE_SECONDS / 2.0;
    // A beat later: 72,000 frames; the second boundary stays at 96,000.
    assert_eq!(
        comp_crossfades(&scene),
        [(72_000, half, half), (96_000, half, half)]
    );
    // Where the first boundary was, the first part still plays its take.
    assert_eq!(left(&render, 48_001), chirp[48_001]);
    assert_eq!(left(&render, 72_000 - 97), chirp[72_000 - 97]);
    // After the new span the second part plays, its take a second further on.
    assert_eq!(left(&render, 72_000 + 96), chirp[72_000 + 96 + 48_000]);
    assert_ne!(left(&render, 72_000), chirp[72_000]);
    assert_ne!(left(&render, 72_000), chirp[72_000 + 48_000]);
}

// Spec: linked-track editing R12. A comp made on one of two linked tracks
// comps both: each track's main lane plays the stretches of its own takes,
// the right track's a tenth of a second (4,800 frames) further into the file.
#[test]
fn a_comp_made_on_one_linked_track_comps_both() {
    let (scene, render) = matches_the_model("linked-comp");
    let chirp = chirp(&scene);
    // Both tracks play at once, so a frame is the two chirps' sum: the left
    // track at the frame of the first take, and then of the second (0.75
    // seconds, 36,000 frames, further in); the right track a tenth of a
    // second further into each.
    for (frame, take) in [(0usize, 0usize), (47_999, 0), (48_000, 36_000)] {
        let sum = chirp[frame + take] + chirp[frame + take + 4_800];
        assert_eq!(
            left(&render, frame).to_bits(),
            sum.to_bits(),
            "frame {frame}"
        );
    }
    let lanes = scene
        .document()
        .tracks()
        .map(|track| (track.take_lanes().count(), track.clips().count()))
        .filter(|held| held.0 > 0)
        .collect::<Vec<_>>();
    assert_eq!(lanes, [(2, 2), (2, 2)]);
}

// Spec: audio-clips R23, R24, R31
#[test]
fn a_crossfade_plays_both_clips_across_its_span_by_the_curve_law() {
    let (scene, render) = matches_the_model("clip-crossfade");
    let tempo = &scene.document().settings().tempo;
    let beat = bl_core::time::Ticks::QUARTER;
    let boundary = tempo.ticks_to_frames(beat * 2, SAMPLE_RATE).get() as usize;
    let sine = |hz: &str| {
        let file = scene
            .files
            .iter()
            .find(|f| f.name == format!("sine-{hz}.wav"))
            .unwrap();
        scene.media.get(file.hash).unwrap().channel(0).unwrap()
    };
    let (low, high) = (sine("440"), sine("660"));
    // 80 ms (3,840 frames) before the boundary and 120 ms (5,760) after.
    let (from, to) = (boundary - 3_840, boundary + 5_760);
    // Before the span only the first sine plays, and from its end only the
    // second; the second's file is read from a second in.
    assert_eq!(left(&render, from - 1), low[from - 1]);
    assert_eq!(left(&render, from), low[from]);
    assert_eq!(left(&render, to), high[48_000 + (to - boundary)]);
    // Inside it both do: the first clip plays on past its end and the second
    // from before its start, which neither's own edges would give.
    for frame in [from + 1, boundary - 1, boundary, boundary + 1, to - 1] {
        let sum = |gain_low: f64, gain_high: f64| {
            (f64::from(low[frame]) * gain_low) as f32
                + (f64::from(high[48_000 + frame - boundary]) * gain_high) as f32
        };
        let x = (frame - from) as f64 / (to - from) as f64;
        let p = 4f64.powf(-0.25);
        assert_eq!(
            left(&render, frame).to_bits(),
            sum((1.0 - x).powf(p), x.powf(p)).to_bits(),
            "frame {frame}"
        );
    }
    assert_ne!(
        left(&render, boundary - 1),
        low[boundary - 1],
        "the second clip is heard"
    );
    assert_ne!(
        left(&render, boundary + 1),
        high[48_000 + 1],
        "the first clip is still heard"
    );
}

// Spec: audio-clips R24, R26. The two parts of a split clip, crossfaded 2 ms
// either side as Create Fades on Clip Edges leaves them, play the whole
// clip to under -120 dBFS, across a tempo ramp.
#[test]
fn the_crossfaded_parts_of_a_split_clip_null_against_the_whole() {
    let fixture = fixture("clip-split-crossfade").expect("the fixture exists");
    let scene = fixture.scene();
    let parts = fixture.render();
    assert!(parts.problems().is_empty(), "{:?}", parts.problems());
    let mut editor = fixture.scene().editor;
    let track = editor.document().top_level().next().unwrap();
    let mut clips: Vec<_> = editor
        .document()
        .track(track)
        .unwrap()
        .clips()
        .cloned()
        .collect();
    clips.sort_by_key(|c| c.start);
    let [left_part, right_part] = &clips[..] else {
        panic!("the clip is in two parts");
    };
    // The whole: the left part with its length run on to the right part's end
    // and no fades.
    let ClipData::Audio(audio) = &left_part.data else {
        panic!("audio");
    };
    editor
        .apply(Command::DeleteClip {
            clip: right_part.id,
        })
        .unwrap();
    editor
        .apply(Command::SetClipLength {
            clip: left_part.id,
            length: right_part.end() - left_part.start,
        })
        .unwrap();
    editor
        .apply(Command::SetClipData {
            clip: left_part.id,
            data: ClipData::Audio(bl_model::AudioData {
                fade_out: 0.0,
                fade_out_after: 0.0,
                ..audio.clone()
            }),
        })
        .unwrap();
    let whole = render_song(&editor, &scene.media, scene.frames);
    assert_eq!(whole.len(), parts.samples.len());
    let worst = whole
        .iter()
        .zip(&parts.samples)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        whole.iter().any(|&s| s != 0.0) && worst < 1e-6,
        "off by {worst}"
    );
}
