//! The Browser's view, index and commands.

use super::{Area, Shell, ShellError};
use crate::auto_warp;
use crate::browser;
use crate::browser::BrowserError;
use crate::browser::BrowserView;
use bl_media::MediaStore;
use bl_shell_spec::browser as browser_ids;
use serde_json::Value;
use std::sync::Arc;

impl Shell {
    /// The browser (WP-1.10).
    #[must_use]
    pub fn browser_view(&self) -> &BrowserView {
        &self.browser_view
    }

    /// The browser's preview as the audio thread last reported it, for the
    /// preview strip.
    #[must_use]
    pub fn browser_preview(&self) -> browser_ids::PreviewState {
        self.browser_view.preview_state(&self.audio)
    }

    /// The media the set's clips play: the browser registers the files it
    /// adds, and the Arrangement's peak cache reads them.
    #[must_use]
    pub fn media_store(&self) -> Arc<MediaStore> {
        Arc::clone(&self.media)
    }

    /// Makes background work (the browser's index and preview decoder) call
    /// `wake` when it has news, so the host runs [`Shell::poll`].
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.browser_view.waker().set(wake);
    }

    /// Waits for the browser's index to finish, up to `timeout`, for tests
    /// and tools. Returns whether it finished.
    #[doc(hidden)]
    pub fn wait_for_browser_index(&mut self, timeout: std::time::Duration) -> bool {
        self.browser_view.wait_for_index(timeout)
    }

    /// Waits for the sets opened in the browser to be read, for tests and
    /// tools (set exchange R1).
    #[doc(hidden)]
    pub fn wait_for_browser_sets(&mut self) {
        self.browser_view.wait_for_sets();
        self.touch();
    }

    /// Runs a browser command and carries out what it asks of the rest of
    /// the shell.
    pub(super) fn run_browser(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        self.analysis_cache();
        let search = self.browser_view.search_folders();
        let context = browser::Context {
            insert_marker: self.arrangement.insert_marker(),
            selected_track: self.arrangement.selected_tracks().first().copied(),
            editor: &mut self.editor,
            audio: &mut self.audio,
            media: &self.media,
            edge_fades: self.prefs.create_fades_on_clip_edges.0,
            warping: auto_warp::Warping {
                prefs: self.prefs.warp,
                analysis: self.analysis.as_ref(),
            },
            collect_files: self.prefs.collect_files_on_export,
            search: &search,
        };
        let outcome = self
            .browser_view
            .run(id, args, context)
            .map_err(|error| match error {
                BrowserError::InvalidArgs(message) => ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                },
                BrowserError::Unavailable(reason) => ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                },
                BrowserError::Failed(message) => ShellError::Failed {
                    command: id.to_owned(),
                    message,
                },
            })?;
        for file in &outcome.media {
            self.store.add_media(file.hash, &file.path, file.size);
        }
        if outcome.document_changed {
            self.dirty = true;
            self.document_changed();
        }
        if outcome.prefs_changed {
            self.prefs.browser = self
                .browser_view
                .prefs(self.prefs.browser.user_library.clone());
            self.save_preferences();
        }
        if outcome.show_browser {
            self.show_browser_in(self.acting_window());
        }
        if let Some(message) = outcome.message {
            self.set_message(message, false);
        }
        if outcome.focus {
            self.focus_area(Area::Browser);
        }
        if id != browser_ids::STATE {
            self.touch();
        }
        Ok(outcome.value)
    }
}
