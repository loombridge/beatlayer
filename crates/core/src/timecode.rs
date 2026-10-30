//! MIDI Timecode: the times it carries, the frame rates it counts them at,
//! and what a time is in seconds (`docs/specs/midi-timecode.md`, R1, R3).
//!
//! A [`Timecode`] is hours, minutes, seconds and frames, as the MIDI Time
//! Code messages spell them; [`FrameRate`] is one of the four rates the
//! messages name, and [`TimecodeRate`] is the choice an input port keeps:
//! one of them for every time, or the rate each message names.

use core::fmt;

/// A frame rate MIDI Timecode names: 24, 25, 29.97 drop frame, or 30 frames
/// a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameRate {
    /// 24 frames a second.
    Fps24,
    /// 25 frames a second.
    Fps25,
    /// 30,000 / 1,001 frames a second, counted as drop frame.
    Fps2997Drop,
    /// 30 frames a second.
    Fps30,
}

impl FrameRate {
    /// The rate a message's two-bit rate code names: 0 for 24, 1 for 25, 2
    /// for 29.97 drop frame, 3 for 30.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code & 3 {
            0 => Self::Fps24,
            1 => Self::Fps25,
            2 => Self::Fps2997Drop,
            _ => Self::Fps30,
        }
    }

    /// Frames in a second as the numbers count them: frame numbers run from
    /// 0 to one less than this.
    #[must_use]
    pub const fn nominal(self) -> u8 {
        match self {
            Self::Fps24 => 24,
            Self::Fps25 => 25,
            Self::Fps2997Drop | Self::Fps30 => 30,
        }
    }

    /// How long a frame lasts, in seconds.
    #[must_use]
    pub fn frame(self) -> f64 {
        match self {
            Self::Fps2997Drop => 1001.0 / 30000.0,
            rate => 1.0 / f64::from(rate.nominal()),
        }
    }
}

/// An input port's MIDI Timecode Frame Rate: one rate for every time, or
/// SMPTE All, which takes each time at the rate its own message names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimecodeRate {
    /// Every time is counted at this rate, whatever the messages say.
    Fixed(FrameRate),
    /// Each time is counted at the rate its message names.
    #[default]
    All,
}

impl TimecodeRate {
    /// The rate a time is counted at when its message names `carried`.
    #[must_use]
    pub const fn of(self, carried: FrameRate) -> FrameRate {
        match self {
            Self::Fixed(rate) => rate,
            Self::All => carried,
        }
    }
}

/// A time in hours, minutes, seconds and frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Timecode {
    /// Hours, 0 to 23.
    pub hours: u8,
    /// Minutes, 0 to 59.
    pub minutes: u8,
    /// Seconds, 0 to 59.
    pub seconds: u8,
    /// Frames, from 0 to one less than the rate's
    /// [`nominal`](FrameRate::nominal) count.
    pub frames: u8,
}

impl Timecode {
    /// 00:00:00:00.
    pub const ZERO: Self = Self {
        hours: 0,
        minutes: 0,
        seconds: 0,
        frames: 0,
    };

    /// The time `text` spells, as `hh:mm:ss:ff` (a `;` before the frames is
    /// read as a `:`), if every part is in range for any rate: hours below
    /// 24, minutes and seconds below 60, frames below 30.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split([':', ';']);
        let mut next = |limit: u8| -> Option<u8> {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 2 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok().filter(|&n| n < limit)
        };
        let time = Self {
            hours: next(24)?,
            minutes: next(60)?,
            seconds: next(60)?,
            frames: next(30)?,
        };
        parts.next().is_none().then_some(time)
    }

    /// Whether every part is in range for `rate`.
    #[must_use]
    pub const fn fits(self, rate: FrameRate) -> bool {
        self.hours < 24 && self.minutes < 60 && self.seconds < 60 && self.frames < rate.nominal()
    }

    /// The time in seconds at `rate`: hours, minutes and seconds plus the
    /// frames over the rate. At 29.97 drop frame the frame numbers 0 and 1
    /// are skipped at the start of every minute but every tenth, so the
    /// frames before a time are counted without them, a frame lasting
    /// 1,001 / 30,000 s.
    #[must_use]
    pub fn seconds(self, rate: FrameRate) -> f64 {
        let whole =
            u32::from(self.hours) * 3600 + u32::from(self.minutes) * 60 + u32::from(self.seconds);
        match rate {
            FrameRate::Fps2997Drop => {
                let minutes = u32::from(self.hours) * 60 + u32::from(self.minutes);
                let counted = whole * 30 + u32::from(self.frames);
                let skipped = 2 * (minutes - minutes / 10);
                f64::from(counted.saturating_sub(skipped)) * rate.frame()
            }
            _ => f64::from(whole) + f64::from(self.frames) * rate.frame(),
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn at(hours: u8, minutes: u8, seconds: u8, frames: u8) -> Timecode {
        Timecode {
            hours,
            minutes,
            seconds,
            frames,
        }
    }

    // Spec: midi-timecode R3
    #[test]
    fn every_rate_counts_a_time_in_seconds() {
        let time = at(1, 2, 3, 12);
        let whole = 3723.0;
        assert_eq!(time.seconds(FrameRate::Fps24), whole + 0.5);
        assert_eq!(time.seconds(FrameRate::Fps25), whole + 12.0 / 25.0);
        assert_eq!(time.seconds(FrameRate::Fps30), whole + 0.4);
        // 29.97 drop frame: 62 minutes less 6 tenths of them skip 112 frames.
        let counted = 3723.0 * 30.0 + 12.0 - 2.0 * (62.0 - 6.0);
        let seconds = time.seconds(FrameRate::Fps2997Drop);
        assert!((seconds - counted * 1001.0 / 30000.0).abs() < 1e-9);
    }

    // Spec: midi-timecode R3
    #[test]
    fn drop_frame_skips_two_frame_numbers_at_every_minute_but_the_tenth() {
        let rate = FrameRate::Fps2997Drop;
        let frame = rate.frame();
        // Minute 1: 00:00:59;29 is followed by 00:01:00;02.
        let before = at(0, 0, 59, 29).seconds(rate);
        let after = at(0, 1, 0, 2).seconds(rate);
        assert!((after - before - frame).abs() < 1e-9);
        // Minute 9, the same.
        let before = at(0, 8, 59, 29).seconds(rate);
        let after = at(0, 9, 0, 2).seconds(rate);
        assert!((after - before - frame).abs() < 1e-9);
        // Minute 10 keeps its frames 0 and 1.
        let before = at(0, 9, 59, 29).seconds(rate);
        let after = at(0, 10, 0, 0).seconds(rate);
        assert!((after - before - frame).abs() < 1e-9);
        // An hour of drop frame numbers holds 107,892 frames.
        let hour = at(1, 0, 0, 0).seconds(rate);
        assert!((hour - 107_892.0 * frame).abs() < 1e-6);
    }

    // Spec: midi-timecode R3
    #[test]
    fn a_fixed_rate_overrides_the_message_and_all_takes_it() {
        assert_eq!(
            TimecodeRate::Fixed(FrameRate::Fps25).of(FrameRate::Fps30),
            FrameRate::Fps25
        );
        assert_eq!(TimecodeRate::All.of(FrameRate::Fps30), FrameRate::Fps30);
        assert_eq!(FrameRate::from_code(2), FrameRate::Fps2997Drop);
    }

    // Spec: midi-timecode R1
    #[test]
    fn a_time_is_read_from_its_text_and_refused_out_of_range() {
        assert_eq!(Timecode::parse("01:02:03:04"), Some(at(1, 2, 3, 4)));
        assert_eq!(Timecode::parse("00:00:10;29"), Some(at(0, 0, 10, 29)));
        for bad in [
            "",
            "1:2:3",
            "24:00:00:00",
            "00:60:00:00",
            "00:00:00:30",
            "00:00:00:0x",
            "0:0:0:0:0",
        ] {
            assert_eq!(Timecode::parse(bad), None, "{bad}");
        }
        assert_eq!(at(1, 2, 3, 4).to_string(), "01:02:03:04");
    }
}
