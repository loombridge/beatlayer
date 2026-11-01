//! H.264, HEVC and ProRes through VideoToolbox on macOS (R26).
//!
//! A decompression session is made for each call to [`VideoDecoder::decode`]
//! and ended at its close, so every call starts afresh. Frames are decoded
//! without VideoToolbox's reordering: each comes back in decode order under
//! the index it was given as its time stamp, so a frame's place never depends
//! on a decoder's choice of order.

use std::ffi::c_void;
use std::sync::Mutex;

use core_foundation_sys::base::{CFRelease, kCFAllocatorDefault};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::{CFNumberCreate, kCFNumberSInt32Type};
use core_foundation_sys::string::CFStringRef;

use crate::annexb::Config;
use crate::decoder::{Codec, CodedSample, Frame, VideoDecoder};
use crate::error::VideoError;
use crate::movie::{MAX_DIMENSION, MAX_PIXELS, VideoTrack};

type OSStatus = i32;
type Ptr = *mut c_void;

/// `CMTime`, packed to four bytes as CoreMedia's headers pack it.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    const INVALID: Self = Self {
        value: 0,
        timescale: 0,
        flags: 0,
        epoch: 0,
    };

    fn at(value: i64) -> Self {
        // `kCMTimeFlags_Valid`.
        Self {
            value,
            timescale: 1,
            flags: 1,
            epoch: 0,
        }
    }
}

#[repr(C)]
struct CMSampleTimingInfo {
    duration: CMTime,
    presentation: CMTime,
    decode: CMTime,
}

type OutputCallback = extern "C" fn(Ptr, Ptr, OSStatus, u32, Ptr, CMTime, CMTime);

#[repr(C)]
struct CallbackRecord {
    callback: OutputCallback,
    refcon: Ptr,
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreate(
        allocator: Ptr,
        codec: u32,
        width: i32,
        height: i32,
        extensions: CFDictionaryRef,
        out: *mut Ptr,
    ) -> OSStatus;
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: Ptr,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_header_length: i32,
        out: *mut Ptr,
    ) -> OSStatus;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: Ptr,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_header_length: i32,
        extensions: CFDictionaryRef,
        out: *mut Ptr,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: Ptr,
        memory_block: Ptr,
        block_length: usize,
        block_allocator: Ptr,
        custom_block_source: *const c_void,
        offset_to_data: usize,
        data_length: usize,
        flags: u32,
        out: *mut Ptr,
    ) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(
        source: *const u8,
        destination: Ptr,
        offset: usize,
        length: usize,
    ) -> OSStatus;
    fn CMSampleBufferCreate(
        allocator: Ptr,
        data_buffer: Ptr,
        data_ready: u8,
        make_ready: *const c_void,
        make_ready_refcon: Ptr,
        format: Ptr,
        samples: isize,
        timing_entries: isize,
        timing: *const CMSampleTimingInfo,
        size_entries: isize,
        sizes: *const usize,
        out: *mut Ptr,
    ) -> OSStatus;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: Ptr,
        format: Ptr,
        specification: CFDictionaryRef,
        destination_attributes: CFDictionaryRef,
        callback: *const CallbackRecord,
        out: *mut Ptr,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: Ptr,
        sample: Ptr,
        flags: u32,
        source_refcon: Ptr,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: Ptr) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: Ptr);
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    fn CVPixelBufferLockBaseAddress(buffer: Ptr, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: Ptr, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: Ptr) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(buffer: Ptr) -> usize;
    fn CVPixelBufferGetWidth(buffer: Ptr) -> usize;
    fn CVPixelBufferGetHeight(buffer: Ptr) -> usize;
}

/// `kCVPixelFormatType_32BGRA`.
const BGRA: i32 = 0x4247_5241;
/// `kCVPixelBufferLock_ReadOnly`.
const LOCK_READ_ONLY: u64 = 1;

pub(crate) struct VideoToolbox {
    /// A `CMVideoFormatDescription`.
    format: Ptr,
}

// SAFETY: the format description is immutable and CoreMedia's are
// thread-safe; the decoder is used from one thread at a time anyway.
unsafe impl Send for VideoToolbox {}

pub(crate) fn open(track: &VideoTrack, codec: Codec) -> Result<VideoToolbox, String> {
    let mut format: Ptr = std::ptr::null_mut();
    let status = match codec {
        Codec::H264 | Codec::Hevc => {
            let config = Config::parse(codec, track.extra())
                .ok_or("the movie's codec configuration is damaged")?;
            let pointers: Vec<*const u8> =
                config.parameter_sets.iter().map(|s| s.as_ptr()).collect();
            let sizes: Vec<usize> = config.parameter_sets.iter().map(Vec::len).collect();
            let length_size = config.length_size as i32;
            // SAFETY: the pointers and sizes describe `parameter_sets`, which
            // outlive the call; `format` is an out parameter.
            unsafe {
                if codec == Codec::H264 {
                    CMVideoFormatDescriptionCreateFromH264ParameterSets(
                        std::ptr::null_mut(),
                        pointers.len(),
                        pointers.as_ptr(),
                        sizes.as_ptr(),
                        length_size,
                        &raw mut format,
                    )
                } else {
                    CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                        std::ptr::null_mut(),
                        pointers.len(),
                        pointers.as_ptr(),
                        sizes.as_ptr(),
                        length_size,
                        std::ptr::null(),
                        &raw mut format,
                    )
                }
            }
        }
        // SAFETY: an out parameter and no extensions.
        _ => unsafe {
            CMVideoFormatDescriptionCreate(
                std::ptr::null_mut(),
                u32::from_be_bytes(track.codec()),
                track.width() as i32,
                track.height() as i32,
                std::ptr::null(),
                &raw mut format,
            )
        },
    };
    if status != 0 || format.is_null() {
        return Err(format!(
            "VideoToolbox refused the codec's description (status {status})"
        ));
    }
    let decoder = VideoToolbox { format };
    // Make a session once, so a codec this computer can't decode is refused
    // here and not at the first frame.
    let output = Output::default();
    decoder.session(&output).map(|session| session.end())?;
    Ok(decoder)
}

/// What the callback collects.
#[derive(Default)]
struct Output {
    frames: Mutex<Vec<(usize, Frame)>>,
}

extern "C" fn on_frame(
    refcon: Ptr,
    _source: Ptr,
    status: OSStatus,
    _info: u32,
    image: Ptr,
    pts: CMTime,
    _duration: CMTime,
) {
    if status != 0 || image.is_null() {
        return;
    }
    let index = pts.value;
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    // SAFETY: `image` is a pixel buffer valid for this callback, in the BGRA
    // format the session asked for; it's locked while read, and its rows are
    // `bytes_per_row` apart with at least `width * 4` bytes in each.
    let frame = unsafe {
        if CVPixelBufferLockBaseAddress(image, LOCK_READ_ONLY) != 0 {
            return;
        }
        let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        let stride = CVPixelBufferGetBytesPerRow(image);
        let base = CVPixelBufferGetBaseAddress(image);
        let frame = (|| {
            let (w32, h32) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
            if w32 == 0
                || h32 == 0
                || w32 > MAX_DIMENSION
                || h32 > MAX_DIMENSION
                || u64::from(w32) * u64::from(h32) > MAX_PIXELS
                || base.is_null()
                || stride < w * 4
            {
                return None;
            }
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                let row = std::slice::from_raw_parts(base.add(y * stride), w * 4);
                for px in row.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
            Frame::new(w32, h32, rgba)
        })();
        CVPixelBufferUnlockBaseAddress(image, LOCK_READ_ONLY);
        frame
    };
    if let Some(frame) = frame {
        // SAFETY: `refcon` is the `Output` of the `decode` call that made the
        // session, which outlives the session.
        let output = unsafe { &*refcon.cast::<Output>() };
        output
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((index, frame));
    }
}

/// A decompression session, ended with [`Session::end`].
struct Session(Ptr);

impl Session {
    fn end(self) {
        // SAFETY: a session from `VTDecompressionSessionCreate`, ended and
        // released once.
        unsafe {
            VTDecompressionSessionInvalidate(self.0);
            CFRelease(self.0.cast_const());
        }
    }
}

impl VideoToolbox {
    fn session(&self, output: &Output) -> Result<Session, String> {
        let record = CallbackRecord {
            callback: on_frame,
            refcon: std::ptr::from_ref(output).cast_mut().cast(),
        };
        let mut session: Ptr = std::ptr::null_mut();
        // SAFETY: the pixel format key and number make a one-entry dictionary
        // of BGRA, which is released after the session has copied it; the
        // callback record is copied by the call.
        let status = unsafe {
            let format = BGRA;
            let number = CFNumberCreate(
                kCFAllocatorDefault,
                kCFNumberSInt32Type,
                std::ptr::from_ref(&format).cast(),
            );
            let keys = [kCVPixelBufferPixelFormatTypeKey.cast::<c_void>()];
            let values = [number.cast::<c_void>()];
            let attributes = CFDictionaryCreate(
                kCFAllocatorDefault,
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );
            let status = VTDecompressionSessionCreate(
                std::ptr::null_mut(),
                self.format,
                std::ptr::null(),
                attributes,
                &raw const record,
                &raw mut session,
            );
            CFRelease(attributes.cast());
            CFRelease(number.cast());
            status
        };
        if status != 0 || session.is_null() {
            return Err(format!(
                "VideoToolbox can't decode this codec on this computer (status {status})"
            ));
        }
        Ok(Session(session))
    }

    /// Hands one coded frame to the session under `index` as its time stamp.
    fn submit(&self, session: &Session, index: usize, data: &[u8]) {
        let timing = CMSampleTimingInfo {
            duration: CMTime::INVALID,
            presentation: CMTime::at(index as i64),
            decode: CMTime::at(index as i64),
        };
        let size = data.len();
        let mut block: Ptr = std::ptr::null_mut();
        let mut sample: Ptr = std::ptr::null_mut();
        // SAFETY: the block buffer allocates `size` bytes itself and the data
        // is copied into it; the sample buffer retains the block, so both are
        // released here; `VTDecompressionSessionDecodeFrame` retains the
        // sample for as long as it needs it.
        unsafe {
            let made = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                size,
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
                size,
                // `kCMBlockBufferAssureMemoryNowFlag`.
                1,
                &raw mut block,
            ) == 0
                && CMBlockBufferReplaceDataBytes(data.as_ptr(), block, 0, size) == 0
                && CMSampleBufferCreate(
                    std::ptr::null_mut(),
                    block,
                    1,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    self.format,
                    1,
                    1,
                    &raw const timing,
                    1,
                    &raw const size,
                    &raw mut sample,
                ) == 0;
            if made {
                let mut info = 0u32;
                // Synchronous: the frame is out before this returns, or the
                // call fails for this frame alone.
                let _ = VTDecompressionSessionDecodeFrame(
                    session.0,
                    sample,
                    0,
                    std::ptr::null_mut(),
                    &raw mut info,
                );
            }
            if !sample.is_null() {
                CFRelease(sample.cast_const());
            }
            if !block.is_null() {
                CFRelease(block.cast_const());
            }
        }
    }
}

impl VideoDecoder for VideoToolbox {
    fn decode(
        &mut self,
        samples: &[CodedSample<'_>],
        sink: &mut dyn FnMut(usize, Frame),
    ) -> Result<(), VideoError> {
        let output = Output::default();
        let session = self.session(&output).map_err(VideoError::Decode)?;
        for sample in samples {
            // H.264 and HEVC samples go in length-prefixed, as the format
            // description says, and ProRes whole: as the movie stores them.
            self.submit(&session, sample.index, sample.data);
        }
        // SAFETY: a live session.
        unsafe { VTDecompressionSessionWaitForAsynchronousFrames(session.0) };
        session.end();
        let frames = output
            .frames
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (index, frame) in frames {
            sink(index, frame);
        }
        Ok(())
    }
}

impl Drop for VideoToolbox {
    fn drop(&mut self) {
        // SAFETY: the format description from the `open`, released once.
        unsafe { CFRelease(self.format.cast_const()) };
    }
}
