//! Note Echo, the note echo MIDI effect (WP-5.31; the note-echo spec):
//! creates rhythmic echo repetitions of incoming MIDI notes at synced 16th-note
//! divisions with swing or in free milliseconds, with per-repeat transposition,
//! velocity decay, feedback, Thru/Mute input modes, and MPE expression feedback.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteExpression, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, value};

/// The Note Echo device type's ID.
pub const NOTE_ECHO: &str = "beatlayer.note_echo";

/// Sync delay time to beat divisions (true) vs free milliseconds (false).
pub const SYNC: ParamId = ParamId(0);
/// Delay time in sixteenth notes (1 to 16).
pub const TIME_SYNC: ParamId = ParamId(1);
/// Swing percentage altering delay timing.
pub const SWING: ParamId = ParamId(2);
/// Free delay time in milliseconds.
pub const TIME_MS: ParamId = ParamId(3);
/// Input playback mode: Thru (0) or Mute (1).
pub const INPUT: ParamId = ParamId(4);
/// Per-repeat transposition in semitones.
pub const PITCH: ParamId = ParamId(5);
/// Velocity decay percentage per repeat.
pub const VELOCITY_DECAY: ParamId = ParamId(6);
/// Echo feedback recirculation percentage.
pub const FEEDBACK: ParamId = ParamId(7);
/// Echo incoming MPE expression data.
pub const MPE: ParamId = ParamId(8);
/// Feedback percentage for pressure expression data.
pub const PRESS_FEEDBACK: ParamId = ParamId(9);
/// Feedback percentage for slide expression data.
pub const SLIDE_FEEDBACK: ParamId = ParamId(10);
/// Feedback percentage for per-note pitch bend expression data.
pub const PITCH_BEND_FEEDBACK: ParamId = ParamId(11);

/// The Input mode choices.
pub const INPUT_MODES: [&str; 2] = ["Thru", "Mute"];

const MAX_ACTIVE: usize = 64;
const MAX_REPEATS: usize = 16;

/// Note Echo's device type.
#[must_use]
pub fn note_echo_type() -> DeviceType {
    DeviceType::native(NOTE_ECHO, "Note Echo", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::toggle(SYNC, "Sync", true)
                .describe("Syncs delay time to beat divisions rather than milliseconds."),
            ParamDescriptor::integer(TIME_SYNC, "Time", (1, 16), 4, ParamUnit::None)
                .describe("Delay time in sixteenth notes."),
            ParamDescriptor::continuous(SWING, "Swing", (-50.0, 50.0), 0.0, ParamUnit::Percent)
                .decimals(0)
                .describe("Swing offset adjusting delay timing."),
            ParamDescriptor::continuous(
                TIME_MS,
                "Time Free",
                (1.0, 5_000.0),
                250.0,
                ParamUnit::Milliseconds,
            )
            .logarithmic()
            .decimals(0)
            .describe("Delay time in milliseconds."),
            ParamDescriptor::choice(INPUT, "Input", &INPUT_MODES, 0).describe(
                "Thru plays incoming notes with echoes; Mute silences incoming notes and outputs \
                 only echoes.",
            ),
            ParamDescriptor::integer(PITCH, "Pitch", (-48, 48), 0, ParamUnit::Semitones)
                .describe("Transposition in semitones applied to each successive echo repeat."),
            ParamDescriptor::continuous(
                VELOCITY_DECAY,
                "Decay",
                (0.0, 100.0),
                80.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Velocity scaling applied to each echo repetition."),
            ParamDescriptor::continuous(
                FEEDBACK,
                "Feedback",
                (0.0, 100.0),
                60.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Amount of output signal fed back into the delay line."),
            ParamDescriptor::toggle(MPE, "MPE", false)
                .describe("Echoes incoming MPE expression data alongside notes."),
            ParamDescriptor::continuous(
                PRESS_FEEDBACK,
                "Press Fback",
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Feedback decay for per-note pressure expressions."),
            ParamDescriptor::continuous(
                SLIDE_FEEDBACK,
                "Slide Fback",
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Feedback decay for per-note slide expressions."),
            ParamDescriptor::continuous(
                PITCH_BEND_FEEDBACK,
                "PB Fback",
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("Feedback decay for per-note pitch bend expressions."),
        ])
        .with_banks(&[
            (
                "Echo",
                &[
                    "Sync",
                    "Time",
                    "Time Free",
                    "Swing",
                    "Feedback",
                    "Decay",
                    "Pitch",
                    "Input",
                ],
            ),
            ("MPE", &["MPE", "Press Fback", "Slide Fback", "PB Fback"]),
        ]),
    )
}

/// Adds Note Echo to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, note_echo_type(), |_, _| NoteEcho::new());
}

#[derive(Clone, Copy, Debug)]
struct ScheduledEcho {
    echo_key: Key,
    start_frame: i64,
    repeat_index: usize,
}

impl Default for ScheduledEcho {
    fn default() -> Self {
        Self {
            echo_key: Key {
                channel: 0,
                key: 0,
                id: None,
            },
            start_frame: 0,
            repeat_index: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveNote {
    source: Key,
    /// Whether the input note's note-on went out, which its note-off follows
    /// whatever Input says by then (R7).
    dry_sent: bool,
    start_frame: i64,
    echoes: [ScheduledEcho; MAX_REPEATS],
    num_echoes: usize,
}

pub struct NoteEcho {
    router: Router,
    active: Vec<ActiveNote>,
    was_playing: bool,
}

impl NoteEcho {
    fn new() -> Self {
        Self {
            router: Router::new(),
            active: Vec::with_capacity(MAX_ACTIVE),
            was_playing: false,
        }
    }
}

impl NativeDevice for NoteEcho {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let is_playing = audio.playing();
        if self.was_playing && !is_playing {
            self.router.clear_queue();
            // A sounding echo's own note-off is only scheduled when its
            // input note ends, so end those echoes here (R7).
            self.router.cut(audio, 0, |_| true);
            self.active.clear();
        }
        self.was_playing = is_playing;

        let sample_rate = audio.sample_rate().as_f64();
        let bpm = audio.bpm();

        let is_sync = value(params, SYNC) >= 0.5;
        let delay_frames = if is_sync {
            let sixteenths = value(params, TIME_SYNC).clamp(1.0, 16.0);
            let swing = value(params, SWING) / 100.0;
            let beats = sixteenths * 0.25 * (1.0 + swing);
            (beats * 60.0 / bpm * sample_rate).round().max(1.0) as i64
        } else {
            (value(params, TIME_MS) * sample_rate / 1000.0)
                .round()
                .max(1.0) as i64
        };

        let mute_input = value(params, INPUT) as usize == 1;
        let pitch_shift = value(params, PITCH) as i32;
        let vel_decay = (value(params, VELOCITY_DECAY) / 100.0).clamp(0.0, 1.0) as f32;
        let feedback = (value(params, FEEDBACK) / 100.0).clamp(0.0, 1.0) as f32;
        let mpe_enabled = value(params, MPE) >= 0.5;
        let press_fb = (value(params, PRESS_FEEDBACK) / 100.0).clamp(0.0, 1.0) as f32;
        let slide_fb = (value(params, SLIDE_FEEDBACK) / 100.0).clamp(0.0, 1.0) as f32;
        let pb_fb = (value(params, PITCH_BEND_FEEDBACK) / 100.0).clamp(0.0, 1.0) as f32;

        let block_start = self.router.now();

        for note in audio.notes() {
            let note_frame = block_start + i64::from(note.offset);
            self.router.advance(audio, note.offset);

            match note.kind {
                NoteKind::On => {
                    let src_key = Key::of(&note);
                    let mut echoes = [ScheduledEcho::default(); MAX_REPEATS];
                    let mut num_echoes = 0;
                    let dry_sent = !mute_input;

                    if dry_sent {
                        // The input note passes through unchanged (R2); it
                        // takes no table room, and an input note without an
                        // ID, as monitored input has, still ends with its
                        // own note-off.
                        audio.send(note);
                    }

                    // A note with no room in `active` makes no echoes: they
                    // would never get their note-offs.
                    let has_room = self.active.len() < MAX_ACTIVE;
                    if !has_room {
                        self.router.note_dropped();
                    }

                    // Compute and schedule echoes
                    let mut k = 1;
                    let mut current_vel = note.velocity * vel_decay;
                    let mut current_fb = 1.0f32;

                    while has_room
                        && k <= MAX_REPEATS
                        && current_vel >= (1.0 / 127.0)
                        && (k == 1 || current_fb > 0.001)
                    {
                        let echo_key_val =
                            (i32::from(note.key) + (k as i32) * pitch_shift).clamp(0, 127) as u8;
                        let echo_note_id = Some(self.router.fresh_id());
                        let echo_key = Key {
                            channel: note.channel,
                            key: echo_key_val,
                            id: echo_note_id,
                        };
                        let echo_start = note_frame + (k as i64) * delay_frames;
                        let echo_on = NoteEvent {
                            offset: 0,
                            kind: NoteKind::On,
                            channel: note.channel,
                            key: echo_key_val,
                            velocity: current_vel * current_fb,
                            note_id: echo_note_id,
                        };

                        if self.router.track(&echo_on, Some(echo_key)) {
                            self.router.hold(echo_start, echo_on, echo_key);
                            if num_echoes < MAX_REPEATS {
                                echoes[num_echoes] = ScheduledEcho {
                                    echo_key,
                                    start_frame: echo_start,
                                    repeat_index: k,
                                };
                                num_echoes += 1;
                            }
                        }

                        current_vel *= vel_decay;
                        current_fb *= feedback;
                        k += 1;
                    }

                    if has_room {
                        self.active.push(ActiveNote {
                            source: src_key,
                            dry_sent,
                            start_frame: note_frame,
                            echoes,
                            num_echoes,
                        });
                    }
                }
                NoteKind::Off | NoteKind::Choke => {
                    let active_idx = self.active.iter().position(|a| a.source.names(&note));

                    // The note-off follows what happened to the note-on; an
                    // untracked note, from a full table, passes on.
                    let pass = match active_idx {
                        Some(idx) => self.active[idx].dry_sent,
                        None => !mute_input || self.active.len() >= MAX_ACTIVE,
                    };

                    if let Some(idx) = active_idx {
                        let active = self.active.remove(idx);
                        let duration = (note_frame - active.start_frame).max(1);

                        // Schedule matching note-off for each echo
                        for echo in &active.echoes[..active.num_echoes] {
                            let echo_off = NoteEvent {
                                offset: 0,
                                kind: NoteKind::Off,
                                channel: echo.echo_key.channel,
                                key: echo.echo_key.key,
                                velocity: note.velocity,
                                note_id: echo.echo_key.id,
                            };
                            self.router
                                .hold(echo.start_frame + duration, echo_off, echo.echo_key);
                        }
                    }

                    if pass {
                        audio.send(note);
                    }
                }
                NoteKind::Expression {
                    expression,
                    value: expr_val,
                } => {
                    let active = self.active.iter().find(|a| a.source.names(&note));
                    if active.map_or(!mute_input, |a| a.dry_sent) {
                        audio.send(note);
                    }

                    if mpe_enabled && let Some(active) = active {
                        let time_offset = note_frame - active.start_frame;
                        let fb_rate = match expression {
                            NoteExpression::Pressure => press_fb,
                            NoteExpression::Slide => slide_fb,
                            NoteExpression::Bend => pb_fb,
                        };

                        for echo in &active.echoes[..active.num_echoes] {
                            let scale = fb_rate.powi(echo.repeat_index as i32);
                            let echoed_expr = NoteEvent {
                                offset: 0,
                                kind: NoteKind::Expression {
                                    expression,
                                    value: expr_val * scale,
                                },
                                channel: echo.echo_key.channel,
                                key: echo.echo_key.key,
                                velocity: note.velocity,
                                note_id: echo.echo_key.id,
                            };
                            self.router.hold(
                                echo.start_frame + time_offset,
                                echoed_expr,
                                echo.echo_key,
                            );
                        }
                    }
                }
                _ => {
                    audio.send(note);
                }
            }
        }

        self.router.finish(audio);
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.router.release(out);
        self.active.clear();
    }

    fn dropped_notes(&self) -> u64 {
        self.router.dropped()
    }
}
