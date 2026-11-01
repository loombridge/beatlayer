//! Decoders behind one trait (R26).

use std::fmt;

use crate::error::VideoError;
use crate::movie::VideoTrack;

/// A decoded picture: RGBA, eight bits a channel, rows top to bottom with no
/// padding, alpha always 255. What the Video Window uploads as a texture.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    rgba: Box<[u8]>,
}

impl Frame {
    /// A picture from its pixels, or `None` if `rgba` isn't `width * height *
    /// 4` bytes.
    #[must_use]
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        (rgba.len() as u64 == u64::from(width) * u64::from(height) * 4).then(|| Self {
            width,
            height,
            rgba: rgba.into_boxed_slice(),
        })
    }

    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels: red, green, blue, alpha, row by row.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// The pixel at column `x`, row `y`, as `[r, g, b, a]`.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[at..at + 4].try_into().ok()
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

/// One coded frame handed to a decoder.
#[derive(Clone, Copy, Debug)]
pub struct CodedSample<'a> {
    /// The frame's index in the movie, in decode order. Decoded frames come
    /// back under it.
    pub index: usize,
    /// Its presentation time, in the track's media time scale units.
    pub pts: i64,
    /// The coded bytes, as the movie stores them.
    pub data: &'a [u8],
}

/// Turns coded frames into pictures.
pub trait VideoDecoder: Send {
    /// Decodes `samples`, in decode order starting at a sync frame, and gives
    /// each picture to `sink` with its frame's index, in any order. A frame
    /// that can't be decoded is left out: the caller shows black (R28).
    ///
    /// Every call starts afresh: nothing before `samples` is a reference.
    ///
    /// # Errors
    ///
    /// Fails if the decoder as a whole can't go on, not for one bad frame.
    fn decode(
        &mut self,
        samples: &[CodedSample<'_>],
        sink: &mut dyn FnMut(usize, Frame),
    ) -> Result<(), VideoError>;
}

/// The codecs Beatlayer's decoders are asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Codec {
    Mjpeg,
    H264,
    Hevc,
    ProRes,
}

impl Codec {
    pub(crate) fn of(track: &VideoTrack) -> Option<Self> {
        match &track.codec() {
            b"jpeg" | b"mjpg" => Some(Self::Mjpeg),
            b"avc1" | b"avc3" => Some(Self::H264),
            b"hvc1" | b"hev1" => Some(Self::Hevc),
            b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => Some(Self::ProRes),
            _ => None,
        }
    }
}

/// A decoder for the track's codec: Motion JPEG in Rust, and for the rest the
/// platform's (R26).
///
/// # Errors
///
/// [`VideoError::NoDecoder`], with every decoder's reason, if none takes it.
pub fn open_decoder(track: &VideoTrack) -> Result<Box<dyn VideoDecoder>, VideoError> {
    let no_decoder = |reason: String| VideoError::NoDecoder {
        codec: track.codec_name(),
        reason,
    };
    let Some(codec) = Codec::of(track) else {
        return Err(no_decoder(
            "Beatlayer reads Motion JPEG, H.264, HEVC and ProRes".into(),
        ));
    };
    if codec == Codec::Mjpeg {
        return Ok(Box::new(crate::mjpeg::Mjpeg::new(track)));
    }
    open_platform(track, codec).map_err(no_decoder)
}

#[cfg(target_os = "macos")]
fn open_platform(track: &VideoTrack, codec: Codec) -> Result<Box<dyn VideoDecoder>, String> {
    crate::videotoolbox::open(track, codec).map(|d| Box::new(d) as Box<dyn VideoDecoder>)
}

#[cfg(windows)]
fn open_platform(track: &VideoTrack, codec: Codec) -> Result<Box<dyn VideoDecoder>, String> {
    crate::mediafoundation::open(track, codec).map(|d| Box::new(d) as Box<dyn VideoDecoder>)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn open_platform(track: &VideoTrack, codec: Codec) -> Result<Box<dyn VideoDecoder>, String> {
    crate::ffmpeg::open_codec(track, codec, None).map(|d| Box::new(d) as Box<dyn VideoDecoder>)
}

/// Refuses a movie whose video no decoder here takes (R1).
pub(crate) fn check_decodable(track: &VideoTrack) -> Result<(), VideoError> {
    open_decoder(track).map(drop)
}
