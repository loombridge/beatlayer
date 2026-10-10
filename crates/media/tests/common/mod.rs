//! Shared helpers for the media tests: the fixture manifest and an
//! independent WAV reader.

#![allow(dead_code)]

pub mod wav;

use std::path::PathBuf;

/// A folder for one test's files, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("bl-media-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The generated fixtures, from `tests/fixtures/gen/gen_media.py`.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/media")
}

/// One row of `manifest.json`.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub file: String,
    pub source: String,
    pub signal: String,
    pub container: String,
    pub codec: String,
    pub lossless: bool,
    pub rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub encoder: String,
}

impl Fixture {
    pub fn path(&self) -> PathBuf {
        fixtures_dir().join(&self.file)
    }

    pub fn source_path(&self) -> PathBuf {
        fixtures_dir().join(&self.source)
    }

    /// Whether this is a source WAV that gen_media.py wrote byte by byte.
    pub fn is_source(&self) -> bool {
        self.file == self.source
    }
}

pub fn manifest() -> Vec<Fixture> {
    let text = std::fs::read_to_string(fixtures_dir().join("manifest.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let text = |key: &str| f[key].as_str().unwrap().to_owned();
            Fixture {
                file: text("file"),
                source: text("source"),
                signal: text("signal"),
                container: text("container"),
                codec: text("codec"),
                lossless: f["lossless"].as_bool().unwrap(),
                rate: f["rate"].as_u64().unwrap() as u32,
                channels: f["channels"].as_u64().unwrap() as u16,
                frames: f["frames"].as_u64().unwrap(),
                encoder: text("encoder"),
            }
        })
        .collect()
}

/// A WAV file read by a parser written for these tests, independent of
/// symphonia: rate, channels, and planar samples scaled the way the media
/// crate promises (a 16-bit sample `s` is `s / 32768`).
pub fn read_wav_independently(bytes: &[u8]) -> (u32, Vec<Vec<f32>>) {
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    let mut format = None;
    let mut data = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..pos + 8 + size];
        match id {
            b"fmt " => format = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        pos += 8 + size + size % 2;
    }
    let format = format.unwrap();
    let data = data.unwrap();
    let mut tag = u16::from_le_bytes([format[0], format[1]]);
    let channels = usize::from(u16::from_le_bytes([format[2], format[3]]));
    let rate = u32::from_le_bytes(format[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes([format[14], format[15]]);
    if tag == 0xFFFE {
        tag = u16::from_le_bytes([format[24], format[25]]);
    }
    let width = usize::from(bits / 8);
    let mut planes = vec![Vec::new(); channels];
    for (i, sample) in data.chunks_exact(width).enumerate() {
        let value = match (tag, bits) {
            (1, 8) => (f32::from(sample[0]) - 128.0) / 128.0,
            (1, 16) => f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0,
            (1, 24) => {
                let v = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8;
                v as f32 / 8_388_608.0
            }
            (3, 32) => f32::from_le_bytes(sample.try_into().unwrap()),
            other => panic!("the test parser doesn't read {other:?}"),
        };
        planes[i % channels].push(value);
    }
    (rate, planes)
}

/// The signal-to-error ratio of `decoded` against `reference`, in dB, over
/// the middle 80% of the frames, per channel; the worst channel's.
pub fn snr_db(reference: &[&[f32]], decoded: &[&[f32]]) -> f64 {
    reference
        .iter()
        .zip(decoded)
        .map(|(r, d)| {
            let n = r.len().min(d.len());
            let (start, end) = (n / 10, n - n / 10);
            let mut signal = 0.0;
            let mut error = 0.0;
            for i in start..end {
                let (r, d) = (f64::from(r[i]), f64::from(d[i]));
                signal += r * r;
                error += (d - r) * (d - r);
            }
            10.0 * (signal / error.max(1e-30)).log10()
        })
        .fold(f64::INFINITY, f64::min)
}

/// The signal-to-error ratio in dB below `band_hz`, per channel; the worst
/// channel's. Lossy encoders low-pass the top octave away on purpose, so a
/// full-band ratio would measure that choice rather than decoding. The error
/// and the reference are both Hann-windowed over the middle 80% of the frames
/// and compared in the frequency domain.
pub fn band_snr_db(reference: &[&[f32]], decoded: &[&[f32]], rate: u32, band_hz: f64) -> f64 {
    use realfft::RealFftPlanner;
    reference
        .iter()
        .zip(decoded)
        .map(|(r, d)| {
            let n = r.len().min(d.len());
            let (start, end) = (n / 10, n - n / 10);
            let len = end - start;
            let window =
                |i: usize| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / len as f64).cos();
            let mut planner = RealFftPlanner::<f64>::new();
            let fft = planner.plan_fft_forward(len);
            let power = |samples: Vec<f64>| {
                let mut input = samples;
                let mut output = fft.make_output_vec();
                fft.process(&mut input, &mut output).unwrap();
                let top = ((band_hz * len as f64 / f64::from(rate)) as usize).min(output.len());
                output[..top].iter().map(|c| c.norm_sqr()).sum::<f64>()
            };
            let signal = power(
                (0..len)
                    .map(|i| f64::from(r[start + i]) * window(i))
                    .collect(),
            );
            let error = power(
                (0..len)
                    .map(|i| f64::from(d[start + i] - r[start + i]) * window(i))
                    .collect(),
            );
            10.0 * (signal / error.max(1e-30)).log10()
        })
        .fold(f64::INFINITY, f64::min)
}

/// The largest absolute sample, in dBFS.
pub fn peak_dbfs(channels: &[&[f32]]) -> f64 {
    let peak = channels
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |m, s| m.max(s.abs()));
    20.0 * f64::from(peak).max(1e-30).log10()
}
