//! FFmpeg's shared libraries, loaded at run time where they're installed
//! (R26): the decoders for H.264, HEVC and ProRes on Linux. Nothing is linked
//! or downloaded: a computer without the libraries refuses those codecs with
//! the reason, and Motion JPEG still decodes in Rust.
//!
//! Only the parts of FFmpeg's structures whose layout has held from FFmpeg 4
//! to 8 are read directly (the head of `AVFrame` and of `AVPacket`); the rest
//! is reached through functions and `AVOption`s. The decoders are fed
//! start-code streams ([`crate::annexb`]), so they need no codec context
//! fields set, and the pictures are converted to RGBA by `libswscale`.

use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};

use libloading::Library;

use crate::annexb::Config;
use crate::decoder::{Codec, CodedSample, Frame, VideoDecoder};
use crate::error::VideoError;
use crate::movie::{MAX_DIMENSION, MAX_PIXELS, VideoTrack};

type Ptr = *mut c_void;

/// The `(libavcodec, libavutil, libswscale)` major versions of FFmpeg 8, 7,
/// 6, 5 and 4, newest first.
const VERSIONS: [(u32, u32, u32); 5] = [
    (62, 60, 9),
    (61, 59, 8),
    (60, 58, 7),
    (59, 57, 6),
    (58, 56, 5),
];

const AV_CODEC_ID_H264: c_int = 27;
const AV_CODEC_ID_PRORES: c_int = 147;
const AV_CODEC_ID_HEVC: c_int = 173;
const AV_PIX_FMT_RGBA: c_int = 26;
const SWS_BICUBIC: c_int = 4;
const SWS_FULL_CHR_H_INT: c_int = 0x2000;
const SWS_ACCURATE_RND: c_int = 0x40000;
const SWS_CS_ITU709: c_int = 1;
const SWS_CS_ITU601: c_int = 5;
const SWS_CS_BT2020: c_int = 9;

/// The functions used, with the libraries that hold them.
struct Api {
    find_decoder: unsafe extern "C" fn(c_int) -> *const c_void,
    alloc_context: unsafe extern "C" fn(*const c_void) -> Ptr,
    open: unsafe extern "C" fn(Ptr, *const c_void, *mut Ptr) -> c_int,
    free_context: unsafe extern "C" fn(*mut Ptr),
    send_packet: unsafe extern "C" fn(Ptr, *const c_void) -> c_int,
    receive_frame: unsafe extern "C" fn(Ptr, Ptr) -> c_int,
    flush_buffers: unsafe extern "C" fn(Ptr),
    packet_alloc: unsafe extern "C" fn() -> Ptr,
    packet_free: unsafe extern "C" fn(*mut Ptr),
    new_packet: unsafe extern "C" fn(Ptr, c_int) -> c_int,
    frame_alloc: unsafe extern "C" fn() -> Ptr,
    frame_free: unsafe extern "C" fn(*mut Ptr),
    frame_unref: unsafe extern "C" fn(Ptr),
    opt_get_int: unsafe extern "C" fn(Ptr, *const c_char, c_int, *mut i64) -> c_int,
    sws_get_context: unsafe extern "C" fn(
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        Ptr,
        Ptr,
        *const f64,
    ) -> Ptr,
    sws_scale: unsafe extern "C" fn(
        Ptr,
        *const *const u8,
        *const c_int,
        c_int,
        c_int,
        *const *mut u8,
        *const c_int,
    ) -> c_int,
    sws_free: unsafe extern "C" fn(Ptr),
    sws_set_details: unsafe extern "C" fn(
        Ptr,
        *const c_int,
        c_int,
        *const c_int,
        c_int,
        c_int,
        c_int,
        c_int,
    ) -> c_int,
    sws_coefficients: unsafe extern "C" fn(c_int) -> *const c_int,
    // Last, so they drop after nothing needs the functions.
    _libraries: [Library; 3],
}

fn library_name(stem: &str, major: u32) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{stem}.{major}.dylib")
    } else {
        format!("lib{stem}.so.{major}")
    }
}

fn load(stem: &str, major: u32, dir: Option<&Path>) -> Result<Library, String> {
    let name = library_name(stem, major);
    let path: PathBuf = dir.map_or_else(|| PathBuf::from(&name), |d| d.join(&name));
    // SAFETY: loading a shared library runs its initialisers; FFmpeg's do
    // nothing but set up tables.
    unsafe { Library::new(&path) }.map_err(|e| format!("{}: {e}", path.display()))
}

macro_rules! symbol {
    ($lib:expr, $name:literal) => {
        // SAFETY: the type of the field this fills is the function's C
        // signature in FFmpeg's headers.
        *unsafe { $lib.get($name) }
            .map_err(|e| format!("{}: {e}", String::from_utf8_lossy($name)))?
    };
}

impl Api {
    fn load(dir: Option<&Path>) -> Result<Self, String> {
        let mut reasons = Vec::new();
        for (codec, util, scale) in VERSIONS {
            let libs = (|| {
                Ok::<_, String>((
                    load("avcodec", codec, dir)?,
                    load("avutil", util, dir)?,
                    load("swscale", scale, dir)?,
                ))
            })();
            match libs {
                Ok((avcodec, avutil, swscale)) => return Self::bind(avcodec, avutil, swscale),
                Err(reason) => reasons.push(reason),
            }
        }
        Err(format!(
            "FFmpeg's libraries aren't installed (tried FFmpeg 4 to 8; first failure: {})",
            reasons.first().map_or("none", String::as_str)
        ))
    }

    fn bind(avcodec: Library, avutil: Library, swscale: Library) -> Result<Self, String> {
        Ok(Self {
            find_decoder: symbol!(avcodec, b"avcodec_find_decoder\0"),
            alloc_context: symbol!(avcodec, b"avcodec_alloc_context3\0"),
            open: symbol!(avcodec, b"avcodec_open2\0"),
            free_context: symbol!(avcodec, b"avcodec_free_context\0"),
            send_packet: symbol!(avcodec, b"avcodec_send_packet\0"),
            receive_frame: symbol!(avcodec, b"avcodec_receive_frame\0"),
            flush_buffers: symbol!(avcodec, b"avcodec_flush_buffers\0"),
            packet_alloc: symbol!(avcodec, b"av_packet_alloc\0"),
            packet_free: symbol!(avcodec, b"av_packet_free\0"),
            new_packet: symbol!(avcodec, b"av_new_packet\0"),
            frame_alloc: symbol!(avutil, b"av_frame_alloc\0"),
            frame_free: symbol!(avutil, b"av_frame_free\0"),
            frame_unref: symbol!(avutil, b"av_frame_unref\0"),
            opt_get_int: symbol!(avutil, b"av_opt_get_int\0"),
            sws_get_context: symbol!(swscale, b"sws_getContext\0"),
            sws_scale: symbol!(swscale, b"sws_scale\0"),
            sws_free: symbol!(swscale, b"sws_freeContext\0"),
            sws_set_details: symbol!(swscale, b"sws_setColorspaceDetails\0"),
            sws_coefficients: symbol!(swscale, b"sws_getCoefficients\0"),
            _libraries: [avcodec, avutil, swscale],
        })
    }
}

/// A scaler to RGBA for one picture size and pixel format.
struct Scaler {
    context: Ptr,
    width: c_int,
    height: c_int,
    format: c_int,
}

/// An FFmpeg decoder.
pub(crate) struct Ffmpeg {
    api: Api,
    context: Ptr,
    frame: Ptr,
    config: Option<Config>,
    scaler: Option<Scaler>,
}

// SAFETY: the decoder's pointers belong to it alone, and it is used from one
// thread at a time: `VideoDecoder::decode` takes `&mut self`.
unsafe impl Send for Ffmpeg {}

/// A decoder for the track from the FFmpeg libraries in `dir`, or on the
/// system's library path if `None`. For H.264, HEVC and ProRes.
///
/// # Errors
///
/// The reason the libraries or the codec aren't there.
pub fn open(track: &VideoTrack, dir: Option<&Path>) -> Result<Box<dyn VideoDecoder>, String> {
    let codec = Codec::of(track).ok_or("FFmpeg isn't asked for this codec")?;
    open_codec(track, codec, dir).map(|d| Box::new(d) as Box<dyn VideoDecoder>)
}

pub(crate) fn open_codec(
    track: &VideoTrack,
    codec: Codec,
    dir: Option<&Path>,
) -> Result<Ffmpeg, String> {
    let id = match codec {
        Codec::H264 => AV_CODEC_ID_H264,
        Codec::Hevc => AV_CODEC_ID_HEVC,
        Codec::ProRes => AV_CODEC_ID_PRORES,
        Codec::Mjpeg => return Err("Motion JPEG decodes in Rust".into()),
    };
    let config = match codec {
        Codec::H264 | Codec::Hevc => Some(
            Config::parse(codec, track.extra())
                .ok_or("the movie's codec configuration is damaged")?,
        ),
        _ => None,
    };
    let api = Api::load(dir)?;
    // SAFETY: these are FFmpeg's documented calls; every pointer passed is
    // one FFmpeg returned, or null where the API takes null.
    let (context, frame) = unsafe {
        let decoder = (api.find_decoder)(id);
        if decoder.is_null() {
            return Err("this FFmpeg has no decoder for the codec".into());
        }
        let mut context = (api.alloc_context)(decoder);
        if context.is_null() {
            return Err("FFmpeg ran out of memory".into());
        }
        if (api.open)(context, decoder, std::ptr::null_mut()) < 0 {
            (api.free_context)(&raw mut context);
            return Err("FFmpeg's decoder wouldn't open".into());
        }
        (context, (api.frame_alloc)())
    };
    if frame.is_null() {
        return Err("FFmpeg ran out of memory".into());
    }
    Ok(Ffmpeg {
        api,
        context,
        frame,
        config,
        scaler: None,
    })
}

impl Ffmpeg {
    /// Sends one packet, or `None` to start the drain. Returns whether the
    /// decoder took it.
    fn send(&mut self, data: Option<&[u8]>) -> bool {
        let api = &self.api;
        // SAFETY: the packet is FFmpeg's own, allocated and filled through its
        // API: `av_new_packet` gives `size` writable bytes at `data`, whose
        // offset in an `AVPacket` (after a pointer and two `int64_t`) has
        // held since FFmpeg 2.
        unsafe {
            let Some(data) = data else {
                return (api.send_packet)(self.context, std::ptr::null()) >= 0;
            };
            let Ok(size) = c_int::try_from(data.len()) else {
                return false;
            };
            let mut packet = (api.packet_alloc)();
            if packet.is_null() {
                return false;
            }
            let taken = if (api.new_packet)(packet, size) >= 0 {
                let bytes = packet
                    .cast::<u8>()
                    .add(24)
                    .cast::<*mut u8>()
                    .read_unaligned();
                std::ptr::copy_nonoverlapping(data.as_ptr(), bytes, data.len());
                (api.send_packet)(self.context, packet) >= 0
            } else {
                false
            };
            (api.packet_free)(&raw mut packet);
            taken
        }
    }

    /// Takes every picture the decoder has ready, giving each to `sink` under
    /// the next frame in presentation order.
    fn receive(
        &mut self,
        order: &[usize],
        next: &mut usize,
        samples: &[CodedSample<'_>],
        sink: &mut dyn FnMut(usize, Frame),
    ) {
        loop {
            // SAFETY: `frame` is an `AVFrame` from `av_frame_alloc`.
            let received = unsafe { (self.api.receive_frame)(self.context, self.frame) };
            // Zero is a picture; anything else is "send more" or the end.
            if received != 0 {
                return;
            }
            if let (Some(&at), Some(picture)) = (order.get(*next), self.convert()) {
                sink(samples[at].index, picture);
            }
            *next += 1;
            // SAFETY: as above.
            unsafe { (self.api.frame_unref)(self.frame) };
        }
    }

    /// The decoded `AVFrame` as RGBA.
    fn convert(&mut self) -> Option<Frame> {
        // SAFETY: the head of `AVFrame`: `data[8]` at 0, `linesize[8]` at 64,
        // `extended_data` at 96, then `width`, `height`, `nb_samples` and
        // `format` as `int`s. It has held since FFmpeg 2.
        let (data, linesize, width, height, format) = unsafe {
            let at = self.frame.cast::<u8>();
            (
                at.cast::<*const u8>().read_unaligned_array::<8>(),
                at.add(64).cast::<c_int>().read_unaligned_array::<8>(),
                at.add(104).cast::<c_int>().read_unaligned(),
                at.add(108).cast::<c_int>().read_unaligned(),
                at.add(116).cast::<c_int>().read_unaligned(),
            )
        };
        let (w, h) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
        if w == 0
            || h == 0
            || w > MAX_DIMENSION
            || h > MAX_DIMENSION
            || u64::from(w) * u64::from(h) > MAX_PIXELS
        {
            return None;
        }
        let scaler = self.scaler_for(width, height, format)?;
        let mut rgba = vec![255u8; w as usize * h as usize * 4];
        let destination = [
            rgba.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        let destination_stride = [width * 4, 0, 0, 0];
        // SAFETY: `data` and `linesize` describe the decoded picture, which
        // stays valid until `frame_unref`; the destination holds `w * h * 4`
        // bytes.
        let rows = unsafe {
            (self.api.sws_scale)(
                scaler,
                data.as_ptr(),
                linesize.as_ptr(),
                0,
                height,
                destination.as_ptr(),
                destination_stride.as_ptr(),
            )
        };
        if rows != height {
            return None;
        }
        // The picture is opaque whatever the codec's alpha.
        rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        Frame::new(w, h, rgba)
    }

    fn scaler_for(&mut self, width: c_int, height: c_int, format: c_int) -> Option<Ptr> {
        if let Some(s) = &self.scaler
            && (s.width, s.height, s.format) == (width, height, format)
        {
            return Some(s.context);
        }
        self.free_scaler();
        // The matrix the stream says, else BT.709 from 720 lines up and
        // BT.601 below, as players do; the range from the pixel format's J
        // forms (`yuvj420p` and kin) or the stream's `color_range`.
        let option = |name: &str| {
            let name = CString::new(name).ok()?;
            let mut value = 0i64;
            // SAFETY: `context` is an `AVCodecContext`, which has AVOptions.
            let found =
                unsafe { (self.api.opt_get_int)(self.context, name.as_ptr(), 0, &raw mut value) };
            (found >= 0).then_some(value)
        };
        let matrix = match option("colorspace") {
            Some(1) => SWS_CS_ITU709,
            Some(9 | 10) => SWS_CS_BT2020,
            Some(5 | 6) => SWS_CS_ITU601,
            _ if height >= 720 => SWS_CS_ITU709,
            _ => SWS_CS_ITU601,
        };
        let full_range = matches!(format, 12..=14 | 32) || option("color_range") == Some(2);
        // SAFETY: the context is made and configured through swscale's API;
        // the coefficient tables it returns are four `int`s that live as long
        // as the library.
        let context = unsafe {
            let context = (self.api.sws_get_context)(
                width,
                height,
                format,
                width,
                height,
                AV_PIX_FMT_RGBA,
                SWS_BICUBIC | SWS_FULL_CHR_H_INT | SWS_ACCURATE_RND,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if context.is_null() {
                return None;
            }
            let table = (self.api.sws_coefficients)(matrix);
            (self.api.sws_set_details)(
                context,
                table,
                c_int::from(full_range),
                table,
                1,
                0,
                1 << 16,
                1 << 16,
            );
            context
        };
        self.scaler = Some(Scaler {
            context,
            width,
            height,
            format,
        });
        Some(context)
    }

    fn free_scaler(&mut self) {
        if let Some(s) = self.scaler.take() {
            // SAFETY: a context from `sws_getContext`, freed once.
            unsafe { (self.api.sws_free)(s.context) };
        }
    }
}

trait ReadArray<T> {
    /// Reads `N` consecutive values from an unaligned pointer.
    ///
    /// # Safety
    ///
    /// `N` values must be readable at the pointer.
    unsafe fn read_unaligned_array<const N: usize>(self) -> [T; N];
}

impl<T: Copy> ReadArray<T> for *const T {
    unsafe fn read_unaligned_array<const N: usize>(self) -> [T; N] {
        // SAFETY: the caller guarantees `N` readable values.
        std::array::from_fn(|i| unsafe { self.add(i).read_unaligned() })
    }
}

impl<T: Copy> ReadArray<T> for *mut T {
    unsafe fn read_unaligned_array<const N: usize>(self) -> [T; N] {
        // SAFETY: as above.
        unsafe { self.cast_const().read_unaligned_array::<N>() }
    }
}

impl VideoDecoder for Ffmpeg {
    fn decode(
        &mut self,
        samples: &[CodedSample<'_>],
        sink: &mut dyn FnMut(usize, Frame),
    ) -> Result<(), VideoError> {
        // SAFETY: `context` is an open codec context. Flushing also leaves
        // the draining state of the call before.
        unsafe { (self.api.flush_buffers)(self.context) };
        // The decoder returns pictures in presentation order, so the k-th is
        // the k-th of the samples by presentation time.
        let mut order: Vec<usize> = (0..samples.len()).collect();
        order.sort_by_key(|&i| samples[i].pts);
        let mut next = 0;
        for (position, sample) in samples.iter().enumerate() {
            let data = match &self.config {
                Some(config) => config.to_annex_b(sample.data, position == 0),
                None => sample.data.to_vec(),
            };
            if !self.send(Some(&data)) {
                // A frame the decoder refused has no picture to match.
                order.retain(|&i| i != position);
                continue;
            }
            self.receive(&order, &mut next, samples, sink);
        }
        self.send(None);
        self.receive(&order, &mut next, samples, sink);
        Ok(())
    }
}

impl Drop for Ffmpeg {
    fn drop(&mut self) {
        self.free_scaler();
        // SAFETY: both were made by FFmpeg and are freed once, here.
        unsafe {
            (self.api.frame_free)(&raw mut self.frame);
            (self.api.free_context)(&raw mut self.context);
        }
    }
}
