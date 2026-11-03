//! Voice pitch (WP-8.32): a streaming monophonic pitch tracker and a
//! formant-keeping shifter for voices.
//!
//! - [`Tracker`] measures the fundamental of a mono stream with the YIN
//!   method (A. de Cheveigné and H. Kawahara, "YIN, a fundamental frequency
//!   estimator for speech and music", JASA 111(4), 2002), giving per
//!   measurement a frequency, a confidence, a voiced decision and an onset
//!   flag. It is the Tuner's detector, moved out of the device. M. Mauch and
//!   S. Dixon's pYIN (ICASSP 2014) is what the voiced decision's threshold
//!   and the confidence are modelled on, without its hidden Markov smoothing.
//! - [`VoiceShifter`] moves a voice's pitch by a ratio, and its formants by
//!   another, by pitch-synchronous overlap-add (E. Moulines and F.
//!   Charpentier, "Pitch-synchronous waveform processing techniques for
//!   text-to-speech synthesis using diphones", Speech Communication 9,
//!   1990), with its pitch marks laid by the tracker's periods.
//!
//! Everything is allocated when the value is made, off the audio thread;
//! processing allocates nothing, locks nothing and never blocks. Both take
//! their samples one at a time, so a render never depends on the block size.

mod tracker;
mod voice;

pub use tracker::{Config, Pitch, Tracker};
pub use voice::VoiceShifter;

#[cfg(test)]
mod tests;
