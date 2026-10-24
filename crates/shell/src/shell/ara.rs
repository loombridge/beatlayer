//! ARA plug-ins in the shell (IMP-20; the ARA hosting spec): what they ask
//! of the transport, and what their editors are told is selected.

use bl_core::time::Ticks;
use bl_plugin_host::ara::PlaybackRequest;
use bl_shell_spec::transport as transport_ids;
use serde_json::json;

use super::Shell;

impl Shell {
    /// Spec: ara-hosting R11, R12. Runs the transport requests the ARA
    /// plug-ins made, as the person pressing the same buttons would, and
    /// tells their editors what the Arrangement has selected when that
    /// changed.
    pub(super) fn follow_ara(&mut self) {
        for request in self.vst3.take_ara_requests() {
            let tempo = &self.editor.document().settings().tempo;
            let ticks = |seconds: f64| tempo.seconds_to_ticks(seconds.max(0.0));
            let (id, args) = match request {
                PlaybackRequest::Start => (transport_ids::PLAY, json!({})),
                PlaybackRequest::Stop => (transport_ids::STOP, json!({})),
                PlaybackRequest::Locate(seconds) => {
                    (transport_ids::LOCATE, json!({ "position": ticks(seconds) }))
                }
                PlaybackRequest::CycleRange { start, duration } => {
                    let (from, to) = (ticks(start), ticks(start + duration));
                    let length = Ticks((to.get() - from.get()).max(1));
                    (
                        transport_ids::SET_LOOP,
                        json!({ "start": from, "length": length }),
                    )
                }
                PlaybackRequest::Cycle(enabled) => (
                    transport_ids::SET_LOOP_ENABLED,
                    json!({ "enabled": enabled }),
                ),
            };
            // A request that can't run, such as a locate past the song, is
            // the plug-in's to live without.
            let _ = self.run(id, args);
        }
        let selected = (
            self.arrangement.selected_clips().to_vec(),
            self.arrangement.selected_tracks().to_vec(),
        );
        if selected != self.ara_selection {
            self.vst3.ara_select(&selected.0, &selected.1);
            self.ara_selection = selected;
        }
    }
}
