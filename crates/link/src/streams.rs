//! Link Audio: the peer's name and switch, the channels other peers offer,
//! a [`Sink`] that offers one of Beatlayer's own, and a [`Source`] that
//! receives one (the Link Audio spec, R1, R3, R4, R6, R11).
//!
//! Sinks and sources are made and dropped off the audio thread. A sink's
//! [`send`](Sink::send) is the one call of this module the audio thread
//! makes; a source's callback runs on Link's own thread.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;

use crate::{Audio, Inner, Link, State, ffi};

/// The quantum Link Audio's beats are exchanged in. Every sink commits and
/// every source maps its buffers with this one, so a peer's beat maps onto
/// the same moment whatever each side's own quantum is.
pub const QUANTUM: f64 = 4.0;

/// How many samples a sink's buffers can hold at most: stereo frames of the
/// largest block (`Audio::send` sends longer blocks in pieces).
const MAX_FRAMES: usize = 8192;

/// Names a channel across its peer's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelId([u8; 8]);

/// Names a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PeerId([u8; 8]);

/// A channel another peer offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    /// The channel's identifier, which a [`Source`] takes.
    pub id: ChannelId,
    /// The channel's name, as its peer shows it.
    pub name: String,
    /// The peer that offers it.
    pub peer: PeerId,
    /// The peer's name.
    pub peer_name: String,
}

/// Turns a C string Link allocated into an owned one, lossily.
///
/// # Safety
///
/// `text` is null or a null-terminated string.
unsafe fn owned(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// A C string from `text`, cut at its first null byte if it has one.
fn c_string(text: &str) -> CString {
    let end = text.find('\0').unwrap_or(text.len());
    CString::new(&text[..end]).unwrap_or_default()
}

impl Link {
    /// Whether Link Audio is switched on. It streams only while Link is on
    /// too (the Link Audio spec, R1).
    #[must_use]
    pub fn audio_enabled(&self) -> bool {
        // SAFETY: thread-safe and real-time-safe, as `abl_link.h` says.
        unsafe { ffi::abl_link_audio_is_link_audio_enabled(self.inner.raw) }
    }

    /// Switches Link Audio.
    pub fn set_audio_enabled(&self, on: bool) {
        // SAFETY: thread-safe, and not real-time-safe: the main thread's.
        unsafe { ffi::abl_link_audio_enable_link_audio(self.inner.raw, on) };
    }

    /// The name other peers see this app by.
    #[must_use]
    pub fn audio_name(&self) -> String {
        // SAFETY: thread-safe; the buffer is `len + 1` bytes, which holds
        // the name and its terminator as the first call reported.
        unsafe {
            let len = ffi::abl_link_audio_peer_name(self.inner.raw, std::ptr::null_mut(), 0);
            let mut bytes = vec![0u8; len + 1];
            ffi::abl_link_audio_peer_name(self.inner.raw, bytes.as_mut_ptr().cast(), len + 1);
            bytes.truncate(len);
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    /// Sets the name other peers see this app by.
    pub fn set_audio_name(&self, name: &str) {
        let name = c_string(name);
        // SAFETY: thread-safe; Link copies the name.
        unsafe { ffi::abl_link_audio_set_peer_name(self.inner.raw, name.as_ptr()) };
    }

    /// The channels the other peers in the session offer now, in the order
    /// Link lists them.
    #[must_use]
    pub fn channels(&self) -> Vec<Channel> {
        // SAFETY: the list is Link's, freed once after it is copied.
        unsafe {
            let list = ffi::abl_link_audio_get_channels(self.inner.raw);
            let channels = if list.channels.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(list.channels, list.count)
                    .iter()
                    .map(|channel| Channel {
                        id: ChannelId(channel.id.bytes),
                        name: owned(channel.name),
                        peer: PeerId(channel.peer_id.bytes),
                        peer_name: owned(channel.peer_name),
                    })
                    .collect()
            };
            ffi::abl_link_audio_free_channel_list(list);
            channels
        }
    }

    /// Offers a channel of `name` to the session, which lasts as long as the
    /// sink does.
    #[must_use]
    pub fn sink(&self, name: &str) -> Sink {
        let name = c_string(name);
        // SAFETY: Link copies the name; the sink keeps the instance alive.
        let raw = unsafe {
            ffi::abl_link_audio_sink_create(self.inner.raw, name.as_ptr(), MAX_FRAMES * 2)
        };
        Sink {
            raw,
            _link: Arc::clone(&self.inner),
        }
    }

    /// Receives `channel`: `callback` runs on Link's thread for each buffer
    /// that arrives, until the source is dropped.
    #[must_use]
    pub fn source(
        &self,
        channel: ChannelId,
        callback: impl FnMut(&Received<'_>) + Send + 'static,
    ) -> Source {
        let context = Box::into_raw(Box::new(SourceContext {
            callback: Box::new(callback),
            state: State::new(),
            link: self.inner.raw,
        }));
        // SAFETY: the context is freed by `Source::drop`, after the source
        // is destroyed, which returns only once no callback is running.
        let raw = unsafe {
            ffi::abl_link_audio_source_create(
                self.inner.raw,
                ffi::Id8 { bytes: channel.0 },
                on_buffer,
                context.cast(),
            )
        };
        Source {
            raw,
            context,
            _link: Arc::clone(&self.inner),
        }
    }
}

/// A channel offered to the session. Its [`send`](Self::send) is for one
/// thread at a time, the audio thread's; everything else is for the main
/// thread.
pub struct Sink {
    raw: ffi::AblSink,
    _link: Arc<Inner>,
}

// SAFETY: Link's sink is thread-safe but for retaining and committing a
// buffer, which `send` does and its caller calls from one thread only.
unsafe impl Send for Sink {}
// SAFETY: as above.
unsafe impl Sync for Sink {}

impl Sink {
    /// Renames the channel.
    pub fn set_name(&self, name: &str) {
        let name = c_string(name);
        // SAFETY: thread-safe; Link copies the name.
        unsafe { ffi::abl_link_audio_sink_set_name(self.raw, name.as_ptr()) };
    }

    /// Sends one block, stereo at `rate`, whose first frame is at session
    /// beat `beats` (mapped with [`QUANTUM`] by `audio`'s captured state).
    /// Nothing is sent while no peer takes the channel. Real-time safe: no
    /// allocation, lock or system call, through Beatlayer's shim rather than
    /// Link's C interface, which allocates a buffer handle. Returns whether
    /// the block was handed to Link.
    pub fn send(&self, audio: &Audio, beats: f64, left: &[f32], right: &[f32], rate: u32) -> bool {
        let frames = left.len().min(right.len());
        let mut sent = frames > 0;
        let mut done = 0;
        while done < frames {
            let part = (frames - done).min(MAX_FRAMES);
            let at = beats + done as f64 * audio.tempo() / 60.0 / f64::from(rate);
            // SAFETY: both slices hold `part` frames from `done` on; the
            // state is the handle's own, captured by this thread.
            sent &= unsafe {
                ffi::bl_link_audio_sink_send(
                    self.raw,
                    audio.state.0,
                    left[done..].as_ptr(),
                    right[done..].as_ptr(),
                    part,
                    at,
                    QUANTUM,
                    rate,
                )
            };
            done += part;
        }
        sent
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        // SAFETY: made by `Link::sink`, destroyed once; the instance is
        // kept alive until the field drops after this.
        unsafe { ffi::abl_link_audio_sink_destroy(self.raw) };
    }
}

/// One buffer a peer sent, as [`Source`]'s callback sees it. Its samples
/// last only for the call.
pub struct Received<'a> {
    /// The samples, interleaved: one a frame for a mono channel, two for a
    /// stereo one.
    pub samples: &'a [i16],
    /// 1 or 2.
    pub channels: usize,
    /// The frames the buffer holds.
    pub frames: usize,
    /// The peer's sample rate.
    pub rate: u32,
    /// When the buffer's first frame falls, in microseconds on Link's clock,
    /// as the session maps the beat it was sent on. Nothing if the buffer
    /// comes from another session.
    pub begin_micros: Option<i64>,
}

struct SourceContext {
    callback: Box<dyn FnMut(&Received<'_>) + Send>,
    state: State,
    link: ffi::AblLink,
}

extern "C" fn on_buffer(buffer: *const ffi::AblSourceBuffer, context: *mut c_void) {
    // SAFETY: `context` is the source's context, alive until the source is
    // destroyed, and Link calls this from one thread at a time; the buffer
    // and its samples last for this call.
    let (context, buffer) = unsafe { (&mut *context.cast::<SourceContext>(), &*buffer) };
    let info = &buffer.info;
    if buffer.samples.is_null() || !(1..=2).contains(&info.num_channels) || info.sample_rate == 0 {
        return;
    }
    let mut beats = 0.0;
    // SAFETY: the state is this context's own; the app pair is thread-safe.
    let begin_micros = unsafe {
        ffi::abl_link_capture_app_session_state(context.link, context.state.0);
        ffi::abl_link_audio_source_buffer_info_begin_beats(
            info,
            context.state.0,
            QUANTUM,
            &raw mut beats,
        )
        .then(|| ffi::abl_link_time_at_beat(context.state.0, beats, QUANTUM))
    };
    // SAFETY: Link guarantees `frames * channels` samples.
    let samples =
        unsafe { std::slice::from_raw_parts(buffer.samples, info.num_frames * info.num_channels) };
    (context.callback)(&Received {
        samples,
        channels: info.num_channels,
        frames: info.num_frames,
        rate: info.sample_rate,
        begin_micros,
    });
}

/// A peer's channel received: its callback is called until this is dropped.
pub struct Source {
    raw: ffi::AblSource,
    context: *mut SourceContext,
    _link: Arc<Inner>,
}

// SAFETY: the context is only used by Link's callback thread until the
// source is dropped, which waits for it.
unsafe impl Send for Source {}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: destroying the source stops its callback, so the context
        // is freed only once nothing uses it.
        unsafe {
            ffi::abl_link_audio_source_destroy(self.raw);
            drop(Box::from_raw(self.context));
        }
    }
}
