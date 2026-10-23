//! Decodes every generated fixture and checks it against its source, within
//! the tolerance of its format.
//!
//! The fixtures and the encoders that made them are listed in
//! `tests/fixtures/media/manifest.json`; `tests/fixtures/gen/gen_media.py`
//! makes them. The tolerances are in the audio import spec.
//!
//! Run with `--nocapture` to print the measurements.

use crate::common;
use std::collections::BTreeSet;
use std::io::Cursor;

use bl_media::{Codec, Container, Decoder, MediaError, SampleData, decode_file};
use common::{Fixture, band_snr_db, manifest, peak_dbfs, read_wav_independently, snr_db};

/// Lossy tolerances are measured below this frequency: every encoder here
/// keeps it at the bit rates used, and most low-pass above it.
const LOSSY_BAND_HZ: f64 = 15_000.0;

/// The least signal-to-error ratio below [`LOSSY_BAND_HZ`] a lossy fixture
/// decodes with, in dB, by codec and source signal: a few dB under what the
/// encoders here achieve, so a decoding error of any size fails.
fn lossy_min_snr_db(codec: &str, signal: &str) -> f64 {
    match (codec, signal) {
        ("mp3", "sine") => 65.0,
        ("mp3", "sweep") => 70.0,
        ("mp3", "noise") => 14.0,
        ("mp3", "clip") => 32.0,
        ("vorbis", "sine") => 36.0,
        ("vorbis", "sweep") => 32.0,
        ("vorbis", "noise") => 16.0,
        ("vorbis", "clip") => 35.0,
        ("aac", "sine") => 35.0,
        ("aac", "sweep") => 43.0,
        ("aac", "noise") => 12.0,
        ("aac", "clip") => 37.0,
        _ => panic!("no tolerance for {codec} {signal}"),
    }
}

/// The most a lossy decode of digital silence may peak at, in dBFS.
const SILENCE_MAX_DBFS: f64 = -90.0;

/// Apple's AAC encoder primes 2,112 frames. A raw ADTS stream can't record
/// that, so its decode starts with them.
const ADTS_PRIMING: usize = 2112;

fn decode(path: &std::path::Path) -> SampleData {
    decode_file(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn planes(data: &SampleData) -> Vec<&[f32]> {
    data.channels().collect()
}

#[test]
fn every_format_has_fixtures() {
    // Spec: audio-import R1
    let have: BTreeSet<(String, String)> = manifest()
        .into_iter()
        .map(|f| (f.container, f.codec))
        .collect();
    for (container, codec) in [
        ("wav", "pcm"),
        ("rf64", "pcm"),
        ("aiff", "pcm"),
        ("flac", "flac"),
        ("mp3", "mp3"),
        ("ogg", "vorbis"),
        ("mp4", "aac"),
        ("mp4", "alac"),
        ("adts", "aac"),
    ] {
        assert!(
            have.contains(&(container.to_owned(), codec.to_owned())),
            "no {codec} in {container} fixture"
        );
    }
    for fixture in manifest() {
        assert!(fixture.path().exists(), "{} is missing", fixture.file);
    }
}

#[test]
fn headers_report_each_file() {
    // Spec: audio-import R1, R2
    for fixture in manifest() {
        let decoder = Decoder::open(fixture.path()).unwrap();
        let info = decoder.info();
        let name = &fixture.file;
        assert_eq!(info.sample_rate, fixture.rate, "{name}");
        assert_eq!(info.channels, fixture.channels, "{name}");
        let (container, codec) = expected_kind(&fixture);
        assert_eq!(info.container, container, "{name}");
        assert_eq!(info.codec, codec, "{name}");
        assert_eq!(info.codec.is_lossless(), fixture.lossless, "{name}");
        if fixture.container != "adts" {
            assert_eq!(info.frames, Some(fixture.frames), "{name}");
        }
    }
}

fn expected_kind(fixture: &Fixture) -> (Container, Codec) {
    let container = match fixture.container.as_str() {
        "wav" => Container::Wav,
        "rf64" => Container::Rf64,
        "aiff" => Container::Aiff,
        "flac" => Container::Flac,
        "mp3" => Container::Mp3,
        "ogg" => Container::Ogg,
        "mp4" => Container::Mp4,
        "adts" => Container::Adts,
        other => panic!("unknown container {other}"),
    };
    let codec = match fixture.codec.as_str() {
        "pcm" => Codec::Pcm,
        "flac" => Codec::Flac,
        "alac" => Codec::Alac,
        "mp3" => Codec::Mp3,
        "vorbis" => Codec::Vorbis,
        "aac" => Codec::Aac,
        other => panic!("unknown codec {other}"),
    };
    (container, codec)
}

#[test]
fn source_wavs_decode_to_their_exact_samples() {
    // Spec: audio-import R3
    for fixture in manifest().into_iter().filter(Fixture::is_source) {
        let bytes = std::fs::read(fixture.path()).unwrap();
        let (rate, expected) = read_wav_independently(&bytes);
        let decoded = decode(&fixture.path());
        assert_eq!(decoded.sample_rate(), rate, "{}", fixture.file);
        assert_eq!(decoded.frames() as u64, fixture.frames, "{}", fixture.file);
        for (c, (got, want)) in decoded.channels().zip(&expected).enumerate() {
            assert!(
                got.iter()
                    .zip(want)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{} channel {c} differs from the bytes in the file",
                fixture.file
            );
        }
    }
}

#[test]
fn lossless_fixtures_decode_bit_exact() {
    // Spec: audio-import R3
    let mut checked = 0;
    for fixture in manifest()
        .into_iter()
        .filter(|f| f.lossless && !f.is_source())
    {
        let source = decode(&fixture.source_path());
        let decoded = decode(&fixture.path());
        assert_eq!(
            decoded.sample_rate(),
            source.sample_rate(),
            "{}",
            fixture.file
        );
        assert_eq!(decoded.frames(), source.frames(), "{}", fixture.file);
        assert_eq!(
            decoded.channel_count(),
            source.channel_count(),
            "{}",
            fixture.file
        );
        for (c, (got, want)) in decoded.channels().zip(source.channels()).enumerate() {
            let first_difference = got
                .iter()
                .zip(want)
                .position(|(a, b)| a.to_bits() != b.to_bits());
            assert_eq!(
                first_difference, None,
                "{} channel {c} isn't bit-exact",
                fixture.file
            );
        }
        checked += 1;
    }
    assert!(checked >= 15, "only {checked} lossless fixtures");
}

#[test]
fn lossy_fixtures_decode_within_tolerance() {
    // Spec: audio-import R4, R5
    let mut failures = Vec::new();
    println!(
        "{:<32} {:>8} {:>8} {:>10} {:>10} {:>10}",
        "fixture", "frames", "source", "full dB", "band dB", "limit dB"
    );
    for fixture in manifest().into_iter().filter(|f| !f.lossless) {
        let source = decode(&fixture.source_path());
        let decoded = decode(&fixture.path());
        let skip = if fixture.container == "adts" {
            ADTS_PRIMING
        } else {
            0
        };
        let name = &fixture.file;

        // Encoder delay and padding are trimmed: the decode is the source's
        // length, in time with it.
        if fixture.container == "adts" {
            if decoded.frames() < source.frames() + skip {
                failures.push(format!("{name}: {} frames", decoded.frames()));
            }
        } else if decoded.frames() != source.frames() {
            failures.push(format!(
                "{name}: {} frames, the source has {}",
                decoded.frames(),
                source.frames()
            ));
        }
        if decoded.channels().flatten().any(|s| !s.is_finite()) {
            failures.push(format!("{name}: a sample isn't finite"));
        }
        // A lossy decode of full-scale audio overshoots full scale, and the
        // overshoot is kept, not clipped.
        if fixture.signal == "clip" {
            let peak = decoded
                .channels()
                .flatten()
                .fold(0.0f32, |m, s| m.max(s.abs()));
            if peak <= 1.0 {
                failures.push(format!("{name}: peaks at {peak}; overs were clipped"));
            }
        }

        let reference = planes(&source);
        let output: Vec<&[f32]> = decoded
            .channels()
            .map(|c| &c[skip..(skip + source.frames()).min(c.len())])
            .collect();
        if fixture.signal == "silence" {
            let peak = peak_dbfs(&output);
            println!(
                "{name:<32} {:>8} {:>8} {:>10} {peak:>7.1} pk {SILENCE_MAX_DBFS:>10.1}",
                decoded.frames(),
                source.frames(),
                "",
            );
            if peak > SILENCE_MAX_DBFS {
                failures.push(format!("{name}: silence peaks at {peak:.1} dBFS"));
            }
            continue;
        }
        let full = snr_db(&reference, &output);
        let snr = band_snr_db(&reference, &output, fixture.rate, LOSSY_BAND_HZ);
        let limit = lossy_min_snr_db(&fixture.codec, &fixture.signal);
        println!(
            "{name:<32} {:>8} {:>8} {full:>10.1} {snr:>10.1} {limit:>10.1}",
            decoded.frames(),
            source.frames()
        );
        if snr < limit {
            failures.push(format!("{name}: SNR {snr:.1} dB, under {limit} dB"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn seeking_lands_on_the_exact_frame() {
    // Spec: audio-import R7
    let mut failures = Vec::new();
    let mut checked = 0;
    for fixture in manifest()
        .into_iter()
        .filter(|f| f.signal == "noise" || f.file.starts_with("sine-44k1-s16-2ch"))
    {
        let whole = decode(&fixture.path());
        let frames = whole.frames();
        for target in [
            0,
            1,
            1000,
            frames / 2 + 7,
            frames.saturating_sub(300),
            frames,
        ] {
            let mut decoder = Decoder::open(fixture.path()).unwrap();
            decoder.seek(target as u64).unwrap();
            assert_eq!(decoder.position(), target as u64);
            let channels = whole.channel_count();
            let mut block = vec![vec![0.0f32; 256]; channels];
            let mut outs: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..]).collect();
            let n = decoder.read(&mut outs).unwrap();
            let expected = 256.min(frames - target);
            if n != expected {
                failures.push(format!(
                    "{}: {n} frames after seeking to {target}",
                    fixture.file
                ));
                continue;
            }
            let reference: Vec<&[f32]> = whole.channels().map(|c| &c[target..target + n]).collect();
            let got: Vec<&[f32]> = block.iter().map(|c| &c[..n]).collect();
            // Lossy decoders too: the seek pre-rolls far enough that the
            // decoder's state matches decoding from the start.
            let exact = reference.iter().zip(&got).all(|(r, g)| {
                r.iter()
                    .zip(g.iter())
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            });
            if !exact {
                failures.push(format!("{}: seeking to {target} isn't exact", fixture.file));
            }
            checked += 1;
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(checked >= 40, "only {checked} seeks checked");
}

#[test]
fn reading_in_small_blocks_matches_reading_whole() {
    for file in [
        "sweep-44k1-s16-2ch.mp3",
        "sine-48k-s24-2ch.aac.m4a",
        "sweep-48k-s24-1ch.flac",
    ] {
        let path = common::fixtures_dir().join(file);
        let whole = decode(&path);
        let mut decoder = Decoder::open(&path).unwrap();
        let channels = whole.channel_count();
        let mut collected = vec![Vec::new(); channels];
        let mut block = vec![vec![0.0f32; 37]; channels];
        loop {
            let mut outs: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..]).collect();
            let n = decoder.read(&mut outs).unwrap();
            if n == 0 {
                break;
            }
            for (dst, src) in collected.iter_mut().zip(&block) {
                dst.extend_from_slice(&src[..n]);
            }
        }
        for (got, want) in collected.iter().zip(whole.channels()) {
            assert_eq!(got.as_slice(), want, "{file}");
        }
    }
}

#[test]
fn reading_with_the_wrong_channel_count_is_an_error() {
    let path = common::fixtures_dir().join("sine-44k1-s16-2ch.wav");
    let mut decoder = Decoder::open(path).unwrap();
    let mut one = [0.0f32; 16];
    assert!(matches!(
        decoder.read(&mut [&mut one[..]]),
        Err(MediaError::Malformed(_))
    ));
}

#[test]
fn damaged_and_foreign_files_fail_cleanly() {
    // Spec: audio-import R8
    let dir = common::fixtures_dir();

    // Not audio.
    let text = Cursor::new(b"This is not an audio file, just some text.".to_vec());
    assert!(Decoder::from_reader(text, Some("wav")).is_err());
    assert!(Decoder::from_reader(Cursor::new(Vec::new()), None).is_err());

    // Cut short: decoding keeps the frames that are there.
    for file in [
        "noise-44k1-s16-2ch.wav",
        "noise-44k1-s16-2ch.flac",
        "noise-44k1-s16-2ch.mp3",
        "noise-44k1-s16-2ch.ogg",
        "sine-48k-s24-2ch.rf64.wav",
    ] {
        // Three quarters in, past every header (ffmpeg pads FLAC headers to
        // 8 KiB, and a Vorbis setup header is about 4 KiB).
        let bytes = std::fs::read(dir.join(file)).unwrap();
        let cut = Cursor::new(bytes[..bytes.len() * 3 / 4].to_vec());
        let extension = file.rsplit('.').next();
        let decoded = Decoder::from_reader(cut, extension)
            .and_then(Decoder::decode_to_end)
            .unwrap_or_else(|e| panic!("{file} cut short: {e}"));
        let whole = decode(&dir.join(file));
        assert!(decoded.frames() > 0, "{file} cut short decoded nothing");
        assert!(decoded.frames() < whole.frames(), "{file}");
    }

    // An MPEG-4 file cut short has a sample table pointing past its end,
    // which symphonia rejects; that's an error, not a panic.
    let bytes = std::fs::read(dir.join("noise-44k1-s16-2ch.aac.m4a")).unwrap();
    let cut = Cursor::new(bytes[..bytes.len() * 3 / 4].to_vec());
    if let Ok(decoder) = Decoder::from_reader(cut, Some("m4a")) {
        let _ = decoder.decode_to_end();
    }

    // Damaged in the middle: no panic, and every sample finite.
    for file in [
        "sine-44k1-s16-2ch.mp3",
        "sine-44k1-s16-2ch.flac",
        "sine-44k1-s16-2ch.ogg",
    ] {
        let mut bytes = std::fs::read(dir.join(file)).unwrap();
        let middle = bytes.len() / 2;
        for (i, byte) in bytes[middle..middle + 200].iter_mut().enumerate() {
            *byte = (i * 151 % 256) as u8;
        }
        let extension = file.rsplit('.').next();
        if let Ok(decoded) =
            Decoder::from_reader(Cursor::new(bytes), extension).and_then(Decoder::decode_to_end)
        {
            assert!(
                decoded.channels().flatten().all(|s| s.is_finite()),
                "{file}"
            );
        }
    }
}

/// A fixed-blocksize FLAC stream whose header counts its short last block in
/// the minimum block size, as some encoders write, decodes exactly as the
/// same stream with the minimum the format calls for. Other decoders read
/// such files; Beatlayer must too.
#[test]
fn a_flac_that_counts_its_short_last_block_decodes_as_written() {
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/single-tone.flac");
    let bytes = std::fs::read(&golden).unwrap();
    let min = u16::from_be_bytes([bytes[8], bytes[9]]);
    let max = u16::from_be_bytes([bytes[10], bytes[11]]);
    assert_eq!(min, max, "the reference holds the format's minimum");

    let mut quirky = bytes.clone();
    quirky[8..10].copy_from_slice(&1792u16.to_be_bytes());
    let decode = |bytes: Vec<u8>| {
        bl_media::Decoder::from_reader(std::io::Cursor::new(bytes), Some("flac"))
            .and_then(bl_media::Decoder::decode_to_end)
            .unwrap()
    };
    let written = decode(bytes);
    let read = decode(quirky);
    assert_eq!(written.frames(), 96_000);
    assert_eq!(read.frames(), written.frames());
    assert!(read == written, "the samples match");
}
