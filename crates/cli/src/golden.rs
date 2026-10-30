//! The golden-test harness: songs built in code (the fixtures), rendered
//! offline and null-tested against reference renders stored in
//! `tests/golden/`.
//!
//! Each fixture ([`fixtures`]) is a document, the audio files its clips
//! play (each made in code, as a WAV file in memory), the commands
//! scheduled on it, the transport's moves (each a [`Cue`] on a frame; most
//! fixtures just play from the start), and a length. The harness renders it
//! with [`render::render_with`], the code behind `bl-cli render`, at
//! [`SAMPLE_RATE`] in blocks of [`BLOCK`] frames, and compares the render
//! with its reference ([`null_test`]):
//!
//! - The reference is a 24-bit FLAC file, `<name>.flac`. The render is
//!   rounded to 24 bits ([`render::to_i24`]) and subtracted from it sample
//!   by sample. The residual is the largest difference, in dBFS.
//! - `manifest.json` records, for each reference, the platform that made it
//!   (operating system and architecture), its format, and a hash of the
//!   32-bit float render it was made from.
//! - On the platform that made the reference, the render must be bit-exact:
//!   every 24-bit sample the same, and the float render the same to the bit,
//!   which the hash checks.
//! - Elsewhere, the residual must be below [`THRESHOLD_DBFS`], −96 dBFS,
//!   because platforms' maths libraries may round the last bit differently.
//!   (Today the test tone and the tempo map use pure-Rust maths, so the
//!   fixtures render to the same bits everywhere; the threshold leaves room
//!   for DSP that uses the platform's.)
//!
//! A render also fails if it has a sample that isn't finite, touches the
//! heap inside an engine block (where the allocation trap is installed),
//! leaves a scheduled command unfired, has a device no node plays, or has a
//! clip whose audio is missing.
//!
//! References change only when someone runs `cargo xtask golden --update`
//! (which runs `bl-cli golden --update`), never in a test, and the pull
//! request that commits a changed reference says why.

mod fixtures;

/// The fader drag the Mixer view's test records (WP-1.9), as the frames and
/// volumes the `fader-gesture` fixture plays.
pub use fixtures::{FADER_GESTURE, fader_gesture_frames};

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bl_core::time::SampleRate;
use bl_engine::{MediaMap, ScheduledCommand};
use bl_model::{ContentHash, Document, Editor};
use serde::{Deserialize, Serialize};

use crate::Outcome;
use crate::render::{self, Cue, Launch, Settings, Summary, dbfs, to_i24};

/// The references' sample rate.
pub const SAMPLE_RATE: SampleRate = SampleRate::HZ_48000;

/// The block size fixtures render in. The output is the same at any size.
pub const BLOCK: usize = 512;

/// Bits per sample in a reference.
pub const BITS: u32 = 24;

/// What a render's residual must stay below on a platform other than the
/// one that made its reference.
pub const THRESHOLD_DBFS: f64 = -96.0;

/// Where the references are, from the top of the repository.
pub const DIR: &str = "tests/golden";

/// The manifest's file name.
pub const MANIFEST: &str = "manifest.json";

/// What the manifest says about itself.
const ABOUT: &str = "Golden references: each fixture in crates/cli/src/golden/fixtures.rs \
    rendered to a 24-bit FLAC file. render_hash is an FNV-1a hash of the 32-bit float render \
    the reference was made from. `cargo test` null-tests every fixture against its reference: \
    bit-exact on the platform in made_on, below -96 dBFS elsewhere. Change references only with \
    `cargo xtask golden --update`, and say why in the pull request.";

/// A song for a golden test.
#[derive(Debug)]
pub struct Fixture {
    /// Its name, which also names its reference: `<name>.flac`.
    pub name: &'static str,
    /// What it covers.
    pub about: &'static str,
    build: fn() -> Scene,
}

/// A fixture's song: its document, the audio files its clips play, what's
/// scheduled on it, how the transport moves, and how long it plays.
#[derive(Debug)]
pub struct Scene {
    /// The editor holding the song, with the IDs it handed out.
    pub editor: Editor,
    /// The audio files the song's clips play, as files on disk would hold
    /// them.
    pub files: Vec<MediaFile>,
    /// The same files decoded, by content hash: what the media store would
    /// give the engine.
    pub media: MediaMap,
    /// Commands scheduled before the first block.
    pub schedule: Vec<ScheduledCommand>,
    /// The transport's moves, each on its frame. Most fixtures play from
    /// the start: [`Cue::PLAY`] alone.
    pub cues: Vec<Cue>,
    /// The launches and stops asked of the Session View, each on its frame
    /// (WP-3.5).
    pub launches: Vec<Launch>,
    /// Frames to render, at [`SAMPLE_RATE`].
    pub frames: u64,
    /// The interface's output channels the reference holds after Main's: the
    /// CV tools' fixtures capture the channels their devices write.
    pub device_channels: usize,
}

/// An audio file a fixture's clips play, made in code.
#[derive(Clone, Debug)]
pub struct MediaFile {
    /// A file name for it, such as `chirp.wav`.
    pub name: String,
    /// The content hash its clips name it by: the BLAKE3 hash of `bytes`.
    pub hash: ContentHash,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

impl Scene {
    /// The song.
    #[must_use]
    pub fn document(&self) -> &Document {
        self.editor.document()
    }

    /// Whether the song plays from its start with nothing scheduled, as
    /// `bl-cli render` plays a saved set.
    #[must_use]
    pub fn plays_as_saved(&self) -> bool {
        // A render that captures device channels isn't one: a saved set sends
        // no CV when it renders offline (the CV tools spec, R7).
        self.schedule.is_empty()
            && self.cues == [Cue::PLAY]
            && self.launches.is_empty()
            && self.device_channels == 0
    }
}

/// Every fixture, in the order they're reported.
#[must_use]
pub fn fixtures() -> &'static [Fixture] {
    fixtures::FIXTURES
}

/// The fixture called `name`.
#[must_use]
pub fn fixture(name: &str) -> Option<&'static Fixture> {
    fixtures().iter().find(|fixture| fixture.name == name)
}

impl Fixture {
    /// Builds the song.
    #[must_use]
    pub fn scene(&self) -> Scene {
        (self.build)()
    }

    /// Renders the song in blocks of [`BLOCK`] frames.
    #[must_use]
    pub fn render(&self) -> Render {
        self.render_in_blocks(BLOCK)
    }

    /// Renders the song in blocks of `block` frames.
    #[must_use]
    pub fn render_in_blocks(&self, block: usize) -> Render {
        self.render_with_workers(block, 0)
    }

    /// Renders the song in blocks of `block` frames, its tasks run by
    /// `workers` workers beside the rendering thread (WP-5.37).
    #[must_use]
    pub fn render_with_workers(&self, block: usize, workers: usize) -> Render {
        let scene = self.scene();
        let settings = Settings {
            sample_rate: SAMPLE_RATE,
            block,
            frames: scene.frames,
        };
        let mut samples = Vec::with_capacity(scene.frames as usize * 2);
        let script = render::Script {
            schedule: &scene.schedule,
            cues: &scene.cues,
            launches: &scene.launches,
            device_channels: scene.device_channels,
        };
        let summary = render::render_launching_with_workers(
            scene.document(),
            Arc::new(scene.media.clone()),
            &script,
            &settings,
            workers,
            |output| render::interleave(output, &mut samples),
        );
        Render {
            sample_rate: SAMPLE_RATE.hz(),
            channels: summary.channels,
            samples,
            scheduled: scene.schedule.len(),
            summary,
        }
    }

    /// Where its reference is in `dir`.
    #[must_use]
    pub fn reference_path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.flac", self.name))
    }
}

/// A render: 32-bit float samples, interleaved.
#[derive(Clone, Debug)]
pub struct Render {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame.
    pub channels: usize,
    /// The samples, each frame's channels in turn.
    pub samples: Vec<f32>,
    /// Commands the song scheduled.
    pub scheduled: usize,
    /// What the render did, besides its audio.
    pub summary: Summary,
}

impl Render {
    /// Frames rendered.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    /// The samples rounded to 24 bits, as a reference holds them.
    #[must_use]
    pub fn quantised(&self) -> Vec<i32> {
        self.samples.iter().map(|&s| to_i24(s)).collect()
    }

    /// An FNV-1a hash of the samples' bits, which changes if any bit of any
    /// sample does.
    #[must_use]
    pub fn hash(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for sample in &self.samples {
            for byte in sample.to_bits().to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("fnv1a64:{hash:016x}")
    }

    /// Whether every sample is the same to the bit as in `other`.
    #[must_use]
    pub fn same_bits(&self, other: &Render) -> bool {
        self.samples.len() == other.samples.len()
            && self
                .samples
                .iter()
                .zip(&other.samples)
                .all(|(a, b)| a.to_bits() == b.to_bits())
    }

    /// What's wrong with the render itself, whatever the reference says.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let summary = &self.summary;
        if summary.non_finite > 0 {
            problems.push(format!(
                "{} samples are NaN or infinite",
                summary.non_finite
            ));
        }
        if summary.heap_operations > 0 {
            problems.push(format!(
                "the engine touched the heap {} times inside its blocks",
                summary.heap_operations
            ));
        }
        if summary.fired() != self.scheduled {
            problems.push(format!(
                "{} of {} scheduled commands fired",
                summary.fired(),
                self.scheduled
            ));
        }
        if !summary.unhosted.is_empty() {
            problems.push(format!(
                "{} devices have no node to play them",
                summary.unhosted.len()
            ));
        }
        if !summary.missing_media.is_empty() {
            problems.push(format!(
                "{} audio files the clips play are missing",
                summary.missing_media.len()
            ));
        }
        problems
    }
}

/// A reference, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame.
    pub channels: usize,
    /// Bits per sample.
    pub bits: u32,
    /// The samples, interleaved.
    pub samples: Vec<i32>,
}

/// Reads a reference.
///
/// # Errors
///
/// If the file can't be read or isn't valid FLAC.
pub fn read_flac(path: &Path) -> Result<Reference, String> {
    let failed = |error: claxon::Error| format!("can't read {}: {error}", path.display());
    let mut reader = claxon::FlacReader::open(path).map_err(failed)?;
    let info = reader.streaminfo();
    let samples = reader
        .samples()
        .collect::<Result<Vec<i32>, _>>()
        .map_err(failed)?;
    Ok(Reference {
        sample_rate: info.sample_rate,
        channels: info.channels as usize,
        bits: info.bits_per_sample,
        samples,
    })
}

/// Writes `render` as a 24-bit FLAC reference, then reads it back and
/// checks that it holds exactly the samples written.
///
/// # Errors
///
/// If the encoder fails, the file can't be written, or it doesn't read back
/// the same.
pub fn write_flac(path: &Path, render: &Render) -> Result<(), String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let samples = render.quantised();
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, error)| format!("the FLAC encoder's settings are invalid: {error:?}"))?;
    let source = flacenc::source::MemSource::from_samples(
        &samples,
        render.channels,
        BITS as usize,
        render.sample_rate as usize,
    );
    let mut stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|error| format!("can't encode {}: {error:?}", path.display()))?;
    // A fixed-blocksize stream's minimum block size leaves out the short
    // last block; the encoder counts it, which makes the stream look
    // variable-blocksize to decoders that check, as Beatlayer's importer
    // (symphonia) does.
    stream
        .stream_info_mut()
        .set_block_sizes(config.block_size, config.block_size)
        .map_err(|error| format!("can't encode {}: {error:?}", path.display()))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|error| format!("can't encode {}: {error:?}", path.display()))?;
    fs::write(path, sink.as_slice())
        .map_err(|error| format!("can't write {}: {error}", path.display()))?;
    let back = read_flac(path)?;
    let expected = Reference {
        sample_rate: render.sample_rate,
        channels: render.channels,
        bits: BITS,
        samples,
    };
    if back != expected {
        return Err(format!(
            "{} doesn't read back as the samples written to it",
            path.display()
        ));
    }
    Ok(())
}

/// What `manifest.json` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// What the file is.
    pub about: String,
    /// Each fixture's reference, by fixture name.
    pub references: BTreeMap<String, Entry>,
}

/// What the manifest records about one reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The platform that made it, as [`platform`] names it. Renders there
    /// must be bit-exact.
    pub made_on: String,
    /// Frames per second.
    pub sample_rate: u32,
    /// Channels in each frame.
    pub channels: usize,
    /// Frames.
    pub frames: usize,
    /// Bits per sample.
    pub bits: u32,
    /// [`Render::hash`] of the render it was made from.
    pub render_hash: String,
}

impl Entry {
    /// The entry for a reference made from `render` on this platform.
    #[must_use]
    pub fn for_render(render: &Render) -> Self {
        Self {
            made_on: platform(),
            sample_rate: render.sample_rate,
            channels: render.channels,
            frames: render.frames(),
            bits: BITS,
            render_hash: render.hash(),
        }
    }
}

impl Manifest {
    /// Reads `dir`'s manifest. A folder without one has an empty manifest.
    ///
    /// # Errors
    ///
    /// If the manifest can't be read or parsed.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(MANIFEST);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("can't read {}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                about: ABOUT.to_owned(),
                references: BTreeMap::new(),
            }),
            Err(error) => Err(format!("can't read {}: {error}", path.display())),
        }
    }

    /// Writes `dir`'s manifest.
    ///
    /// # Errors
    ///
    /// If it can't be written.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(MANIFEST);
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("can't write {}: {error}", path.display()))?;
        json.push('\n');
        fs::write(&path, json).map_err(|error| format!("can't write {}: {error}", path.display()))
    }
}

/// This platform: its operating system and architecture, such as
/// `macos-aarch64` or `windows-x86_64`.
#[must_use]
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// A null test's result: a render against its reference.
#[derive(Clone, Debug)]
pub struct Null {
    /// The fixture.
    pub fixture: String,
    /// The platform that made the reference.
    pub made_on: String,
    /// The platform the render ran on.
    pub platform: String,
    /// The render's peak, in dBFS.
    pub peak_dbfs: f64,
    /// What's wrong apart from the residual: the render's own problems, a
    /// missing reference, a format that doesn't match.
    pub problems: Vec<String>,
    /// The largest difference between a 24-bit sample of the render and of
    /// the reference, as a fraction of full scale.
    pub residual: f64,
    /// Where the largest difference is: frame and channel.
    pub worst: Option<(usize, usize)>,
    /// How many samples differ at 24 bits.
    pub differing: usize,
    /// Whether the float render hashes to the manifest's hash.
    pub same_hash: bool,
}

impl Null {
    /// Whether the render must be bit-exact: it ran on the platform that
    /// made the reference.
    #[must_use]
    pub fn exact_required(&self) -> bool {
        self.made_on == self.platform
    }

    /// Whether the render matches the reference to the bit.
    #[must_use]
    pub fn bit_exact(&self) -> bool {
        self.problems.is_empty() && self.differing == 0 && self.same_hash
    }

    /// The residual in dBFS: −∞ when every 24-bit sample matches.
    #[must_use]
    pub fn residual_dbfs(&self) -> f64 {
        dbfs(self.residual)
    }

    /// Whether the render passes: bit-exact on the platform that made the
    /// reference, and with a residual below [`THRESHOLD_DBFS`] elsewhere.
    #[must_use]
    pub fn passed(&self) -> bool {
        if !self.problems.is_empty() {
            return false;
        }
        if self.exact_required() {
            self.bit_exact()
        } else {
            self.residual_dbfs() < THRESHOLD_DBFS
        }
    }

    fn worst(&self) -> String {
        match self.worst {
            Some((frame, channel)) => format!(" at frame {frame}, channel {channel}"),
            None => String::new(),
        }
    }
}

impl fmt::Display for Null {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.fixture)?;
        if !self.problems.is_empty() {
            return write!(f, "FAILED: {}", self.problems.join("; "));
        }
        let differ = match self.differing {
            1 => "1 sample differs".to_owned(),
            n => format!("{n} samples differ"),
        };
        match (self.passed(), self.exact_required()) {
            (true, _) if self.bit_exact() => write!(
                f,
                "bit-exact (reference made on {}, rendered on {})",
                self.made_on, self.platform
            ),
            (true, _) if self.differing == 0 => write!(
                f,
                "every 24-bit sample matches, though the 32-bit float render differs below them (reference made on {}, rendered on {})",
                self.made_on, self.platform
            ),
            (true, _) => write!(
                f,
                "residual {:.1} dBFS{} ({differ}), below {THRESHOLD_DBFS} dBFS (reference made on {}, rendered on {})",
                self.residual_dbfs(),
                self.worst(),
                self.made_on,
                self.platform
            ),
            (false, true) if self.differing == 0 => write!(
                f,
                "FAILED: not bit-exact on {}, which made the reference: every 24-bit sample matches, but the 32-bit float render doesn't hash to the manifest's render_hash",
                self.made_on
            ),
            (false, true) => write!(
                f,
                "FAILED: not bit-exact on {}, which made the reference: {differ}, the largest by {:.1} dBFS{}",
                self.made_on,
                self.residual_dbfs(),
                self.worst()
            ),
            (false, false) => write!(
                f,
                "FAILED: residual {:.1} dBFS{} ({differ}), not below {THRESHOLD_DBFS} dBFS (reference made on {}, rendered on {})",
                self.residual_dbfs(),
                self.worst(),
                self.made_on,
                self.platform
            ),
        }
    }
}

/// Null-tests `render` against `reference`, which `entry` describes.
#[must_use]
pub fn null_test(fixture: &str, render: &Render, reference: &Reference, entry: &Entry) -> Null {
    let mut null = Null {
        fixture: fixture.to_owned(),
        made_on: entry.made_on.clone(),
        platform: platform(),
        peak_dbfs: render.summary.peak_dbfs(),
        problems: render.problems(),
        residual: 0.0,
        worst: None,
        differing: 0,
        same_hash: render.hash() == entry.render_hash,
    };
    let format = |rate: u32, channels: usize, bits: u32| {
        format!("{rate} Hz, {channels} channels, {bits} bits")
    };
    let rendered = format(render.sample_rate, render.channels, BITS);
    let stored = format(reference.sample_rate, reference.channels, reference.bits);
    let recorded = format(entry.sample_rate, entry.channels, entry.bits);
    if stored != recorded {
        null.problems.push(format!(
            "the reference is {stored}, but the manifest says {recorded}"
        ));
    }
    if rendered != stored {
        null.problems.push(format!(
            "the render is {rendered}, but the reference is {stored}"
        ));
    }
    let channels = reference.channels.max(1);
    let stored_frames = reference.samples.len() / channels;
    if stored_frames != entry.frames {
        null.problems.push(format!(
            "the reference has {stored_frames} frames, but the manifest says {}",
            entry.frames
        ));
    }
    if render.frames() != stored_frames {
        null.problems.push(format!(
            "the render has {} frames, but the reference has {stored_frames}",
            render.frames()
        ));
    }
    let mut largest = 0i64;
    for (i, (&ours, &theirs)) in render
        .quantised()
        .iter()
        .zip(&reference.samples)
        .enumerate()
    {
        let difference = (i64::from(ours) - i64::from(theirs)).abs();
        if difference != 0 {
            null.differing += 1;
            if difference > largest {
                largest = difference;
                null.worst = Some((i / channels, i % channels));
            }
        }
    }
    null.residual = largest as f64 / 8_388_608.0;
    null
}

/// Renders a fixture and null-tests it against its reference in `dir`.
#[must_use]
pub fn check_fixture(fixture: &Fixture, dir: &Path, manifest: &Manifest) -> Null {
    let render = fixture.render();
    let path = fixture.reference_path(dir);
    let reference = read_flac(&path);
    match (manifest.references.get(fixture.name), reference) {
        (Some(entry), Ok(reference)) => null_test(fixture.name, &render, &reference, entry),
        (entry, reference) => {
            let mut problems = render.problems();
            if entry.is_none() {
                problems.push(format!("{MANIFEST} has no entry for it"));
            }
            if let Err(error) = reference {
                problems.push(error);
            }
            problems.push("make its reference with `cargo xtask golden --update`".to_owned());
            Null {
                fixture: fixture.name.to_owned(),
                made_on: entry.map_or_else(String::new, |entry| entry.made_on.clone()),
                platform: platform(),
                peak_dbfs: render.summary.peak_dbfs(),
                problems,
                residual: 0.0,
                worst: None,
                differing: 0,
                same_hash: false,
            }
        }
    }
}

/// References in `dir` that belong to no fixture: manifest entries, and
/// FLAC files.
///
/// # Errors
///
/// If the manifest or the folder can't be read.
pub fn stale(dir: &Path) -> Result<Vec<String>, String> {
    let manifest = Manifest::load(dir)?;
    let mut stale: Vec<String> = manifest
        .references
        .keys()
        .filter(|name| fixture(name).is_none())
        .map(|name| format!("{MANIFEST}: {name}"))
        .collect();
    let entries =
        fs::read_dir(dir).map_err(|error| format!("can't read {}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("can't read {}: {error}", dir.display()))?
            .path();
        let flac = path.extension().is_some_and(|e| e == "flac");
        let name = path.file_stem().and_then(|stem| stem.to_str());
        if flac && name.is_none_or(|name| fixture(name).is_none()) {
            stale.push(path.display().to_string());
        }
    }
    stale.sort();
    Ok(stale)
}

/// What `--update` did to one reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The render matches the reference to the bit: nothing to do.
    Unchanged,
    /// The 24-bit samples match, but the float render differs below them:
    /// only the manifest's hash (and platform) changed.
    Hash,
    /// The reference was made or remade.
    Written,
    /// The reference belonged to no fixture and was removed.
    Removed,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unchanged => "unchanged",
            Self::Hash => "same 24-bit samples; the manifest's render_hash and made_on updated",
            Self::Written => "reference written",
            Self::Removed => "removed: no fixture has this name",
        })
    }
}

/// Makes or remakes the references of `selected` fixtures in `dir`, where
/// their renders changed, and records this platform as the one that made
/// them. With every fixture selected, it also removes references that
/// belong to none.
///
/// # Errors
///
/// If a reference or the manifest can't be read or written.
pub fn update(dir: &Path, selected: &[&Fixture]) -> Result<Vec<(String, Change)>, String> {
    fs::create_dir_all(dir).map_err(|error| format!("can't create {}: {error}", dir.display()))?;
    let mut manifest = Manifest::load(dir)?;
    manifest.about = ABOUT.to_owned();
    let mut changes = Vec::new();
    for fixture in selected {
        let render = fixture.render();
        let problems = render.problems();
        if !problems.is_empty() {
            return Err(format!(
                "{} can't be a reference: {}",
                fixture.name,
                problems.join("; ")
            ));
        }
        let path = fixture.reference_path(dir);
        let entry = Entry::for_render(&render);
        let old = manifest.references.get(fixture.name);
        let same_samples = read_flac(&path).is_ok_and(|reference| {
            reference.sample_rate == entry.sample_rate
                && reference.channels == entry.channels
                && reference.bits == BITS
                && reference.samples == render.quantised()
        });
        // A render that matches its reference to the bit leaves it alone,
        // even on another platform.
        let matches = |old: &Entry| {
            Entry {
                made_on: entry.made_on.clone(),
                ..old.clone()
            } == entry
        };
        let change = match old {
            Some(old) if same_samples && matches(old) => Change::Unchanged,
            Some(_) if same_samples => Change::Hash,
            _ => {
                write_flac(&path, &render)?;
                Change::Written
            }
        };
        if change != Change::Unchanged {
            manifest.references.insert(fixture.name.to_owned(), entry);
        }
        changes.push((fixture.name.to_owned(), change));
    }
    if selected.len() == fixtures().len() {
        let unknown: Vec<String> = manifest
            .references
            .keys()
            .filter(|name| fixture(name).is_none())
            .cloned()
            .collect();
        for name in unknown {
            manifest.references.remove(&name);
            let _ = fs::remove_file(dir.join(format!("{name}.flac")));
            changes.push((name, Change::Removed));
        }
    }
    manifest.save(dir)?;
    Ok(changes)
}

#[derive(Debug, clap::Args)]
pub struct GoldenArgs {
    /// The folder holding the references and their manifest.
    #[arg(long, default_value = DIR)]
    pub dir: PathBuf,

    /// Remake the references whose renders changed, and make missing ones,
    /// recording this platform as the one that made them. A pull request
    /// that changes a reference must say why.
    #[arg(long)]
    pub update: bool,

    /// The fixtures to render. Defaults to all of them.
    pub fixtures: Vec<String>,
}

/// Runs `bl-cli golden`.
///
/// # Errors
///
/// A message if a fixture name is unknown or a file can't be read or
/// written. The exit code is a failure if any fixture fails its null test.
pub fn run(args: &GoldenArgs) -> Outcome {
    let selected: Vec<&Fixture> = if args.fixtures.is_empty() {
        fixtures().iter().collect()
    } else {
        args.fixtures
            .iter()
            .map(|name| {
                fixture(name).ok_or_else(|| {
                    let names: Vec<&str> = fixtures().iter().map(|f| f.name).collect();
                    format!(
                        "there's no fixture called {name}; there are {}",
                        names.join(", ")
                    )
                })
            })
            .collect::<Result<_, _>>()?
    };

    if args.update {
        for (name, change) in update(&args.dir, &selected)? {
            println!("{name}: {change}");
        }
        println!();
    }

    let manifest = Manifest::load(&args.dir)?;
    println!(
        "Rendering on {}, against the references in {}",
        platform(),
        args.dir.display()
    );
    println!();
    println!(
        "{:<20} {:>8} {:>10} {:>14}  result",
        "fixture", "frames", "peak dBFS", "residual dBFS"
    );
    let mut failures = Vec::new();
    for fixture in &selected {
        let null = check_fixture(fixture, &args.dir, &manifest);
        let frames = manifest
            .references
            .get(fixture.name)
            .map_or_else(|| "-".to_owned(), |entry| entry.frames.to_string());
        let result = if !null.passed() {
            "FAILED"
        } else if null.bit_exact() {
            "bit-exact"
        } else {
            "below threshold"
        };
        println!(
            "{:<20} {:>8} {:>10.1} {:>14.1}  {result}",
            fixture.name,
            frames,
            null.peak_dbfs,
            null.residual_dbfs()
        );
        if !null.passed() {
            failures.push((null, fixture.about));
        }
    }
    let stale = if selected.len() == fixtures().len() {
        stale(&args.dir)?
    } else {
        Vec::new()
    };
    println!();
    println!(
        "{} fixtures: {} passed, {} failed",
        selected.len(),
        selected.len() - failures.len(),
        failures.len()
    );
    for (failure, about) in &failures {
        eprintln!("{failure}\n  The fixture: {about}");
    }
    for name in &stale {
        eprintln!("stale reference, no fixture: {name}");
    }
    Ok(if failures.is_empty() && stale.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
