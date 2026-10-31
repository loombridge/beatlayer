//! Wavetables for Wavetable (WP-8.8; the wavetable spec, R1 to R10):
//! Beatlayer's own factory tables from [`generate`], in categories; user
//! tables from a WAV or AIFF file ([`user`]); the choosers' lists; and the
//! worker that builds a table off the audio thread into the kit's
//! process-wide cache, shared by every device that uses it.
//!
//! No table is stored as audio: a factory table is made the first time
//! something asks for it.

mod generate;
pub mod user;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use lspk_wavetable::{Cache, Interpolation, SharedTable, Wavetable};
use rtrb::{Consumer, Producer, RingBuffer};

use generate::Frame;

/// The harness device type `device.load_wavetable` loads into until
/// Wavetable exists (WP-8.9).
pub const BENCH: &str = "test.framework.wavetable";

/// Whether the native device type `device_type` plays wavetables that
/// `device.load_wavetable` loads into it.
#[must_use]
pub fn plays_wavetables(device_type: &str) -> bool {
    device_type == BENCH
}

/// The factory categories, in the order the chooser lists them (R1).
pub const CATEGORIES: [&str; 7] = [
    "Shape Morphs",
    "Additive",
    "Pulse and Sync",
    "FM",
    "Voices",
    "Bit and Step",
    "Grain and Noise",
];

/// One factory table: where it is listed, how many frames it draws, how the
/// frames between them are made, and the formula for a drawn frame.
#[derive(Clone, Copy)]
pub struct Factory {
    /// The category it is listed in.
    pub category: &'static str,
    /// Its name in the category.
    pub name: &'static str,
    /// The frames it draws, up to 256.
    pub frames: usize,
    /// How the frames between drawn frames are made (R1, R9).
    pub interpolation: Interpolation,
    /// Frame `frame` of `frames`.
    make: fn(usize, usize) -> Frame,
}

const fn table(
    category: &'static str,
    name: &'static str,
    frames: usize,
    interpolation: Interpolation,
    make: fn(usize, usize) -> Frame,
) -> Factory {
    Factory {
        category,
        name,
        frames,
        interpolation,
        make,
    }
}

use Interpolation::{Crossfade, None as Plain, Spectral};
use generate as g;

static FACTORY: [Factory; 26] = [
    table("Shape Morphs", "Sine to Saw", 64, Plain, g::sine_to_saw),
    table("Shape Morphs", "Saw to Square", 64, Plain, g::saw_to_square),
    table(
        "Shape Morphs",
        "Square to Triangle",
        64,
        Plain,
        g::square_to_triangle,
    ),
    table("Shape Morphs", "Sine Folds", 48, Plain, g::sine_folds),
    table("Additive", "Odd Rise", 32, Plain, g::odd_rise),
    table("Additive", "Harmonic Ladder", 32, Plain, g::harmonic_ladder),
    table("Additive", "Falling Bells", 48, Plain, g::falling_bells),
    table("Additive", "Octave Stack", 48, Plain, g::octave_stack),
    table(
        "Pulse and Sync",
        "Pulse Width Sweep",
        48,
        Plain,
        g::pulse_width_sweep,
    ),
    table(
        "Pulse and Sync",
        "Sync Saw Sweep",
        64,
        Plain,
        g::sync_saw_sweep,
    ),
    table(
        "Pulse and Sync",
        "Sync Sine Sweep",
        64,
        Plain,
        g::sync_sine_sweep,
    ),
    table("Pulse and Sync", "Twin Pulses", 48, Plain, g::twin_pulses),
    table("FM", "Index Sweep", 64, Plain, g::index_sweep),
    table("FM", "Ratio Steps", 8, Crossfade, g::ratio_steps),
    table("FM", "Feedback Bloom", 48, Plain, g::feedback_bloom),
    table("Voices", "Vowel Walk", 10, Spectral, g::vowel_walk),
    table("Voices", "Front Vowels", 4, Spectral, g::front_vowels),
    table("Voices", "Back Vowels", 4, Spectral, g::back_vowels),
    table("Voices", "Formant Glide", 48, Plain, g::formant_glide),
    table(
        "Bit and Step",
        "Bit Crush Sweep",
        48,
        Plain,
        g::bit_crush_sweep,
    ),
    table("Bit and Step", "Stair Rise", 32, Crossfade, g::stair_rise),
    table("Bit and Step", "Hold Squeeze", 48, Plain, g::hold_squeeze),
    table(
        "Grain and Noise",
        "Noise Smear",
        12,
        Crossfade,
        g::noise_smear,
    ),
    table("Grain and Noise", "Rough Sine", 48, Plain, g::rough_sine),
    table(
        "Grain and Noise",
        "Scatter Pulses",
        24,
        Plain,
        g::scatter_pulses,
    ),
    table("Grain and Noise", "Grit Drift", 32, Plain, g::grit_drift),
];

/// Every factory table, by category in [`CATEGORIES`]' order, and by name
/// within each (R5).
#[must_use]
pub fn factory_tables() -> Vec<Factory> {
    let mut all = FACTORY.to_vec();
    all.sort_by_key(|t| {
        let category = CATEGORIES.iter().position(|&c| c == t.category);
        (category, t.name)
    });
    all
}

/// The factory tables of `category`, in name order (R5).
#[must_use]
pub fn tables_in(category: &str) -> Vec<Factory> {
    factory_tables()
        .into_iter()
        .filter(|t| t.category == category)
        .collect()
}

/// The factory table named `name` in `category`.
#[must_use]
pub fn factory(category: &str, name: &str) -> Option<Factory> {
    FACTORY
        .iter()
        .find(|t| t.category == category && t.name == name)
        .copied()
}

/// The factory table `steps` places after `category` and `name` in the
/// chooser's order, crossing into the next category at a category's end
/// and stopping at the first and last tables (R5).
#[must_use]
pub fn step(category: &str, name: &str, steps: isize) -> Option<Factory> {
    let all = factory_tables();
    let at = all
        .iter()
        .position(|t| t.category == category && t.name == name)?;
    let to = at.saturating_add_signed(steps).min(all.len() - 1);
    Some(all[to])
}

impl Factory {
    /// The table as drawn, made from its formula (R1, R2).
    ///
    /// # Panics
    ///
    /// Never for a table in the list: each draws 1 to 256 frames.
    #[must_use]
    pub fn source(&self) -> Wavetable {
        let frames: Vec<Frame> = (0..self.frames)
            .map(|i| (self.make)(i, self.frames))
            .collect();
        Wavetable::new(&frames)
            .expect("a factory table draws 1 to 256 frames")
            .with_interpolation(self.interpolation)
    }

    /// The playable table, built the first time anything asks for it and
    /// shared through the kit's cache after (R3). Not for the audio thread.
    #[must_use]
    pub fn shared(&self) -> SharedTable {
        let key = format!("factory:{}/{}", self.category, self.name);
        Cache::global()
            .load(&key, || Ok(self.source()))
            .expect("a factory table is made without error")
    }
}

/// A hash of a table's drawn frames and interpolation that is the same on
/// every platform, for R2's test (FNV-1a over the samples' bits).
#[must_use]
pub fn hash(table: &Wavetable) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |byte: u8| hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    feed(table.interpolation().digit());
    for frame in table.frames() {
        for x in frame.iter() {
            x.to_bits().to_le_bytes().into_iter().for_each(&mut feed);
        }
    }
    hash
}

/// The WAV and AIFF files beside `file`, in name order: what the choosers
/// list once a user table has loaded from it (R7).
#[must_use]
pub fn folder_files(file: &Path) -> Vec<PathBuf> {
    let Some(folder) = file.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["wav", "aif", "aiff"]
                    .iter()
                    .any(|k| e.eq_ignore_ascii_case(k))
            })
        })
        .collect();
    files.sort_by_key(|path| path.file_name().map(std::ffi::OsStr::to_ascii_lowercase));
    files
}

/// What an oscillator's table is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A factory table.
    Factory {
        /// Its category.
        category: String,
        /// Its name.
        name: String,
    },
    /// A user table's file.
    File {
        /// The file.
        path: PathBuf,
        /// Whether its frames are used as they are (R6).
        raw: bool,
    },
}

impl Source {
    /// Builds the table: a factory table through the cache, a file read and
    /// cut into frames. Not for the audio thread.
    ///
    /// # Errors
    ///
    /// Why the table can't be made: an unknown factory table, or a file
    /// that can't be read or has no audio.
    pub fn build(&self) -> Result<SharedTable, String> {
        match self {
            Self::Factory { category, name } => factory(category, name)
                .map(|t| t.shared())
                .ok_or_else(|| format!("no wavetable {name} in {category}")),
            Self::File { path, raw } => Ok(Cache::global().share(user::read(path, *raw)?)),
        }
    }
}

/// A table built for an oscillator by a [`Loader`] worker.
pub struct Built {
    /// The oscillator it is for, 1 or 2.
    pub oscillator: u8,
    /// The table to swap in.
    pub table: SharedTable,
    /// Which request made it: later requests have higher numbers.
    sequence: u64,
}

/// Starts workers that build tables off the audio thread (R3, R10).
#[derive(Clone)]
pub struct Loader {
    sink: Arc<Mutex<Producer<Built>>>,
    requests: Arc<AtomicU64>,
}

/// The audio thread's end of a [`Loader`]: lock-free, and nothing it does
/// frees a table (the cache owns them until its collector runs).
pub struct Ready {
    built: Consumer<Built>,
    /// The latest request each oscillator has had a table for.
    newest: [u64; 2],
}

/// A [`Loader`] and the [`Ready`] that hears from its workers.
#[must_use]
pub fn loader() -> (Loader, Ready) {
    let (producer, consumer) = RingBuffer::new(8);
    let loader = Loader {
        sink: Arc::new(Mutex::new(producer)),
        requests: Arc::new(AtomicU64::new(0)),
    };
    let ready = Ready {
        built: consumer,
        newest: [0; 2],
    };
    (loader, ready)
}

impl Loader {
    /// Builds `source` on a new thread; when it is ready it reaches the
    /// [`Ready`] as `oscillator`'s table. A source that can't be built
    /// reaches nothing, so the oscillator keeps playing its previous table.
    pub fn request(&self, oscillator: u8, source: Source) {
        let sink = Arc::clone(&self.sink);
        let sequence = self.requests.fetch_add(1, Ordering::Relaxed) + 1;
        thread::spawn(move || {
            if let Ok(table) = source.build() {
                // A full queue drops the table: the oscillator is then
                // behind by a load, and the next one catches it up.
                let _ = sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(Built {
                        oscillator,
                        table,
                        sequence,
                    });
            }
        });
    }
}

impl Ready {
    /// The next table a worker finished, if one did. A table whose request
    /// a later one has overtaken is dropped, so a slow build never replaces
    /// a newer choice. For the audio thread.
    pub fn poll(&mut self) -> Option<Built> {
        while let Ok(built) = self.built.pop() {
            let newest = self
                .newest
                .get_mut(usize::from(built.oscillator).wrapping_sub(1));
            match newest {
                Some(newest) if built.sequence < *newest => {}
                Some(newest) => {
                    *newest = built.sequence;
                    return Some(built);
                }
                None => return Some(built),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow build of an older request never replaces a newer table.
    #[test]
    fn an_older_request_finishing_late_is_dropped() {
        let (loader, mut ready) = loader();
        let table = |name| factory("Additive", name).unwrap().shared();
        let built = |sequence, name| Built {
            oscillator: 1,
            table: table(name),
            sequence,
        };
        {
            let mut sink = loader.sink.lock().unwrap();
            sink.push(built(2, "Odd Rise")).ok().unwrap();
            sink.push(built(1, "Falling Bells")).ok().unwrap();
        }
        let first = ready.poll().expect("the newer one");
        assert_eq!(first.sequence, 2);
        assert!(ready.poll().is_none(), "the older one is dropped");
    }
}
