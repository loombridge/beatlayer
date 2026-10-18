//! Warped clips' playback (WP-4.2, the warping spec R12 to R19 and the warp
//! modes spec R1 to R13): which file frame a warped clip plays at each song
//! frame, and the reading of the file that follows.
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
//! The player reads, for each run of frames over which the position advances
//! smoothly, a window of the file around the run, and a warp mode turns the
//! positions and the window into audio. The position at a frame depends on
//! that frame alone, so a render is the same at any block size.

use bl_core::time::{SampleRate, TempoMap, Ticks};
use bl_media::stream::{AudioSource, PlanarMut, Transition};
use bl_model::{AudioData, Clip};
use bl_stretch::{Interpolation, Piece, RePitch, Request, WarpMap, WarpMode, WarpTable, Window};

use crate::clip_source::WINDOW_FRAMES;

/// How many file frames before a position a warped clip may read: the widest
/// interpolator's reach, so a point a jump lands on is cued that far early.
pub(crate) const CUE_BACK: u64 = 264;

/// The most passes round a loop a table holds (a clip looped over a tick
/// would otherwise be a table without end): past it the clip is silent.
const MAX_RUNS: usize = 1 << 20;

/// How far, in file frames, a table's straight pieces may stray from the
/// exact position at their middles, where a smooth curve strays most (the
/// warping spec, R12 allows a hundredth of a frame).
const TOLERANCE: f64 = 0.005;

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
    /// The Arrangement clip's table; empty for a Session slot clip, and for
    /// a clip placed before its file's length is known.
    table: WarpTable,
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
        let mut warped = Self {
            contents,
            table: WarpTable::new(Vec::new()),
        };
        if tabulate {
            warped.table = warped.tabulate(clip, tempo, r);
        }
        Some(warped)
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
        }
    }
}

/// The player's room for warped playback: the positions of a run, the window
/// of the file around them, and what the mode rendered, all planar and made
/// when the player was.
pub(crate) struct WarpScratch {
    channels: usize,
    max_frames: usize,
    positions: Vec<f64>,
    window: Vec<f32>,
    rendered: Vec<f32>,
    /// Every mode renders through Re-Pitch until its package builds it.
    mode: RePitch,
}

impl WarpScratch {
    pub(crate) fn new(channels: usize, max_frames: usize) -> Self {
        Interpolation::warm();
        Self {
            channels,
            max_frames,
            positions: vec![0.0; max_frames],
            window: vec![0.0; channels * WINDOW_FRAMES],
            rendered: vec![0.0; channels * max_frames],
            mode: RePitch,
        }
    }

    /// Channel `c` of the frames the last [`render`](Self::render) made.
    pub(crate) fn rendered(&self, c: usize, frames: usize) -> &[f32] {
        &self.rendered[c * self.max_frames..c * self.max_frames + frames]
    }

    /// Renders the clip's next frames from song frame `first`, up to `want`:
    /// how many it covered, and whether they hold sound (if not, nothing was
    /// rendered). Starts the mode afresh when the clip starts or jumps is
    /// the caller's business ([`WarpMode::reset`]).
    pub(crate) fn render(
        &mut self,
        source: &mut dyn AudioSource,
        positions: &Positions<'_>,
        first: i64,
        want: usize,
        high_quality: bool,
    ) -> (usize, bool) {
        let want = want.min(self.max_frames);
        let fill = positions.fill(first, &mut self.positions[..want]);
        if !fill.sound {
            return (fill.frames, false);
        }
        let Self {
            channels,
            max_frames,
            positions: all,
            window,
            rendered,
            mode,
        } = self;
        let mut n = fill.frames;
        let (request, lo, len) = loop {
            let run = &all[..n];
            let request = Request {
                positions: run,
                speed: fill.speed,
                neutral: fill.neutral,
                high_quality,
            };
            let (before, after) = mode.reach(&request);
            let (lo, hi) = if fill.neutral {
                (run[0].round() as i64, run[n - 1].round() as i64 + 1)
            } else {
                (
                    run[0].floor() as i64 - before as i64,
                    run[n - 1].floor() as i64 + after as i64 + 1,
                )
            };
            let len = usize::try_from(hi - lo).unwrap_or(usize::MAX);
            if len <= WINDOW_FRAMES {
                break (request, lo, len);
            }
            if n == 1 {
                // shortcut: a clip read faster than a window holds plays
                // silence there; upgrade if a package needs speeds that high.
                return (1, false);
            }
            n = (n / 2).max(1);
        };
        load(window, *channels, source, lo, len);
        let view = Window {
            data: window,
            stride: WINDOW_FRAMES,
            channels: *channels,
            frames: len,
            first: lo,
        };
        mode.render(&request, &view, rendered, *max_frames);
        (n, true)
    }
}

/// Reads file frames `lo..lo + len` into the planar `window`, silence where
/// they fall before the file; a mono file plays on every channel.
fn load(window: &mut [f32], channels: usize, source: &mut dyn AudioSource, lo: i64, len: usize) {
    let skip = usize::try_from(-lo).map_or(0, |before| before.min(len));
    for c in 0..channels {
        window[c * WINDOW_FRAMES..c * WINDOW_FRAMES + skip].fill(0.0);
    }
    let read = len - skip;
    if read > 0 {
        let mut view = PlanarMut::new(&mut window[skip..], channels, read, WINDOW_FRAMES);
        source.read(lo.max(0) as u64, &mut view, Transition::Cut);
    }
    let source_channels = source.channels().clamp(1, channels);
    for c in source_channels..channels {
        let (from, to) = window.split_at_mut(c * WINDOW_FRAMES);
        let last = (source_channels - 1) * WINDOW_FRAMES;
        to[skip..len].copy_from_slice(&from[last + skip..last + len]);
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TempoMap};
    use bl_model::{Clip, ClipData, ClipId, ContentHash, WarpMarker};

    use super::*;

    fn clip(audio: &AudioData, start: Ticks, length: Ticks) -> Clip {
        Clip::new(
            ClipId(bl_core::id::Id::new(1).unwrap()),
            start,
            length,
            ClipData::Audio(audio.clone()),
        )
    }

    fn loop_at(bpm: f64) -> AudioData {
        // A four-beat loop, one second a beat at 60 BPM, warped at `bpm`
        // from the file's start.
        AudioData {
            warp: true,
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
        let on = Warped::new(&c, &audio, &slow, rate, Some(480_000), true).unwrap();
        assert_eq!(on.table.pieces().len(), 1);
        assert_eq!(on.table.pieces()[0].slope, 1.0);
        assert_eq!(on.table.position_at(1_000), 1_000.0);
        let twice = Warped::new(&c, &audio, &fast, rate, Some(480_000), true).unwrap();
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
        let warped = Warped::new(&c, &audio, &tempo, rate, Some(48_000 * 60), true).unwrap();
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
        let warped = Warped::new(&c, &audio, &tempo, rate, Some(480_000), true).unwrap();
        // Two beats at 60 BPM are two seconds: the file position goes back
        // to 0 on frame 96,000.
        assert_eq!(warped.table.position_at(95_999), 95_999.0);
        assert_eq!(warped.table.position_at(96_000), 0.0);
        assert_eq!(warped.table.position_at(96_001), 1.0);
    }
}
