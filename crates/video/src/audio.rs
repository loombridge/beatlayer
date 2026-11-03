//! A movie's sound (R3, R4): decoded by `bl-media` as an audio file is, and
//! placed on the movie's timeline by the track's edit list, with silence in
//! its gaps.

use bl_media::{Decoder, SampleData, limits};

use crate::edits::Edit;
use crate::error::VideoError;
use crate::movie::Movie;
use crate::rational::Rational;

/// A movie's audio track, read in blocks at any timeline position.
///
/// Decoding reads the file and allocates: it never runs on an audio thread.
pub struct MovieAudio {
    decoder: Decoder,
    edits: Vec<Edit>,
    movie_scale: u32,
    media_scale: u32,
    rate: u32,
}

impl MovieAudio {
    /// Opens the movie's audio track.
    ///
    /// # Errors
    ///
    /// [`VideoError::NoAudio`] if the movie has none; otherwise fails if
    /// `bl-media` can't read the track's codec.
    pub fn open(movie: &Movie) -> Result<Self, VideoError> {
        let track = movie.audio().ok_or(VideoError::NoAudio)?;
        // The edit list places the audio here, so the decoder keeps every
        // frame the codec produces.
        let decoder = Decoder::from_reader_untrimmed(movie.reader()?, Some("mov"))?;
        let rate = decoder.info().sample_rate;
        Ok(Self {
            decoder,
            edits: track.edits().edits().to_vec(),
            movie_scale: track.edits().movie_scale(),
            media_scale: track.edits().media_scale(),
            rate,
        })
    }

    /// Frames per second.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// The number of channels.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.decoder.info().channels
    }

    /// The timeline frame where `units` movie time scale units fall.
    fn timeline_frame(&self, units: u128) -> u64 {
        let frames =
            Rational::new(units as i128, i128::from(self.movie_scale)).round_units(self.rate);
        u64::try_from(frames).unwrap_or(if frames < 0 { 0 } else { u64::MAX })
    }

    /// The length of the track's timeline in frames: where its last edit ends.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.edits.last().map_or(0, |e| {
            self.timeline_frame(u128::from(e.start) + u128::from(e.duration))
        })
    }

    /// Whether the track's timeline has no length.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `out` (one slice per channel, all one length) with the audio at
    /// timeline frame `start` onwards, and returns how many frames lie within
    /// the track: the rest of `out` is silence. A gap in the track is silence
    /// too (R4).
    ///
    /// # Errors
    ///
    /// Fails if the slices aren't the channel count, or on a read error.
    pub fn read_at(&mut self, start: u64, out: &mut [&mut [f32]]) -> Result<usize, VideoError> {
        let len = out.iter().map(|c| c.len()).min().unwrap_or(0);
        for channel in out.iter_mut() {
            channel.fill(0.0);
        }
        let end = start.saturating_add(len as u64);
        let mut within = 0;
        for i in 0..self.edits.len() {
            let edit = self.edits[i];
            let from = self.timeline_frame(u128::from(edit.start));
            let to = self.timeline_frame(u128::from(edit.start) + u128::from(edit.duration));
            let (a, b) = (from.max(start), to.min(end));
            if a >= b {
                continue;
            }
            within = within.max((b - start) as usize);
            let Some(media_time) = edit.media_time else {
                continue;
            };
            let media_from = u64::try_from(
                Rational::new(i128::from(media_time), i128::from(self.media_scale))
                    .round_units(self.rate),
            )
            .unwrap_or(u64::MAX);
            let at = media_from.saturating_add(a - from);
            let mut done = (a - start) as usize;
            let stop = (b - start) as usize;
            if self.decoder.position() != at {
                self.decoder.seek(at)?;
            }
            while done < stop {
                let mut planes: Vec<&mut [f32]> =
                    out.iter_mut().map(|c| &mut c[done..stop]).collect();
                let got = self.decoder.read(&mut planes)?;
                if got == 0 {
                    // The media is shorter than its edit says: silence.
                    break;
                }
                done += got;
            }
        }
        Ok(within)
    }

    /// The whole timeline decoded into memory, silence in its gaps.
    ///
    /// # Errors
    ///
    /// As for [`MovieAudio::read_at`].
    pub fn decode_all(&mut self) -> Result<SampleData, VideoError> {
        let channels = usize::from(self.channels());
        let len = usize::try_from(self.len())
            .ok()
            .filter(|&len| len.saturating_mul(channels) <= limits::MAX_PROGRESSIVE_SAMPLES)
            .ok_or_else(|| {
                VideoError::Unsupported("audio too long to decode into memory".into())
            })?;
        let mut planes = vec![vec![0.0; len]; channels];
        let mut slices: Vec<&mut [f32]> = planes.iter_mut().map(Vec::as_mut_slice).collect();
        self.read_at(0, &mut slices)?;
        Ok(SampleData::new(self.rate, planes)?)
    }
}
