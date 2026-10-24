//! The perception commands.

use super::{Shell, ShellError, to_value};
use bl_shell_spec::perception as perception_ids;
use serde_json::Value;

impl Shell {
    /// Runs a perception command (WP-1.16, WP-2.28): an audio preview or an
    /// analysis, rendered offline from the set as it is now. Neither
    /// changes the set, the view, or the engine playing on the device.
    pub(super) fn run_perception(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let engine_rate = self.engine_rate();
        let playing = self.audio.transport_state().playing;
        let source = crate::perception::Source {
            document: self.editor.document(),
            media: self.engine_media(),
            selection: self.arrangement.time_selection(),
            engine_rate,
            plugins: self.plugin_hosts(),
            playing,
            cancel: None,
        };
        match id {
            perception_ids::PREVIEW => {
                let args = Self::args(id, args)?;
                let outcome = crate::perception::render_preview(&source, &args)?;
                let preview = self.previews.add(
                    outcome.samples,
                    args.bit_depth,
                    outcome.rendered,
                    self.editor.document().clone(),
                );
                let result = preview
                    .result(outcome.peak_dbfs, outcome.took_seconds)
                    .map_err(|message| ShellError::Failed {
                        command: perception_ids::PREVIEW.to_owned(),
                        message,
                    })?;
                Ok(to_value(&result))
            }
            perception_ids::ANALYZE => {
                let args = Self::args(id, args)?;
                crate::perception::analyze(&self.previews, &source, &args)
                    .map(|result| to_value(&result))
            }
            _ => {
                let _: perception_ids::CaptureArgs = Self::args(id, args)?;
                Err(ShellError::Unavailable {
                    command: id.to_owned(),
                    reason: "a view capture is an image of the app's window, which only the \
                             agent API's `capture_view` tool returns, while the app's window \
                             is open"
                        .to_owned(),
                })
            }
        }
    }
}
