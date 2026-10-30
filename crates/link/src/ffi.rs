//! Hand-written bindings to the parts of Link's C interface (`abl_link.h`)
//! Beatlayer uses. Link Audio's functions aren't bound until Link Audio is.

use std::ffi::c_void;

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
