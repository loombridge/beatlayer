//! H.264 through Media Foundation's encoder transform on Windows (the export
//! spec, R47): the software encoder the system offers, fed NV12 pictures and
//! asked for start-code streams, which are rewritten as a movie stores them
//! ([`crate::annexb`]). A computer without the encoder is not offered it: the
//! chooser lists what a trial encode of a few frames made.
//!
//! HEVC is not offered: Windows' HEVC encoders are hardware transforms that
//! deliver their output through events, which this synchronous driver does
//! not read.

use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFSample, IMFTransform, MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Video, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_LOCALMFT,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::CoTaskMemFree;

use crate::annexb::{self, ParameterSets};
use crate::decoder::Frame;
use crate::encoder::{CodedFrame, EncoderKind, EncoderSettings, SampleEntry, VideoEncoder};
use crate::error::VideoError;
use crate::mediafoundation::start;
use crate::rational::Rational;

/// `eAVEncH264VProfile_Main`: no B-frames, so frames come out as they went in.
const MAIN_PROFILE: u32 = 77;

/// Media Foundation's H.264 encoder.
pub(crate) struct Encoder {
    transform: IMFTransform,
    width: usize,
    height: usize,
    /// Frames a second as a fraction.
    rate: (u64, u64),
    next: i64,
    sets: ParameterSets,
}

// SAFETY: the encoder transform is free-threaded, and the encoder is used
// from one thread at a time: its methods take `&mut self`.
unsafe impl Send for Encoder {}

/// Whether this computer has an H.264 encoder that makes frames: a trial.
pub(crate) fn encodes(kind: EncoderKind) -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    kind == EncoderKind::H264
        && *FOUND.get_or_init(|| {
            let trial = || -> Result<bool, VideoError> {
                let mut encoder = open_encoder(
                    EncoderKind::H264,
                    128,
                    128,
                    Rational::new(30, 1),
                    &EncoderSettings::default(),
                )?;
                let black = Frame::new(128, 128, [0, 0, 0, 255].repeat(128 * 128))
                    .ok_or_else(|| VideoError::Encode("a picture too large".into()))?;
                let mut out = Vec::new();
                for _ in 0..4 {
                    encoder.encode(&black, &mut out)?;
                }
                encoder.finish(&mut out)?;
                Ok(!out.is_empty() && encoder.entry().is_ok())
            };
            trial().unwrap_or(false)
        })
}

pub(crate) fn open_encoder(
    kind: EncoderKind,
    width: u32,
    height: u32,
    frame_rate: Rational,
    settings: &EncoderSettings,
) -> Result<Encoder, VideoError> {
    let fail = |why: String| VideoError::Encode(why);
    if kind != EncoderKind::H264 {
        return Err(fail("Media Foundation is asked for H.264 only".into()));
    }
    start().map_err(fail)?;
    let transform = find_encoder()
        .ok_or_else(|| fail("Media Foundation has no H.264 encoder on this computer".into()))?;
    let rate = (
        frame_rate.numer().max(1) as u64,
        u64::try_from(frame_rate.denom()).unwrap_or(1).max(1),
    );
    // Quality 100 spends about a quarter of a bit on a pixel of a frame, and
    // 1 about a hundredth.
    let bits_per_pixel = 0.01 + f64::from(settings.quality.clamp(1, 100)) / 100.0 * 0.24;
    let bitrate = (f64::from(width) * f64::from(height) * frame_rate.seconds() * bits_per_pixel)
        .clamp(100_000.0, 400_000_000.0) as u32;
    let size = (u64::from(width) << 32) | u64::from(height);
    let fraction = (rate.0 << 32) | rate.1;
    // SAFETY: Media Foundation's own objects through its API.
    let configured = unsafe {
        (|| -> windows::core::Result<()> {
            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            output.SetUINT64(&MF_MT_FRAME_RATE, fraction)?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, MAIN_PROFILE)?;
            transform.SetOutputType(0, &output, 0)?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
            input.SetUINT64(&MF_MT_FRAME_RATE, fraction)?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &input, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
        })()
    };
    configured.map_err(|e| {
        fail(format!(
            "Media Foundation's H.264 encoder refused the job: {e}"
        ))
    })?;
    Ok(Encoder {
        transform,
        width: width as usize,
        height: height as usize,
        rate,
        next: 0,
        sets: None,
    })
}

/// The first software encoder transform that makes H.264.
fn find_encoder() -> Option<IMFTransform> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the enumeration fills `activates` with `count` activation
    // objects in memory the caller frees with `CoTaskMemFree`, after dropping
    // each object.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&raw const output),
            &raw mut activates,
            &raw mut count,
        )
        .ok()?;
        let mut found = None;
        for i in 0..count as usize {
            let slot = activates.add(i);
            if found.is_none()
                && let Some(activate) = &*slot
            {
                found = activate.ActivateObject::<IMFTransform>().ok();
            }
            std::ptr::drop_in_place(slot);
        }
        CoTaskMemFree(Some(activates.cast_const().cast()));
        found
    }
}

/// An RGBA picture as NV12, in the limited range: BT.709 from 720 lines up,
/// BT.601 below, as the decoders here and players assume.
fn nv12(frame: &Frame) -> Vec<u8> {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let rgba = frame.rgba();
    let hd = h >= 720;
    let mut out = vec![0u8; w * h * 3 / 2];
    let (luma, chroma) = out.split_at_mut(w * h);
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            let (r, g, b) = (
                i32::from(rgba[at]),
                i32::from(rgba[at + 1]),
                i32::from(rgba[at + 2]),
            );
            let value = if hd {
                (47 * r + 157 * g + 16 * b + 128) >> 8
            } else {
                (66 * r + 129 * g + 25 * b + 128) >> 8
            } + 16;
            luma[y * w + x] = value.clamp(16, 235) as u8;
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let at = ((y + dy).min(h - 1) * w + (x + dx).min(w - 1)) * 4;
                r += i32::from(rgba[at]);
                g += i32::from(rgba[at + 1]);
                b += i32::from(rgba[at + 2]);
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            let (u, v) = if hd {
                (
                    ((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128,
                    ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128,
                )
            } else {
                (
                    ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128,
                    ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128,
                )
            };
            let at = (y / 2) * w + x / 2 * 2;
            chroma[at] = u.clamp(16, 240) as u8;
            chroma[at + 1] = v.clamp(16, 240) as u8;
        }
    }
    out
}

impl Encoder {
    /// A time of `frames` frames, in Media Foundation's 100 ns units.
    fn time(&self, frames: i64) -> i64 {
        (i128::from(frames) * 10_000_000 * i128::from(self.rate.1) / i128::from(self.rate.0)) as i64
    }

    fn sample(&self, data: &[u8]) -> windows::core::Result<IMFSample> {
        // SAFETY: the buffer is `data.len()` bytes from Media Foundation,
        // locked while written.
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut at = std::ptr::null_mut();
            buffer.Lock(&raw mut at, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), at, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(self.time(self.next))?;
            sample.SetSampleDuration(self.time(1))?;
            Ok(sample)
        }
    }

    /// Takes every frame the transform has ready.
    fn drain(&mut self, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        loop {
            // SAFETY: the output buffer struct is filled and read as the API
            // documents: an output sample if the transform doesn't provide
            // one, and a status and events to release after the call.
            let sample = unsafe {
                let info = self
                    .transform
                    .GetOutputStreamInfo(0)
                    .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
                let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                let given: Option<IMFSample> =
                    if provides {
                        None
                    } else {
                        let sample = MFCreateSample()
                            .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
                        let size = (info.cbSize as usize).max(1 << 20);
                        sample
                            .AddBuffer(&MFCreateMemoryBuffer(size as u32).map_err(|e| {
                                VideoError::Encode(format!("Media Foundation: {e}"))
                            })?)
                            .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
                        Some(sample)
                    };
                let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(given),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self
                    .transform
                    .ProcessOutput(0, &mut buffers, &raw mut status);
                let sample = ManuallyDrop::take(&mut buffers[0].pSample);
                ManuallyDrop::drop(&mut buffers[0].pEvents);
                match result {
                    Ok(()) => sample,
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => continue,
                    Err(e) => {
                        return Err(VideoError::Encode(format!(
                            "Media Foundation's H.264 encoder failed: {e}"
                        )));
                    }
                }
            };
            let Some(sample) = sample else {
                continue;
            };
            // SAFETY: a sample the transform gave back, its buffer locked
            // while read.
            let bytes = unsafe {
                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
                let mut at = std::ptr::null_mut();
                let mut length = 0u32;
                buffer
                    .Lock(&raw mut at, None, Some(&raw mut length))
                    .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
                let bytes = std::slice::from_raw_parts(at, length as usize).to_vec();
                let _ = buffer.Unlock();
                bytes
            };
            let (data, sync) = annexb::length_prefixed(&bytes, &mut self.sets);
            out.push(CodedFrame { data, sync });
        }
    }
}

impl VideoEncoder for Encoder {
    fn encode(&mut self, frame: &Frame, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        if (frame.width() as usize, frame.height() as usize) != (self.width, self.height) {
            return Err(VideoError::Encode(
                "a picture of another size than the encoder's".into(),
            ));
        }
        let data = nv12(frame);
        let sample = self
            .sample(&data)
            .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
        // SAFETY: a sample made for this transform.
        let mut result = unsafe { self.transform.ProcessInput(0, &sample, 0) };
        if result
            .as_ref()
            .is_err_and(|e| e.code() == MF_E_NOTACCEPTING)
        {
            // It wants its output taken first.
            self.drain(out)?;
            // SAFETY: as above.
            result = unsafe { self.transform.ProcessInput(0, &sample, 0) };
        }
        result.map_err(|e| VideoError::Encode(format!("Media Foundation's H.264 encoder: {e}")))?;
        self.next += 1;
        self.drain(out)
    }

    fn finish(&mut self, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        // SAFETY: messages to the transform `open_encoder` made.
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .and_then(|()| self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0))
                .map_err(|e| VideoError::Encode(format!("Media Foundation: {e}")))?;
        }
        self.drain(out)
    }

    fn entry(&self) -> Result<SampleEntry, VideoError> {
        // Where the stream carried no parameter sets, the output type's
        // sequence header, a start-code stream of its own, has them.
        let mut sets = self.sets.clone();
        if sets.as_ref().is_none_or(|(_, pps)| pps.is_empty()) {
            // SAFETY: the transform's current output type, a blob read into
            // a buffer of the size it reports.
            let header = unsafe {
                self.transform
                    .GetOutputCurrentType(0)
                    .ok()
                    .and_then(|current| {
                        let size = current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
                        let mut blob = vec![0u8; size as usize];
                        current
                            .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)
                            .ok()?;
                        Some(blob)
                    })
            };
            if let Some(header) = header {
                sets = None;
                annexb::length_prefixed(&header, &mut sets);
            }
        }
        let avcc = annexb::avcc(&sets)
            .ok_or_else(|| VideoError::Encode("no frame was encoded".to_owned()))?;
        Ok(SampleEntry {
            fourcc: *b"avc1",
            extra: vec![(*b"avcC", avcc)],
        })
    }
}
