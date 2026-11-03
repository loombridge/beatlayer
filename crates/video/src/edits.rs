//! A track's edit list: where the track's media plays on the movie's timeline.

use crate::error::VideoError;
use crate::rational::Rational;

/// One edit: a stretch of the movie's timeline that plays a stretch of the
/// track's media, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Where the edit starts on the timeline, in movie time scale units.
    pub start: u64,
    /// How long it lasts, in movie time scale units.
    pub duration: u64,
    /// The media time it plays from, in the track's media time scale units,
    /// or `None` for an empty edit: a gap.
    pub media_time: Option<u64>,
}

/// A track's edits in timeline order. Time not covered by any edit is a gap
/// too (R4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditList {
    movie_scale: u32,
    media_scale: u32,
    edits: Vec<Edit>,
}

impl EditList {
    /// The edits `elst` lists (its body), or, without one, a single edit
    /// that plays all of `media_duration`.
    pub(crate) fn parse(
        elst: Option<&[u8]>,
        movie_scale: u32,
        media_scale: u32,
        media_duration: u64,
    ) -> Result<Self, VideoError> {
        let mut edits = Vec::new();
        match elst {
            None => {
                let duration = u64::try_from(
                    u128::from(media_duration) * u128::from(movie_scale) / u128::from(media_scale),
                )
                .map_err(|_| malformed("a track longer than 2^64 units"))?;
                edits.push(Edit {
                    start: 0,
                    duration,
                    media_time: Some(0),
                });
            }
            Some(body) => {
                let version = *body.first().ok_or_else(|| malformed("an empty elst"))?;
                let count = u32::from_be_bytes(
                    body.get(4..8)
                        .and_then(|b| b.try_into().ok())
                        .ok_or_else(|| malformed("a truncated elst"))?,
                );
                let entry_len = if version == 1 { 20 } else { 12 };
                let entries = body.get(8..).unwrap_or_default();
                let mut start = 0u64;
                for entry in entries.chunks_exact(entry_len).take(count as usize) {
                    let be32 = |at: usize| {
                        u32::from_be_bytes(entry[at..at + 4].try_into().unwrap_or_default())
                    };
                    let be64 = |at: usize| {
                        u64::from_be_bytes(entry[at..at + 8].try_into().unwrap_or_default())
                    };
                    let (duration, media_time, rate) = if version == 1 {
                        (be64(0), be64(8) as i64, be32(16))
                    } else {
                        (u64::from(be32(0)), i64::from(be32(4) as i32), be32(8))
                    };
                    // The rate is 16.16 fixed point; only an edit at the
                    // media's own speed is read.
                    if rate != 1 << 16 {
                        return Err(VideoError::Unsupported(
                            "an edit list with a rate other than 1".into(),
                        ));
                    }
                    let media_time = match media_time {
                        -1 => None,
                        t if t >= 0 => Some(t as u64),
                        _ => return Err(malformed("an edit from before the start of the media")),
                    };
                    edits.push(Edit {
                        start,
                        duration,
                        media_time,
                    });
                    start = start
                        .checked_add(duration)
                        .ok_or_else(|| malformed("edits longer than 2^64 units"))?;
                }
                if edits.len() != count as usize {
                    return Err(malformed("an elst shorter than it says"));
                }
            }
        }
        Ok(Self {
            movie_scale,
            media_scale,
            edits,
        })
    }

    /// The edits, in timeline order.
    #[must_use]
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    /// The movie time scale the edits' timeline units are in.
    #[must_use]
    pub fn movie_scale(&self) -> u32 {
        self.movie_scale
    }

    /// The track's media time scale.
    #[must_use]
    pub fn media_scale(&self) -> u32 {
        self.media_scale
    }

    /// The media time that plays at timeline time `t`, in media time scale
    /// units with the fraction kept, or `None` in a gap.
    #[must_use]
    pub fn to_media(&self, t: Rational) -> Option<Rational> {
        let edit = self.edits.iter().find(|e| {
            let start = Rational::new(i128::from(e.start), i128::from(self.movie_scale));
            let end = Rational::new(
                i128::from(e.start) + i128::from(e.duration),
                i128::from(self.movie_scale),
            );
            e.media_time.is_some() && start <= t && t < end
        })?;
        let into = t.minus(Rational::new(
            i128::from(edit.start),
            i128::from(self.movie_scale),
        ));
        Some(into.plus(Rational::new(
            i128::from(edit.media_time?),
            i128::from(self.media_scale),
        )))
    }

    /// The timeline time at which media time `m` (in media time scale
    /// units) plays, in the first edit that plays it, or `None` if no edit
    /// does.
    #[must_use]
    pub fn to_timeline(&self, m: i64) -> Option<Rational> {
        self.edits.iter().find_map(|e| {
            let from = i128::from(e.media_time?);
            let into = i128::from(m) - from;
            // into / media_scale < duration / movie_scale
            let inside = into >= 0
                && into * i128::from(self.movie_scale)
                    < i128::from(e.duration) * i128::from(self.media_scale);
            inside.then(|| {
                Rational::new(i128::from(e.start), i128::from(self.movie_scale))
                    .plus(Rational::new(into, i128::from(self.media_scale)))
            })
        })
    }
}

fn malformed(what: &str) -> VideoError {
    VideoError::Malformed(what.into())
}
