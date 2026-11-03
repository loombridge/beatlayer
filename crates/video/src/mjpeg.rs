//! Motion JPEG in Rust (R26): every frame is a JPEG of its own, so test
//! movies decode on every computer with no codec installed.

use std::panic::{AssertUnwindSafe, catch_unwind};

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::decoder::{CodedSample, Frame, VideoDecoder};
use crate::error::VideoError;
use crate::movie::{MAX_DIMENSION, MAX_PIXELS, VideoTrack};

pub(crate) struct Mjpeg;

impl Mjpeg {
    pub fn new(_track: &VideoTrack) -> Self {
        Self
    }
}

impl VideoDecoder for Mjpeg {
    fn decode(
        &mut self,
        samples: &[CodedSample<'_>],
        sink: &mut dyn FnMut(usize, Frame),
    ) -> Result<(), VideoError> {
        for sample in samples {
            // A damaged frame is left out; so is one the decoder panics on,
            // which its authors count a bug but a movie from anywhere can
            // reach.
            if let Ok(Some(frame)) = catch_unwind(AssertUnwindSafe(|| decode_frame(sample.data))) {
                sink(sample.index, frame);
            }
        }
        Ok(())
    }
}

fn decode_frame(data: &[u8]) -> Option<Frame> {
    let options = DecoderOptions::default()
        .set_max_width(MAX_DIMENSION as usize)
        .set_max_height(MAX_DIMENSION as usize)
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let mut rgba = vec![0; decoder.output_buffer_size()?];
    decoder.decode_into(&mut rgba).ok()?;
    Frame::new(width, height, rgba)
}
