//! Snapshot, the native sampler (WP-2.16, part 1; the sampler spec):
//! Classic and One-Shot playback of one sample, its region and loop with a
//! constant-power crossfade, Transpose, Detune, Glide and Portamento, the
//! amplitude envelope, velocity and pan, and voices with stealing.
//!
//! The sample is decoded audio the media crate gives the engine, sent to
//! this node as node data (the sampler spec, R11): the compiler opens it
//! when the node is made, and the sampler reads it at the positions its
//! voices need, so nothing here reads a file or allocates.

use std::sync::Arc;

use bl_engine::{DeviceType, NodeData, NoteEvent, NoteKind, Registry};
use bl_media::SampleData;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// The sampler device type's id.
pub const SAMPLER: &str = "beatlayer.snapshot";

/// Classic (polyphonic, looping) or One-Shot (monophonic, straight
/// through).
pub const MODE: ParamId = ParamId(0);
/// Where the played region starts, as a share of the sample.
pub const START: ParamId = ParamId(1);
/// How long the played region is, as a share of the sample.
pub const LENGTH: ParamId = ParamId(2);
/// Whether the played region's end loops into its start.
pub const LOOP: ParamId = ParamId(3);
/// How much of the played region loops, counted back from its end.
pub const LOOP_LENGTH: ParamId = ParamId(4);
/// The constant-power crossfade at the loop's wrap, as a share of the
/// loop.
pub const FADE: ParamId = ParamId(5);
/// The sample's own gain stage.
pub const GAIN: ParamId = ParamId(6);
/// Transposes the sample, in whole semitones.
pub const TRANSPOSE: ParamId = ParamId(7);
/// Fine-tunes the sample, in cents.
pub const DETUNE: ParamId = ParamId(8);
/// How many voices Classic plays at once.
pub const VOICES: ParamId = ParamId(9);
/// Whether a note already sounding on the same key is cut off by a new
/// one.
pub const RETRIG: ParamId = ParamId(10);
/// The amplitude envelope's attack time.
pub const ATTACK: ParamId = ParamId(11);
/// The amplitude envelope's decay time.
pub const DECAY: ParamId = ParamId(12);
/// The amplitude envelope's sustain level.
pub const SUSTAIN: ParamId = ParamId(13);
/// The amplitude envelope's release time.
pub const RELEASE: ParamId = ParamId(14);
/// The output level.
pub const VOLUME: ParamId = ParamId(15);
/// Where the voice sits in the stereo field.
pub const PAN: ParamId = ParamId(16);
/// How much note velocity scales the volume.
pub const VELOCITY: ParamId = ParamId(17);
/// Whether a new note slides from the last note's pitch.
pub const GLIDE: ParamId = ParamId(18);
/// Glide (monophonic) or Portamento (polyphonic).
pub const GLIDE_MODE: ParamId = ParamId(19);
/// How long a glide takes.
pub const GLIDE_TIME: ParamId = ParamId(20);
/// One-Shot: whether a note's end is ignored (on) or fades the voice out.
pub const TRIGGER: ParamId = ParamId(21);
/// One-Shot: the fade in at the region's start.
pub const FADE_IN: ParamId = ParamId(22);
/// One-Shot: the fade out ending at the region's end, and a note's end's
/// fade with Trigger off.
pub const FADE_OUT: ParamId = ParamId(23);

/// The Mode choices: Classic then One-Shot.
pub const MODES: [&str; 2] = ["Classic", "One-Shot"];

/// The Glide Mode choices: Glide (monophonic) then Portamento
/// (polyphonic).
pub const GLIDE_MODES: [&str; 2] = ["Glide", "Portamento"];

/// How many voices Classic plays at once at most (the Voices parameter's
/// top).
pub const MAX_VOICES: usize = 16;

/// The slots a voice can sound in: [`MAX_VOICES`] sounding voices and room
/// for the ones being stolen while they fade out. A note that finds every
/// slot busy takes the oldest fading one, which is nearly silent, so a
/// fade is cut short rather than a sounding voice.
const SLOTS: usize = 24;

/// How long a stolen or choked voice fades out, in seconds (Beatlayer's
/// own; the manual only says stealing is made subtle).
const STEAL_FADE_SECONDS: f64 = 0.005;

/// The envelope's longest stage, in seconds: the top of the Attack, Decay
/// and Release ranges.
const ENVELOPE_MAX_SECONDS: f64 = 20.0;

/// The one-shot fades' longest, in seconds: the top of their range.
const FADE_MAX_SECONDS: f64 = 2.0;

/// The longest glide, in seconds: the top of its range.
const GLIDE_MAX_SECONDS: f64 = 1.0;

/// The Snapshot device type.
#[must_use]
pub fn sampler_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
            "Classic plays the region polyphonically with the loop and the envelope; \
                       One-Shot plays it monophonically straight through, whatever the note \
                       does.",
        ),
        ParamDescriptor::continuous(START, "Start", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Where in the sample the played region starts, as a share of it."),
        ParamDescriptor::continuous(LENGTH, "Length", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("How long the played region is, as a share of the sample from Start."),
        ParamDescriptor::toggle(LOOP, "Loop", false)
            .describe("Loops the region's end into its start for as long as a voice sounds."),
        ParamDescriptor::continuous(
            LOOP_LENGTH,
            "Loop Length",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe(
            "How much of the played region the loop repeats, counted back from the \
                   region's end; the whole region at 100 %.",
        ),
        ParamDescriptor::continuous(FADE, "Fade", (0.0, 100.0), 0.0, ParamUnit::Percent).describe(
            "Crossfades the loop's end into its start over this share of the loop, so \
                       the wrap doesn't click.",
        ),
        ParamDescriptor::gain(GAIN, "Gain", 24.0, 0.0).describe(
            "The sample's own gain stage, before the volume; reaches silence at its \
                       bottom.",
        ),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-48, 48), 0, ParamUnit::Semitones)
            .describe(
                "Transposes the sample in whole semitones; C3 plays it at its own \
                       pitch.",
            ),
        ParamDescriptor::continuous(DETUNE, "Detune", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Fine-tunes the sample in cents."),
        ParamDescriptor::integer(VOICES, "Voices", (1, MAX_VOICES as i64), 8, ParamUnit::None)
            .describe(
                "How many voices Classic plays at once; past it the oldest voice is faded \
                       out in favor of the new note.",
            ),
        ParamDescriptor::toggle(RETRIG, "Retrig", false).describe(
            "Cuts a note already sounding on the same key off when it plays again, \
                       instead of letting the two overlap.",
        ),
        ParamDescriptor::continuous(
            ATTACK,
            "Attack",
            (0.0, ENVELOPE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe(
            "The time the amplitude envelope takes to reach its peak after a note \
                   starts.",
        ),
        ParamDescriptor::continuous(
            DECAY,
            "Decay",
            (0.0, ENVELOPE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("The time the amplitude envelope takes to fall to the sustain level."),
        ParamDescriptor::continuous(SUSTAIN, "Sustain", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The level the amplitude envelope holds while the note sounds."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release",
            (0.0, ENVELOPE_MAX_SECONDS * 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .describe(
            "The time the amplitude envelope takes to fall from the sustain level to \
                   silence after the note ends.",
        ),
        ParamDescriptor::gain(VOLUME, "Volume", 24.0, 0.0)
            .describe("The instrument's output level; reaches silence at its bottom."),
        ParamDescriptor::continuous(PAN, "Pan", (-50.0, 50.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Where the voice sits in the stereo field."),
        ParamDescriptor::continuous(VELOCITY, "Velocity", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "How much a note's velocity scales its volume: at 0 it doesn't, at 100 % \
                       the level follows it.",
            ),
        ParamDescriptor::toggle(GLIDE, "Glide", false).describe(
            "Starts a new note from the last note's pitch and slides it to its \
                       own.",
        ),
        ParamDescriptor::choice(GLIDE_MODE, "Glide Mode", &GLIDE_MODES, 0).describe(
            "Glide takes the sounding voice over, monophonically; Portamento gives \
                       each note a voice of its own.",
        ),
        ParamDescriptor::continuous(
            GLIDE_TIME,
            "Glide Time",
            (0.0, GLIDE_MAX_SECONDS * 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long a glide from one pitch to the next takes."),
        ParamDescriptor::toggle(TRIGGER, "Trigger", true).describe(
            "One-Shot: plays the region to its end after the note is released; off, \
                       the note's end fades the voice out.",
        ),
        ParamDescriptor::continuous(
            FADE_IN,
            "Fade In",
            (0.0, FADE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe(
            "One-Shot: how long the region takes to reach its level after a note \
                   starts.",
        ),
        ParamDescriptor::continuous(
            FADE_OUT,
            "Fade Out",
            (0.0, FADE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe(
            "One-Shot: fades the region out over this time, ending at its end, and with \
                   Trigger off, fades the voice out when the note ends.",
        ),
    ];
    DeviceType::native(SAMPLER, "Snapshot", DeviceRole::Instrument)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// A layout of fixed control bands and the sample's display, which grows.
fn layout() -> DeviceLayout {
    let mode = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (MODE, Widget::Chooser),
            (LOOP, Widget::Toggle),
            (RETRIG, Widget::Toggle),
            (GLIDE, Widget::Toggle),
            (TRIGGER, Widget::Toggle),
        ],
    );
    let sample = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (START, Widget::Knob),
            (LENGTH, Widget::Knob),
            (LOOP_LENGTH, Widget::Knob),
            (FADE, Widget::Knob),
            (GAIN, Widget::Knob),
        ],
    );
    let pitch = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (TRANSPOSE, Widget::Number),
            (DETUNE, Widget::Knob),
            (GLIDE_MODE, Widget::Chooser),
            (GLIDE_TIME, Widget::Knob),
            (VOICES, Widget::Number),
        ],
    );
    let amp = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (ATTACK, Widget::Knob),
            (DECAY, Widget::Knob),
            (SUSTAIN, Widget::Knob),
            (RELEASE, Widget::Knob),
        ],
    );
    let level = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (VOLUME, Widget::Knob),
            (PAN, Widget::Knob),
            (VELOCITY, Widget::Knob),
            (FADE_IN, Widget::Knob),
            (FADE_OUT, Widget::Knob),
        ],
    );
    // The sample's waveform has no drawing yet (WP-2.16, part 1): the
    // display cell is left for it.
    let display = DisplayCell::new("Sample", 160.0, BODY_HEIGHT, 1.0);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(mode),
        LayoutNode::Band(sample),
        LayoutNode::Cell(display),
        LayoutNode::Band(pitch),
        LayoutNode::Band(amp),
        LayoutNode::Band(level),
    ]))
}

/// Adds the sampler to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, sampler_type(), |_, _| Sampler::new());
}

/// Which playback mode a device is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Polyphonic, with the loop and the envelope.
    Classic,
    /// Monophonic, straight through the region.
    OneShot,
}

/// The sampler's state between blocks: the sample and its voices.
pub struct Sampler {
    /// The decoded sample: the media crate's audio, at the engine's rate.
    data: Option<NodeData>,
    voices: [Voice; SLOTS],
    /// How many notes have been played: voices are stolen oldest first.
    counter: u64,
    /// The pitch of the last note played, in semitones from C3: where a
    /// gliding voice starts.
    last_pitch: Option<f64>,
}

impl Sampler {
    fn new() -> Self {
        Self {
            data: None,
            voices: [Voice::idle(); SLOTS],
            counter: 0,
            last_pitch: None,
        }
    }

    /// The sample, if the media library gave one.
    fn sample(&self) -> Option<&SampleData> {
        self.data
            .as_ref()
            .and_then(|data| data.downcast_ref::<Arc<SampleData>>())
            .map(Arc::as_ref)
    }

    /// The settings one stretch of a block plays by.
    fn settings(&self, params: &ParamBank, rate: f64) -> Settings {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let mode = if params.value(at(MODE)) >= 0.5 {
            Mode::OneShot
        } else {
            Mode::Classic
        };
        let sample_frames = self.sample().map_or(0, |sample| sample.frames() as u64);
        let start = params.value(at(START)) / 100.0;
        let length = params.value(at(LENGTH)) / 100.0;
        let region_start = (start * sample_frames as f64) as u64;
        let region_end = ((start + length).min(1.0) * sample_frames as f64) as u64;
        let region_end = region_end.min(sample_frames);
        let loop_share = params.value(at(LOOP_LENGTH)) / 100.0;
        let loop_start =
            region_end - ((loop_share * (region_end.saturating_sub(region_start)) as f64) as u64);
        let fade = (params.value(at(FADE)) / 100.0 * (region_end - loop_start) as f64) as u32;
        // One-Shot has Fade In and Fade Out instead of the envelope: its
        // attack is Fade In and it holds at the peak.
        let (attack_ms, decay_ms, sustain) = if mode == Mode::OneShot {
            (params.value(at(FADE_IN)), 0.0, 1.0)
        } else {
            (
                params.value(at(ATTACK)),
                params.value(at(DECAY)),
                params.value(at(SUSTAIN)) / 100.0,
            )
        };
        Settings {
            mode,
            region_start,
            region_end,
            looping: mode == Mode::Classic && params.value(at(LOOP)) >= 0.5,
            loop_start,
            loop_end: region_end,
            fade,
            transpose: params.value(at(TRANSPOSE)),
            detune: params.value(at(DETUNE)),
            voices: params.value(at(VOICES)).max(1.0) as usize,
            retrig: params.value(at(RETRIG)) >= 0.5,
            glide: params.value(at(GLIDE)) >= 0.5,
            glide_portamento: params.value(at(GLIDE_MODE)) >= 0.5,
            glide_frames: frames_of(params.value(at(GLIDE_TIME)), rate),
            velocity: (params.value(at(VELOCITY)) / 100.0) as f32,
            attack_frames: frames_of(attack_ms, rate),
            decay_frames: frames_of(decay_ms, rate),
            sustain: sustain as f32,
            release_frames: frames_of(params.value(at(RELEASE)), rate),
            steal_frames: frames_of(STEAL_FADE_SECONDS * 1000.0, rate).max(1),
            trigger: params.value(at(TRIGGER)) >= 0.5,
            fade_out_frames: frames_of(params.value(at(FADE_OUT)), rate),
        }
    }

    /// Takes a note: starts, releases or chokes voices by the mode's rules.
    fn play(&mut self, note: NoteEvent, settings: &Settings) {
        match note.kind {
            NoteKind::On => self.note_on(note, settings),
            NoteKind::Off => self.note_off(note.key, settings),
            NoteKind::Choke => self.choke(note.key, settings),
        }
    }

    fn note_on(&mut self, note: NoteEvent, s: &Settings) {
        if s.region_end <= s.region_start {
            return;
        }
        let pitch = f64::from(note.key) - 60.0 + s.transpose + s.detune / 100.0;
        let velocity = note.velocity.clamp(0.0, 1.0);
        // Glide's monophonic mode takes the sounding voice over: the new
        // note glides from its pitch, keeping its position and envelope.
        if s.glide
            && !s.glide_portamento
            && let Some(voice) = self.newest_playing()
        {
            voice.key = note.key;
            voice.target = pitch;
            voice.step = if s.glide_frames == 0 {
                voice.pitch = pitch;
                voice.ratio = 2f64.powf(pitch / 12.0);
                0.0
            } else {
                (pitch - voice.pitch) / f64::from(s.glide_frames)
            };
            self.last_pitch = Some(pitch);
            return;
        }
        // One-Shot is monophonic: the new note fades any sounding voice
        // out. Classic fades a voice on the same key with Retrig on, and
        // past the voice limit the oldest sounding voice.
        if s.mode == Mode::OneShot {
            for voice in self.playing() {
                voice.fade(s);
            }
        } else {
            if s.retrig {
                for voice in self.playing() {
                    if voice.key == note.key {
                        voice.fade(s);
                    }
                }
            }
            let over = (self.playing().count() + 1).saturating_sub(s.voices);
            for _ in 0..over {
                if let Some(voice) = self.playing().min_by_key(|voice| voice.counter) {
                    voice.fade(s);
                }
            }
        }
        self.counter += 1;
        let counter = self.counter;
        let from = if s.glide {
            self.last_pitch.unwrap_or(pitch)
        } else {
            pitch
        };
        let Some(slot) = self.free_slot() else {
            return;
        };
        let mut voice = Voice::idle();
        voice.active = true;
        voice.key = note.key;
        voice.counter = counter;
        voice.position = s.region_start as f64;
        voice.pitch = from;
        voice.target = pitch;
        voice.step = if s.glide_frames == 0 {
            voice.pitch = pitch;
            0.0
        } else {
            (pitch - from) / f64::from(s.glide_frames)
        };
        voice.ratio = 2f64.powf(voice.pitch / 12.0);
        voice.velocity = velocity;
        voice.one_shot = s.mode == Mode::OneShot;
        voice.env.trigger(s);
        *slot = voice;
        self.last_pitch = Some(pitch);
    }

    fn note_off(&mut self, key: u8, s: &Settings) {
        for voice in self.voices.iter_mut() {
            if !voice.active || voice.fading || voice.key != key {
                continue;
            }
            if voice.one_shot {
                if !s.trigger {
                    voice.env.release(s.fade_out_frames);
                }
            } else {
                voice.env.release(s.release_frames);
            }
        }
    }

    fn choke(&mut self, key: u8, s: &Settings) {
        for voice in self.voices.iter_mut() {
            if voice.active && !voice.fading && voice.key == key {
                voice.fade(s);
            }
        }
    }

    /// The sounding voices: active and not already fading out.
    fn playing(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.voices.iter_mut().filter(|v| v.active && !v.fading)
    }

    /// The newest sounding voice: the one a glide takes over.
    fn newest_playing(&mut self) -> Option<&mut Voice> {
        let counter = self.playing().map(|v| v.counter).max()?;
        self.playing().find(|v| v.counter == counter)
    }

    /// A slot for a new voice: an idle one, else the oldest fading one,
    /// which is closest to silent.
    fn free_slot(&mut self) -> Option<&mut Voice> {
        let index = self.voices.iter().position(|v| !v.active).or_else(|| {
            let oldest = self
                .voices
                .iter()
                .filter(|v| v.fading)
                .map(|v| v.counter)
                .min()?;
            self.voices
                .iter()
                .position(|v| v.fading && v.counter == oldest)
        })?;
        self.voices.get_mut(index)
    }

    /// Renders one frame of every voice: their sample reads, their
    /// envelopes, and the loop's crossfade.
    fn render(&mut self, s: &Settings, gain: f32, volume: f32, pan: f32) -> (f32, f32) {
        let sample = match self
            .data
            .as_ref()
            .and_then(|data| data.downcast_ref::<Arc<SampleData>>())
        {
            Some(sample) if sample.frames() > 0 => sample.as_ref(),
            _ => {
                for voice in &mut self.voices {
                    voice.active = false;
                }
                return (0.0, 0.0);
            }
        };
        let (mut left, mut right) = (0.0f32, 0.0f32);
        let level = gain * volume;
        let b = (pan / 50.0).clamp(-1.0, 1.0);
        let (left_pan, right_pan) = (1.0 - b.max(0.0), 1.0 + b.min(0.0));
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            voice.env.step(s);
            if voice.env.stage == Stage::Idle {
                voice.active = false;
                continue;
            }
            let ratio = voice.ratio;
            let sample_value = read(sample, voice.position);
            let mut value = sample_value;
            // The loop's wrap: a constant-power crossfade from the loop's
            // end into its start over the Fade share of it.
            if s.looping && s.fade > 0 && s.loop_end > s.loop_start {
                let fade_start = s.loop_end as f64 - f64::from(s.fade);
                if voice.position >= fade_start {
                    let t = ((voice.position - fade_start) / f64::from(s.fade)) as f32;
                    let (out, in_) = (
                        (t * core::f32::consts::FRAC_PI_2).cos(),
                        (t * core::f32::consts::FRAC_PI_2).sin(),
                    );
                    let from_start =
                        read(sample, s.loop_start as f64 + (voice.position - fade_start));
                    value = (
                        value.0 * out + from_start.0 * in_,
                        value.1 * out + from_start.1 * in_,
                    );
                }
            }
            let mut voice_level = voice.env.level * (1.0 - s.velocity * (1.0 - voice.velocity));
            // One-Shot fades the region out over Fade Out, ending at its
            // end.
            if voice.one_shot && s.fade_out_frames > 0 {
                let fade_start = s.region_end as f64 - f64::from(s.fade_out_frames);
                if voice.position >= fade_start {
                    let t = ((s.region_end as f64 - voice.position) / f64::from(s.fade_out_frames))
                        .clamp(0.0, 1.0) as f32;
                    voice_level *= t;
                }
            }
            let voice_level = voice_level * level;
            left += value.0 * voice_level * left_pan;
            right += value.1 * voice_level * right_pan;
            // The voice moves on: its pitch first, then its position.
            if voice.step != 0.0 {
                voice.pitch += voice.step;
                if (voice.step > 0.0 && voice.pitch >= voice.target)
                    || (voice.step < 0.0 && voice.pitch <= voice.target)
                {
                    voice.pitch = voice.target;
                    voice.step = 0.0;
                }
                voice.ratio = 2f64.powf(voice.pitch / 12.0);
            }
            voice.position += ratio;
            if s.looping && s.loop_end > s.loop_start && voice.position >= s.loop_end as f64 {
                // The wrap resumes past the head the crossfade already
                // played, so the fade's end and the resumed audio meet.
                let length = (s.loop_end - s.loop_start) as f64;
                voice.position = s.loop_start as f64
                    + (voice.position + f64::from(s.fade) - s.loop_end as f64).rem_euclid(length);
            } else if voice.position >= s.region_end as f64 {
                voice.active = false;
            }
        }
        (left, right)
    }
}

impl NativeDevice for Sampler {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || self.sample().is_none() {
            return;
        }
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let rate = audio.sample_rate().as_f64();
        let settings = self.settings(params, rate);
        let gain = params.ramp(at(GAIN));
        let volume = params.ramp(at(VOLUME));
        let pan = params.ramp(at(PAN));
        let mut notes = audio.notes().peekable();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        for i in 0..frames {
            while notes.peek().is_some_and(|note| note.offset as usize <= i) {
                let note = notes.next().expect("peeked");
                self.play(note, &settings);
            }
            let (l, r) = self.render(
                &settings,
                gain.get(i).copied().unwrap_or(1.0),
                volume.get(i).copied().unwrap_or(1.0),
                pan.get(i).copied().unwrap_or(0.0),
            );
            left[i] = l;
            if let Some(right) = &mut right {
                right[i] = r;
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.data = Some(data);
    }
}

/// What one stretch plays by, worked out from the parameters and the
/// sample.
#[derive(Clone, Copy)]
struct Settings {
    mode: Mode,
    region_start: u64,
    region_end: u64,
    looping: bool,
    loop_start: u64,
    loop_end: u64,
    /// The crossfade at the loop's wrap, in frames.
    fade: u32,
    transpose: f64,
    detune: f64,
    voices: usize,
    retrig: bool,
    glide: bool,
    glide_portamento: bool,
    glide_frames: u32,
    /// How much velocity scales the level, 0 to 1.
    velocity: f32,
    attack_frames: u32,
    decay_frames: u32,
    sustain: f32,
    release_frames: u32,
    steal_frames: u32,
    trigger: bool,
    fade_out_frames: u32,
}

/// Where an amplitude envelope is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// One voice's amplitude envelope: a level moving in a straight line
/// through its stages.
#[derive(Clone, Copy)]
struct Env {
    stage: Stage,
    level: f32,
    from: f32,
    to: f32,
    /// Frames left in the current stage; 0 means the stage is done.
    left: u32,
    /// The stage's whole length, for the level's place in it.
    total: u32,
}

impl Env {
    fn idle() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            from: 0.0,
            to: 0.0,
            left: 0,
            total: 1,
        }
    }

    /// Starts a stage from the current level.
    fn enter(&mut self, stage: Stage, to: f32, frames: u32) {
        self.stage = stage;
        self.from = self.level;
        self.to = to;
        self.left = frames;
        self.total = frames.max(1);
        if frames == 0 {
            self.level = to;
        }
    }

    /// Starts the envelope at its attack.
    fn trigger(&mut self, s: &Settings) {
        self.level = 0.0;
        self.enter(Stage::Attack, 1.0, s.attack_frames);
    }

    /// Releases the envelope: the level falls to silence over `frames`.
    fn release(&mut self, frames: u32) {
        self.enter(Stage::Release, 0.0, frames);
    }

    /// Moves the envelope on by one frame, through any stage it finishes.
    fn step(&mut self, s: &Settings) {
        if self.stage == Stage::Idle {
            return;
        }
        if self.left > 0 {
            self.left -= 1;
            let place = 1.0 - self.left as f32 / self.total as f32;
            self.level = self.from + (self.to - self.from) * place;
        }
        while self.left == 0 {
            match self.stage {
                Stage::Attack => self.enter(Stage::Decay, s.sustain, s.decay_frames),
                Stage::Decay => self.enter(Stage::Sustain, s.sustain, 0),
                Stage::Sustain => break,
                Stage::Release => {
                    self.stage = Stage::Idle;
                    self.level = 0.0;
                    break;
                }
                Stage::Idle => break,
            }
        }
    }
}

/// One sounding sample position.
#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    /// Fading out because it was stolen or choked: not counted as a
    /// sounding voice.
    fading: bool,
    key: u8,
    /// When it started: the oldest voice is stolen first.
    counter: u64,
    /// Its position in the sample, in frames.
    position: f64,
    /// Its pitch in semitones from C3, and where a glide is taking it.
    pitch: f64,
    target: f64,
    /// Semitones per frame while gliding.
    step: f64,
    /// Its playback rate: sample frames per output frame.
    ratio: f64,
    velocity: f32,
    /// Whether it plays One-Shot's straight-through rules.
    one_shot: bool,
    env: Env,
}

impl Voice {
    fn idle() -> Self {
        Self {
            active: false,
            fading: false,
            key: 0,
            counter: 0,
            position: 0.0,
            pitch: 0.0,
            target: 0.0,
            step: 0.0,
            ratio: 1.0,
            velocity: 1.0,
            one_shot: false,
            env: Env::idle(),
        }
    }

    /// Fades it out over the steal fade, keeping its sample playing.
    fn fade(&mut self, s: &Settings) {
        self.fading = true;
        self.env.release(s.steal_frames);
    }
}

/// One sample frame at `position` in each channel: linear interpolation
/// between the neighbouring frames, silence past the end, and a mono sample
/// on both sides.
fn read(sample: &SampleData, position: f64) -> (f32, f32) {
    let frames = sample.frames();
    if frames == 0 || position < 0.0 {
        return (0.0, 0.0);
    }
    let index = position.floor() as usize;
    if index >= frames {
        return (0.0, 0.0);
    }
    let next = (index + 1).min(frames - 1);
    let place = (position - index as f64) as f32;
    let channel = |c: usize| {
        let samples = sample
            .channel(c)
            .or_else(|| sample.channel(0))
            .unwrap_or(&[]);
        let a = samples.get(index).copied().unwrap_or(0.0);
        let b = samples.get(next).copied().unwrap_or(0.0);
        a + (b - a) * place
    };
    (channel(0), channel(1))
}

/// Milliseconds as whole frames at `rate`.
fn frames_of(milliseconds: f64, rate: f64) -> u32 {
    (milliseconds.max(0.0) / 1000.0 * rate).round() as u32
}
