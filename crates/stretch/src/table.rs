//! The table from song frames to file positions (the warping spec, R12 and
//! R17): what a clip player reads on the audio thread to find the file frame
//! a warped clip plays at each song frame.
//!
//! The compiler works the table out off the audio thread from the clip's warp
//! map under the song's tempo map. It is a list of pieces, in order of song
//! frame: within a piece the position advances in a straight line, and a
//! loop's wrap or the clip running out of audio starts a new piece. The
//! position at a frame is a function of that frame alone, so a render is the
//! same at any block size.

/// One stretch of a clip's playback over which the position advances in a
/// straight line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    /// The song frame the piece starts at. Perhaps between frames: the
    /// piece covers the frames from the first at or after it.
    pub frame: f64,
    /// The file position, in frames and perhaps fractional, at [`frame`](Self::frame);
    /// NaN where the clip plays nothing.
    pub position: f64,
    /// File frames per song frame: 1 where the song runs at the speed of the
    /// warp segment playing.
    pub slope: f64,
}

impl Piece {
    /// The file position at song frame `frame`: NaN where nothing plays.
    #[must_use]
    pub fn position_at(&self, frame: i64) -> f64 {
        self.position + (frame as f64 - self.frame) * self.slope
    }

    /// Whether the clip plays nothing over this piece.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.position.is_nan()
    }
}

/// How far from 1 a slope is still taken to be 1, so a clip warped to the
/// song's own tempo reads the file frame for frame (the warp modes spec, R4)
/// whatever the rounding of the arithmetic that found it.
const UNIT_SLOPE: f64 = 1e-9;

/// A clip's table: its pieces in order of song frame.
#[derive(Clone, Debug, PartialEq)]
pub struct WarpTable {
    pieces: Vec<Piece>,
}

impl WarpTable {
    /// A table of `pieces`, which must be in increasing order of frame. A
    /// slope within a billionth of 1 becomes exactly 1.
    #[must_use]
    pub fn new(mut pieces: Vec<Piece>) -> Self {
        for piece in &mut pieces {
            if (piece.slope - 1.0).abs() < UNIT_SLOPE {
                piece.slope = 1.0;
            }
        }
        Self { pieces }
    }

    /// The pieces, in order.
    #[must_use]
    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// The piece that covers song frame `frame` and how many of the next
    /// `want` frames from it (at least 1) it covers; `None` for a table
    /// without pieces. A frame before the first piece takes the first, and
    /// the last piece runs on.
    #[must_use]
    pub fn run(&self, frame: i64, want: usize) -> Option<(Piece, usize)> {
        let at = frame as f64;
        let after = self.pieces.partition_point(|piece| piece.frame <= at);
        let current = after.saturating_sub(1);
        let piece = *self.pieces.get(current)?;
        let frames = self.pieces.get(current + 1).map_or(want, |next| {
            // The frames before the next piece's start.
            let left = (next.frame - at).ceil();
            if left >= want as f64 {
                want
            } else {
                left.max(1.0) as usize
            }
        });
        Some((piece, frames))
    }

    /// The file position at song frame `frame`; NaN where nothing plays.
    #[must_use]
    pub fn position_at(&self, frame: i64) -> f64 {
        self.run(frame, 1)
            .map_or(f64::NAN, |(piece, _)| piece.position_at(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(frame: f64, position: f64, slope: f64) -> Piece {
        Piece {
            frame,
            position,
            slope,
        }
    }

    // Spec: warping R17
    #[test]
    fn a_table_finds_the_piece_a_frame_is_in_and_how_long_it_runs() {
        let table = WarpTable::new(vec![
            piece(10.5, 100.0, 2.0),
            piece(20.0, 500.0, 1.0 + 1e-12),
            piece(30.0, f64::NAN, 1.0),
        ]);
        // Before the first piece, it extends back.
        let (first, frames) = table.run(8, 100).unwrap();
        assert_eq!((first.position_at(8), frames), (95.0, 12));
        // The first piece starts at frame 11, the first at or after 10.5.
        assert_eq!(table.run(11, 100).unwrap().1, 9);
        assert_eq!(table.position_at(11), 101.0);
        // A slope within a billionth of 1 is 1.
        let (second, frames) = table.run(25, 3).unwrap();
        assert_eq!(
            (second.slope, second.position_at(25), frames),
            (1.0, 505.0, 3)
        );
        assert!(table.position_at(30).is_nan());
        assert!(table.run(40, 4).unwrap().0.is_silent());
        assert!(WarpTable::new(Vec::new()).run(0, 1).is_none());
    }
}
