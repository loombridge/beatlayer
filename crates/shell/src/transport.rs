//! The Control Bar's transport section without a window (WP-1.18): what
//! each of its value fields shows, and the registered command each change
//! runs, so the UI, agents, and tests share one implementation
//! (the control bar transport spec).
//!
//! The fields come in groups: the Arrangement Position, Loop Start, and
//! Loop Length (each a bar, a beat, and a sixteenth field), the Tempo, and
//! the time signature's numerator and denominator. A change of any field
//! becomes one invocation of a registered command, which the window runs
//! through [`Shell::invoke`]:
//!
//! | Group | Command |
//! |---|---|
//! | Arrangement Position | `transport.locate` |
//! | Loop Start, Loop Length | `transport.set_loop` |
//! | Tempo | `song.set_tempo` |
//! | Numerator, Denominator | `song.set_time_signatures` |
//! | Global Amount (L-CTRL-008) | `groove.set_global_amount` |
//!
//! [`step`] is a change by the arrow keys, a drag, or a screen reader;
//! [`typed`] a value typed in and confirmed; [`reset`] Delete or a
//! double-click. None of them changes anything itself.

use bl_core::time::{BarBeatSixteenth, Bpm, TempoMap, Ticks, TimeSignature, TimeSignatureMap};
use bl_model::Document;
use bl_shell_spec::transport::{self as ids, TransportState};
use serde_json::json;

use crate::audio::EngineTransport;
use crate::{Invocation, Shell};

/// The slowest tempo the Tempo field sets: Live's, as far as it's known
/// (to observe in Live).
pub const TEMPO_MIN: f64 = 20.0;
/// The fastest tempo the Tempo field sets.
pub const TEMPO_MAX: f64 = 999.0;
/// The tempo Delete and a double-click return the Tempo field to: a new
/// set's.
pub const DEFAULT_TEMPO: f64 = 120.0;
/// The Global Amount's largest value, in percent (the grooves spec, R2).
pub const GLOBAL_AMOUNT_MAX: f64 = bl_model::GLOBAL_AMOUNT_MAX;
/// The Global Amount Delete and a double-click return its field to: every
/// groove plays as set (the grooves spec, R10).
pub const DEFAULT_GLOBAL_AMOUNT: f64 = 100.0;
/// The denominators the time-signature field takes, as the document does.
pub const DENOMINATORS: [u8; 5] = [1, 2, 4, 8, 16];
/// A new set's numerator and denominator, which Delete returns them to.
pub const DEFAULT_SIGNATURE: (u8, u8) = (4, 4);
/// The loop length Delete returns the Loop Length fields to: a new set's
/// four 4/4 bars (transport R18).
pub const DEFAULT_LOOP_LENGTH: Ticks = Ticks(Ticks::WHOLE.get() * 4);

/// A group of the section's value fields, which show one value together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// The Arrangement Position fields (L-CTRL-013): bar, beat, sixteenth.
    Position,
    /// The Loop Start fields (L-CTRL-016): bar, beat, sixteenth.
    LoopStart,
    /// The Loop Length fields (L-CTRL-018): bars, beats, and sixteenths
    /// counted from 0.
    LoopLength,
    /// The Tempo field (L-CTRL-005).
    Tempo,
    /// The time signature's numerator (L-CTRL-007).
    Numerator,
    /// The time signature's denominator (L-CTRL-007).
    Denominator,
    /// The Global Amount, in percent (L-CTRL-008, the grooves spec R30).
    GlobalAmount,
}

impl Group {
    /// How many fields show the group's value: three for a bar, a beat,
    /// and a sixteenth, one otherwise.
    #[must_use]
    pub const fn parts(self) -> usize {
        match self {
            Self::Position | Self::LoopStart | Self::LoopLength => 3,
            Self::Tempo | Self::Numerator | Self::Denominator | Self::GlobalAmount => 1,
        }
    }

    /// The undo step a drag on the group's fields makes, for a group whose
    /// changes are document edits.
    #[must_use]
    pub const fn gesture_label(self) -> Option<&'static str> {
        match self {
            Self::Position => None,
            Self::LoopStart | Self::LoopLength => Some("Change loop"),
            Self::Tempo => Some("Change tempo"),
            Self::Numerator | Self::Denominator => Some("Change time signatures"),
            Self::GlobalAmount => Some("Change Global Amount"),
        }
    }
}

/// One value field: a group, and which of its fields (0 for the bar, 1 the
/// beat, 2 the sixteenth; 0 for a group of one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    /// The group.
    pub group: Group,
    /// Which of the group's fields.
    pub part: usize,
}

impl Field {
    /// The `part`th field of `group`.
    #[must_use]
    pub const fn new(group: Group, part: usize) -> Self {
        Self { group, part }
    }
}

/// Where the Arrangement Position fields are: the playhead while the song
/// plays, and the insert marker while it's stopped (R4).
#[must_use]
pub fn shown_position(shell: &Shell) -> Ticks {
    let transport = shell.arrangement().transport();
    if transport.playing {
        transport.position.max(Ticks::ZERO)
    } else {
        shell.arrangement().insert_marker().max(Ticks::ZERO)
    }
}

/// `at` as the bar, beat, and sixteenth the fields show.
fn bbs(signatures: &TimeSignatureMap, at: Ticks) -> [i64; 3] {
    let shown = signatures.to_bbs(at.max(Ticks::ZERO));
    [shown.bar, i64::from(shown.beat), i64::from(shown.sixteenth)]
}

/// A length from `start` as bars, beats, and sixteenths counted from 0, in
/// the signature at `start`: four 4/4 bars are `[4, 0, 0]`.
#[must_use]
pub fn length_parts(signatures: &TimeSignatureMap, start: Ticks, length: Ticks) -> [i64; 3] {
    let signature = signatures.signature_at(start);
    let (bar, beat) = (signature.bar_length().get(), signature.beat_length().get());
    let length = length.get().max(0);
    [
        length / bar,
        length % bar / beat,
        length % beat / Ticks::SIXTEENTH.get(),
    ]
}

/// The group's three numbers as the fields show them (a group of one uses
/// the first).
fn parts(doc: &Document, shell: &Shell, group: Group) -> [i64; 3] {
    let settings = doc.settings();
    let signatures = &settings.time_signatures;
    let brace = settings.arrangement_loop;
    match group {
        Group::Position => bbs(signatures, shown_position(shell)),
        Group::LoopStart => bbs(signatures, brace.start),
        Group::LoopLength => length_parts(signatures, brace.start, brace.length),
        Group::Tempo | Group::GlobalAmount => [0; 3],
        Group::Numerator => [i64::from(signature(doc, shell).numerator()), 0, 0],
        Group::Denominator => [i64::from(signature(doc, shell).denominator()), 0, 0],
    }
}

/// The time signature where the Arrangement Position fields are.
fn signature(doc: &Document, shell: &Shell) -> TimeSignature {
    doc.settings()
        .time_signatures
        .signature_at(shown_position(shell))
}

/// The tempo where the Arrangement Position fields are, in BPM.
#[must_use]
pub fn tempo(shell: &Shell) -> f64 {
    shell
        .editor()
        .document()
        .settings()
        .tempo
        .bpm_at(shown_position(shell))
        .get()
}

/// Whether the field's group can be changed here: the Tempo field while the
/// song has one constant tempo, the time-signature fields while it has one
/// signature throughout (tempo and signature changes come with WP-2.20),
/// and the rest always.
#[must_use]
pub fn editable(shell: &Shell, group: Group) -> bool {
    let settings = shell.editor().document().settings();
    match group {
        Group::Tempo => {
            // Spec: warping R45. While a clip leads the tempo the field is
            // off: the clips set the tempo, and the commands refuse it.
            if shell.editor().document().any_tempo_leader() {
                return false;
            }
            let mut segments = settings.tempo.segments();
            segments.len() == 1
                && segments
                    .next()
                    .is_some_and(|segment| segment.start_bpm() == segment.end_bpm())
        }
        Group::Numerator | Group::Denominator => settings.time_signatures.changes().len() == 1,
        Group::Position | Group::LoopStart | Group::LoopLength | Group::GlobalAmount => true,
    }
}

/// The Global Amount, in percent.
#[must_use]
pub fn global_amount(shell: &Shell) -> f64 {
    shell.editor().document().settings().grooves.global_amount
}

/// A percentage as the Global Amount's field reads it: `100 %`, or `62.5 %`.
#[must_use]
pub fn percent_text(percent: f64) -> String {
    if (percent - percent.round()).abs() < 0.05 {
        format!("{} %", percent.round())
    } else {
        format!("{percent:.1} %")
    }
}

/// What a field shows: `3` for the bar of 3.2.1, `120.00` for the tempo.
#[must_use]
pub fn text(shell: &Shell, field: Field) -> String {
    match field.group {
        Group::Tempo => format!("{:.2}", tempo(shell)),
        Group::GlobalAmount => percent_text(global_amount(shell)),
        group => parts(shell.editor().document(), shell, group)[field.part.min(2)].to_string(),
    }
}

/// A field's value as a number, for assistive technology.
#[must_use]
pub fn number(shell: &Shell, field: Field) -> f64 {
    match field.group {
        Group::Tempo => tempo(shell),
        Group::GlobalAmount => global_amount(shell),
        group => parts(shell.editor().document(), shell, group)[field.part.min(2)] as f64,
    }
}

/// The whole value the group shows, such as `3.2.1`, `4.0.0`, `120.00`, or
/// `4`.
#[must_use]
pub fn group_text(shell: &Shell, group: Group) -> String {
    match group {
        Group::Tempo => format!("{:.2}", tempo(shell)),
        Group::GlobalAmount => percent_text(global_amount(shell)),
        group if group.parts() == 3 => {
            let [a, b, c] = parts(shell.editor().document(), shell, group);
            format!("{a}.{b}.{c}")
        }
        group => parts(shell.editor().document(), shell, group)[0].to_string(),
    }
}

/// The length of one step of a bar, beat, or sixteenth field, in the
/// signature at `at`.
fn unit(signatures: &TimeSignatureMap, at: Ticks, part: usize) -> Ticks {
    let signature = signatures.signature_at(at);
    match part {
        0 => signature.bar_length(),
        1 => signature.beat_length(),
        _ => Ticks::SIXTEENTH,
    }
}

/// The command that changes a field by `steps` steps (negative steps go
/// down): a bar, a beat, or a sixteenth; 1 BPM, or 0.01 BPM when `fine`;
/// 1 for the numerator; the next allowed denominator. The arrow keys, a
/// drag, and a screen reader's increment and decrement step a field. `None`
/// if the value wouldn't change or the field can't be changed.
#[must_use]
pub fn step(shell: &Shell, field: Field, steps: i32, fine: bool) -> Option<Invocation> {
    if steps == 0 || !editable(shell, field.group) {
        return None;
    }
    let doc = shell.editor().document();
    let settings = doc.settings();
    let signatures = &settings.time_signatures;
    let brace = settings.arrangement_loop;
    let steps64 = i64::from(steps);
    match field.group {
        Group::Position => {
            let at = shown_position(shell);
            Some(locate(at + unit(signatures, at, field.part) * steps64))
        }
        Group::LoopStart => {
            let start = brace.start + unit(signatures, brace.start, field.part) * steps64;
            set_loop(doc, start.max(Ticks::ZERO), brace.length)
        }
        Group::LoopLength => {
            let length = brace.length + unit(signatures, brace.start, field.part) * steps64;
            set_loop(doc, brace.start, length.max(Ticks::SIXTEENTH))
        }
        Group::Tempo => {
            let by = if fine { 0.01 } else { 1.0 };
            set_tempo(shell, tempo(shell) + by * f64::from(steps))
        }
        Group::GlobalAmount => {
            let by = if fine { 0.1 } else { 1.0 };
            set_global_amount(shell, global_amount(shell) + by * f64::from(steps))
        }
        Group::Numerator => {
            let now = i64::from(signature(doc, shell).numerator());
            set_numerator(shell, now + steps64)
        }
        Group::Denominator => {
            let now = signature(doc, shell).denominator();
            let index = DENOMINATORS.iter().position(|d| *d == now).unwrap_or(2) as i64;
            let next = (index + steps64).clamp(0, DENOMINATORS.len() as i64 - 1) as usize;
            set_denominator(shell, DENOMINATORS[next])
        }
    }
}

/// The command that returns a field's group to its default, as Delete and
/// a double-click do: 1.1.1 for the position and the loop's start, four
/// bars for its length, 120 BPM, and 4/4. `None` if it's there already.
#[must_use]
pub fn reset(shell: &Shell, field: Field) -> Option<Invocation> {
    if !editable(shell, field.group) {
        return None;
    }
    let doc = shell.editor().document();
    let brace = doc.settings().arrangement_loop;
    match field.group {
        Group::Position => Some(locate(Ticks::ZERO)),
        Group::LoopStart => set_loop(doc, Ticks::ZERO, brace.length),
        Group::LoopLength => set_loop(doc, brace.start, DEFAULT_LOOP_LENGTH),
        Group::Tempo => set_tempo(shell, DEFAULT_TEMPO),
        Group::GlobalAmount => set_global_amount(shell, DEFAULT_GLOBAL_AMOUNT),
        Group::Numerator => set_numerator(shell, i64::from(DEFAULT_SIGNATURE.0)),
        Group::Denominator => set_denominator(shell, DEFAULT_SIGNATURE.1),
    }
}

/// The command that sets a group to a typed value: `typed` holds what was
/// typed into each of its fields, and a field not typed in keeps the value
/// shown. Out-of-range values are brought into range (R9): a bar of 1 or
/// more, a beat and a sixteenth that exist in the bar, a length of at least
/// a sixteenth, a tempo from 20 to 999, a numerator from 1 to 99, and the
/// allowed denominator at or below the one typed. `None` if the text isn't
/// a number, or nothing would change.
#[must_use]
pub fn typed(shell: &Shell, group: Group, typed: &[Option<String>; 3]) -> Option<Invocation> {
    if !editable(shell, group) {
        return None;
    }
    let doc = shell.editor().document();
    let settings = doc.settings();
    let signatures = &settings.time_signatures;
    let brace = settings.arrangement_loop;
    let entered = |part: usize| -> Option<Option<&str>> {
        match typed[part].as_deref().map(str::trim) {
            None | Some("") => Some(None),
            Some(text) => Some(Some(text)),
        }
    };
    let whole = |part: usize, shown: i64| -> Option<i64> {
        match entered(part)? {
            None => Some(shown),
            Some(text) => text.parse::<i64>().ok(),
        }
    };
    match group {
        Group::Position | Group::LoopStart => {
            let shown = parts(doc, shell, group);
            let bar = whole(0, shown[0])?.max(1);
            let start = signatures.bar_start(bar - 1);
            let signature = signatures.signature_at(start);
            let beat = whole(1, shown[1])?.clamp(1, i64::from(signature.numerator()));
            let sixteenth =
                whole(2, shown[2])?.clamp(1, i64::from(signature.sixteenths_per_beat()));
            let at = signatures
                .from_bbs(BarBeatSixteenth::new(bar, beat as u32, sixteenth as u32))
                .ok()?;
            if group == Group::Position {
                Some(locate(at))
            } else {
                set_loop(doc, at, brace.length)
            }
        }
        Group::LoopLength => {
            let shown = parts(doc, shell, group);
            let signature = signatures.signature_at(brace.start);
            let bars = whole(0, shown[0])?.max(0);
            let beats = whole(1, shown[1])?.max(0);
            let sixteenths = whole(2, shown[2])?.max(0);
            let length = signature
                .bar_length()
                .get()
                .checked_mul(bars)?
                .checked_add(signature.beat_length().get().checked_mul(beats)?)?
                .checked_add(Ticks::SIXTEENTH.get().checked_mul(sixteenths)?)?;
            set_loop(doc, brace.start, Ticks(length).max(Ticks::SIXTEENTH))
        }
        Group::Tempo => match entered(0)? {
            None => None,
            Some(text) => set_tempo(shell, text.replace(',', ".").parse::<f64>().ok()?),
        },
        Group::GlobalAmount => match entered(0)? {
            None => None,
            Some(text) => set_global_amount(shell, text.replace(',', ".").parse::<f64>().ok()?),
        },
        Group::Numerator => set_numerator(shell, whole(0, -1)?),
        Group::Denominator => {
            let typed = whole(0, -1)?;
            let denominator = DENOMINATORS
                .iter()
                .rev()
                .copied()
                .find(|d| i64::from(*d) <= typed)
                .unwrap_or(1);
            set_denominator(shell, denominator)
        }
    }
}

/// `transport.locate` at `at`, which moves the insert marker (and playback,
/// while the song plays).
fn locate(at: Ticks) -> Invocation {
    Invocation::new(ids::LOCATE, json!({ "position": at.max(Ticks::ZERO) }))
}

/// `transport.set_loop`, if it changes the brace.
fn set_loop(doc: &Document, start: Ticks, length: Ticks) -> Option<Invocation> {
    let brace = doc.settings().arrangement_loop;
    (brace.start != start || brace.length != length)
        .then(|| Invocation::new(ids::SET_LOOP, json!({ "start": start, "length": length })))
}

/// `song.set_tempo` with one constant tempo, brought within the field's
/// range and to hundredths, if it changes the tempo.
fn set_tempo(shell: &Shell, bpm: f64) -> Option<Invocation> {
    if !bpm.is_finite() {
        return None;
    }
    let bpm = ((bpm.clamp(TEMPO_MIN, TEMPO_MAX)) * 100.0).round() / 100.0;
    let map = TempoMap::constant(Bpm::new(bpm).ok()?);
    (shell.editor().document().settings().tempo != map)
        .then(|| Invocation::new(ids::SET_TEMPO, json!({ "tempo": map })))
}

/// `groove.set_global_amount`, brought within 0 to 130 percent and to tenths
/// of a percent, if it changes the amount.
fn set_global_amount(shell: &Shell, percent: f64) -> Option<Invocation> {
    if !percent.is_finite() {
        return None;
    }
    let amount = (percent.clamp(0.0, GLOBAL_AMOUNT_MAX) * 10.0).round() / 10.0;
    ((global_amount(shell) - amount).abs() > f64::EPSILON).then(|| {
        Invocation::new(
            bl_shell_spec::groove::SET_GLOBAL_AMOUNT,
            json!({ "amount": amount }),
        )
    })
}

/// `song.set_time_signatures` with the numerator `numerator` (brought
/// within 1 to 99) and the denominator shown, if it changes the signature.
fn set_numerator(shell: &Shell, numerator: i64) -> Option<Invocation> {
    let now = signature(shell.editor().document(), shell);
    set_signature(shell, numerator.clamp(1, 99) as u8, now.denominator())
}

/// `song.set_time_signatures` with the numerator shown and `denominator`,
/// if it changes the signature.
fn set_denominator(shell: &Shell, denominator: u8) -> Option<Invocation> {
    let now = signature(shell.editor().document(), shell);
    set_signature(shell, now.numerator(), denominator)
}

fn set_signature(shell: &Shell, numerator: u8, denominator: u8) -> Option<Invocation> {
    let signature = TimeSignature::new(numerator, denominator).ok()?;
    let map = TimeSignatureMap::constant(signature);
    (shell.editor().document().settings().time_signatures != map)
        .then(|| Invocation::new(ids::SET_TIME_SIGNATURES, json!({ "time_signatures": map })))
}

/// What `transport.state` reports: the engine's transport, as `engine`
/// gives it, and the section's fields.
#[must_use]
pub fn state(shell: &Shell, engine: EngineTransport) -> TransportState {
    let doc = shell.editor().document();
    let brace = doc.settings().arrangement_loop;
    let signature = signature(doc, shell);
    TransportState {
        engine_running: engine.running,
        playing: engine.playing,
        position: engine.position,
        insert_marker: engine.insert_marker,
        arrangement_position: group_text(shell, Group::Position),
        loop_start: brace.start,
        loop_length: brace.length,
        loop_on: brace.enabled,
        loop_start_position: group_text(shell, Group::LoopStart),
        loop_length_bars: group_text(shell, Group::LoopLength),
        tempo: tempo(shell),
        tempo_editable: editable(shell, Group::Tempo),
        time_signature: format!("{}/{}", signature.numerator(), signature.denominator()),
        time_signature_editable: editable(shell, Group::Numerator),
        metronome: doc.settings().metronome,
        arrangement_record: engine.recording,
        take_running: engine.take_running,
        counting_in: engine.counting_in,
        punch_in: doc.settings().punch_in,
        punch_out: doc.settings().punch_out,
        midi_overdub: doc.settings().midi_overdub,
        record_quantization: doc.settings().record_quantization,
        record_quantization_last: doc.settings().record_quantization_last,
        automation_arm: shell.arrangement().automation_arm(),
        automation_record_mode: shell.automation_record_mode(),
        recording: shell.running_recording(),
        last_recording: shell.last_recording().cloned(),
        capture_midi_ready: shell.capture_midi_ready(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_as_bars_beats_and_sixteenths_from_zero() {
        // Spec: control-bar-transport R5
        let four_four = TimeSignatureMap::default();
        assert_eq!(
            length_parts(&four_four, Ticks::ZERO, DEFAULT_LOOP_LENGTH),
            [4, 0, 0]
        );
        let odd = Ticks::WHOLE + Ticks::QUARTER * 2 + Ticks::SIXTEENTH * 3;
        assert_eq!(length_parts(&four_four, Ticks::ZERO, odd), [1, 2, 3]);
        let six_eight = TimeSignatureMap::constant(TimeSignature::new(6, 8).unwrap());
        // Six eighths to the bar: a 4/4 bar is a bar and two eighths.
        assert_eq!(
            length_parts(&six_eight, Ticks::ZERO, Ticks::WHOLE),
            [1, 2, 0]
        );
    }

    #[test]
    fn the_default_loop_length_is_a_new_sets() {
        assert_eq!(
            DEFAULT_LOOP_LENGTH,
            bl_model::ArrangementLoop::DEFAULT.length
        );
    }
}
