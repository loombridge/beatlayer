//! Each track's current scale (WP-6.8; the scales spec, R15): the scale of
//! the clip playing on a track, or the set's current scale, or the chromatic
//! one where Scale Mode is off, changing on the frame a clip starts or stops.
//!
//! A track's clip player works it out for each block from its clip list,
//! which carries each clip's scale, and hands it to every device on the
//! track through [`Block`](crate::Block). A scale change reaches the player
//! as node data and the set's as a transport command, never as a program, and
//! nothing here allocates, locks or blocks.

use bl_model::{Scale, ScaleName, SceneId, TrackId};

use crate::node::Block;

/// The scale where Scale Mode is off: the chromatic one, whose degree is a
/// semitone (the scales spec, R2).
pub const OFF: Scale = Scale {
    enabled: false,
    root: 0,
    name: ScaleName::Major,
};

/// How many times a track's scale changes inside one block.
///
/// `shortcut:` a block in which more clips than this start or stop on one
/// track keeps the first changes and then the last scale; raise it if a real
/// set ever packs more clip edges than this into a block.
const CHANGES: usize = 7;

/// A track's current scale over one block: the scale at its first frame, and
/// the frames within the block where it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleRuns {
    first: Scale,
    changes: [(u32, Scale); CHANGES],
    len: usize,
}

impl ScaleRuns {
    /// A block over which the scale is `scale` throughout.
    #[must_use]
    pub const fn of(scale: Scale) -> Self {
        Self {
            first: scale,
            changes: [(0, OFF); CHANGES],
            len: 0,
        }
    }

    /// The scale at `offset` frames into the block.
    #[must_use]
    pub fn at(&self, offset: usize) -> Scale {
        self.changes[..self.len]
            .iter()
            .take_while(|(frame, _)| *frame as usize <= offset)
            .last()
            .map_or(self.first, |(_, scale)| *scale)
    }

    /// The scale at the block's last frame, which the next block starts
    /// from.
    #[must_use]
    pub fn last(&self) -> Scale {
        self.len
            .checked_sub(1)
            .map_or(self.first, |last| self.changes[last].1)
    }

    /// Changes the scale to `scale` from `offset` on, after the changes
    /// already made. For the player that works the scale out, and for tests
    /// of devices outside a program.
    pub fn push(&mut self, offset: u32, scale: Scale) {
        if scale == self.last() {
            return;
        }
        match self.len {
            CHANGES => self.changes[CHANGES - 1].1 = scale,
            len => {
                self.changes[len] = (offset, scale);
                self.len += 1;
            }
        }
    }
}

/// A clip as the scale sees it: where it plays, in song frames, and its
/// scale.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    /// The Session slot the clip sits in, or `None` for an Arrangement clip.
    pub(crate) scene: Option<SceneId>,
    /// The song frame of the clip's start, which it plays.
    pub(crate) start: i64,
    /// The song frame of its end, which it doesn't.
    pub(crate) end: i64,
    pub(crate) scale: Scale,
}

/// What a track's player keeps between blocks to work out its scale.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TrackScale {
    /// The scale at the last frame of the block before, if there was one:
    /// where a Session clip launched inside the next block takes over from.
    before: Option<Scale>,
}

/// The most clip edges one block's search visits.
const EDGES: usize = 2 * CHANGES;

/// The most Arrangement clips one block's search keeps.
const NEAR: usize = 16;

impl Span {
    /// A span that holds nothing, to fill a fixed array with.
    const NONE: Self = Self {
        scene: None,
        start: 0,
        end: 0,
        scale: OFF,
    };
}

impl TrackScale {
    /// The scale of `track` over `block`, given the placed `spans` of its
    /// clips (the scales spec, R15): the Session clip playing in its slot,
    /// or while the Arrangement plays and the transport runs, the
    /// Arrangement clip under the playhead; the set's current scale where no
    /// clip plays. A Session clip's start and stop land on their frames, an
    /// Arrangement clip's where the playhead enters and leaves it.
    pub(crate) fn runs(
        &mut self,
        track: TrackId,
        spans: impl Iterator<Item = Span> + Clone,
        block: &Block<'_>,
    ) -> ScaleRuns {
        let set = block.set_scale();
        let transport = block.transport();
        let from = transport.position.get();
        let until = transport.window_end(block.frames());
        let offset = |frame: i64| transport.offset(frame, block.frames()) as u32;
        let scale_of = |scene: SceneId| {
            spans
                .clone()
                .find(|span| span.scene == Some(scene))
                .map_or(set, |span| span.scale)
        };
        let runs = match block.session().iter().find(|entry| entry.track == track) {
            Some(entry) => match (entry.scene, entry.last) {
                (Some(scene), _) => {
                    let launched = entry.frame.get();
                    let mut runs = ScaleRuns::of(if launched > from {
                        self.before.unwrap_or(set)
                    } else {
                        scale_of(scene)
                    });
                    if launched > from && launched < until {
                        runs.push(offset(launched), scale_of(scene));
                    }
                    runs
                }
                // The Session clip stopped, and the track stays taken over.
                (None, Some(last)) if last.stopped.get() > from => {
                    let mut runs = ScaleRuns::of(scale_of(last.scene));
                    if last.stopped.get() < until {
                        runs.push(offset(last.stopped.get()), set);
                    }
                    runs
                }
                (None, _) => ScaleRuns::of(set),
            },
            None if transport.playing => {
                // One pass over the clips keeps the few that touch the block.
                //
                // `shortcut:` a block that more Arrangement clips than this
                // touch on one track ignores the rest; raise it if a real set
                // ever lays that many clips inside one block.
                let mut near = [Span::NONE; NEAR];
                let mut count = 0;
                for span in spans.filter(|span| span.scene.is_none()) {
                    if span.start < until && span.end > from && count < NEAR {
                        near[count] = span;
                        count += 1;
                    }
                }
                let near = &near[..count];
                let at = |frame: i64| {
                    near.iter()
                        .rfind(|span| span.start <= frame && frame < span.end)
                        .map_or(set, |span| span.scale)
                };
                let mut runs = ScaleRuns::of(at(from));
                let mut seen = from;
                for _ in 0..EDGES {
                    let next = near
                        .iter()
                        .flat_map(|span| [span.start, span.end])
                        .filter(|edge| *edge > seen && *edge < until)
                        .min();
                    let Some(edge) = next else { break };
                    runs.push(offset(edge), at(edge));
                    seen = edge;
                }
                runs
            }
            None => ScaleRuns::of(set),
        };
        self.before = Some(runs.last());
        runs
    }
}
