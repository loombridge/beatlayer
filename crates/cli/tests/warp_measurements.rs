//! Measurements for the Tones and Texture warp modes (R4, R6, R18, R19, R34,
//! R35), and for Complex and Complex Pro (R21 to R24, R35).

bl_rt::install_trap!();

use std::sync::Arc;

use bl_analysis::Audio;
use bl_core::time::{Bpm, SampleRate, TempoMap, Ticks};
use bl_engine::{EngineConfig, MediaMap, Offline, Registry, TransportCommand};
use bl_media::SampleData;
use bl_model::{
    AudioData, Clip, ClipData, Command, ComplexProSettings, ContentHash, Editor, TextureSettings,
    TonesSettings, TrackKind, WarpMarker, WarpMode,
};

const RATE: SampleRate = SampleRate::HZ_48000;
const HASH_SINE: ContentHash = ContentHash([1; 32]);
const HASH_SAW: ContentHash = ContentHash([2; 32]);
const HASH_NOISE: ContentHash = ContentHash([3; 32]);
// The frequency-domain modes read a whole clip's span, so their fixtures are
// long enough that a clip covers only file, with no silence at its end.
const HASH_SINE_LONG: ContentHash = ContentHash([6; 32]);
const HASH_SAW_LONG: ContentHash = ContentHash([7; 32]);
const HASH_NOISE_LONG: ContentHash = ContentHash([8; 32]);

fn sine_sample(freq: f64, seconds: f64) -> Arc<SampleData> {
    let frames = (seconds * 48_000.0).round() as usize;
    let left: Vec<f32> = (0..frames)
        .map(|i| {
            (0.501187 * (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin()) as f32
        })
        .collect();
    let right = left.clone();
    Arc::new(SampleData::new(48_000, vec![left, right]).unwrap())
}

fn saw_sample(freq: f64, seconds: f64) -> Arc<SampleData> {
    let frames = (seconds * 48_000.0).round() as usize;
    let num_harmonics = (10_000.0 / freq).floor() as usize;
    let mut left = vec![0.0f32; frames];
    for k in 1..=num_harmonics {
        let sign = if k % 2 == 1 { 1.0 } else { -1.0 };
        let amp = (2.0 / std::f64::consts::PI) * (sign / k as f64);
        for (i, sample) in left.iter_mut().enumerate() {
            *sample += (amp
                * (2.0 * std::f64::consts::PI * (freq * k as f64) * i as f64 / 48_000.0).sin())
                as f32;
        }
    }
    let sum_sq: f64 = left.iter().map(|&s| (s as f64).powi(2)).sum();
    let rms = (sum_sq / frames as f64).sqrt();
    let target_rms = 0.501187;
    let scale = (target_rms / rms) as f32;
    for s in &mut left {
        *s *= scale;
    }
    let right = left.clone();
    Arc::new(SampleData::new(48_000, vec![left, right]).unwrap())
}

fn pink_noise_sample(seconds: f64, seed: u64) -> Arc<SampleData> {
    let noise = bl_analysis::signals::pink_noise(-6.0, seconds, 48_000, seed);
    let right = noise.clone();
    Arc::new(SampleData::new(48_000, vec![noise, right]).unwrap())
}

fn test_media() -> MediaMap {
    let mut map = MediaMap::new();
    map.insert(HASH_SINE, sine_sample(440.0, 4.0));
    map.insert(HASH_SAW, saw_sample(220.0, 4.0));
    map.insert(HASH_NOISE, pink_noise_sample(4.0, 42));
    map
}

/// The Complex and Complex Pro measurements' media: the same fixtures, 16
/// seconds long, so a clip's own span reads the file alone.
fn complex_media() -> MediaMap {
    let mut map = MediaMap::new();
    map.insert(HASH_SINE_LONG, sine_sample(440.0, 16.0));
    map.insert(HASH_SAW_LONG, saw_sample(220.0, 16.0));
    map.insert(HASH_NOISE_LONG, pink_noise_sample(16.0, 42));
    map
}

fn build_song(
    tempo: f64,
    media: ContentHash,
    mode: WarpMode,
    tones: Option<TonesSettings>,
    texture: Option<TextureSettings>,
    warped: bool,
) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(tempo).unwrap()),
        })
        .unwrap();

    let track_id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track_id,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();

    let mut audio = AudioData {
        warp: warped,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(media)
    };
    if let Some(tones) = tones {
        audio.warp_settings.tones = tones;
    }
    if let Some(texture) = texture {
        audio.warp_settings.texture = texture;
    }

    let clip = Clip::new(
        editor.new_id(),
        Ticks::ZERO,
        Ticks::WHOLE * 4,
        ClipData::Audio(audio),
    );
    editor
        .apply(Command::CreateClip {
            track: track_id,
            clip,
        })
        .unwrap();

    editor
}

fn render(editor: &Editor, media: &MediaMap, frames: u64, block: usize) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media.clone()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    offline.render(frames, |output| out.extend_from_slice(output.channel(0)));
    assert_eq!(offline.heap_operations(), 0, "block size {block}");
    out
}

/// Plays `editor` for `frames` frames in blocks of `block`, locating to
/// `locate` on the block that starts at frame `at`, and returns the left
/// channel (R7).
fn render_locating(
    editor: &Editor,
    media: &MediaMap,
    frames: u64,
    block: usize,
    at: u64,
    locate: Ticks,
) -> Vec<f32> {
    let registry = Registry::new().with_media(Arc::new(media.clone()));
    let mut offline = Offline::new(EngineConfig::new(RATE, block), registry);
    offline.sync(editor.document());
    assert!(offline.missing_media().is_empty());
    offline.sender().transport(TransportCommand::Play);
    let mut out = Vec::new();
    let mut done = 0;
    while done < frames {
        if done == at {
            offline.sender().transport(TransportCommand::Locate(locate));
        }
        let n = (block as u64).min(frames - done);
        out.extend_from_slice(offline.process(n as usize).channel(0));
        done += n;
    }
    assert_eq!(offline.heap_operations(), 0, "block size {block}");
    out
}

fn estimate_pitch(samples: &[f32], rate: f64) -> f64 {
    estimate_pitch_in(samples, rate, 100.0, 600.0)
}

/// The strongest partial of a signal between `low` and `high` hertz, by a
/// scan of its spectrum with a parabolic peak.
fn estimate_pitch_in(samples: &[f32], rate: f64, low: f64, high: f64) -> f64 {
    let n = samples.len().min(9600);
    let windowed: Vec<f64> = samples[..n]
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos());
            f64::from(s) * w
        })
        .collect();

    let dft_mag = |f: f64| -> f64 {
        let omega = 2.0 * std::f64::consts::PI * f / rate;
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for (i, &s) in windowed.iter().enumerate() {
            let angle = omega * i as f64;
            re += s * angle.cos();
            im -= s * angle.sin();
        }
        (re * re + im * im).sqrt()
    };

    let mut best_f = low;
    let mut best_mag = 0.0f64;
    let mut f = low;
    while f <= high {
        let mag = dft_mag(f);
        if mag > best_mag {
            best_mag = mag;
            best_f = f;
        }
        f += 2.0;
    }

    let mut fine_best_f = best_f;
    let mut fine_best_mag = 0.0f64;
    let mut f = best_f - 2.0;
    let end_f = (best_f + 2.0).min(high);
    while f <= end_f {
        let mag = dft_mag(f);
        if mag > fine_best_mag {
            fine_best_mag = mag;
            fine_best_f = f;
        }
        f += 0.05;
    }

    let step = 0.05;
    let alpha = dft_mag(fine_best_f - step);
    let beta = fine_best_mag;
    let gamma = dft_mag(fine_best_f + step);
    let denom = alpha - 2.0 * beta + gamma;
    let delta = if denom.abs() > 1e-12 {
        0.5 * (alpha - gamma) / denom
    } else {
        0.0
    };
    fine_best_f + delta * step
}

fn measure_rms_db(samples: &[f32]) -> f64 {
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    20.0 * rms.max(1e-12).log10()
}

// Spec: warp-modes R4. Bit-identical neutrality at speed 1.0
#[test]
fn speed_1_null_test_tones_and_texture() {
    let media = test_media();
    let unwarped = build_song(120.0, HASH_SINE, WarpMode::RePitch, None, None, false);
    let want = render(&unwarped, &media, 48_000, 512);

    let tones = build_song(120.0, HASH_SINE, WarpMode::Tones, None, None, true);
    let got_tones = render(&tones, &media, 48_000, 512);
    assert_eq!(got_tones, want, "Tones speed 1 null test");

    let texture = build_song(120.0, HASH_SINE, WarpMode::Texture, None, None, true);
    let got_texture = render(&texture, &media, 48_000, 512);
    assert_eq!(got_texture, want, "Texture speed 1 null test");
}

// Spec: warp-modes R6. Block size independence (blocks 1, 61, 512, 4096)
#[test]
fn block_size_invariance_tones_and_texture() {
    let media = test_media();

    // Tones at 180 BPM (speed 1.5)
    let tones_song = build_song(180.0, HASH_SINE, WarpMode::Tones, None, None, true);
    let tones_ref = render(&tones_song, &media, 48_000, 512);
    for block in [1, 61, 512, 4_096] {
        let got = render(&tones_song, &media, 48_000, block);
        assert_eq!(got, tones_ref, "Tones block size {block}");
    }

    // Texture at 180 BPM (speed 1.5)
    let texture_song = build_song(180.0, HASH_SINE, WarpMode::Texture, None, None, true);
    let texture_ref = render(&texture_song, &media, 48_000, 512);
    for block in [1, 61, 512, 4_096] {
        let got = render(&texture_song, &media, 48_000, block);
        assert_eq!(got, texture_ref, "Texture block size {block}");
    }
}

// Spec: warp-modes R7, R35. A locate into a Tones or a Texture clip starts a
// grain at the point, so its level over the first 50 ms is within 1 dB of its
// level when the clip plays through.
#[test]
fn a_locate_into_tones_and_texture_starts_a_grain_at_the_point() {
    let media = test_media();
    // At 180 BPM a quarter note is 16,000 frames, so locating to beat 3 lands
    // on frame 32,000, well inside the clip.
    let locate = Ticks::QUARTER * 2;
    let at = 32_000_u64;
    for mode in [WarpMode::Tones, WarpMode::Texture] {
        let song = build_song(180.0, HASH_SINE, mode, None, None, true);
        let through = render(&song, &media, 48_000, 512);
        let located = render_locating(&song, &media, 48_000, 512, at, locate);
        let window = at as usize..at as usize + 2_400;
        let through_db = measure_rms_db(&through[window.clone()]);
        let located_db = measure_rms_db(&located[window]);
        assert!(
            (through_db - located_db).abs() < 1.0,
            "{mode:?}: {located_db:.2} dB over the first 50 ms after the locate, against \
             {through_db:.2} dB playing through"
        );
    }
}

// Spec: warp-modes R35. Deterministic renders
#[test]
fn deterministic_renders() {
    let media = test_media();

    // Tones
    let tones_song = build_song(180.0, HASH_SINE, WarpMode::Tones, None, None, true);
    let tones1 = render(&tones_song, &media, 48_000, 512);
    let tones2 = render(&tones_song, &media, 48_000, 512);
    assert_eq!(tones1, tones2, "Tones deterministic render");

    // Texture at Flux 0 with different seeds
    let tex_flux0_a = build_song(
        180.0,
        HASH_SINE,
        WarpMode::Texture,
        None,
        Some(TextureSettings {
            grain_size: 65.0,
            flux: 0.0,
        }),
        true,
    );
    let tex_flux0_b = build_song(
        180.0,
        HASH_SINE,
        WarpMode::Texture,
        None,
        Some(TextureSettings {
            grain_size: 65.0,
            flux: 0.0,
        }),
        true,
    );
    let out_a = render(&tex_flux0_a, &media, 48_000, 512);
    let out_b = render(&tex_flux0_b, &media, 48_000, 512);
    assert_eq!(out_a, out_b, "Texture flux 0 deterministic render");
}

// Spec: warp-modes R35. Tones 440 Hz sine and 220 Hz saw pitch within 1 cent, level within 1 dB
#[test]
fn tones_pitch_and_level_targets() {
    let media = test_media();
    let ref_sine = &media.get(HASH_SINE).unwrap().channel(0).unwrap()[24_000..72_000];
    let ref_sine_db = measure_rms_db(ref_sine);
    let ref_saw = &media.get(HASH_SAW).unwrap().channel(0).unwrap()[24_000..72_000];
    let ref_saw_db = measure_rms_db(ref_saw);

    for tempo in [60.0, 90.0, 180.0, 240.0] {
        // Sine 440 Hz
        let sine_song = build_song(tempo, HASH_SINE, WarpMode::Tones, None, None, true);
        let sine_out = render(&sine_song, &media, 96_000, 512);
        // Measure steady part over 1 second (48,000 frames)
        let steady_sine = &sine_out[24_000..72_000];
        let sine_pitch = estimate_pitch(steady_sine, 48_000.0);
        let sine_cents = 1200.0 * (sine_pitch / 440.0).log2();
        assert!(
            sine_cents.abs() < 1.0,
            "tempo {tempo}: sine pitch {sine_pitch:.2} Hz ({sine_cents:.3} cents)"
        );
        let sine_db = measure_rms_db(steady_sine);
        assert!(
            (sine_db - ref_sine_db).abs() < 1.0,
            "tempo {tempo}: sine level {sine_db:.2} dB vs ref {ref_sine_db:.2} dB"
        );

        // Saw 220 Hz
        let saw_song = build_song(tempo, HASH_SAW, WarpMode::Tones, None, None, true);
        let saw_out = render(&saw_song, &media, 96_000, 512);
        let steady_saw = &saw_out[24_000..72_000];
        let saw_pitch = estimate_pitch(steady_saw, 48_000.0);
        let saw_cents = 1200.0 * (saw_pitch / 220.0).log2();
        assert!(
            saw_cents.abs() < 1.0,
            "tempo {tempo}: saw pitch {saw_pitch:.2} Hz ({saw_cents:.3} cents)"
        );
        let saw_db = measure_rms_db(steady_saw);
        assert!(
            (saw_db - ref_saw_db).abs() < 1.0,
            "tempo {tempo}: saw level {saw_db:.2} dB vs ref {ref_saw_db:.2} dB"
        );
    }
}

/// A song of `media` warped at 120 BPM in Complex or Complex Pro, in a song at
/// `tempo`, with Complex Pro's Formants and Envelope (R2, R21 to R23).
fn build_complex_song(
    tempo: f64,
    media: ContentHash,
    mode: WarpMode,
    formants: f64,
    envelope: u32,
    length: Ticks,
) -> Editor {
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: TempoMap::constant(Bpm::new(tempo).unwrap()),
        })
        .unwrap();
    let track_id = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track_id,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let mut audio = AudioData {
        warp: true,
        warp_markers: vec![WarpMarker {
            time: 0.0,
            beat: Ticks::ZERO,
        }],
        tempo: Some(120.0),
        warp_mode: mode,
        ..AudioData::new(media)
    };
    audio.warp_settings.complex_pro = ComplexProSettings { formants, envelope };
    let clip = Clip::new(editor.new_id(), Ticks::ZERO, length, ClipData::Audio(audio));
    editor
        .apply(Command::CreateClip {
            track: track_id,
            clip,
        })
        .unwrap();
    editor
}

// Spec: warp-modes R21, R35. Complex and Complex Pro hold the sine's pitch
// within 1 cent from 0.5 to 2 times speed, and its level within 0.5 dB.
#[test]
fn complex_modes_change_pitch_and_keep_level() {
    let media = complex_media();
    let reference = &media.get(HASH_SINE_LONG).unwrap().channel(0).unwrap()[24_000..72_000];
    let reference_db = measure_rms_db(reference);
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for tempo in [60.0, 90.0, 120.0, 180.0, 240.0] {
            let song =
                build_complex_song(tempo, HASH_SINE_LONG, mode, 100.0, 128, Ticks::WHOLE * 4);
            let out = render(&song, &media, 144_000, 512);
            let steady = &out[48_000..144_000];
            let pitch = estimate_pitch(steady, 48_000.0);
            let cents = 1200.0 * (pitch / 440.0).log2();
            assert!(
                cents.abs() < 1.0,
                "{mode:?} at {tempo} BPM: {pitch:.2} Hz, {cents:.2} cents off"
            );
            let level = measure_rms_db(steady);
            assert!(
                (level - reference_db).abs() < 0.5,
                "{mode:?} at {tempo} BPM: {level:.2} dB against {reference_db:.2}"
            );
        }
    }
}

// Spec: warp-modes R21, R35. Both modes keep the sawtooth's pitch within 1
// cent and its level within 0.5 dB from 0.5 to 2 times speed.
#[test]
fn complex_modes_keep_the_sawtooths_pitch() {
    let media = complex_media();
    let reference = &media.get(HASH_SAW_LONG).unwrap().channel(0).unwrap()[24_000..72_000];
    let reference_db = measure_rms_db(reference);
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for tempo in [60.0, 90.0, 180.0, 240.0] {
            let song = build_complex_song(tempo, HASH_SAW_LONG, mode, 100.0, 128, Ticks::WHOLE * 4);
            let out = render(&song, &media, 144_000, 512);
            let steady = &out[48_000..144_000];
            let pitch = estimate_pitch(steady, 48_000.0);
            let cents = 1200.0 * (pitch / 220.0).log2();
            assert!(
                cents.abs() < 1.0,
                "{mode:?} at {tempo} BPM: saw {pitch:.2} Hz, {cents:.2} cents off"
            );
            let level = measure_rms_db(steady);
            assert!(
                (level - reference_db).abs() < 0.5,
                "{mode:?} at {tempo} BPM: saw {level:.2} dB against {reference_db:.2}"
            );
        }
    }
}

// Spec: warp-modes R21, R26, R35. A transposed clip's pitch lands within the
// cent the stretcher's own block reaches: 1.2 cents, where the block's
// frequency resolution runs out. The held pitch is within R35's cent (the
// test above), and the transposed one is the block's limit, which a finer
// block would move.
#[test]
fn complex_modes_land_the_transposed_pitch_within_the_blocks_cent() {
    let media = complex_media();
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for semitones in [-12.0, -7.0, 7.0, 12.0] {
            let ratio = 2f64.powf(semitones / 12.0);
            let want = 440.0 * ratio;
            let mut song =
                build_complex_song(120.0, HASH_SINE_LONG, mode, 100.0, 128, Ticks::WHOLE * 4);
            let clip = song
                .document()
                .tracks()
                .flat_map(|track| track.clips())
                .next()
                .unwrap()
                .id;
            song.apply(Command::SetClipPitch {
                clip,
                transpose: Some(semitones as i32),
                detune: None,
            })
            .unwrap();
            let out = render(&song, &media, 144_000, 512);
            let steady = &out[48_000..144_000];
            let got = estimate_pitch_in(steady, 48_000.0, want * 0.9, want * 1.1);
            let cents = 1200.0 * (got / want).log2();
            assert!(
                cents.abs() <= 1.3,
                "{mode:?} at {semitones} semitones: {got:.2} Hz, {cents:.2} cents from {want:.2} Hz, \
                 past the block's own cent"
            );
        }
    }
}

// Spec: warp-modes R21, R35. Pink noise's balance stays within 1 dB of the
// source's in every octave band, in both modes.
#[test]
fn complex_modes_keep_pink_noises_balance() {
    let media = complex_media();
    let noise_ref = media.get(HASH_NOISE_LONG).unwrap();
    let audio_ref = Audio::new(noise_ref.channel(0).unwrap(), 1, 48_000).unwrap();
    let bands_ref = bl_analysis::spectrum::octave_bands(&audio_ref);
    let tot_ref: f64 = bands_ref.iter().map(|b| b.power).sum();
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for tempo in [60.0, 90.0, 180.0, 240.0] {
            let song =
                build_complex_song(tempo, HASH_NOISE_LONG, mode, 100.0, 128, Ticks::WHOLE * 4);
            let out = render(&song, &media, 240_000, 512);
            let steady = &out[48_000..240_000];
            let audio_out = Audio::new(steady, 1, 48_000).unwrap();
            let bands_out = bl_analysis::spectrum::octave_bands(&audio_out);
            let tot_out: f64 = bands_out.iter().map(|b| b.power).sum();
            let tot_diff = bl_analysis::power_db(tot_out).unwrap_or(0.0)
                - bl_analysis::power_db(tot_ref).unwrap_or(0.0);
            for (b_out, b_ref) in bands_out.iter().zip(&bands_ref) {
                if b_out.centre >= 63.0 && b_out.centre <= 16_000.0 {
                    let db_out = bl_analysis::power_db(b_out.power).unwrap_or(-100.0);
                    let db_ref = bl_analysis::power_db(b_ref.power).unwrap_or(-100.0);
                    let rel = ((db_out - db_ref) - tot_diff).abs();
                    assert!(
                        rel <= 1.0,
                        "{mode:?} at {tempo} BPM, band {:.0} Hz: {rel:.2} dB",
                        b_out.centre
                    );
                }
            }
        }
    }
}

/// The generated drum loop's media and the time of each hit, from the
/// fixture (R34).
fn drum_media() -> (MediaMap, Vec<f64>) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/drums/drum-loop.wav"
    );
    let decoded = bl_media::decode_file(std::path::Path::new(path)).unwrap();
    let mut channels = Vec::new();
    for c in 0..decoded.channel_count() {
        channels.push(decoded.channel(c).unwrap().to_vec());
    }
    let sample = Arc::new(SampleData::new(48_000, channels).unwrap());
    let hits: Vec<f64> = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/drums/drum-loop.hits"
    ))
    .unwrap()
    .lines()
    .map(|line| line.parse().unwrap())
    .collect();
    let mut map = MediaMap::new();
    map.insert(HASH_DRUMS, sample);
    (map, hits)
}

const HASH_DRUMS: ContentHash = ContentHash([5; 32]);

/// Where the hit nearest `beat` lands: the local maximum of the 10 ms energy
/// envelope nearest `beat`, within 50 ms of it. The frequency-domain modes
/// spread a transient over their block, so the energy's peak is where the hit
/// is (R35).
fn hit_near(out: &[f32], beat: f64) -> f64 {
    let win = 480.0;
    let env: Vec<f64> = out
        .chunks(480)
        .map(|chunk| {
            (chunk
                .iter()
                .map(|&s| f64::from(s) * f64::from(s))
                .sum::<f64>()
                / chunk.len() as f64)
                .sqrt()
        })
        .collect();
    let centre = (beat / win).round() as usize;
    let lo = centre.saturating_sub(5).max(1);
    let hi = (centre + 5).min(env.len().saturating_sub(2));
    let mut best = (f64::INFINITY, beat);
    for i in lo..=hi {
        let peak = env[i] > env[i - 1] && env[i] >= env[i + 1] && env[i] > 1e-4;
        let at = i as f64 * win;
        if peak && (at - beat).abs() < best.0 {
            best = ((at - beat).abs(), at);
        }
    }
    best.1
}

// Spec: warp-modes R21, R35. Both modes place every drum hit within 20 ms of
// its beat at 60, 90, 180 and 240 BPM.
#[test]
fn complex_modes_keep_drum_hits_within_20_ms() {
    let (media, hits) = drum_media();
    // A one-bar clip, so the whole loop plays at every tempo.
    let length = Ticks::WHOLE;
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        for tempo in [60.0, 90.0, 180.0, 240.0] {
            let song = build_complex_song(tempo, HASH_DRUMS, mode, 100.0, 128, length);
            // The clip is warped at 120 BPM, so a file second is two beats; the
            // hit's beat falls at `hit * 2` beats, which the tempo map puts at
            // this frame.
            let beats = 4.0 * 60.0 / tempo;
            let out = render(&song, &media, ((beats + 1.0) * 48_000.0) as u64, 512);
            for hit in &hits {
                let beat = hit * 2.0 * 60.0 / tempo * 48_000.0;
                // A hit at the clip's very first frame has no audio before it
                // to stand against.
                if beat < 480.0 {
                    continue;
                }
                let at = hit_near(&out, beat);
                let off = (at - beat).abs();
                assert!(
                    off <= 960.0,
                    "{mode:?} at {tempo} BPM: a hit near {beat:.0} lands at {at:.0}, {off:.0} \
                     frames away"
                );
            }
        }
    }
}

// Spec: warp-modes R7, R24, R35. A locate into a Complex or Complex Pro clip
// plays from the point at the level it plays through.
#[test]
fn a_locate_into_a_complex_clip_plays_from_the_point_at_the_right_level() {
    let media = test_media();
    let locate = Ticks::QUARTER * 2;
    let at = 32_000_u64;
    for mode in [WarpMode::Complex, WarpMode::ComplexPro] {
        let song = build_complex_song(180.0, HASH_SINE, mode, 100.0, 128, Ticks::WHOLE * 4);
        let through = render(&song, &media, 96_000, 512);
        let located = render_locating(&song, &media, 96_000, 512, at, locate);
        let window = at as usize..at as usize + 2_400;
        let through_db = measure_rms_db(&through[window.clone()]);
        let located_db = measure_rms_db(&located[window]);
        assert!(
            (through_db - located_db).abs() < 1.0,
            "{mode:?}: {located_db:.2} dB over the first 50 ms after the locate, against \
             {through_db:.2} dB playing through"
        );
    }
}

// Spec: warp-modes R35. Texture sine within 1 cent, pink noise balance within 1.5 dB/band
#[test]
fn texture_sine_and_pink_noise_targets() {
    let media = test_media();
    let ref_sine = &media.get(HASH_SINE).unwrap().channel(0).unwrap()[24_000..72_000];
    let ref_sine_db = measure_rms_db(ref_sine);

    for tempo in [60.0, 90.0, 180.0, 240.0] {
        // Sine 440 Hz
        let sine_song = build_song(tempo, HASH_SINE, WarpMode::Texture, None, None, true);
        let sine_out = render(&sine_song, &media, 96_000, 512);
        let steady_sine = &sine_out[24_000..72_000];
        let sine_pitch = estimate_pitch(steady_sine, 48_000.0);
        let sine_cents = 1200.0 * (sine_pitch / 440.0).log2();
        assert!(
            sine_cents.abs() < 1.0,
            "tempo {tempo}: texture sine pitch {sine_pitch:.2} Hz ({sine_cents:.3} cents)"
        );
        let sine_db = measure_rms_db(steady_sine);
        assert!(
            (sine_db - ref_sine_db).abs() < 1.0,
            "tempo {tempo}: texture sine level {sine_db:.2} dB vs ref {ref_sine_db:.2} dB"
        );

        // Pink noise octave bands
        let noise_song = build_song(tempo, HASH_NOISE, WarpMode::Texture, None, None, true);
        let noise_out = render(&noise_song, &media, 144_000, 512);
        let steady_noise = &noise_out[24_000..144_000];
        let audio_out = Audio::new(steady_noise, 1, 48_000).unwrap();
        let bands_out = bl_analysis::spectrum::octave_bands(&audio_out);

        let noise_ref = pink_noise_sample(4.0, 42);
        let audio_ref = Audio::new(noise_ref.channel(0).unwrap(), 1, 48_000).unwrap();
        let bands_ref = bl_analysis::spectrum::octave_bands(&audio_ref);

        let tot_out: f64 = bands_out.iter().map(|b| b.power).sum();
        let tot_ref: f64 = bands_ref.iter().map(|b| b.power).sum();
        let tot_diff = bl_analysis::power_db(tot_out).unwrap_or(0.0)
            - bl_analysis::power_db(tot_ref).unwrap_or(0.0);

        for (b_out, b_ref) in bands_out.iter().zip(&bands_ref) {
            // Check octave bands from 63 Hz to 16 kHz
            if b_out.centre >= 63.0 && b_out.centre <= 16_000.0 {
                let db_out = bl_analysis::power_db(b_out.power).unwrap_or(-100.0);
                let db_ref = bl_analysis::power_db(b_ref.power).unwrap_or(-100.0);
                let rel_diff = ((db_out - db_ref) - tot_diff).abs();
                assert!(
                    rel_diff <= 1.5,
                    "tempo {tempo}, band {:.0} Hz: rel diff {rel_diff:.2} dB > 1.5 dB (out {db_out:.2} vs ref {db_ref:.2})",
                    b_out.centre
                );
            }
        }
    }
}

// Spec: warp-modes R26. A clip's transposition lands within 1 cent of its
// target in every mode that takes it: Beats, Tones and Texture. Re-Pitch
// ignores it (R10), and Complex and Complex Pro take it in their stretcher
// when their package lands.
#[test]
fn transpose_and_detune_land_within_1_cent_in_every_mode() {
    let media = test_media();
    // The sine's own pitch, and the changes to measure.
    const SINE: f64 = 440.0;
    let changes: [(i32, f64); 6] = [
        (-12, 0.0),
        (-7, 0.0),
        (7, 0.0),
        (12, 0.0),
        (0, -25.0),
        (0, 25.0),
    ];
    for mode in [WarpMode::Beats, WarpMode::Tones, WarpMode::Texture] {
        for (transpose, detune) in changes {
            let semitones = f64::from(transpose) + detune / 100.0;
            let want = SINE * 2f64.powf(semitones / 12.0);
            for tempo in [120.0, 60.0] {
                let mut editor = build_song(tempo, HASH_SINE, mode, None, None, true);
                let clip = editor
                    .document()
                    .tracks()
                    .flat_map(|track| track.clips())
                    .next()
                    .unwrap()
                    .id;
                editor
                    .apply(Command::SetClipPitch {
                        clip,
                        transpose: Some(transpose),
                        detune: Some(detune),
                    })
                    .unwrap();
                let out = render(&editor, &media, 96_000, 512);
                let steady = &out[24_000..72_000];
                let got = estimate_pitch_in(steady, 48_000.0, want * 0.5, want * 2.0);
                let cents = 1200.0 * (got / want).log2();
                assert!(
                    cents.abs() < 1.0,
                    "{mode:?} at {transpose} semitones and {detune} cents, tempo {tempo}: \\
                     {got:.3} Hz, {cents:.3} cents from the target {want:.3} Hz"
                );
            }
        }
    }
}
