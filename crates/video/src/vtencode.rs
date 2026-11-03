//! H.264, HEVC and ProRes 422 encoders through VideoToolbox on macOS (the
//! export spec, R47): the compression sessions AVFoundation writes movies
//! through, driven without frame reordering so the frames come out in the
//! order they went in, and a movie of ours holds them.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
use core_foundation_sys::base::{CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::data::{CFDataGetBytePtr, CFDataGetLength, CFDataRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::{
    CFBooleanGetValue, CFNumberCreate, kCFBooleanFalse, kCFNumberFloat64Type, kCFNumberSInt32Type,
};
use core_foundation_sys::string::{CFStringCreateWithCString, CFStringRef, kCFStringEncodingUTF8};

use crate::decoder::Frame;
use crate::encoder::{CodedFrame, EncoderKind, EncoderSettings, SampleEntry, VideoEncoder};
use crate::error::VideoError;
use crate::rational::Rational;
use crate::videotoolbox::{CMTime, OSStatus, Ptr};

type OutputCallback = extern "C" fn(Ptr, Ptr, OSStatus, u32, Ptr);

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    static kCMSampleAttachmentKey_NotSync: CFStringRef;
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: CFStringRef;
    fn CMSampleBufferGetDataBuffer(sample: Ptr) -> Ptr;
    fn CMSampleBufferGetFormatDescription(sample: Ptr) -> Ptr;
    fn CMSampleBufferGetSampleAttachmentsArray(
        sample: Ptr,
        create_if_necessary: u8,
    ) -> *const c_void;
    fn CMBlockBufferGetDataLength(buffer: Ptr) -> usize;
    fn CMBlockBufferCopyDataBytes(
        buffer: Ptr,
        offset: usize,
        length: usize,
        destination: *mut u8,
    ) -> OSStatus;
    fn CMFormatDescriptionGetExtension(format: Ptr, key: CFStringRef) -> CFTypeRef;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTCompressionPropertyKey_Quality: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    fn VTCompressionSessionCreate(
        allocator: Ptr,
        width: i32,
        height: i32,
        codec: u32,
        specification: CFDictionaryRef,
        source_attributes: CFDictionaryRef,
        compressed_allocator: Ptr,
        callback: OutputCallback,
        refcon: Ptr,
        out: *mut Ptr,
    ) -> OSStatus;
    fn VTSessionSetProperty(session: Ptr, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: Ptr) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: Ptr,
        image: Ptr,
        presentation: CMTime,
        duration: CMTime,
        properties: CFDictionaryRef,
        source_refcon: Ptr,
        info_flags: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: Ptr, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: Ptr);
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferCreate(
        allocator: Ptr,
        width: usize,
        height: usize,
        format: u32,
        attributes: CFDictionaryRef,
        out: *mut Ptr,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: Ptr, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: Ptr, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: Ptr) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(buffer: Ptr) -> usize;
}

/// `kCVPixelFormatType_32BGRA`.
const BGRA: u32 = 0x4247_5241;

fn codec_of(kind: EncoderKind) -> Option<u32> {
    match kind {
        EncoderKind::H264 => Some(u32::from_be_bytes(*b"avc1")),
        EncoderKind::Hevc => Some(u32::from_be_bytes(*b"hvc1")),
        EncoderKind::ProRes422 => Some(u32::from_be_bytes(*b"apcn")),
        EncoderKind::MotionJpeg => None,
    }
}

fn fourcc_of(kind: EncoderKind) -> [u8; 4] {
    match kind {
        EncoderKind::H264 => *b"avc1",
        EncoderKind::Hevc => *b"hvc1",
        EncoderKind::ProRes422 => *b"apcn",
        EncoderKind::MotionJpeg => *b"jpeg",
    }
}

/// What the callback collects: coded frames in the order they come out, the
/// first failure, and the sample entry's extra boxes once a frame has them.
#[derive(Default)]
struct Output {
    frames: Mutex<Collected>,
}

#[derive(Default)]
struct Collected {
    frames: Vec<CodedFrame>,
    failure: Option<String>,
    extra: Option<Vec<([u8; 4], Vec<u8>)>>,
}

extern "C" fn on_frame(refcon: Ptr, _source: Ptr, status: OSStatus, _info: u32, sample: Ptr) {
    // SAFETY: `refcon` is the `Output` the session was made with, which the
    // encoder keeps alive until the session is ended.
    let output = unsafe { &*refcon.cast::<Output>() };
    let mut collected = output.frames.lock().unwrap_or_else(PoisonError::into_inner);
    if status != 0 {
        collected
            .failure
            .get_or_insert_with(|| format!("VideoToolbox failed on a frame (status {status})"));
        return;
    }
    if sample.is_null() {
        // A dropped frame: the movie would lose a frame's time.
        collected
            .failure
            .get_or_insert_with(|| "VideoToolbox dropped a frame".to_owned());
        return;
    }
    // SAFETY: `sample` is a sample buffer valid for this callback; the block
    // buffer, the attachments and the format description are read, not kept.
    unsafe {
        let buffer = CMSampleBufferGetDataBuffer(sample);
        if buffer.is_null() {
            collected
                .failure
                .get_or_insert_with(|| "VideoToolbox gave a frame without data".to_owned());
            return;
        }
        let length = CMBlockBufferGetDataLength(buffer);
        let mut data = vec![0u8; length];
        if CMBlockBufferCopyDataBytes(buffer, 0, length, data.as_mut_ptr()) != 0 {
            collected
                .failure
                .get_or_insert_with(|| "VideoToolbox gave a frame that can't be read".to_owned());
            return;
        }
        // A frame is a sync frame unless its attachments say it is not.
        let mut sync = true;
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 0);
        if !attachments.is_null() && CFArrayGetCount(attachments.cast()) > 0 {
            let dictionary = CFArrayGetValueAtIndex(attachments.cast(), 0).cast();
            let not_sync = CFDictionaryGetValue(dictionary, kCMSampleAttachmentKey_NotSync.cast());
            sync = not_sync.is_null() || !CFBooleanGetValue(not_sync.cast());
        }
        if collected.extra.is_none() {
            let format = CMSampleBufferGetFormatDescription(sample);
            if !format.is_null() {
                collected.extra = Some(extension_atoms(format));
            }
        }
        collected.frames.push(CodedFrame { data, sync });
    }
}

/// The boxes the format description keeps for the sample entry: `avcC` or
/// `hvcC`, as the bytes a movie stores.
///
/// # Safety
///
/// `format` is a live `CMFormatDescription`.
unsafe fn extension_atoms(format: Ptr) -> Vec<([u8; 4], Vec<u8>)> {
    let mut atoms = Vec::new();
    // SAFETY: by the caller; the values read are owned by the description.
    unsafe {
        let extension = CMFormatDescriptionGetExtension(
            format,
            kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
        );
        if extension.is_null() {
            return atoms;
        }
        for kind in [*b"avcC", *b"hvcC"] {
            let key = CFStringCreateWithCString(
                kCFAllocatorDefault,
                [&kind[..], b"\0"].concat().as_ptr().cast(),
                kCFStringEncodingUTF8,
            );
            let data = CFDictionaryGetValue(extension.cast(), key.cast()) as CFDataRef;
            if !data.is_null() {
                let bytes = std::slice::from_raw_parts(
                    CFDataGetBytePtr(data),
                    usize::try_from(CFDataGetLength(data)).unwrap_or(0),
                );
                atoms.push((kind, bytes.to_vec()));
            }
            CFRelease(key.cast());
        }
    }
    atoms
}

/// Whether this computer has an encoder for `kind`: a session made for a
/// small picture.
pub(crate) fn encodes(kind: EncoderKind) -> bool {
    static FOUND: OnceLock<[bool; 4]> = OnceLock::new();
    let found = FOUND.get_or_init(|| {
        EncoderKind::ALL.map(|kind| {
            codec_of(kind).is_some_and(|_| {
                let rate = Rational::new(30, 1);
                open_encoder(kind, 128, 128, rate, &EncoderSettings::default()).is_ok()
            })
        })
    });
    EncoderKind::ALL
        .iter()
        .position(|&k| k == kind)
        .is_some_and(|i| found[i])
}

/// A compression session making `kind`'s frames.
pub(crate) struct Compression {
    session: Ptr,
    kind: EncoderKind,
    output: Arc<Output>,
    frame_rate: Rational,
    width: u32,
    height: u32,
    next: i64,
}

// SAFETY: a compression session is thread-safe, and the encoder is used
// from one thread at a time.
unsafe impl Send for Compression {}

fn number(value: f64) -> CFTypeRef {
    // SAFETY: a double read from `value` during the call.
    unsafe {
        CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberFloat64Type,
            std::ptr::from_ref(&value).cast(),
        )
        .cast()
    }
}

pub(crate) fn open_encoder(
    kind: EncoderKind,
    width: u32,
    height: u32,
    frame_rate: Rational,
    settings: &EncoderSettings,
) -> Result<Compression, VideoError> {
    let codec = codec_of(kind)
        .ok_or_else(|| VideoError::Encode(format!("VideoToolbox has no {kind} encoder")))?;
    let output = Arc::new(Output::default());
    let mut session: Ptr = std::ptr::null_mut();
    // SAFETY: the refcon is the `Output` in the `Arc` the encoder keeps for
    // the session's life; `session` is an out parameter.
    let status = unsafe {
        VTCompressionSessionCreate(
            std::ptr::null_mut(),
            width as i32,
            height as i32,
            codec,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            on_frame,
            Arc::as_ptr(&output).cast_mut().cast(),
            &raw mut session,
        )
    };
    if status != 0 || session.is_null() {
        return Err(VideoError::Encode(format!(
            "VideoToolbox has no {kind} encoder for this size on this computer (status {status})"
        )));
    }
    // SAFETY: a live session and CoreFoundation values released after they
    // are set.
    unsafe {
        VTSessionSetProperty(
            session,
            kVTCompressionPropertyKey_RealTime,
            kCFBooleanFalse.cast(),
        );
        // The frames come out in the order they went in.
        VTSessionSetProperty(
            session,
            kVTCompressionPropertyKey_AllowFrameReordering,
            kCFBooleanFalse.cast(),
        );
        let rate = number(frame_rate.seconds());
        VTSessionSetProperty(session, kVTCompressionPropertyKey_ExpectedFrameRate, rate);
        CFRelease(rate);
        if kind.has_quality() {
            let quality = number(f64::from(settings.quality.clamp(1, 100)) / 100.0);
            VTSessionSetProperty(session, kVTCompressionPropertyKey_Quality, quality);
            CFRelease(quality);
        }
        // A sync frame about every two seconds.
        let interval = (frame_rate.seconds() * 2.0).ceil().max(1.0) as i32;
        let interval = CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberSInt32Type,
            std::ptr::from_ref(&interval).cast(),
        );
        VTSessionSetProperty(
            session,
            kVTCompressionPropertyKey_MaxKeyFrameInterval,
            interval.cast(),
        );
        CFRelease(interval.cast());
        VTCompressionSessionPrepareToEncodeFrames(session);
    }
    Ok(Compression {
        session,
        kind,
        output,
        frame_rate,
        width,
        height,
        next: 0,
    })
}

impl Compression {
    fn time(&self, value: i64, frames: i64) -> CMTime {
        CMTime {
            value: value * frames,
            timescale: i32::try_from(self.frame_rate.numer()).unwrap_or(i32::MAX),
            // `kCMTimeFlags_Valid`.
            flags: 1,
            epoch: 0,
        }
    }

    /// Takes what the callback has collected so far.
    fn drain(&self, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        let mut collected = self
            .output
            .frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        out.append(&mut collected.frames);
        match collected.failure.take() {
            Some(failure) => Err(VideoError::Encode(failure)),
            None => Ok(()),
        }
    }
}

impl VideoEncoder for Compression {
    fn encode(&mut self, frame: &Frame, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        if (frame.width(), frame.height()) != (self.width, self.height) {
            return Err(VideoError::Encode(
                "a picture of another size than the encoder's".into(),
            ));
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let mut buffer: Ptr = std::ptr::null_mut();
        // SAFETY: the pixel buffer is made, written row by row inside the
        // stride it reports, unlocked, handed to the session (which retains
        // it) and released.
        let status = unsafe {
            if CVPixelBufferCreate(
                std::ptr::null_mut(),
                w,
                h,
                BGRA,
                std::ptr::null(),
                &raw mut buffer,
            ) != 0
                || buffer.is_null()
                || CVPixelBufferLockBaseAddress(buffer, 0) != 0
            {
                return Err(VideoError::Encode(
                    "couldn't make a pixel buffer".to_owned(),
                ));
            }
            let base = CVPixelBufferGetBaseAddress(buffer).cast_mut();
            let stride = CVPixelBufferGetBytesPerRow(buffer);
            for y in 0..h {
                let row = std::slice::from_raw_parts_mut(base.add(y * stride), w * 4);
                let source = &frame.rgba()[y * w * 4..(y + 1) * w * 4];
                for (to, from) in row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(source.as_chunks::<4>().0)
                {
                    to.copy_from_slice(&[from[2], from[1], from[0], 255]);
                }
            }
            CVPixelBufferUnlockBaseAddress(buffer, 0);
            let den = i64::try_from(self.frame_rate.denom()).unwrap_or(1);
            let mut info = 0u32;
            let status = VTCompressionSessionEncodeFrame(
                self.session,
                buffer,
                self.time(self.next, den),
                self.time(1, den),
                std::ptr::null(),
                std::ptr::null_mut(),
                &raw mut info,
            );
            CFRelease(buffer.cast_const());
            status
        };
        self.next += 1;
        if status != 0 {
            return Err(VideoError::Encode(format!(
                "VideoToolbox refused a frame (status {status})"
            )));
        }
        self.drain(out)
    }

    fn finish(&mut self, out: &mut Vec<CodedFrame>) -> Result<(), VideoError> {
        // SAFETY: a live session; an invalid time asks for every frame.
        let status = unsafe {
            VTCompressionSessionCompleteFrames(
                self.session,
                CMTime {
                    value: 0,
                    timescale: 0,
                    flags: 0,
                    epoch: 0,
                },
            )
        };
        self.drain(out)?;
        if status != 0 {
            return Err(VideoError::Encode(format!(
                "VideoToolbox couldn't finish (status {status})"
            )));
        }
        Ok(())
    }

    fn entry(&self) -> Result<SampleEntry, VideoError> {
        let collected = self
            .output
            .frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(SampleEntry {
            fourcc: fourcc_of(self.kind),
            extra: collected
                .extra
                .clone()
                .ok_or_else(|| VideoError::Encode("no frame was encoded".to_owned()))?,
        })
    }
}

impl Drop for Compression {
    fn drop(&mut self) {
        // SAFETY: the session from `open_encoder`, ended and released once.
        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session.cast_const());
        }
    }
}
