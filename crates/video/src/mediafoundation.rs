//! H.264 and HEVC through Media Foundation on Windows (R26): the video
//! decoder transform the system offers for the codec, found by enumerating
//! the transforms that take it, so a codec pack that adds one (HEVC's, ProRes
//! from a vendor) is used too, and a computer without one refuses the codec
//! with the reason.
//!
//! Each call to [`VideoDecoder::decode`] flushes the transform and starts
//! afresh. The transform is fed start-code streams ([`crate::annexb`]) and
//! returns NV12, which [`crate::convert`] makes RGBA. Each picture comes back
//! with its frame's presentation time as its sample time, which says whose it
//! is, as with FFmpeg.

use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFSample, IMFTransform, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Video, MFSTARTUP_FULL, MFStartup, MFT_CATEGORY_VIDEO_DECODER,
    MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};

use crate::annexb::Config;
use crate::convert::nv12_to_rgba;
use crate::decoder::{Codec, CodedSample, Frame, VideoDecoder};
use crate::error::VideoError;
use crate::movie::{MAX_DIMENSION, MAX_PIXELS, VideoTrack};

pub(crate) struct MediaFoundation {
    transform: IMFTransform,
    config: Config,
    /// The visible picture.
    width: usize,
    height: usize,
    /// The decoder's buffer: its luma rows' stride and its height, which can
    /// be padded to a macroblock row.
    stride: usize,
    aligned_height: usize,
}

// SAFETY: Media Foundation's decoder transforms are free-threaded (they
// register as both-apartment), and the decoder is used from one thread at a
// time: `VideoDecoder::decode` takes `&mut self`.
unsafe impl Send for MediaFoundation {}

thread_local! {
    static COM: Cell<bool> = const { Cell::new(false) };
}

/// Starts COM on this thread, once, and Media Foundation in the process.
fn start() -> Result<(), String> {
    static MEDIA_FOUNDATION: OnceLock<Result<(), String>> = OnceLock::new();
    COM.with(|started| {
        if !started.get() {
            // SAFETY: plain COM initialisation. A thread already started in
            // another mode answers with a failure that changes nothing here.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            started.set(true);
        }
    });
    MEDIA_FOUNDATION
        .get_or_init(|| {
            // SAFETY: no preconditions; the process never shuts it down.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
                .map_err(|e| format!("Media Foundation won't start: {e}"))
        })
        .clone()
}

pub(crate) fn open(track: &VideoTrack, codec: Codec) -> Result<MediaFoundation, String> {
    let (subtype, name) = match codec {
        Codec::H264 => (MFVideoFormat_H264, "H.264"),
        Codec::Hevc => (
            MFVideoFormat_HEVC,
            "HEVC (it needs Microsoft's HEVC Video Extensions)",
        ),
        _ => return Err("Media Foundation has no decoder for ProRes".into()),
    };
    let config =
        Config::parse(codec, track.extra()).ok_or("the movie's codec configuration is damaged")?;
    start()?;
    let transform = find_transform(subtype)
        .ok_or_else(|| format!("Media Foundation has no decoder for {name} on this computer"))?;
    let (width, height) = (track.width() as usize, track.height() as usize);
    // SAFETY: the transform and the types are Media Foundation's own, made
    // and passed through its API.
    let configured = unsafe {
        (|| -> windows::core::Result<()> {
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            input.SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(track.width()) << 32) | u64::from(track.height()),
            )?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform.SetInputType(0, &input, 0)
        })()
    };
    configured.map_err(|e| format!("Media Foundation's decoder refused the stream: {e}"))?;
    let mut decoder = MediaFoundation {
        transform,
        config,
        width,
        height,
        stride: width,
        aligned_height: height,
    };
    decoder.choose_output()?;
    Ok(decoder)
}

/// The first decoder transform that takes `subtype`.
fn find_transform(subtype: windows::core::GUID) -> Option<IMFTransform> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the enumeration fills `activates` with `count` activation
    // objects in memory the caller frees with `CoTaskMemFree`, after dropping
    // each object.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&raw const input),
            None,
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

impl MediaFoundation {
    /// Asks the transform for NV12 and reads back the buffer's shape.
    fn choose_output(&mut self) -> Result<(), String> {
        // SAFETY: Media Foundation's own objects through its API.
        let result = unsafe {
            (|| -> windows::core::Result<()> {
                let mut index = 0;
                loop {
                    let available = self.transform.GetOutputAvailableType(0, index)?;
                    if available.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                        self.transform.SetOutputType(0, &available, 0)?;
                        break;
                    }
                    index += 1;
                }
                let current = self.transform.GetOutputCurrentType(0)?;
                let size = current.GetUINT64(&MF_MT_FRAME_SIZE)?;
                let (aligned_width, aligned_height) =
                    ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
                self.aligned_height = aligned_height.max(self.height);
                self.stride = current
                    .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                    .ok()
                    .map(|s| s as i32)
                    .filter(|&s| s > 0)
                    .map_or(aligned_width.max(self.width), |s| s as usize);
                Ok(())
            })()
        };
        result.map_err(|e| format!("Media Foundation's decoder has no NV12 output: {e}"))
    }

    /// Feeds one coded frame, time-stamped with its presentation time.
    fn feed(&self, data: &[u8], pts: i64) -> windows::core::Result<()> {
        // SAFETY: the buffer is `data.len()` bytes from Media Foundation,
        // locked while written.
        unsafe {
            let buffer =
                MFCreateMemoryBuffer(u32::try_from(data.len()).map_err(|_| {
                    windows::core::Error::from_hresult(windows::core::HRESULT(-1))
                })?)?;
            let mut at = std::ptr::null_mut();
            buffer.Lock(&raw mut at, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), at, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            self.transform.ProcessInput(0, &sample, 0)
        }
    }

    /// Takes every picture ready, each under the frame whose presentation
    /// time it carries, so one the transform leaves out moves no other.
    fn drain(&mut self, samples: &[CodedSample<'_>], sink: &mut dyn FnMut(usize, Frame)) {
        loop {
            // SAFETY: the output buffer struct is filled and read as the API
            // documents: an output sample if the transform doesn't provide
            // one, and a status and events to release after the call.
            let sample = unsafe {
                let provides = self
                    .transform
                    .GetOutputStreamInfo(0)
                    .map(|info| info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0);
                let Ok(provides) = provides else {
                    return;
                };
                let given: Option<IMFSample> = if provides { None } else { self.output_sample() };
                let mut out = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(given),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.transform.ProcessOutput(0, &mut out, &raw mut status);
                let sample = ManuallyDrop::take(&mut out[0].pSample);
                ManuallyDrop::drop(&mut out[0].pEvents);
                match result {
                    Ok(()) => sample,
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        if self.choose_output().is_err() {
                            return;
                        }
                        continue;
                    }
                    Err(e) => {
                        debug_assert!(
                            e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT || e.code().is_err()
                        );
                        return;
                    }
                }
            };
            let Some(sample) = sample else {
                continue;
            };
            // SAFETY: a sample the transform gave back.
            let pts = unsafe { sample.GetSampleTime() }.ok();
            if let (Some(at), Some(frame)) = (
                samples.iter().find(|s| Some(s.pts) == pts),
                self.picture(&sample),
            ) {
                sink(at.index, frame);
            }
        }
    }

    /// An empty sample as large as the transform's output.
    ///
    /// # Safety
    ///
    /// Media Foundation must be started.
    unsafe fn output_sample(&self) -> Option<IMFSample> {
        // SAFETY: as the caller guarantees.
        unsafe {
            let info = self.transform.GetOutputStreamInfo(0).ok()?;
            let size = (info.cbSize as usize).max(self.stride * self.aligned_height * 3 / 2);
            let sample = MFCreateSample().ok()?;
            sample
                .AddBuffer(&MFCreateMemoryBuffer(u32::try_from(size).ok()?).ok()?)
                .ok()?;
            Some(sample)
        }
    }

    /// An output sample as an RGBA picture.
    fn picture(&self, sample: &IMFSample) -> Option<Frame> {
        let (w, h) = (
            u32::try_from(self.width).ok()?,
            u32::try_from(self.height).ok()?,
        );
        if w == 0
            || h == 0
            || w > MAX_DIMENSION
            || h > MAX_DIMENSION
            || u64::from(w) * u64::from(h) > MAX_PIXELS
        {
            return None;
        }
        // SAFETY: the buffer is locked while read, and `length` bytes at
        // `at` are readable until it's unlocked.
        let rgba = unsafe {
            let buffer = sample.ConvertToContiguousBuffer().ok()?;
            let mut at = std::ptr::null_mut();
            let mut length = 0u32;
            buffer.Lock(&raw mut at, None, Some(&raw mut length)).ok()?;
            let data = std::slice::from_raw_parts(at, length as usize);
            // BT.709 from 720 lines up, BT.601 below, as players do.
            let rgba = nv12_to_rgba(
                data,
                self.stride,
                self.aligned_height,
                self.width,
                self.height,
                self.height >= 720,
            );
            let _ = buffer.Unlock();
            rgba
        };
        Frame::new(w, h, rgba?)
    }
}

impl VideoDecoder for MediaFoundation {
    fn decode(
        &mut self,
        samples: &[CodedSample<'_>],
        sink: &mut dyn FnMut(usize, Frame),
    ) -> Result<(), VideoError> {
        start().map_err(VideoError::Decode)?;
        // SAFETY: messages to a transform made by `open`.
        unsafe {
            for message in [
                MFT_MESSAGE_COMMAND_FLUSH,
                MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
                MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            ] {
                self.transform
                    .ProcessMessage(message, 0)
                    .map_err(|e| VideoError::Decode(format!("Media Foundation's decoder: {e}")))?;
            }
        }
        for (position, sample) in samples.iter().enumerate() {
            let data = self.config.to_annex_b(sample.data, position == 0);
            if self.feed(&data, sample.pts).is_ok() {
                self.drain(samples, sink);
            }
        }
        // SAFETY: as above.
        let _ = unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) };
        self.drain(samples, sink);
        Ok(())
    }
}
