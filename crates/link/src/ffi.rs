//! Hand-written bindings to the parts of Link's C interface (`abl_link.h`)
//! Beatlayer uses, and to its own sink shim (`shim.cpp`).

use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct AblLink {
    pub(crate) imp: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct AblSessionState {
    pub(crate) imp: *mut c_void,
}

unsafe extern "C" {
    pub(crate) fn abl_link_create(bpm: f64) -> AblLink;
    pub(crate) fn abl_link_destroy(link: AblLink);
    pub(crate) fn abl_link_is_enabled(link: AblLink) -> bool;
    pub(crate) fn abl_link_enable(link: AblLink, enable: bool);
    pub(crate) fn abl_link_is_start_stop_sync_enabled(link: AblLink) -> bool;
    pub(crate) fn abl_link_enable_start_stop_sync(link: AblLink, enabled: bool);
    pub(crate) fn abl_link_num_peers(link: AblLink) -> u64;
    pub(crate) fn abl_link_set_num_peers_callback(
        link: AblLink,
        callback: extern "C" fn(u64, *mut c_void),
        context: *mut c_void,
    );
    pub(crate) fn abl_link_set_tempo_callback(
        link: AblLink,
        callback: extern "C" fn(f64, *mut c_void),
        context: *mut c_void,
    );
    pub(crate) fn abl_link_set_start_stop_callback(
        link: AblLink,
        callback: extern "C" fn(bool, *mut c_void),
        context: *mut c_void,
    );
    pub(crate) fn abl_link_clock_micros(link: AblLink) -> i64;
    pub(crate) fn abl_link_create_session_state() -> AblSessionState;
    pub(crate) fn abl_link_destroy_session_state(state: AblSessionState);
    pub(crate) fn abl_link_capture_audio_session_state(link: AblLink, state: AblSessionState);
    pub(crate) fn abl_link_commit_audio_session_state(link: AblLink, state: AblSessionState);
    pub(crate) fn abl_link_capture_app_session_state(link: AblLink, state: AblSessionState);
    pub(crate) fn abl_link_commit_app_session_state(link: AblLink, state: AblSessionState);
    pub(crate) fn abl_link_tempo(state: AblSessionState) -> f64;
    pub(crate) fn abl_link_set_tempo(state: AblSessionState, bpm: f64, at_time: i64);
    pub(crate) fn abl_link_beat_at_time(state: AblSessionState, time: i64, quantum: f64) -> f64;
    pub(crate) fn abl_link_phase_at_time(state: AblSessionState, time: i64, quantum: f64) -> f64;
    pub(crate) fn abl_link_time_at_beat(state: AblSessionState, beat: f64, quantum: f64) -> i64;
    pub(crate) fn abl_link_request_beat_at_time(
        state: AblSessionState,
        beat: f64,
        time: i64,
        quantum: f64,
    );
    pub(crate) fn abl_link_set_is_playing(state: AblSessionState, is_playing: bool, time: i64);
    pub(crate) fn abl_link_is_playing(state: AblSessionState) -> bool;
    pub(crate) fn abl_link_time_for_is_playing(state: AblSessionState) -> i64;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id8 {
    pub(crate) bytes: [u8; 8],
}

#[repr(C)]
pub(crate) struct AblChannel {
    pub(crate) id: Id8,
    pub(crate) name: *const c_char,
    pub(crate) peer_id: Id8,
    pub(crate) peer_name: *const c_char,
}

#[repr(C)]
pub(crate) struct AblChannelList {
    pub(crate) channels: *mut AblChannel,
    pub(crate) count: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct AblSink {
    pub(crate) imp: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct AblSource {
    pub(crate) imp: *mut c_void,
}

#[repr(C)]
pub(crate) struct AblSourceInfo {
    pub(crate) num_channels: usize,
    pub(crate) num_frames: usize,
    pub(crate) sample_rate: u32,
    pub(crate) count: u64,
    pub(crate) session_beat_time: f64,
    pub(crate) tempo: f64,
    pub(crate) session_id: Id8,
}

#[repr(C)]
pub(crate) struct AblSourceBuffer {
    pub(crate) samples: *mut i16,
    pub(crate) info: AblSourceInfo,
}

unsafe extern "C" {
    pub(crate) fn abl_link_audio_is_link_audio_enabled(link: AblLink) -> bool;
    pub(crate) fn abl_link_audio_enable_link_audio(link: AblLink, enabled: bool);
    pub(crate) fn abl_link_audio_peer_name(
        link: AblLink,
        buffer: *mut c_char,
        buffer_size: usize,
    ) -> usize;
    pub(crate) fn abl_link_audio_set_peer_name(link: AblLink, name: *const c_char);
    pub(crate) fn abl_link_audio_get_channels(link: AblLink) -> AblChannelList;
    pub(crate) fn abl_link_audio_free_channel_list(list: AblChannelList);
    pub(crate) fn abl_link_audio_set_channels_changed_callback(
        link: AblLink,
        callback: extern "C" fn(*mut c_void),
        context: *mut c_void,
    );
    pub(crate) fn abl_link_audio_sink_create(
        link: AblLink,
        name: *const c_char,
        max_num_samples: usize,
    ) -> AblSink;
    pub(crate) fn abl_link_audio_sink_destroy(sink: AblSink);
    pub(crate) fn abl_link_audio_sink_set_name(sink: AblSink, name: *const c_char);
    pub(crate) fn abl_link_audio_source_create(
        link: AblLink,
        channel: Id8,
        callback: extern "C" fn(*const AblSourceBuffer, *mut c_void),
        context: *mut c_void,
    ) -> AblSource;
    pub(crate) fn abl_link_audio_source_destroy(source: AblSource);
    pub(crate) fn abl_link_audio_source_buffer_info_begin_beats(
        info: *const AblSourceInfo,
        state: AblSessionState,
        quantum: f64,
        out_beats: *mut f64,
    ) -> bool;
    pub(crate) fn bl_link_audio_sink_send(
        sink: AblSink,
        state: AblSessionState,
        left: *const f32,
        right: *const f32,
        frames: usize,
        beats_at_buffer_begin: f64,
        quantum: f64,
        sample_rate: u32,
    ) -> bool;
}
