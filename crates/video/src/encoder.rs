//! Video encoders behind one trait (the export spec, R47, R48): Motion JPEG
//! in Rust on every computer, and the platform's H.264, HEVC and ProRes 422
//! where it has them.

use std::fmt;

use crate::decoder::Frame;
use crate::error::VideoError;
use crate::movie::VideoTrack;
use crate::rational::Rational;

/// The encoders Beatlayer lists (R47).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EncoderKind {
    /// H.264.
    H264,
    /// HEVC.
    Hevc,
    /// Apple ProRes 422.
    ProRes422,
    /// Motion JPEG, Beatlayer's own, on every computer.
    MotionJpeg,
}

impl EncoderKind {
    /// Every kind, in the order the chooser lists them.
    pub const ALL: [Self; 4] = [Self::H264, Self::Hevc, Self::ProRes422, Self::MotionJpeg];

    /// The name the chooser shows.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::ProRes422 => "ProRes 422",
            Self::MotionJpeg => "Motion JPEG",
        }
    }

    /// Whether the encoder takes a quality: ProRes 422 has one fixed.
    #[must_use]
    pub const fn has_quality(self) -> bool {
        !matches!(self, Self::ProRes422)
    }
}

impl fmt::Display for EncoderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What the encoder's settings dialog changes (R48). A setting left `None`
/// is the source movie's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncoderSettings {
    /// From 1 to 100: how much the picture is kept. Ignored by an encoder
    /// without a quality ([`EncoderKind::has_quality`]).
    pub quality: u8,
    /// Frames a second.
    pub frame_rate: Option<Rational>,
    /// The picture's width and height in pixels.
    pub size: Option<(u32, u32)>,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            quality: 85,
            frame_rate: None,
            size: None,
        }
    }
}

/// One coded frame, in the order the movie stores them, which is the order
/// they show in: the encoders here never reorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodedFrame {
    /// The coded bytes, as a movie stores them.
    pub data: Vec<u8>,
    /// Whether decoding can start here.
    pub sync: bool,
}

/// The video sample description a movie keeps for the track: the sample
/// entry's four-character code and the boxes inside it that the decoder
/// needs (`avcC`, `hvcC`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleEntry {
    /// The codec's four-character code.
    pub fourcc: [u8; 4],
    /// Boxes to put inside the entry, each a type and its contents.
    pub extra: Vec<([u8; 4], Vec<u8>)>,
}

/// Turns pictures into coded frames.
pub trait VideoEncoder: Send {
    /// Encodes `frame`, which has the size the encoder was opened with, and
    /// appends to `out` the frames that are ready: none yet, or several
    /// after a delay.
    ///
    /// # Errors
    ///
    /// Fails if the encoder can't go on.
    fn encode(&mut self, frame: &Frame, out: &mut Vec<CodedFrame>) -> Result<(), VideoError>;

    /// Appends the frames still held back.
    ///
    /// # Errors
    ///
    /// Fails if the encoder can't finish.
    fn finish(&mut self, out: &mut Vec<CodedFrame>) -> Result<(), VideoError>;

    /// The track's sample entry: known once the first frame is out.
    ///
    /// # Errors
    ///
    /// Fails if no frame was encoded.
    fn entry(&self) -> Result<SampleEntry, VideoError>;
}

/// The encoders this computer has, in the chooser's order (R47).
#[must_use]
pub fn available_encoders() -> Vec<EncoderKind> {
    EncoderKind::ALL
        .into_iter()
        .filter(|&kind| kind == EncoderKind::MotionJpeg || platform_has(kind))
        .collect()
}

#[cfg(target_os = "macos")]
fn platform_has(kind: EncoderKind) -> bool {
    crate::vtencode::encodes(kind)
}

#[cfg(windows)]
fn platform_has(kind: EncoderKind) -> bool {
    crate::mfencode::encodes(kind)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_has(kind: EncoderKind) -> bool {
    crate::ffmpeg::encodes(kind)
}

/// An encoder for pictures `width` by `height` at `frame_rate` frames a
/// second, with `settings`' quality.
///
/// # Errors
///
/// [`VideoError::Encode`] if this computer has no such encoder, or it
/// refuses the size.
pub fn open_encoder(
    kind: EncoderKind,
    width: u32,
    height: u32,
    frame_rate: Rational,
    settings: &EncoderSettings,
) -> Result<Box<dyn VideoEncoder>, VideoError> {
    if width == 0 || height == 0 || width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return Err(VideoError::Encode(format!(
            "a picture {width} by {height} pixels can't be encoded"
        )));
    }
    if kind == EncoderKind::MotionJpeg {
        return Ok(Box::new(crate::jpeg::MotionJpeg::new(settings.quality)));
    }
    open_platform(kind, width, height, frame_rate, settings)
}

#[cfg(target_os = "macos")]
fn open_platform(
    kind: EncoderKind,
    width: u32,
    height: u32,
    frame_rate: Rational,
    settings: &EncoderSettings,
) -> Result<Box<dyn VideoEncoder>, VideoError> {
    crate::vtencode::open_encoder(kind, width, height, frame_rate, settings)
        .map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

#[cfg(windows)]
fn open_platform(
    kind: EncoderKind,
    width: u32,
    height: u32,
    frame_rate: Rational,
    settings: &EncoderSettings,
) -> Result<Box<dyn VideoEncoder>, VideoError> {
    crate::mfencode::open_encoder(kind, width, height, frame_rate, settings)
        .map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn open_platform(
    kind: EncoderKind,
    width: u32,
    height: u32,
    frame_rate: Rational,
    settings: &EncoderSettings,
) -> Result<Box<dyn VideoEncoder>, VideoError> {
    crate::ffmpeg::open_encoder(kind, width, height, frame_rate, settings)
        .map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

/// The frame rate of a movie's picture: the reciprocal of the first frame's
/// duration, in frames a second. `None` for a movie of one frame.
#[must_use]
pub fn frame_rate_of(track: &VideoTrack) -> Option<Rational> {
    let order = track.presentation_order();
    let first = track.frame_start(*order.first()? as usize)?;
    let second = track.frame_start(*order.get(1)? as usize)?;
    let step = second.minus(first);
    (step.numer() > 0).then(|| Rational::new(i128::from(step.denom()), i128::from(step.numer())))
}

/// The size an encoder is opened with for a picture `width` by `height`:
/// even, since the platform encoders take no odd sizes.
#[must_use]
pub fn encoded_size(width: u32, height: u32) -> (u32, u32) {
    ((width & !1).max(2), (height & !1).max(2))
}

/// `frame` at `width` by `height`, by bilinear sampling. The same frame if
/// it has that size already.
#[must_use]
pub fn scale(frame: &Frame, width: u32, height: u32) -> Frame {
    if (frame.width(), frame.height()) == (width, height) {
        return frame.clone();
    }
    let (sw, sh) = (frame.width() as usize, frame.height() as usize);
    let source = frame.rgba();
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        // Pixel centres, so a scaled picture isn't shifted.
        let fy = ((y as f32 + 0.5) * sh as f32 / height as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, wy) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..width as usize {
            let fx =
                ((x as f32 + 0.5) * sw as f32 / width as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, wx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..4 {
                let at = |xx: usize, yy: usize| f32::from(source[(yy * sw + xx) * 4 + c]);
                let top = at(x0, y0) * (1.0 - wx) + at(x1, y0) * wx;
                let bottom = at(x0, y1) * (1.0 - wx) + at(x1, y1) * wx;
                rgba.push((top * (1.0 - wy) + bottom * wy).round() as u8);
            }
        }
    }
    Frame::new(width, height, rgba).unwrap_or_else(|| frame.clone())
}
