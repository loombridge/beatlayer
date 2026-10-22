//! Warped clips' playback (WP-4.2, WP-4.4, the warping spec R12 to R19 and the
//! warp modes spec R1 to R19): which file frame a warped clip plays at each
//! song frame, and the reading of the file that follows.
//!
//! The compiler works out, off the audio thread, a [`WarpTable`] from song
//! frames to file positions for each warped Arrangement clip: its warp map
//! under the song's tempo map, through the clip's contents (its Start marker,
//! End marker and loop). The table travels with the clip list as node data,
//! so a warp edit or a tempo change sends data and no program. A warped clip
//! launched from a Session slot has no place in the Arrangement to tabulate
//! for: it follows the tempo map from its launch frame, frame by frame
//! ([`Positions::Launched`]).
//!
//! A Beats clip's table is its [`Plan`] instead (R14 to R17): the segments the
//! clip plays, each started on its song frame and read at the clip's pitch
//! ratio, with the loop modes filling a short one. The plan is made with the
//! clip list, off the audio thread, and the player reads its stretches.
//!
//! The player reads, for each run of frames over which the position advances
//! smoothly, a window of the file around the run, and a warp mode turns the
//! positions and the window into audio. The position at a frame depends on
//! that frame alone, so a render is the same at any block size.

use std::sync::Arc;

use bl_core::time::{SampleRate, TempoMap, Ticks};
use bl_media::SampleData;
use bl_media::stream::{AudioSource, PlanarMut, Transition};
use bl_model::WarpMode as ClipWarpMode;
use bl_model::{
    AudioData, BeatsSettings, Clip, ClipParam, Preserve as ClipPreserve, TransientLoopMode,
};
use bl_stretch::beats::{self, Loop, Plan, Preserve, Segment};
use bl_stretch::{
    Beats, Complex, Interpolation, Piece, RePitch, Request, Texture, Tones, WarpMap, WarpMode,
    WarpTable, Window, grain_window_frames, stretch_window_frames,
};

use crate::clip_sound::ClipSound;
use crate::clip_source::WINDOW_FRAMES as NARROW_WINDOW;

/// How many file frames before a position a warped clip may read: the widest
/// interpolator's reach, so a point a jump lands on is cued that far early.
pub(crate) const CUE_BACK: u64 = 264;

/// The file frames a mode's reads need at `rate`: a grain mode's own window
/// (Tones and Texture, R18 and R19), the frequency-domain modes' read-ahead
/// (Complex and Complex Pro, R21, R24), the narrow one for the rest. Each mode
/// pays for its own reach.
pub(crate) fn window_of(mode: ClipWarpMode, rate: f64) -> usize {
    match mode {
        ClipWarpMode::Tones | ClipWarpMode::Texture => grain_window_frames(rate),
        ClipWarpMode::Complex | ClipWarpMode::ComplexPro => stretch_window_frames(rate),
        _ => NARROW_WINDOW,
    }
}

/// The room a player reads a clip's file into, and hands its mode as a
/// [`Window`]: made with the track's clip list, off the audio thread, and
/// carried to the player like its clips' sources (R5). Its width follows the
/// widest window the list's clips and their mode changes need.
#[derive(Debug)]
pub(crate) struct WindowBuf {
    /// How many frames each channel holds.
    pub(crate) frames: usize,
    /// The frames, channel after channel, `frames` apart.
    pub(crate) data: Vec<f32>,
}

impl WindowBuf {
    /// A window `channels` channels wide, of `frames` frames each.
    pub(crate) fn new(channels: usize, frames: usize) -> Self {
        Self {
            frames,
            data: vec![0.0; channels * frames],
        }
    }
}

/// The most passes round a loop a table holds (a clip looped over a tick
/// would otherwise be a table without end): past it the clip is silent.
const MAX_RUNS: usize = 1 << 20;

/// The most passes a Beats plan holds: past it the clip plays silence, since
/// a plan of a million segments is not worth the memory.
const MAX_SEGMENT_RUNS: usize = 4_096;

/// Ticks in a sixteenth note, the unit of Sample Offset (R30).
const SIXTEENTH: i64 = Ticks::WHOLE.get() / 16;

/// How far, in file frames, a table's straight pieces may stray from the
/// exact position at their middles, where a smooth curve strays most (the
/// warping spec, R12 allows a hundredth of a frame).
const TOLERANCE: f64 = 0.005;

/// How long a mode change takes to crossfade into the new one, in seconds
/// (R8, Beatlayer's own).
const CHANGE_SECONDS: f64 = 0.020;

/// How a warped clip is played: its mode, its HiQ switch, settings and clip ID.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Playing {
    pub(crate) mode: ClipWarpMode,
    pub(crate) high_quality: bool,
    pub(crate) tones: bl_model::TonesSettings,
    pub(crate) texture: bl_model::TextureSettings,
    /// Complex Pro's Formants and Envelope (R22, R23); Complex ignores them.
    pub(crate) complex_pro: bl_model::ComplexProSettings,
    pub(crate) clip_id: u64,
    /// The clip's pitch ratio (R25): 1 where it isn't transposed. Every mode
    /// but Re-Pitch takes it on its own path (R26); Re-Pitch ignores it
    /// (R10).
    pub(crate) pitch: f64,
    /// Whether the clip is warped: an unwarped one reads its file at the
    /// pitch ratio instead of through a mode (R27).
    pub(crate) warp: bool,
    /// Beats' Transient Envelope, 0 to 100 (R16).
    pub(crate) transient_envelope: f64,
}

impl Playing {
    /// How `audio`, clip `clip_id`, plays with its settings as they stand.
    pub(crate) fn of(clip_id: u64, audio: &AudioData) -> Self {
        Self {
            mode: audio.warp_mode,
            high_quality: audio.high_quality,
            tones: audio.warp_settings.tones,
            texture: audio.warp_settings.texture,
            complex_pro: audio.warp_settings.complex_pro,
            clip_id,
            pitch: audio.pitch_ratio(),
            warp: audio.warp,
            transient_envelope: f64::from(audio.warp_settings.beats.transient_envelope),
        }
    }
}

/// A warped clip's contents, in beats of the clip: where it starts, ends and
/// loops, and the map from a beat to the file.
#[derive(Debug)]
pub(crate) struct Contents {
    map: WarpMap,
    /// File frames per second.
    rate: f64,
    start: f64,
    end: Option<f64>,
    looping: bool,
    loop_start: f64,
    /// Infinite where the brace runs to the end of a file of unknown length.
    loop_end: f64,
}

impl Contents {
    /// The file frame beat `beat` plays: fractional, and before the file's
    /// start (negative) where the map reaches back past it.
    fn position(&self, beat: f64) -> f64 {
        self.map.beat_to_time(beat) * self.rate
    }

    /// The beat of the contents reached `played` ticks after the clip's
    /// start, and how many times the loop has wrapped by then; `None` once
    /// the clip has played past its End marker (the warping spec, R5).
    fn beat_after(&self, played: f64) -> Option<(f64, u64)> {
        let x = self.start + played;
        if !self.looping {
            return self.end.is_none_or(|end| x < end).then_some((x, 0));
        }
        let length = self.loop_end - self.loop_start;
        if length <= 0.0 {
            return None;
        }
        if x < self.loop_end {
            return Some((x, 0));
        }
        let over = x - self.loop_end;
        let into = over.rem_euclid(length);
        let wraps = ((over - into) / length).round() as u64 + 1;
        Some((self.loop_start + into, wraps))
    }
}

/// A warped clip as the engine plays it.
#[derive(Debug)]
pub(crate) struct Warped {
    contents: Contents,
    /// The clip's warp mode, which says how wide a window its reads need
    /// (R18, R19).
    mode: ClipWarpMode,
    /// The Arrangement clip's table; empty for a Session slot clip, and for
    /// a clip placed before its file's length is known.
    table: WarpTable,
    /// A Beats clip's segments (R14 to R17); empty in every other mode.
    plan: Arc<Plan>,
}

/// What a Beats plan needs of the clip's media: the file's transients and,
/// where the loop turns must be found, its audio.
pub(crate) struct BeatsMedia<'a> {
    /// What moves the clip's settings (R29, R31), where something does: Beats
    /// reads its pitch ratio and its Sample Offset where each segment begins,
    /// and its Preserve and Transient Loop Mode where the clip does.
    pub(crate) sound: Option<&'a ClipSound>,
    /// The clip's transients in seconds into its file, its own edits applied
    /// (the warping spec, R23).
    pub(crate) transients: &'a [f64],
    /// The clip's file at the engine's rate, called only when a segment needs
    /// its turn: the media library's own decode, cached.
    pub(crate) samples: &'a dyn Fn() -> Option<Arc<SampleData>>,
}

impl Warped {
    /// `clip`'s warped playback under `tempo` at `rate`, for a file of
    /// `length` frames if known; `None` if its warp map is damaged. A
    /// Session slot clip (`tabulate` false) gets no table.
    pub(crate) fn new(
        clip: &Clip,
        audio: &AudioData,
        tempo: &TempoMap,
        rate: SampleRate,
        length: Option<u64>,
        tabulate: bool,
        media: &BeatsMedia<'_>,
    ) -> Option<Self> {
        let map = audio.warp_map().ok()?;
        let r = rate.as_f64();
        let loop_end = match (audio.loop_end, length) {
            (Some(end), _) => end,
            (None, Some(frames)) => map.time_to_beat(frames as f64 / r),
            (None, None) => f64::INFINITY,
        };
        let contents = Contents {
            map,
            rate: r,
            start: audio.start,
            end: audio.end,
            looping: audio.looping,
            loop_start: audio.loop_start,
            loop_end,
        };
        let plan = if audio.warp_mode == ClipWarpMode::Beats {
            // Spec: warp-modes R29. Preserve and Transient Loop Mode, which
            // shape the whole plan, take the value they have where the clip
            // begins.
            let mut settings = audio.warp_settings.beats;
            if let Some(sound) = media.sound {
                let begins = tempo.ticks_to_frames(clip.start, rate).get();
                settings.preserve =
                    ClipPreserve::from_index(sound.value(ClipParam::Preserve, begins));
                settings.transient_loop_mode = TransientLoopMode::from_index(
                    sound.value(ClipParam::TransientLoopMode, begins),
                );
            }
            Arc::new(contents.plan(clip, tempo, r, settings, media))
        } else {
            Arc::new(Plan::default())
        };
        let mut warped = Self {
            contents,
            mode: audio.warp_mode,
            table: WarpTable::new(Vec::new()),
            plan,
        };
        if tabulate {
            warped.table = if warped.plan.is_empty() {
                warped.tabulate(clip, tempo, r)
            } else {
                warped.beats_table(clip, tempo, r, audio.pitch_ratio(), media.sound)
            };
        }
        Some(warped)
    }

    /// A Beats clip's plan (R14, R15).
    pub(crate) fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Where an Arrangement clip reads: its table.
    pub(crate) fn positions(&self) -> Positions<'_> {
        Positions::Table(&self.table)
    }

    /// Where a Session clip launched on song frame `launch` reads: the tempo
    /// map followed from there, at `rate` frames a second.
    pub(crate) fn launched<'a>(
        &'a self,
        tempo: &'a TempoMap,
        launch: i64,
        rate: f64,
    ) -> Positions<'a> {
        Positions::Launched {
            warped: self,
            tempo,
            launch,
            rate,
        }
    }

    /// The file frame the clip plays at song frame `at`.
    pub(crate) fn file_frame(&self, at: i64) -> Option<u64> {
        let position = self.table.position_at(at);
        (position.is_finite()).then(|| position.floor().max(0.0) as u64)
    }

    /// The file frame the clip first plays, for opening its source: its
    /// Start marker's.
    pub(crate) fn first_frame(&self, start: i64) -> u64 {
        let position = self.table.position_at(start);
        let position = if position.is_finite() {
            position
        } else {
            self.contents.position(self.contents.start)
        };
        position.round().max(0.0) as u64
    }

    /// The file frame the loop brace starts at.
    pub(crate) fn loop_start_frame(&self) -> u64 {
        self.contents
            .position(self.contents.loop_start)
            .round()
            .max(0.0) as u64
    }

    /// How wide a window the clip's mode reads through: its reads, and the
    /// room its source keeps, are this wide (R5, R18, R19).
    pub(crate) fn window_frames(&self) -> usize {
        window_of(self.mode, self.contents.rate)
    }

    /// How many file frames the clip's reads reach back into, at most: the
    /// longest stretch from a Beats segment's turn to its end (R15), a grain
    /// mode's window (its reads overlap the last one by nearly that much, and
    /// the room serves them), or nothing where a mode's reads only go
    /// forwards.
    pub(crate) fn history_frames(&self) -> usize {
        if self.plan.is_empty() {
            return match self.mode {
                ClipWarpMode::Tones | ClipWarpMode::Texture => self.window_frames(),
                _ => 0,
            };
        }
        let longest = self
            .plan
            .segments()
            .iter()
            .filter(|segment| segment.end > segment.turn)
            .map(|segment| segment.end - segment.turn)
            .fold(0.0_f64, f64::max);
        (longest.ceil() as usize).max(self.window_frames())
    }

    /// The table of a Beats clip: its plan's stretches (R14 to R17), read at
    /// the clip's pitch ratio (R17).
    fn beats_table(
        &self,
        clip: &Clip,
        tempo: &TempoMap,
        r: f64,
        pitch: f64,
        sound: Option<&ClipSound>,
    ) -> WarpTable {
        // Spec: warp-modes R17, R29. Each segment reads at the ratio the clip
        // has where it begins.
        let mut pieces: Vec<Piece> = self
            .plan
            .stretches_by(|segment| sound.map_or(pitch, |s| s.pitch_at(segment.frame)))
            .into_iter()
            .map(|stretch| Piece {
                frame: stretch.frame,
                position: stretch.position,
                slope: stretch.slope,
            })
            .collect();
        // Past the last segment the clip plays on in silence, where its own
        // length runs longer than the plan (a plan stops where the audio
        // does).
        let end = r * tempo.ticks_to_seconds(Ticks(clip.end().get()));
        if let Some(last) = self.plan.segments().last()
            && (last.until as f64) < end
            && pieces.last().is_some_and(|piece| !piece.is_silent())
        {
            pieces.push(Piece {
                frame: last.until as f64,
                position: f64::NAN,
                slope: 1.0,
            });
        }
        WarpTable::new(pieces)
    }

    /// The table of `clip`: each pass through its contents, from the Start
    /// marker to the loop's end or the clip's, then the loop round again.
    fn tabulate(&self, clip: &Clip, tempo: &TempoMap, r: f64) -> WarpTable {
        let c = &self.contents;
        let end = clip.end().get();
        let mut pieces = Vec::new();
        let silent = |tick: i64| Piece {
            frame: r * tempo.ticks_to_seconds(Ticks(tick)),
            position: f64::NAN,
            slope: 1.0,
        };
        let mut tick = clip.start.get();
        let mut beat = c.start;
        if c.looping {
            let length = c.loop_end - c.loop_start;
            if length <= 0.0 {
                return WarpTable::new(vec![silent(tick)]);
            }
            if beat >= c.loop_end {
                beat = c.loop_start + (beat - c.loop_end).rem_euclid(length);
            }
        }
        for pass in 0..=MAX_RUNS {
            if tick >= end {
                break;
            }
            if pass == MAX_RUNS {
                pieces.push(silent(tick));
                break;
            }
            let room = if c.looping {
                c.loop_end - beat
            } else {
                c.end.map_or(f64::INFINITY, |stop| stop - beat)
            };
            if room <= 0.0 {
                pieces.push(silent(tick));
                break;
            }
            let reaches = tick as f64 + room;
            let stop = if reaches >= end as f64 {
                end
            } else {
                reaches.round() as i64
            };
            self.pass(&mut pieces, tempo, r, (tick, stop), beat);
            if stop >= end {
                break;
            }
            if c.looping {
                tick = stop;
                beat = c.loop_start;
            } else {
                pieces.push(silent(stop));
                break;
            }
        }
        if pieces.is_empty() {
            pieces.push(silent(clip.start.get()));
        }
        WarpTable::new(pieces)
    }

    /// The pieces of one pass through the contents, over song ticks
    /// `from..to`, the first of which plays `beat`: a piece between each
    /// tempo change and warp marker, and more where the tempo ramps.
    fn pass(
        &self,
        pieces: &mut Vec<Piece>,
        tempo: &TempoMap,
        r: f64,
        (from, to): (i64, i64),
        beat: f64,
    ) {
        if to <= from {
            return;
        }
        let at = |tick: i64| {
            let frame = r * tempo.ticks_to_seconds(Ticks(tick));
            let position = self.contents.position(beat + (tick - from) as f64);
            (frame, position)
        };
        let mut ticks = vec![from, to];
        ticks.extend(
            tempo
                .segments()
                .map(|segment| segment.start().get())
                .filter(|&tick| from < tick && tick < to),
        );
        ticks.extend(
            self.contents
                .map
                .markers()
                .iter()
                .map(|marker| from + (marker.beat as f64 - beat).round() as i64)
                .filter(|&tick| from < tick && tick < to),
        );
        ticks.sort_unstable();
        ticks.dedup();
        let mut knots = Vec::with_capacity(ticks.len());
        for pair in ticks.windows(2) {
            knots.push((pair[0], at(pair[0])));
            Self::refine(
                &at,
                (pair[0], at(pair[0])),
                (pair[1], at(pair[1])),
                0,
                &mut knots,
            );
        }
        knots.push((to, at(to)));
        for pair in knots.windows(2) {
            let ((_, (frame, position)), (_, (next_frame, next_position))) = (pair[0], pair[1]);
            if next_frame - frame > 1e-9 {
                pieces.push(Piece {
                    frame,
                    position,
                    slope: (next_position - position) / (next_frame - frame),
                });
            }
        }
    }

    /// Adds knots between `a` and `b` (not themselves) wherever a straight
    /// line from one to the other strays from the exact position at the
    /// middle by more than the tolerance: a tempo ramp curves.
    fn refine(
        at: &impl Fn(i64) -> (f64, f64),
        a: (i64, (f64, f64)),
        b: (i64, (f64, f64)),
        depth: u32,
        knots: &mut Vec<(i64, (f64, f64))>,
    ) {
        let middle = a.0 + (b.0 - a.0) / 2;
        if depth >= 24 || middle == a.0 {
            return;
        }
        let (frame, position) = at(middle);
        let (a_frame, a_position) = a.1;
        let (b_frame, b_position) = b.1;
        let straight =
            a_position + (b_position - a_position) * (frame - a_frame) / (b_frame - a_frame);
        let stray = (position - straight).abs();
        if stray.is_nan() || stray <= TOLERANCE {
            return;
        }
        let mid = (middle, (frame, position));
        Self::refine(at, a, mid, depth + 1, knots);
        knots.push(mid);
        Self::refine(at, mid, b, depth + 1, knots);
    }
}

impl Contents {
    /// The plan of a Beats clip (R14 to R17): a segment at the clip's Start
    /// marker, at each warp marker and at each of the file's transients, or
    /// at each line of the clip's beats under a grid, over every pass through
    /// its contents.
    fn plan(
        &self,
        clip: &Clip,
        tempo: &TempoMap,
        rate: f64,
        settings: BeatsSettings,
        media: &BeatsMedia<'_>,
    ) -> Plan {
        let preserve = match settings.preserve {
            ClipPreserve::Transients => Preserve::Transients,
            ClipPreserve::Bar => Preserve::Grid {
                division: beats::grid(4.0),
            },
            ClipPreserve::Half => Preserve::Grid {
                division: beats::grid(2.0),
            },
            ClipPreserve::Quarter => Preserve::Grid {
                division: beats::grid(1.0),
            },
            ClipPreserve::Eighth => Preserve::Grid {
                division: beats::grid(0.5),
            },
            ClipPreserve::Sixteenth => Preserve::Grid {
                division: beats::grid(0.25),
            },
            ClipPreserve::ThirtySecond => Preserve::Grid {
                division: beats::grid(0.125),
            },
        };
        let loop_mode = match settings.transient_loop_mode {
            TransientLoopMode::LoopOff => Loop::Off,
            TransientLoopMode::LoopForward => Loop::Forward,
            TransientLoopMode::LoopBackAndForth => Loop::BackAndForth,
        };
        let frame_of = |tick: i64| rate * tempo.ticks_to_seconds(Ticks(tick));
        let end = clip.end().get();
        let mut segments: Vec<Segment> = Vec::new();
        let mut tick = clip.start.get();
        let mut beat = self.start;
        if self.looping {
            let length = self.loop_end - self.loop_start;
            if length <= 0.0 {
                return Plan::new(Vec::new(), loop_mode, settings.transient_envelope, rate);
            }
            if beat >= self.loop_end {
                beat = self.loop_start + (beat - self.loop_end).rem_euclid(length);
            }
        }
        for pass in 0..=MAX_SEGMENT_RUNS {
            if tick >= end || pass == MAX_SEGMENT_RUNS {
                break;
            }
            let room = if self.looping {
                self.loop_end - beat
            } else {
                self.end.map_or(f64::INFINITY, |stop| stop - beat)
            };
            if room <= 0.0 {
                break;
            }
            let reaches = tick as f64 + room;
            let stop = if reaches >= end as f64 {
                end
            } else {
                reaches.round() as i64
            };
            self.pass_segments(
                &mut segments,
                (tick, stop),
                beat,
                preserve,
                media.transients,
                &frame_of,
                &|frame| {
                    media.sound.map_or(0.0, |sound| {
                        sound.value(ClipParam::SampleOffset, frame) * SIXTEENTH as f64
                    })
                },
            );
            if stop >= end {
                break;
            }
            if self.looping {
                tick = stop;
                beat = self.loop_start;
            } else {
                break;
            }
        }
        // Fill in `until` and the turns: the segments tile the clip's passes,
        // so each one ends where the next begins.
        for i in 0..segments.len() {
            let until = segments
                .get(i + 1)
                .map_or_else(|| frame_of(end).round() as i64, |next| next.frame);
            segments[i].until = until;
        }
        let needs_fill = loop_mode != Loop::Off
            && segments.iter().any(|segment| {
                segment.end > segment.position
                    && segment.end - segment.position < (segment.until - segment.frame) as f64
            });
        if needs_fill && let Some(samples) = (media.samples)() {
            let channels: Vec<&[f32]> = samples.channels().collect();
            for segment in &mut segments {
                segment.turn = segment_turn(segment, &channels, samples.frames());
            }
        }
        Plan::new(segments, loop_mode, settings.transient_envelope, rate)
    }

    /// Adds the segments of one pass through the contents, over song ticks
    /// `from..to`, the first of which plays `beat` (R14).
    fn pass_segments(
        &self,
        segments: &mut Vec<Segment>,
        (from, to): (i64, i64),
        beat: f64,
        preserve: Preserve,
        transients: &[f64],
        frame_of: &dyn Fn(i64) -> f64,
        offset: &dyn Fn(i64) -> f64,
    ) {
        let ticks = (to - from) as f64;
        if ticks <= 0.0 {
            return;
        }
        let mut beats = vec![beat];
        match preserve {
            Preserve::Transients => {
                beats.extend(self.map.markers().iter().map(|marker| marker.beat as f64));
                beats.extend(transients.iter().map(|time| self.map.time_to_beat(*time)));
            }
            Preserve::Grid { division } => {
                beats.extend(beats::grid_lines(beat, ticks, division));
            }
        }
        let mut lines: Vec<i64> = beats
            .into_iter()
            .filter(|b| b.is_finite() && *b >= beat && *b < beat + ticks)
            .map(|b| from + (b - beat).round() as i64)
            .filter(|tick| (from..to).contains(tick))
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines.push(to);
        for pair in lines.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // Spec: warp-modes R30. Sample Offset moves where the segment
            // is taken from, as it is where the segment begins.
            let moved = offset(frame_of(a).round() as i64);
            let position = self.position(beat + (a - from) as f64 + moved);
            let end = self.position(beat + (b - from) as f64 + moved);
            segments.push(Segment {
                frame: frame_of(a).round() as i64,
                until: frame_of(b).round() as i64,
                position,
                end,
                turn: position + (end - position) / 2.0,
            });
        }
    }
}

/// The file frame a segment's loop turns at (R15): the zero crossing of the
/// channels' sum nearest its middle, or the middle where the file isn't at
/// hand or the segment holds none.
fn segment_turn(segment: &Segment, channels: &[&[f32]], frames: usize) -> f64 {
    let middle = segment.position + (segment.end - segment.position) / 2.0;
    if channels.is_empty() || segment.end <= segment.position {
        return middle;
    }
    let from = segment.position.ceil().max(0.0) as usize;
    let to = (segment.end.floor().max(0.0) as usize).min(frames);
    if to <= from {
        return middle;
    }
    let turn = beats::turn(channels, from, to) as f64;
    turn.clamp(segment.position, segment.end)
}

/// Where a run of frames reads.
pub(crate) enum Positions<'a> {
    /// An Arrangement clip's table.
    Table(&'a WarpTable),
    /// A Session clip, launched on a song frame: the tempo map, followed
    /// from there.
    Launched {
        warped: &'a Warped,
        tempo: &'a TempoMap,
        launch: i64,
        rate: f64,
    },
    /// An unwarped clip transposed: its file read at the pitch ratio (R27),
    /// from the song frame it starts playing at.
    Flat(Flat),
}

/// Where an unwarped clip transposed reads (R27): its file at the pitch
/// ratio, so its speed and its pitch move together.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flat {
    /// The song frame the clip starts playing at.
    pub(crate) origin: i64,
    /// The file frame it plays there.
    pub(crate) start: f64,
    /// The file frame its region ends at.
    pub(crate) end: f64,
    /// Its loop brace, where it loops.
    pub(crate) loop_brace: Option<(f64, f64)>,
    /// The pitch ratio (R25).
    pub(crate) ratio: f64,
}

/// A run of frames over which the position advances smoothly.
struct Fill {
    frames: usize,
    /// Whether the clip plays any audio over it.
    sound: bool,
    /// File frames per song frame.
    speed: f64,
    /// Whether it reads the file frame for frame (the warp modes spec, R4).
    neutral: bool,
}

impl Positions<'_> {
    /// Works out the file position of the frames from song frame `first`
    /// into `out`, for as many as run on smoothly (at least one).
    fn fill(&self, first: i64, out: &mut [f64]) -> Fill {
        match self {
            Positions::Table(table) => {
                let Some((piece, frames)) = table.run(first, out.len()) else {
                    return Fill {
                        frames: out.len(),
                        sound: false,
                        speed: 1.0,
                        neutral: false,
                    };
                };
                for (i, position) in out[..frames].iter_mut().enumerate() {
                    *position = piece.position_at(first + i as i64);
                }
                Fill {
                    frames,
                    sound: !piece.is_silent(),
                    speed: piece.slope,
                    neutral: piece.slope == 1.0,
                }
            }
            Positions::Launched {
                warped,
                tempo,
                launch,
                rate,
            } => {
                let started = tempo.seconds_to_ticks_exact(*launch as f64 / rate);
                let mut frames = 0;
                let mut wraps = 0;
                for (i, position) in out.iter_mut().enumerate() {
                    let now = tempo.seconds_to_ticks_exact((first + i as i64) as f64 / rate);
                    // Frames before the launch frame play its first.
                    let played = (now - started).max(0.0);
                    let Some((beat, passes)) = warped.contents.beat_after(played) else {
                        if i == 0 {
                            return Fill {
                                frames: out.len(),
                                sound: false,
                                speed: 1.0,
                                neutral: false,
                            };
                        }
                        break;
                    };
                    if i == 0 {
                        wraps = passes;
                    } else if passes != wraps {
                        break;
                    }
                    *position = warped.contents.position(beat);
                    frames = i + 1;
                }
                let span = out[frames - 1] - out[0];
                let (speed, neutral) = if frames > 1 {
                    (
                        span / (frames - 1) as f64,
                        (span - (frames - 1) as f64).abs() < 1e-6,
                    )
                } else {
                    (1.0, false)
                };
                Fill {
                    frames,
                    sound: true,
                    speed,
                    neutral,
                }
            }
            Positions::Flat(flat) => {
                // Spec: warp-modes R27. The file is read at the pitch ratio
                // from the clip's Start marker, so its speed and its pitch
                // move together.
                let mut frames = 0;
                let mut previous: Option<f64> = None;
                for (i, position) in out.iter_mut().enumerate() {
                    let elapsed = (first + i as i64 - flat.origin).max(0) as f64;
                    let linear = flat.start + elapsed * flat.ratio;
                    let at = match flat.loop_brace {
                        Some((loop_start, loop_end)) if loop_end > loop_start => {
                            let length = loop_end - loop_start;
                            if linear < loop_end {
                                linear
                            } else {
                                loop_start + (linear - loop_end) % length
                            }
                        }
                        _ => {
                            if linear >= flat.end {
                                break;
                            }
                            linear
                        }
                    };
                    // A loop's wrap: the run ends there, and the next starts
                    // afresh at the brace.
                    if previous.is_some_and(|before| at < before) {
                        break;
                    }
                    *position = at;
                    previous = Some(at);
                    frames = i + 1;
                }
                Fill {
                    frames,
                    sound: frames > 0,
                    speed: flat.ratio,
                    neutral: flat.ratio == 1.0,
                }
            }
        }
    }
}

/// What a clip has read of its file, kept so a Beats loop mode can read back
/// into it without another read of the source (the warp modes spec, R15).
///
/// A ring of powers of two frames, holding the last frames read: a read the
/// ring covers is served from memory, and the rest is read from the source
/// and kept. A read behind what it holds, which is what the loop modes do, is
/// therefore served without touching the disk.
pub(crate) struct History {
    channels: usize,
    /// Frames per channel, a power of two.
    capacity: usize,
    mask: usize,
    data: Vec<f32>,
    /// How many frames have been kept: the frame numbers held are the ones
    /// before it, `written - held..written`.
    written: u64,
    /// How many frames are held, at most the capacity.
    held: usize,
}

impl History {
    /// A history of `channels` channels holding `frames` frames at most, as
    /// it rounds up to a power of two.
    pub(crate) fn new(channels: usize, frames: usize) -> Self {
        let capacity = frames.max(1).next_power_of_two();
        Self {
            channels,
            capacity,
            mask: capacity - 1,
            data: vec![0.0; channels * capacity],
            written: 0,
            held: 0,
        }
    }

    /// How many frames it holds at most.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    /// The frame numbers it holds: what a read inside them is served from
    /// memory.
    fn held(&self) -> std::ops::Range<u64> {
        self.written.saturating_sub(self.held as u64)..self.written
    }

    /// Fills `out` (planar, `channels` channels `stride` apart) with the file
    /// frames `from..from + len`, from what it holds and from `source`.
    pub(crate) fn read(
        &mut self,
        source: &mut dyn AudioSource,
        from: i64,
        len: usize,
        channels: usize,
        stride: usize,
        out: &mut [f32],
    ) {
        let start = from.max(0) as u64;
        let end = start + len as u64;
        if !self.held().contains(&start) && start != self.written {
            // The read begins outside what it holds (a locate, or a loop's
            // read back past it): what it held is no use, and the whole read
            // comes from the source.
            self.written = start;
            self.held = 0;
        }
        let held = self.held();
        let covered = start.max(held.start)..end.min(held.end);
        if covered.end > covered.start {
            let mut at = (covered.start - start) as usize;
            let mut frame = covered.start;
            let mut left = (covered.end - covered.start) as usize;
            while left > 0 {
                let index = (frame & self.mask as u64) as usize;
                let run = left.min(self.capacity - index);
                for c in 0..self.channels.min(channels) {
                    let ring = &self.data[c * self.capacity..(c + 1) * self.capacity];
                    out[c * stride + at..c * stride + at + run]
                        .copy_from_slice(&ring[index..index + run]);
                }
                at += run;
                frame += run as u64;
                left -= run;
            }
        }
        let missing = held.end..end;
        if missing.end <= missing.start {
            return;
        }
        let at = (missing.start - start) as usize;
        let n = (missing.end - missing.start) as usize;
        let mut view = PlanarMut::new(&mut out[at..], channels, n, stride);
        source.read(missing.start, &mut view, Transition::Cut);
        // The frames read are kept, for a loop mode's read back (R15).
        self.push(n, &view);
    }

    /// Keeps `n` frames just read, given by `view`.
    fn push(&mut self, frames: usize, view: &PlanarMut<'_>) {
        let keep = frames.min(self.capacity);
        let mut at = (self.written & self.mask as u64) as usize;
        let mut left = keep;
        let mut from = frames - keep;
        while left > 0 {
            let run = left.min(self.capacity - at);
            for c in 0..self.channels.min(view.channels()) {
                let ring = &mut self.data[c * self.capacity..(c + 1) * self.capacity];
                ring[at..at + run].copy_from_slice(&view.channel(c)[from..from + run]);
            }
            at = (at + run) & self.mask;
            from += run;
            left -= run;
        }
        self.written += keep as u64;
        self.held = (self.held + keep).min(self.capacity);
    }
}

/// The plan a mode that isn't Beats is given: no segment at all.
static EMPTY_PLAN: Plan = Plan::EMPTY;

/// A list's loop read-back rooms, one per clip, in the list's order (R15).
pub(crate) type Histories = Box<[Option<Box<History>>]>;

/// What a clip was playing before a mode change, made with the new clip list
/// so the change can crossfade from it (R8).
pub(crate) struct Past {
    pub(crate) playing: Playing,
    pub(crate) warped: Arc<Warped>,
}

/// A mode change crossfading into the new one over 20 ms (R8): the clip it is
/// for and how far the fade has come. It owns nothing, so nothing of it is
/// freed on the audio thread.
#[derive(Clone, Copy)]
pub(crate) struct Changing {
    pub(crate) clip: bl_model::ClipId,
    /// The fade's progress, 0 to 1, a frame at a time.
    pub(crate) fade: crate::mixer::Ramp,
}

/// A track's Complex and Complex Pro stretchers (R5, R21): one pool per mode,
/// sized by the most of the track's clips that can sound at once, made with
/// the track's clip list off the audio thread, and handed to the player with
/// it, so a fresh player adopts the pool rather than building one. A clip
/// plays through the stretcher of its own mode at its lane, so two of the
/// track's clips that can sound at once never share one (R6).
pub(crate) struct Stretchers {
    complex: Box<[Complex]>,
    complex_pro: Box<[Complex]>,
}

impl Stretchers {
    /// Pools of `lanes` stretchers for each mode some clip of the track uses;
    /// a mode no clip uses gets no stretcher at all, so a track that doesn't
    /// sound through the library pays nothing for it.
    pub(crate) fn new(channels: usize, rate: f64, lanes: usize, complex: bool, pro: bool) -> Self {
        let pool = |used: bool, pro: bool| -> Box<[Complex]> {
            if used {
                (0..lanes)
                    .map(|_| Complex::new(channels, rate, pro))
                    .collect()
            } else {
                Box::new([])
            }
        };
        Self {
            complex: pool(complex, false),
            complex_pro: pool(pro, true),
        }
    }

    /// The Complex stretcher for a clip in `lane`.
    fn complex(&mut self, lane: usize) -> &mut Complex {
        &mut self.complex[lane]
    }

    /// The Complex Pro stretcher for a clip in `lane`.
    fn complex_pro(&mut self, lane: usize) -> &mut Complex {
        &mut self.complex_pro[lane]
    }
}

/// What a track's stretcher pool holds (WP-4.6, R5, R21): how many lanes, and
/// which of the frequency-domain modes. The compiler keeps this per track, so
/// it knows what the player already has: a list whose needs that pool still
/// meets carries no pool of its own, and the player keeps the one it has,
/// which is what spares a parameter drag (Formants, gain) the cost of making
/// the stretchers again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PoolSpec {
    /// How many lanes the pool has, one per clip that can sound at once.
    pub(crate) lanes: usize,
    /// Whether it holds Complex stretchers.
    pub(crate) complex: bool,
    /// Whether it holds Complex Pro stretchers.
    pub(crate) pro: bool,
}

impl PoolSpec {
    /// Whether a pool of this shape serves a list that needs `needs`: it does
    /// while it has at least the lanes and every mode the list asks for.
    pub(crate) fn covers(self, needs: Self) -> bool {
        self.lanes >= needs.lanes && (!needs.complex || self.complex) && (!needs.pro || self.pro)
    }
}

/// The player's room for warped playback: the positions of a run and what the
/// mode rendered, both planar and made when the player was. The window of the
/// file around the run comes with the clip list ([`WindowBuf`]), since how
/// wide it must be is the clips' business, and so do the Complex and Complex
/// Pro stretchers ([`Stretchers`]).
pub(crate) struct WarpScratch {
    channels: usize,
    max_frames: usize,
    positions: Vec<f64>,
    rendered: Vec<f32>,
    /// The mode that was playing before a change, crossfaded in (R8).
    previous: Option<Changing>,
    /// How many frames a mode change's crossfade takes: 20 ms at the
    /// engine's rate (R8).
    change_frames: u32,
    /// What the previous mode rendered, blended into [`rendered`](Self::rendered).
    rendered_previous: Vec<f32>,
    /// Every mode renders through Re-Pitch until its package builds it; Beats
    /// is WP-4.4's.
    repitch: RePitch,
    beats: Beats,
    tones: Tones,
    texture: Texture,
}

impl WarpScratch {
    pub(crate) fn new(channels: usize, max_frames: usize, rate: SampleRate) -> Self {
        Interpolation::warm();
        Self {
            channels,
            max_frames,
            positions: vec![0.0; max_frames],
            rendered: vec![0.0; channels * max_frames],
            previous: None,
            change_frames: (CHANGE_SECONDS * rate.as_f64()).round() as u32,
            rendered_previous: vec![0.0; channels * max_frames],
            repitch: RePitch,
            beats: Beats,
            tones: Tones::new(channels, rate.as_f64()),
            texture: Texture::new(channels, rate.as_f64(), 0),
        }
    }

    /// Begins crossfading a clip's new mode in from the one it was playing
    /// (R8), from the block the new list arrived in. A second change within
    /// the fade replaces the first.
    pub(crate) fn change(&mut self, clip: bl_model::ClipId) {
        let mut fade = crate::mixer::Ramp::at(0.0);
        fade.toward(1.0, self.change_frames.max(1));
        self.previous = Some(Changing { clip, fade });
    }

    /// Forgets the crossfade a new list brings, so the next clip's change
    /// starts a fresh one.
    pub(crate) fn forget_change(&mut self) {
        self.previous = None;
    }

    /// Channel `c` of the frames the last [`render`](Self::render) made.
    pub(crate) fn rendered(&self, c: usize, frames: usize) -> &[f32] {
        &self.rendered[c * self.max_frames..c * self.max_frames + frames]
    }

    /// Renders the clip's next frames from song frame `first`, up to `want`:
    /// how many it covered, and whether they hold sound (if not, nothing was
    /// rendered). Starts the mode afresh when the clip starts or jumps is
    /// the caller's business ([`WarpMode::reset`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        clip: bl_model::ClipId,
        source: &mut dyn AudioSource,
        window: &mut WindowBuf,
        positions: &Positions<'_>,
        launched: Option<i64>,
        first: i64,
        want: usize,
        playing: Playing,
        plan: Option<&Plan>,
        past: Option<&Past>,
        history: Option<&mut History>,
        stretchers: &mut Stretchers,
        lane: usize,
    ) -> (usize, bool) {
        // Spec: warp-modes R8. A clip the new list says was playing something
        // else crossfades from it, from this block's first frame.
        if past.is_some()
            && self
                .previous
                .as_ref()
                .is_none_or(|previous| previous.clip != clip)
        {
            self.change(clip);
        }
        let mut fade = self
            .previous
            .as_ref()
            .filter(|previous| previous.clip == clip)
            .map(|previous| previous.fade);
        let (n, sound) = self.paint(
            source,
            window,
            positions,
            first,
            want,
            playing,
            plan,
            history,
            Out::New,
            stretchers,
            lane,
        );
        let (Some(mut fade), Some(past)) = (fade.take(), past) else {
            return (n, sound);
        };
        if !sound || fade.is_settled() {
            return (n, sound);
        }
        let old_positions = match launched {
            Some(launch) => past
                .warped
                .launched(positions.tempo(), launch, positions.rate()),
            None => past.warped.positions(),
        };
        let (old_n, old_sound) = self.paint(
            source,
            window,
            &old_positions,
            first,
            n,
            past.playing,
            launched.is_none().then(|| past.warped.plan()),
            None,
            Out::Previous,
            stretchers,
            lane,
        );
        let channels = self.channels;
        let max_frames = self.max_frames;
        if old_sound {
            // A frame at a time, whatever the block size (R6).
            let blend = old_n.min(n);
            for i in 0..blend {
                let now = fade.next();
                for c in 0..channels {
                    let at = c * max_frames + i;
                    let was = f64::from(self.rendered_previous[at]);
                    self.rendered[at] =
                        (was * (1.0 - now) + f64::from(self.rendered[at]) * now) as f32;
                }
            }
        }
        // Whatever the old mode didn't cover goes on at the new mode's own
        // level.
        for _ in 0..(n - old_n.min(n)) {
            fade.next();
        }
        if let Some(previous) = self.previous.as_mut()
            && previous.clip == clip
        {
            previous.fade = fade;
        }
        (n, true)
    }

    /// Renders one mode's frames of the run, into the output `out` names.
    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        source: &mut dyn AudioSource,
        window: &mut WindowBuf,
        positions: &Positions<'_>,
        first: i64,
        want: usize,
        playing: Playing,
        plan: Option<&Plan>,
        history: Option<&mut History>,
        out: Out,
        stretchers: &mut Stretchers,
        lane: usize,
    ) -> (usize, bool) {
        let want = want.min(self.max_frames);
        let fill = positions.fill(first, &mut self.positions[..want]);
        if !fill.sound {
            return (fill.frames, false);
        }
        let width = window.frames;
        let Self {
            channels,
            max_frames,
            positions: all,
            rendered,
            rendered_previous,
            repitch,
            beats,
            tones,
            texture,
            ..
        } = self;
        let rendered = match out {
            Out::New => rendered,
            Out::Previous => rendered_previous,
        };
        let mut n = fill.frames;
        // Spec: warp-modes R10, R26, R27. An unwarped clip reads its file at
        // the pitch ratio, and Re-Pitch reads the file at the clip's speed
        // and changes nothing else, so both read the positions they're given.
        let mode: &mut dyn WarpMode = if !playing.warp {
            repitch
        } else {
            match playing.mode {
                ClipWarpMode::Beats => beats,
                ClipWarpMode::Tones => {
                    tones.set_grain_size(playing.tones.grain_size);
                    tones
                }
                ClipWarpMode::Texture => {
                    texture.set_clip_id(playing.clip_id);
                    texture.set_grain_size(playing.texture.grain_size);
                    texture.set_flux(playing.texture.flux);
                    texture
                }
                ClipWarpMode::Complex => stretchers.complex(lane),
                ClipWarpMode::ComplexPro => {
                    let pro = stretchers.complex_pro(lane);
                    pro.set_formants(playing.complex_pro.formants);
                    pro.set_envelope(playing.complex_pro.envelope);
                    pro
                }
                ClipWarpMode::RePitch => repitch,
            }
        };
        let plan = plan.unwrap_or(&EMPTY_PLAN);
        // Spec: warp-modes R10, R20, R26. A mode that takes the clip's
        // pitch reads the file at the ratio, so its run isn't the neutral
        // one; Re-Pitch and the modes that have not landed yet ignore the
        // clip's pitch, and keep the neutral run. The frequency-domain modes
        // are never neutral, and read their own latency either side however
        // steady the run is (R21, R24).
        let takes_pitch = !playing.warp
            || matches!(
                playing.mode,
                ClipWarpMode::Beats
                    | ClipWarpMode::Tones
                    | ClipWarpMode::Texture
                    | ClipWarpMode::Complex
                    | ClipWarpMode::ComplexPro
            );
        let neutral = fill.neutral
            && !(takes_pitch && playing.pitch != 1.0)
            && !matches!(
                playing.mode,
                ClipWarpMode::Complex | ClipWarpMode::ComplexPro
            );
        let (request, lo, len) = loop {
            let run = &all[..n];
            let request = Request {
                positions: run,
                first,
                speed: fill.speed,
                neutral,
                high_quality: playing.high_quality,
                beats: plan,
                transient_envelope: playing.transient_envelope,
                pitch: playing.pitch,
                clip: playing.clip_id,
            };
            let (before, after) = mode.reach(&request);
            // The window holds every position the run reads, whichever way it
            // runs: a Back-and-Forth pass reads backwards, so the run's first
            // and last frames don't bound it.
            let (mut lo, mut hi) = (i64::MAX, i64::MIN);
            for &position in run {
                if neutral {
                    let frame = position.round() as i64;
                    lo = lo.min(frame);
                    hi = hi.max(frame + 1);
                } else {
                    let floor = position.floor() as i64;
                    lo = lo.min(floor - before as i64);
                    hi = hi.max(floor + after as i64 + 1);
                }
            }
            let len = usize::try_from(hi - lo).unwrap_or(usize::MAX);
            if len <= width {
                break (request, lo, len);
            }
            if n == 1 {
                // shortcut: a clip read faster than a window holds plays
                // silence there; upgrade if a package needs speeds that high.
                return (1, false);
            }
            n = (n / 2).max(1);
        };
        load(window, *channels, source, lo, len, history);
        let view = Window {
            data: &window.data,
            stride: width,
            channels: *channels,
            frames: len,
            first: lo,
        };
        mode.render(&request, &view, rendered, *max_frames);
        (n, true)
    }
}

/// Which of the scratch's outputs a render writes.
#[derive(Clone, Copy)]
enum Out {
    New,
    Previous,
}

impl Positions<'_> {
    /// The tempo map a launched clip follows; empty for a table.
    fn tempo(&self) -> &TempoMap {
        match self {
            Self::Table(_) | Self::Flat(_) => unreachable!("no tempo map"),
            Self::Launched { tempo, .. } => tempo,
        }
    }

    /// The rate a launched clip reads at.
    fn rate(&self) -> f64 {
        match self {
            Self::Table(_) | Self::Flat(_) => unreachable!("no rate"),
            Self::Launched { rate, .. } => *rate,
        }
    }
}

/// Reads file frames `lo..lo + len` into the planar `window`, silence where
/// they fall before the file; a mono file plays on every channel. A `history`
/// serves what it holds and keeps what it reads (R15).
fn load(
    window: &mut WindowBuf,
    channels: usize,
    source: &mut dyn AudioSource,
    lo: i64,
    len: usize,
    history: Option<&mut History>,
) {
    let width = window.frames;
    let data = &mut window.data;
    let skip = usize::try_from(-lo).map_or(0, |before| before.min(len));
    for c in 0..channels {
        data[c * width..c * width + skip].fill(0.0);
    }
    let read = len - skip;
    if read > 0 {
        match history {
            Some(history) => {
                history.read(source, lo.max(0), read, channels, width, &mut data[skip..])
            }
            None => {
                let mut view = PlanarMut::new(&mut data[skip..], channels, read, width);
                source.read(lo.max(0) as u64, &mut view, Transition::Cut);
            }
        }
    }
    let source_channels = source.channels().clamp(1, channels);
    for c in source_channels..channels {
        let (from, to) = data.split_at_mut(c * width);
        let last = (source_channels - 1) * width;
        to[skip..len].copy_from_slice(&from[last + skip..last + len]);
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TempoMap};
    use bl_model::{Clip, ClipData, ClipId, ContentHash, WarpMarker};

    use super::*;

    /// What a Beats clip's plan asks of the media: nothing in these tests.
    fn media() -> BeatsMedia<'static> {
        BeatsMedia {
            sound: None,
            transients: &[],
            samples: &|| None,
        }
    }

    // Spec: warp-modes R15. A loop's read back, behind what the history
    // holds or overlapping its start, plays the file's own frames.
    #[test]
    fn a_history_serves_reads_behind_what_it_holds() {
        let ramp: Vec<f32> = (0..256).map(|i| i as f32).collect();
        let data = Arc::new(SampleData::new(48_000, vec![ramp]).unwrap());
        let mut source = bl_media::stream::MemorySource::new(data);
        let mut history = History::new(1, 8);
        let mut out = vec![0.0_f32; 8];
        let mut read = |history: &mut History, from: i64, len: usize| {
            history.read(&mut source, from, len, 1, 8, &mut out);
            out[..len].to_vec()
        };
        assert_eq!(read(&mut history, 100, 4), [100.0, 101.0, 102.0, 103.0]);
        // Backwards, wholly before what it holds (a Back-and-Forth pass).
        assert_eq!(read(&mut history, 96, 4), [96.0, 97.0, 98.0, 99.0]);
        // Overlapping its start (a Forward loop's turn).
        assert_eq!(
            read(&mut history, 94, 6),
            [94.0, 95.0, 96.0, 97.0, 98.0, 99.0]
        );
        // Overlapping its end, the rest read on.
        assert_eq!(read(&mut history, 98, 5), [98.0, 99.0, 100.0, 101.0, 102.0]);
    }

    fn clip(audio: &AudioData, start: Ticks, length: Ticks) -> Clip {
        Clip::new(
            ClipId(bl_core::id::Id::new(1).unwrap()),
            start,
            length,
            ClipData::Audio(audio.clone()),
        )
    }

    /// A four-beat loop, one second a beat at 60 BPM, warped at `bpm` from
    /// the file's start. It plays through Re-Pitch, whose table follows the
    /// warp map frame for frame; a Beats clip's table is its plan's segments
    /// instead, which these tests don't cover.
    fn loop_at(bpm: f64) -> AudioData {
        AudioData {
            warp: true,
            warp_mode: bl_model::WarpMode::RePitch,
            warp_markers: vec![WarpMarker {
                time: 0.0,
                beat: Ticks::ZERO,
            }],
            tempo: Some(bpm),
            ..AudioData::new(ContentHash([1; 32]))
        }
    }

    // Spec: warping R12. A song at the clip's own tempo reads the file frame
    // for frame; at twice it, twice as fast.
    #[test]
    fn a_table_reads_at_the_speed_the_tempo_sets() {
        let rate = SampleRate::HZ_48000;
        let audio = loop_at(60.0);
        let c = clip(&audio, Ticks::ZERO, Ticks::WHOLE * 4);
        let slow = TempoMap::constant(Bpm::new(60.0).unwrap());
        let fast = TempoMap::constant(Bpm::new(120.0).unwrap());
        let on = Warped::new(&c, &audio, &slow, rate, Some(480_000), true, &media()).unwrap();
        assert_eq!(on.table.pieces().len(), 1);
        assert_eq!(on.table.pieces()[0].slope, 1.0);
        assert_eq!(on.table.position_at(1_000), 1_000.0);
        let twice = Warped::new(&c, &audio, &fast, rate, Some(480_000), true, &media()).unwrap();
        assert!((twice.table.pieces()[0].slope - 2.0).abs() < 1e-12);
        assert!((twice.table.position_at(24_000) - 48_000.0).abs() < 1e-6);
    }

    // Spec: warping R12. Across a tempo ramp the table stays within a
    // hundredth of a frame of the exact position.
    #[test]
    fn a_table_follows_a_tempo_ramp_to_a_hundredth_of_a_frame() {
        let rate = SampleRate::HZ_48000;
        let audio = loop_at(100.0);
        let c = clip(&audio, Ticks::QUARTER, Ticks::WHOLE * 8);
        let tempo = TempoMap::from_breakpoints([
            (Ticks::ZERO, Bpm::new(60.0).unwrap()),
            (Ticks::WHOLE * 6, Bpm::new(180.0).unwrap()),
        ])
        .unwrap();
        let warped =
            Warped::new(&c, &audio, &tempo, rate, Some(48_000 * 60), true, &media()).unwrap();
        assert!(warped.table.pieces().len() > 4, "a ramp takes many pieces");
        let r = 48_000.0;
        let start = tempo.ticks_to_frames(c.start, rate).get();
        let end = tempo.ticks_to_frames(c.end(), rate).get();
        let mut worst: f64 = 0.0;
        for frame in (start..end).step_by(97) {
            let tick = tempo.seconds_to_ticks_exact(frame as f64 / r);
            let beat = audio.start + (tick - c.start.get() as f64);
            let exact = warped.contents.position(beat);
            worst = worst.max((warped.table.position_at(frame) - exact).abs());
        }
        assert!(worst < 0.01, "{worst}");
    }

    // Spec: warping R5. A loop wraps on the exact frame its beat falls on.
    #[test]
    fn a_loop_starts_a_new_pass_where_its_brace_ends() {
        let rate = SampleRate::HZ_48000;
        let audio = AudioData {
            looping: true,
            start: 0.0,
            loop_start: 0.0,
            loop_end: Some(Ticks::QUARTER.get() as f64 * 2.0),
            ..loop_at(60.0)
        };
        let c = clip(&audio, Ticks::ZERO, Ticks::WHOLE * 2);
        let tempo = TempoMap::constant(Bpm::new(60.0).unwrap());
        let warped = Warped::new(&c, &audio, &tempo, rate, Some(480_000), true, &media()).unwrap();
        // Two beats at 60 BPM are two seconds: the file position goes back
        // to 0 on frame 96,000.
        assert_eq!(warped.table.position_at(95_999), 95_999.0);
        assert_eq!(warped.table.position_at(96_000), 0.0);
        assert_eq!(warped.table.position_at(96_001), 1.0);
    }
}
