//! Several MIDI clips in one note editor (WP-6.6): where each clip's
//! contents play on the editor's shared time axis, and Duplicate Loop.
//!
//! The clip view's selection, the foreground clip and Focus Mode are
//! [`ClipView`](crate::clip_view::ClipView)'s; the note commands across clips
//! are `clip_view::run`'s. Spec: multi-clip-editing.

use bl_core::time::Ticks;
use bl_model::{ClipData, ClipId, Command, Document, Editor, MidiData, Note};

use crate::arrangement::editing::EditFailure;

/// The most loop passes of the longest loop the Session View's span may
/// cover (Spec: multi-clip-editing R5).
pub const MOST_PASSES: i64 = 64;

/// The most stretches one clip's contents are cut into on the axis.
// shortcut: a clip with a very short loop and a very long length draws its
// first 256 passes only; upgrade if a view needs more.
const MOST_WINDOWS: i64 = 256;

/// A stretch of a clip's contents, from contents tick `from` to `to`, and
/// where it's drawn: its left edge at `at` on the editor's shared axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// The axis tick the stretch starts at.
    pub at: Ticks,
    /// The contents tick it starts at.
    pub from: Ticks,
    /// The contents tick it ends at.
    pub to: Ticks,
}

/// The editor's shared time axis and where each shown clip plays on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The axis tick at the editor's left edge.
    pub from: Ticks,
    /// The axis tick at its right edge.
    pub to: Ticks,
    /// Each clip's stretches, in the clips' order.
    pub clips: Vec<(ClipId, Vec<Window>)>,
}

impl Layout {
    /// The stretches of `clip`, or none if it isn't shown.
    #[must_use]
    pub fn windows(&self, clip: ClipId) -> &[Window] {
        self.clips
            .iter()
            .find(|(other, _)| *other == clip)
            .map_or(&[][..], |(_, windows)| windows.as_slice())
    }
}

fn midi_of(doc: &Document, clip: ClipId) -> Option<(&bl_model::Clip, &MidiData)> {
    let value = doc.clip(clip)?;
    match &value.data {
        ClipData::Midi(midi) => Some((value, midi)),
        ClipData::Audio(_) => None,
    }
}

/// A clip's loop brace in contents ticks: its ends, the end being the clip's
/// end where it's unset.
fn loop_of(midi: &MidiData, length: Ticks) -> (Ticks, Ticks) {
    (
        midi.loop_start,
        midi.loop_end.unwrap_or(midi.start + length),
    )
}

/// The stretches an Arrangement clip plays, with the clip's start at `at`
/// on the song's time axis: from its Start marker to its End marker, or
/// into its loop and round it until the clip ends (Spec: midi-clips R3, R4).
fn arranged(at: Ticks, length: Ticks, midi: &MidiData) -> Vec<Window> {
    let end = at + length;
    if !midi.looping {
        let to = midi
            .end
            .map_or(midi.start + length, |end| end.min(midi.start + length));
        return vec![Window {
            at,
            from: midi.start,
            to,
        }];
    }
    let (loop_start, loop_end) = loop_of(midi, length);
    let span = loop_end - loop_start;
    if span <= Ticks::ZERO {
        return Vec::new();
    }
    let mut windows = Vec::new();
    let mut here = at;
    let mut from = midi.start;
    while here < end && (windows.len() as i64) < MOST_WINDOWS {
        let to = loop_end.min(from + (end - here));
        windows.push(Window { at: here, from, to });
        here += loop_end - from;
        from = loop_start;
    }
    windows
}

/// How far a clip's loop passes repeat on the Session's aligned axis: the
/// least common multiple of their lengths, but no more than
/// [`MOST_PASSES`] times the longest (R5).
fn session_span(lengths: &[Ticks]) -> Ticks {
    let longest = lengths.iter().copied().max().unwrap_or(Ticks::ZERO);
    let cap = longest.get().saturating_mul(MOST_PASSES);
    let mut span = 1_i64;
    for length in lengths {
        let length = length.get().max(1);
        let gcd = gcd(span, length);
        span = (span / gcd).saturating_mul(length);
        if span >= cap {
            return Ticks(cap);
        }
    }
    Ticks(span)
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Where the clips `shown` play on the editor's shared axis (R4, R5): the
/// Arrangement's clips along the song's time, from the first start to the
/// last end; the Session's from their loop starts, over as many passes as it
/// takes them to line up again.
#[must_use]
pub fn layout(doc: &Document, shown: &[ClipId], session: bool) -> Layout {
    let clips: Vec<(&bl_model::Clip, &MidiData)> = shown
        .iter()
        .filter_map(|clip| midi_of(doc, *clip))
        .collect();
    if session {
        let lengths: Vec<Ticks> = clips
            .iter()
            .map(|(clip, midi)| {
                let (from, to) = loop_of(midi, clip.length);
                (to - from).max(Ticks(1))
            })
            .collect();
        let span = session_span(&lengths);
        return Layout {
            from: Ticks::ZERO,
            to: span,
            clips: clips
                .iter()
                .zip(&lengths)
                .map(|((clip, midi), length)| {
                    let (from, to) = loop_of(midi, clip.length);
                    let passes = (span.get() / length.get()).min(MOST_WINDOWS);
                    let windows = (0..passes)
                        .map(|pass| Window {
                            at: Ticks(pass * length.get()),
                            from,
                            to,
                        })
                        .collect();
                    (clip.id, windows)
                })
                .collect(),
        };
    }
    Layout {
        from: clips
            .iter()
            .map(|(clip, _)| clip.start)
            .min()
            .unwrap_or(Ticks::ZERO),
        to: clips
            .iter()
            .map(|(clip, _)| clip.end())
            .max()
            .unwrap_or(Ticks::ZERO),
        clips: clips
            .iter()
            .map(|(clip, midi)| (clip.id, arranged(clip.start, clip.length, midi)))
            .collect(),
    }
}

/// Doubles a MIDI clip's loop brace and copies the notes inside it into the
/// new half; notes after the loop's end, and an End marker there, move later
/// by the loop's length (Spec: multi-clip-editing R11). A clip too short to
/// hold the doubled loop is made longer.
///
/// # Errors
///
/// [`EditFailure`] if the clip isn't a MIDI clip or the set refuses the edit.
pub fn duplicate_loop(editor: &mut Editor, clip: ClipId) -> Result<(), EditFailure> {
    let doc = editor.document();
    let (value, midi) = midi_of(doc, clip)
        .ok_or_else(|| EditFailure::InvalidArgs(format!("clip {} isn't a MIDI clip", clip.0)))?;
    let (from, to) = loop_of(midi, value.length);
    let span = to - from;
    if span <= Ticks::ZERO {
        return Err(EditFailure::Refused("the clip's loop is empty".to_owned()));
    }
    let mut data = midi.clone();
    // The copies in the new half are in groups of their own, as pasted
    // notes are (Spec: note-editing R31).
    let mut copies: Vec<Note> = midi
        .notes
        .iter()
        .filter(|note| note.start >= from && note.start < to)
        .map(|note| {
            let mut copy = note.clone();
            copy.start += span;
            copy
        })
        .collect();
    crate::clip_view::regroup(&mut data, &mut copies, &midi.groups);
    data.notes = midi
        .notes
        .iter()
        .map(|note| {
            let mut kept = note.clone();
            if note.start >= to {
                kept.start += span;
            }
            kept
        })
        .chain(copies)
        .collect();
    data.loop_end = Some(to + span);
    data.end = midi.end.map(|end| if end >= to { end + span } else { end });
    let length = value.length.max(to + span - midi.start);
    let grows = length > value.length;
    editor.apply(Command::SetClipData {
        clip,
        data: ClipData::Midi(data),
    })?;
    if grows {
        editor.apply(Command::SetClipLength { clip, length })?;
    }
    Ok(())
}
