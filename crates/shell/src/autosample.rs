//! The autosampler (WP-5.16, IMP-27; the multisampler spec, R64 to R73).
//!
//! This module holds the plan: which notes are sampled, at which
//! velocities, the zones the samples take and the round robins among them
//! (R66), and the names the sample files get (R69). The run itself is in
//! [`crate::shell::autosample`].

use bl_model::{
    Clip, ClipData, ClipId, Command, DeviceId, Document, Editor, Layer, MidiData, Note, TrackId,
    TrackKind, Zone,
};

/// The settings the autosampler's dialog holds (R65).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The lowest note sampled.
    pub lowest: u8,
    /// The highest note sampled.
    pub highest: u8,
    /// How many semitones between the sampled notes.
    pub step: u8,
    /// How many velocity layers each note is sampled at.
    pub velocity_layers: u8,
    /// How long each note is held, in seconds.
    pub note_length: f64,
    /// How long the render goes on after the note is released, in seconds.
    pub release_tail: f64,
    /// How many times each note is sampled, for the round robins.
    pub round_roins: u8,
    /// Whether a loop is found in each sample.
    pub detect_loops: bool,
    /// The name of the instrument the samples are named for.
    pub name: String,
}

impl Default for Settings {
    /// The defaults R65 gives: C1 to C6, a step of 3, one velocity layer,
    /// notes of 2 s, tails of 1 s, one round robin and no loops.
    fn default() -> Self {
        Self {
            lowest: 36,
            highest: 84,
            step: 3,
            velocity_layers: 1,
            note_length: 2.0,
            release_tail: 1.0,
            round_roins: 1,
            detect_loops: false,
            name: String::new(),
        }
    }
}

impl Settings {
    /// The settings, with every value inside its range (R65).
    #[must_use]
    pub fn checked(mut self) -> Self {
        self.lowest = self.lowest.min(127);
        self.highest = self.highest.min(127).max(self.lowest);
        self.step = self.step.clamp(1, 12);
        self.velocity_layers = self.velocity_layers.clamp(1, 16);
        self.note_length = self.note_length.clamp(0.1, 30.0);
        self.release_tail = self.release_tail.clamp(0.0, 30.0);
        self.round_roins = self.round_roins.clamp(1, 8);
        self
    }

    /// How long one sample is rendered for, in seconds: the note held, and
    /// the tail after it.
    #[must_use]
    pub fn sample_seconds(&self) -> f64 {
        self.note_length + self.release_tail
    }
}

/// One sample to make: the note, the velocity, the zones it takes and
/// which round robin of them it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    /// The note played.
    pub note: u8,
    /// The velocity played at.
    pub velocity: u8,
    /// The notes it plays over.
    pub key: Zone,
    /// The velocities it plays at.
    pub velocity_zone: Zone,
    /// Which round robin of them it is, from 0.
    pub round_robin: u8,
}

/// The samples a set of settings makes (R66).
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The samples, in note, velocity and round robin order.
    pub samples: Vec<Planned>,
    /// The notes sampled, in order.
    pub notes: Vec<u8>,
}

/// The plan the settings make: the notes sampled, their velocities, and the
/// zones the samples take (R66).
#[must_use]
pub fn plan(settings: &Settings) -> Plan {
    let settings = settings.clone().checked();
    let notes = notes(&settings);
    let mut samples = Vec::new();
    for index in 0..notes.len() {
        let key = key_zone(&notes, index);
        for layer in 1..=settings.velocity_layers {
            let (velocity, velocity_zone) = velocity_layer(settings.velocity_layers, layer);
            for round_robin in 0..settings.round_roins {
                samples.push(Planned {
                    note: notes[index],
                    velocity,
                    key,
                    velocity_zone,
                    round_robin,
                });
            }
        }
    }
    Plan { samples, notes }
}

/// The notes sampled: the lowest one and every `step` above it up to the
/// highest, which is always sampled (R66).
fn notes(settings: &Settings) -> Vec<u8> {
    let mut notes = vec![settings.lowest];
    let mut note = u16::from(settings.lowest) + u16::from(settings.step);
    while note < u16::from(settings.highest) {
        notes.push(note as u8);
        note += u16::from(settings.step);
    }
    if *notes.last().unwrap_or(&settings.lowest) != settings.highest {
        notes.push(settings.highest);
    }
    notes
}

/// The key zone of the sampled note at `index`: it runs halfway to its
/// neighbours, and the lowest zone reaches down to 0 and the highest up to
/// 127, so every key plays (R66).
fn key_zone(notes: &[u8], index: usize) -> Zone {
    let low = match index {
        0 => 0,
        _ => {
            let previous = u16::from(notes[index - 1]);
            let note = u16::from(notes[index]);
            ((previous + note) / 2 + 1) as u8
        }
    };
    let high = match notes.get(index + 1) {
        Some(next) => ((u16::from(notes[index]) + u16::from(*next)) / 2) as u8,
        None => 127,
    };
    Zone {
        low,
        high,
        fade_low: 0,
        fade_high: 0,
    }
}

/// The velocity of the `layer`th of `layers` velocity layers, and the
/// velocity zone it takes (R66).
fn velocity_layer(layers: u8, layer: u8) -> (u8, Zone) {
    let layers = f64::from(layers);
    let layer = f64::from(layer);
    let high = (127.0 * layer / layers).round() as u8;
    let low = (127.0 * (layer - 1.0) / layers).round() as u8 + 1;
    (
        high,
        Zone {
            low,
            high,
            fade_low: 0,
            fade_high: 0,
        },
    )
}

/// The name of a sample's file, without its extension (R69): the
/// instrument's name, the note and the velocity, and the round robin when
/// there is more than one.
#[must_use]
pub fn file_name(settings: &Settings, sample: &Planned) -> String {
    let mut name = format!(
        "{} {} v{}",
        settings.name,
        bl_model::note_name(sample.note),
        sample.velocity
    );
    if settings.round_roins > 1 {
        name.push_str(&format!(" rr{}", sample.round_robin + 1));
    }
    name
}

/// A layer for a sample, with the zones the plan gives it (R66, R69).
#[must_use]
pub fn layer(sample: &Planned, name: &str, media: bl_model::ContentHash, length: u64, rate: u32, channels: u8) -> Layer {
    let mut layer = Layer::new(bl_model::LayerId(0), name, media, length, rate, channels);
    layer.key = sample.key;
    layer.velocity = sample.velocity_zone;
    layer.root_key = sample.note;
    layer
}

/// The document that renders an instrument alone with one note (R67): a
/// MIDI track with the instrument and the note, and nothing else, so the
/// devices around the instrument on its own track don't play.
pub fn render_document(
    source: &Document,
    device: DeviceId,
    note: u8,
    velocity: u8,
    settings: &Settings,
) -> Result<Document, String> {
    let instrument = source
        .device(device)
        .cloned()
        .ok_or_else(|| "the instrument is gone".to_owned())?;
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: instrument.name.clone(),
            parent: None,
            index: 0,
        })
        .map_err(|error| error.to_string())?;
    let id: DeviceId = editor.new_id();
    let mut device = instrument.clone();
    device.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .map_err(|error| error.to_string())?;
    let tempo = editor.document().tempo_map();
    let held = tempo.seconds_to_ticks(settings.note_length);
    let length = tempo.seconds_to_ticks(settings.sample_seconds());
    let clip: ClipId = editor.new_id();
    let notes = [Note::new(note, bl_core::time::Ticks::ZERO, held, velocity)];
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                bl_core::time::Ticks::ZERO,
                length,
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .map_err(|error| error.to_string())?;
    Ok(editor.document().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings the acceptance uses: three octaves at a step of 3 with
    /// two velocity layers.
    fn settings() -> Settings {
        Settings {
            lowest: 36,
            highest: 84,
            step: 3,
            velocity_layers: 2,
            ..Settings::default()
        }
    }

    #[test]
    fn the_plan_samples_each_step_and_spreads_the_zones() {
        let plan = plan(&settings());
        // C1 to C6 at a step of 3: the highest note is always sampled.
        assert_eq!(plan.notes.first(), Some(&36));
        assert_eq!(plan.notes.last(), Some(&84));
        assert!(plan.notes.windows(2).all(|pair| pair[1] > pair[0]));
        assert_eq!(plan.notes.len(), 17);
        // Two velocity layers for each note.
        assert_eq!(plan.samples.len(), 34);
        let first = &plan.samples[0];
        assert_eq!(first.note, 36);
        assert_eq!(first.velocity, 64);
        assert_eq!((first.velocity_zone.low, first.velocity_zone.high), (1, 64));
        let second = &plan.samples[1];
        assert_eq!(second.velocity, 127);
        assert_eq!((second.velocity_zone.low, second.velocity_zone.high), (65, 127));
        // The key zones run halfway to the neighbours, so every key plays.
        assert_eq!((first.key.low, first.key.high), (0, 37));
        let middle = &plan.samples[2];
        assert_eq!((middle.key.low, middle.key.high), (38, 40));
        let last = &plan.samples[plan.samples.len() - 2];
        assert_eq!((last.key.low, last.key.high), (83, 127));
        // Every key is in exactly one zone.
        let mut covered = vec![0u8; 128];
        for sample in plan.samples.iter().filter(|s| s.velocity == 64) {
            for key in sample.key.low..=sample.key.high {
                covered[usize::from(key)] += 1;
            }
        }
        assert!(covered.iter().all(|count| *count == 1), "{covered:?}");
    }

    #[test]
    fn the_round_robins_share_their_zones() {
        let mut settings = settings();
        settings.round_roins = 3;
        let plan = plan(&settings);
        assert_eq!(plan.samples.len(), 51);
        let first = &plan.samples[0];
        assert_eq!(first.round_robin, 0);
        assert_eq!(plan.samples[1].round_robin, 1);
        assert_eq!(plan.samples[2].round_robin, 2);
        assert_eq!(plan.samples[0].key, plan.samples[1].key);
        assert_eq!(plan.samples[0].note, plan.samples[2].note);
    }

    #[test]
    fn the_files_are_named_for_the_instrument_the_note_and_the_velocity() {
        let mut settings = settings();
        settings.name = "Drift".to_owned();
        settings.round_roins = 2;
        let plan = plan(&settings);
        assert_eq!(file_name(&settings, &plan.samples[0]), "Drift C1 v64");
        assert_eq!(file_name(&settings, &plan.samples[1]), "Drift C1 v64 rr1");
        assert_eq!(file_name(&settings, &plan.samples[2]), "Drift C1 v127");
        assert_eq!(file_name(&settings, &plan.samples[3]), "Drift C1 v127 rr1");
    }
}
