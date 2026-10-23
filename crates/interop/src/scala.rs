//! Tuning files (WP-6.9; the tunings spec, R3 to R5): reading a Scala `.scl`
//! file, and an `.ascl` file as one, into a [`Tuning`].
//!
//! A `.scl` file is lines of text: lines beginning with `!` are comments;
//! the first other line is the description (it may be empty); the next is
//! the number of pitches; then one pitch per line, in cents (written with a
//! decimal point) or as a ratio (`a/b`, or a whole number), the last being
//! the interval of repetition, the pseudo-octave. The unison is implied and
//! text after a pitch on its line is ignored.
//!
//! The reference pitch and range of the result are the defaults the spec
//! gives (R7, R8). What an `.ascl` file's comments add (note names, a
//! reference pitch, a range and a link) is not read yet.

use std::fmt;
use std::path::Path;

use bl_model::{MAX_CENTS, MAX_DEGREES, Tuning};

/// The most bytes a tuning file may hold; a bigger one is refused rather
/// than read.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// Why a tuning file was refused (the tunings spec, R3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalaError {
    /// The file couldn't be read.
    Unreadable(String),
    /// The file is bigger than [`MAX_FILE_BYTES`].
    TooBig,
    /// The file has no description line, so it isn't a tuning file.
    NoDescription,
    /// The line after the description isn't a count of pitches.
    NoCount,
    /// The file holds no pitches.
    NoPitches,
    /// More pitches than a tuning may hold ([`MAX_DEGREES`]).
    TooMany(usize),
    /// The count of pitches doesn't match the lines that follow: fewer
    /// lines than the count, or more.
    CountMismatch {
        /// The count the file gives.
        count: usize,
    },
    /// A pitch line that isn't cents or a ratio, by its place among the
    /// pitches, from 1.
    BadPitch(usize),
    /// A ratio of 0 or less, or with a denominator of 0, or a pitch too far
    /// from the unison to play, by its place among the pitches.
    OutOfRange(usize),
    /// The pseudo-octave is at or below the unison.
    PseudoOctaveFalls,
}

impl fmt::Display for ScalaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "the tuning file can't be read: {why}"),
            Self::TooBig => f.write_str("the tuning file is too big"),
            Self::NoDescription => f.write_str("the tuning file has no description line"),
            Self::NoCount => {
                f.write_str("the tuning file's second line is not a number of pitches")
            }
            Self::NoPitches => f.write_str("the tuning file holds no pitches"),
            Self::TooMany(n) => write!(
                f,
                "the tuning file holds {n} pitches, more than the {MAX_DEGREES} a tuning may"
            ),
            Self::CountMismatch { count } => write!(
                f,
                "the tuning file says it holds {count} pitches, but the lines that follow don't"
            ),
            Self::BadPitch(n) => write!(f, "pitch {n} is neither cents nor a ratio"),
            Self::OutOfRange(n) => write!(
                f,
                "pitch {n} is a ratio of 0 or less, or too far from the unison to play"
            ),
            Self::PseudoOctaveFalls => {
                f.write_str("the tuning's last pitch, the pseudo-octave, is not above the unison")
            }
        }
    }
}

impl std::error::Error for ScalaError {}

/// Reads the `.scl` or `.ascl` file at `path` as a tuning named after the
/// file (R5).
///
/// # Errors
///
/// [`ScalaError`] if the file can't be read or breaks R3.
pub fn read(path: &Path) -> Result<Tuning, ScalaError> {
    let unreadable = |error: std::io::Error| ScalaError::Unreadable(error.to_string());
    let size = std::fs::metadata(path).map_err(unreadable)?.len();
    if size > MAX_FILE_BYTES {
        return Err(ScalaError::TooBig);
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    // The name is the file's, without the extension and otherwise whole:
    // a dash and a number in it stay (R5).
    let name = path
        .file_stem()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    parse(&bytes, &name)
}

/// Reads tuning file text (UTF-8, or Latin-1 where it isn't) as a tuning
/// called `name`.
///
/// # Errors
///
/// [`ScalaError`] if the text breaks R3.
pub fn parse(bytes: &[u8], name: &str) -> Result<Tuning, ScalaError> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut lines = text.lines().filter(|line| !line.starts_with('!'));
    let description = lines.next().ok_or(ScalaError::NoDescription)?.trim();
    let count: usize = lines
        .next()
        .and_then(|line| line.split_whitespace().next())
        .and_then(|word| word.parse().ok())
        .ok_or(ScalaError::NoCount)?;
    if count == 0 {
        return Err(ScalaError::NoPitches);
    }
    if count > MAX_DEGREES {
        return Err(ScalaError::TooMany(count));
    }
    // A blank line between pitches is skipped, as other readers do.
    let mut rest = lines.filter(|line| !line.trim().is_empty());
    let mut cents = Vec::with_capacity(count);
    for place in 1..=count {
        let line = rest.next().ok_or(ScalaError::CountMismatch { count })?;
        cents.push(pitch(line, place)?);
    }
    if rest.next().is_some() {
        return Err(ScalaError::CountMismatch { count });
    }
    if cents.last().is_some_and(|&last| last <= 0.0) {
        return Err(ScalaError::PseudoOctaveFalls);
    }
    // Every limit but the pseudo-octave's and the pitches' has been checked.
    Tuning::new(name.to_owned(), description.to_owned(), cents)
        .map_err(|_| ScalaError::OutOfRange(count))
}

/// One pitch line in cents: a number with a decimal point is cents, anything
/// else is a ratio.
fn pitch(line: &str, place: usize) -> Result<f64, ScalaError> {
    let word = line
        .split_whitespace()
        .next()
        .ok_or(ScalaError::BadPitch(place))?;
    let bad = || ScalaError::BadPitch(place);
    let cents = if word.contains('.') {
        word.parse::<f64>().map_err(|_| bad())?
    } else {
        if word.starts_with('-') {
            return Err(ScalaError::OutOfRange(place));
        }
        let (top, bottom) = match word.split_once('/') {
            Some((top, bottom)) => (top, bottom),
            None => (word, "1"),
        };
        let top: u64 = top.parse().map_err(|_| bad())?;
        let bottom: u64 = bottom.parse().map_err(|_| bad())?;
        if top == 0 || bottom == 0 {
            return Err(ScalaError::OutOfRange(place));
        }
        1200.0 * (top as f64 / bottom as f64).log2()
    };
    if !cents.is_finite() {
        Err(bad())
    } else if cents.abs() > MAX_CENTS {
        Err(ScalaError::OutOfRange(place))
    } else {
        Ok(cents)
    }
}
