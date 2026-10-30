//! Export formats: every file type, channel count, bit depth and sample rate
//! `export.audio` accepts, decoded back and held against a 32-bit float WAV
//! of the same bar at the engine's rate. Each file must hold the range's
//! frames at its own rate and the channels asked for (MP3 within its
//! encoder's padding), and its level must match the reference: lossless
//! files within 0.1 dB, MP3 within 1 dB. A case that fails is either
//! listed in `known-failures.toml` under suite `live_offline`, key
//! `<type>/<channels>/<bits>/<rate>`, property "length" or "level", or
//! fails.
//!
//! Quick depth exports every type, bit depth and channel count at the
//! engine's rate and a few other rates; full depth exports each at every
//! rate the dialog lists (MP3 at those of its rates the dialog lists).

use bl_e2e::audio::{Audio, assert_finite};
use bl_e2e::{BAR, ExportSpec, Host, Known, Ratchet, full_depth};

use crate::live_offline::{RATE, SUITE, clear, fixture_file, partials, place, track};

/// The level tolerance for a lossless file, in dB.
const LOSSLESS_DB: f64 = 0.1;

/// The level tolerance for MP3, in dB. At MPEG-1 rates 320 kbps keeps the
/// partials within 0.01 dB; at MPEG-2's 22.05 kHz the encoder's low-pass
/// takes about 0.45 dB off the top partial.
const LOSSY_DB: f64 = 1.0;

/// The most frames an MP3 decode may differ from the range by: two MP3
/// frames, the encoder's padding at most.
const MP3_PADDING: usize = 2 * 1_152;

/// The rates the export dialog lists (export R35).
const DIALOG_RATES: [u32; 8] = [
    22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
];

/// The rates MP3 holds that the dialog lists (export R23, R35).
const MP3_DIALOG_RATES: [u32; 4] = [22_050, 32_000, 44_100, 48_000];

/// One export: a file type by its extension, its bit depth (0 for MP3,
/// which has none), its rate and whether it is mono.
#[derive(Clone, Copy, Debug)]
struct Format {
    extension: &'static str,
    bits: u8,
    rate: u32,
    mono: bool,
}

impl Format {
    /// The ratchet's key: `<type>/<channels>/<bits>/<rate>`.
    fn key(self) -> String {
        let bits = if self.bits == 0 {
            "-".to_owned()
        } else {
            self.bits.to_string()
        };
        let channels = if self.mono { "mono" } else { "stereo" };
        format!("{}/{channels}/{bits}/{}", self.extension, self.rate)
    }

    fn lossy(self) -> bool {
        self.extension == "mp3"
    }

    fn spec(self) -> ExportSpec {
        ExportSpec {
            extension: self.extension,
            // MP3 ignores the bit depth (R23).
            bit_depth: if self.bits == 0 { 24 } else { self.bits },
            sample_rate: Some(self.rate),
            mono: self.mono,
            ..ExportSpec::default()
        }
    }
}

/// Every type and bit depth export R5 accepts: (extension, bits).
const TYPES: [(&str, u8); 9] = [
    ("wav", 16),
    ("wav", 24),
    ("wav", 32),
    ("aif", 16),
    ("aif", 24),
    ("aif", 32),
    ("flac", 16),
    ("flac", 24),
    ("mp3", 0),
];

/// The formats this depth exports.
fn formats() -> Vec<Format> {
    let mut formats = Vec::new();
    for (extension, bits) in TYPES {
        let rates: Vec<u32> = match (full_depth(), extension) {
            (true, "mp3") => MP3_DIALOG_RATES.to_vec(),
            (true, _) => DIALOG_RATES.to_vec(),
            (false, "mp3") => vec![RATE, 44_100, 22_050],
            (false, "wav") if bits == 24 => vec![RATE, 44_100, 96_000, 22_050],
            (false, "flac") if bits == 16 => vec![RATE, 44_100],
            (false, _) => vec![RATE],
        };
        for rate in rates {
            for mono in [false, true] {
                formats.push(Format {
                    extension,
                    bits,
                    rate,
                    mono,
                });
            }
        }
    }
    formats
}

/// RMS of `audio` over the middle of the bar, from 0.1 s to 1.9 s, in dBFS:
/// away from the edges, where a rate converter's filters and an MP3
/// encoder's frames settle.
fn middle_rms_dbfs(audio: &Audio) -> f64 {
    let at = |seconds: f64| ((seconds * f64::from(audio.rate)) as usize).min(audio.frames());
    audio.slice(at(0.1), at(1.9)).rms_dbfs()
}

/// A set of one audio track playing five partials, stereo, for one bar.
fn set() -> Host {
    let mut host = Host::new();
    clear(&mut host);
    let file = fixture_file(&host, "partials.wav", &partials(2.0, 0.05));
    let track = track(&mut host, "Partials", "audio", 0, None);
    place(&mut host, track, &file, 0);
    host.settle();
    host
}

/// Exports `format` and checks its length and level against `reference`.
fn check(ratchet: &mut Ratchet, host: &mut Host, format: Format, reference: &Audio) {
    let key = format.key();
    // An export that fails has no length to check, and no level.
    let audio = match host.export_file(0, BAR, &format.spec()) {
        Ok((_, path)) => Audio::decode(&path).unwrap_or_else(|error| panic!("{key}: {error}")),
        Err(error) => {
            eprintln!("{key}: {error}");
            ratchet.check(&key, "length", Err(format!("the export failed: {error}")));
            return;
        }
    };
    assert_finite(&audio, &key);
    // Spec: export R3, R6, R12, R23, R33. The bar's frames at the file's
    // rate, two channels or one.
    let frames = 2 * format.rate as usize;
    let channels = if format.mono { 1 } else { 2 };
    let slack = if format.lossy() { MP3_PADDING } else { 0 };
    let length = if audio.rate != format.rate {
        Err(format!("the file is at {} Hz", audio.rate))
    } else if audio.channels.len() != channels {
        Err(format!("{} channels, not {channels}", audio.channels.len()))
    } else if audio.frames().abs_diff(frames) > slack {
        Err(format!(
            "{} frames, not {frames} (within {slack})",
            audio.frames()
        ))
    } else {
        Ok(())
    };
    // Spec: export R4, R33. The level is the engine's; mono is the average
    // of the two channels.
    let expected = if format.mono {
        middle_rms_dbfs(&Audio::new(reference.rate, vec![reference.mono()]))
    } else {
        middle_rms_dbfs(reference)
    };
    let level_db = middle_rms_dbfs(&audio) - expected;
    let tolerance = if format.lossy() {
        LOSSY_DB
    } else {
        LOSSLESS_DB
    };
    let level = if level_db.abs() <= tolerance {
        Ok(())
    } else {
        Err(format!(
            "{level_db:+.2} dB against the 32-bit WAV (within {tolerance} dB)"
        ))
    };
    eprintln!(
        "{key}: {} frames, {} channels, level {level_db:+.3} dB",
        audio.frames(),
        audio.channels.len()
    );
    ratchet.check(&key, "length", length);
    ratchet.check(&key, "level", level);
}

// Spec: export R5, R6, R12, R23, R33, R35. Every file type at every bit
// depth it holds, mono and stereo, at the rates the dialog lists, decodes to
// the bar's frames at its rate and the engine's level.
#[test]
fn every_format_exports_the_whole_range_at_its_level() {
    let mut ratchet = Known::load().ratchet(SUITE);
    let mut host = set();
    let reference = host.export(0, BAR, &ExportSpec::default());
    assert_eq!(reference.frames(), 2 * RATE as usize, "the reference");
    for format in formats() {
        check(&mut ratchet, &mut host, format, &reference);
    }
    ratchet.finish();
}

// Spec: export R5. FLAC holds integers only, so a 32-bit FLAC export is
// refused before anything renders; MP3 at a rate it can't hold is refused
// too (R23).
#[test]
fn formats_a_file_cannot_hold_are_refused() {
    let mut host = set();
    for format in [
        Format {
            extension: "flac",
            bits: 32,
            rate: RATE,
            mono: false,
        },
        Format {
            extension: "mp3",
            bits: 0,
            rate: 96_000,
            mono: false,
        },
    ] {
        let refused = host.export_file(0, BAR, &format.spec());
        assert!(
            refused.is_err(),
            "{} was exported: {refused:?}",
            format.key()
        );
    }
}
