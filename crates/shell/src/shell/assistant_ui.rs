//! The assistant's panel, key, activity log and turns.

use super::{Area, Shell};
use crate::assistant as assistant_ids;
use crate::assistant::ActivityEntry;
use crate::assistant::AssistantSettings;
use crate::assistant::AssistantState;
use crate::assistant::Outcome;
use crate::assistant::keychain_account;
use bl_assistant::keychain::Keychain;
use serde_json::Value;
use std::sync::Arc;

use super::{ShellError, to_value};
use crate::assistant::SendArgs;
use crate::assistant::SetBaseUrlArgs;
use crate::assistant::SetKeyArgs;
use crate::assistant::SetModelArgs;
use crate::assistant::SetPanelWidthArgs;
use crate::assistant::SetProviderArgs;
use bl_commands::NoArgs;

impl Shell {
    /// The built-in assistant's provider, base URL and model (WP-2.23,
    /// part 4). Its API key is in the keychain, not here.
    #[must_use]
    pub fn assistant_settings(&self) -> &AssistantSettings {
        &self.prefs.assistant
    }

    /// The assistant's settings and whether a key is stored, for the
    /// Settings window and the settings' commands. The key itself is never
    /// here.
    #[must_use]
    pub fn assistant_state(&self) -> AssistantState {
        AssistantState {
            provider: self.prefs.assistant.provider,
            base_url: self.prefs.assistant.base_url.clone(),
            model: self.prefs.assistant.model.clone(),
            key_set: self.assistant_key_set,
            activity_log_open: self.assistant_log_open,
        }
    }

    /// Every command the assistant ran this session, in order (WP-2.23,
    /// part 4).
    #[must_use]
    pub fn assistant_log(&self) -> &[ActivityEntry] {
        &self.assistant_log
    }

    /// Whether the activity log is open.
    #[must_use]
    pub fn assistant_log_open(&self) -> bool {
        self.assistant_log_open
    }

    /// Whether an API key is stored for the assistant's current provider.
    /// Refreshed by [`Shell::refresh_assistant_key`], after a key changes,
    /// and when the Settings window opens.
    #[must_use]
    pub fn assistant_key_set(&self) -> bool {
        self.assistant_key_set
    }

    /// Runs the assistant's keychain operations through `keychain`, such as
    /// an in-memory one in tests, and refreshes whether a key is stored.
    pub fn use_keychain(&mut self, keychain: Arc<dyn Keychain>) {
        self.keychain = keychain;
        self.refresh_assistant_key();
    }

    /// Reads whether an API key is stored for the assistant's current
    /// provider, so the Settings window can show it without reading the
    /// keychain every frame.
    pub fn refresh_assistant_key(&mut self) {
        let account = keychain_account(self.prefs.assistant.provider);
        self.assistant_key_set = matches!(self.keychain.read(account), Ok(Some(_)));
    }

    /// The panel's conversation, oldest first.
    #[must_use]
    pub fn assistant_rows(&self) -> Vec<assistant_ids::Row> {
        crate::assistant::lock(&self.assistant_conversation)
            .rows
            .clone()
    }

    /// Whether an assistant turn is running.
    #[must_use]
    pub fn assistant_running(&self) -> bool {
        crate::assistant::lock(&self.assistant_conversation).running
    }

    /// The message the person sent that no turn has taken yet, which the
    /// host answers by running a turn ([`run_turn`]).
    pub fn take_assistant_prompt(&mut self) -> Option<String> {
        crate::assistant::lock(&self.assistant_conversation)
            .pending
            .take()
    }

    /// Adds the person's message to the conversation, to be taken by
    /// [`Shell::take_assistant_prompt`].
    pub(crate) fn assistant_send(&mut self, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("the message is empty".to_owned());
        }
        let mut conversation = crate::assistant::lock(&self.assistant_conversation);
        if conversation.running || conversation.pending.is_some() {
            return Err("the assistant is still answering the last message".to_owned());
        }
        conversation
            .rows
            .push(assistant_ids::Row::You(text.to_owned()));
        conversation.pending = Some(text.to_owned());
        // The assistant is at work from the message on, so the panel shows
        // Stop until the turn ends.
        conversation.running = true;
        Ok(())
    }

    /// Moves the keyboard focus to the main view when the pane that had it
    /// closed or floated away, as closing the browser does.
    pub(super) fn leave_hidden_assistant(&mut self) {
        if self.focused_area == Area::Assistant && self.assistant_docked_width().is_none() {
            self.focused_area = Area::MainView;
        }
    }

    /// Asks the running turn to stop between two calls; a message no turn
    /// has taken yet is withdrawn instead.
    pub(crate) fn assistant_stop(&self) {
        let mut conversation = crate::assistant::lock(&self.assistant_conversation);
        if conversation.pending.take().is_some() {
            conversation.running = false;
            conversation
                .rows
                .push(assistant_ids::Row::Notice("Stopped.".to_owned()));
        } else if conversation.running {
            self.assistant_stop_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The docked panel's width in points while it is open and docked: the
    /// main window lays itself out beside it (D24).
    #[must_use]
    pub fn assistant_docked_width(&self) -> Option<f32> {
        (self.assistant_open && !self.prefs.assistant_panel.floating)
            .then(|| assistant_ids::clamp_panel_width(self.prefs.assistant_panel.width))
    }

    /// Whether the panel is open in a window of its own.
    #[must_use]
    pub fn assistant_window_open(&self) -> bool {
        self.assistant_open && self.prefs.assistant_panel.floating
    }

    /// The panel's state, for the commands' results.
    #[must_use]
    pub fn assistant_panel_state(&self) -> assistant_ids::AssistantPanelState {
        assistant_ids::AssistantPanelState {
            open: self.assistant_open,
            floating: self.prefs.assistant_panel.floating,
            width: assistant_ids::clamp_panel_width(self.prefs.assistant_panel.width),
            running: self.assistant_running(),
        }
    }

    /// The flag Stop sets, for a host that runs a turn while it also takes
    /// the person's clicks: setting it ends the turn between two calls, as
    /// `assistant.stop` does.
    #[must_use]
    pub fn assistant_stop_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.assistant_stop_flag)
    }

    /// The conversation and the Stop flag, for [`crate::assistant::run_turn`]
    /// to hold while the shell runs the turn's calls.
    pub(crate) fn assistant_handles(
        &self,
    ) -> (
        crate::assistant::SharedConversation,
        crate::assistant::StopFlag,
    ) {
        (
            Arc::clone(&self.assistant_conversation),
            Arc::clone(&self.assistant_stop_flag),
        )
    }

    /// Tells the window the panel changed.
    pub(crate) fn assistant_touch(&mut self) {
        self.touch();
    }

    /// Gives the call that waits for `command` its result in the panel.
    pub(crate) fn assistant_finish_call(&self, command: &str, outcome: Result<Value, String>) {
        crate::assistant::lock(&self.assistant_conversation).finish_call(command, outcome);
    }

    /// Opens a turn's changes as one undo step named `label`, which the
    /// assistant loop closes with [`Shell::end_assistant_turn`]
    /// (WP-2.23, part 2).
    pub(crate) fn begin_assistant_turn(&mut self, label: &str) {
        self.editor
            .begin_by(bl_model::Maker::Assistant, label.to_owned());
    }

    /// Closes the turn's undo step, keeping whatever ran.
    pub(crate) fn end_assistant_turn(&mut self) {
        if self.editor.commit().unwrap_or(false) {
            self.dirty = true;
        }
        self.document_changed();
        self.touch();
    }

    /// Whether the person has chosen "always allow" for `command`.
    pub(crate) fn assistant_always_allows(&self, command: &str) -> bool {
        self.prefs.assistant_always.contains(command)
    }

    /// Remembers "always allow" for `command`, so later calls don't ask.
    pub(crate) fn assistant_remember_command(&mut self, command: &str) {
        if self.prefs.assistant_always.insert(command.to_owned()) {
            self.save_preferences();
        }
    }

    /// Records one command the assistant ran in the activity log.
    pub(crate) fn assistant_record(&mut self, entry: ActivityEntry) {
        self.assistant_log.push(entry);
        self.touch();
    }

    /// Records one command the assistant was refused, in the activity log.
    pub(crate) fn assistant_record_denied(&mut self, command: &str, message: String) {
        self.assistant_log.push(ActivityEntry {
            command: command.to_owned(),
            at: std::time::SystemTime::now(),
            outcome: Outcome::Denied(message),
        });
        self.touch();
    }

    /// Runs the `assistant` commands.
    pub(super) fn run_assistant(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        match id {
            assistant_ids::ASSISTANT_SET_PROVIDER => {
                let SetProviderArgs { provider } = Self::args(id, args)?;
                self.prefs.assistant.provider = provider;
                self.save_preferences();
                self.refresh_assistant_key();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_BASE_URL => {
                let SetBaseUrlArgs { base_url } = Self::args(id, args)?;
                self.prefs.assistant.base_url = base_url.trim().to_owned();
                self.save_preferences();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_MODEL => {
                let SetModelArgs { model } = Self::args(id, args)?;
                self.prefs.assistant.model = model.trim().to_owned();
                self.save_preferences();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_KEY => {
                // Spec: agent-permissions R7, by analogy. The key is written
                // to the keychain and never returned or logged.
                let SetKeyArgs { key } = Self::args(id, args)?;
                if key.trim().is_empty() {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "the API key is empty".to_owned(),
                    });
                }
                let account = keychain_account(self.prefs.assistant.provider);
                self.keychain
                    .write(account, key.trim())
                    .map_err(|error| failed(error.to_string()))?;
                self.refresh_assistant_key();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_FORGET_KEY => {
                Self::args::<NoArgs>(id, args)?;
                let account = keychain_account(self.prefs.assistant.provider);
                self.keychain
                    .delete(account)
                    .map_err(|error| failed(error.to_string()))?;
                self.refresh_assistant_key();
                Ok(to_value(&self.assistant_state()))
            }
            assistant_ids::ASSISTANT_SET_PANEL_WIDTH => {
                let SetPanelWidthArgs { width } = Self::args(id, args)?;
                self.prefs.assistant_panel.width = assistant_ids::clamp_panel_width(width);
                self.save_preferences();
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_TOGGLE_FLOATING => {
                Self::args::<NoArgs>(id, args)?;
                self.prefs.assistant_panel.floating = !self.prefs.assistant_panel.floating;
                self.save_preferences();
                self.leave_hidden_assistant();
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_SEND => {
                let SendArgs { text } = Self::args(id, args)?;
                self.assistant_send(&text).map_err(failed)?;
                self.touch();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_STOP => {
                Self::args::<NoArgs>(id, args)?;
                self.assistant_stop();
                Ok(to_value(&self.assistant_panel_state()))
            }
            assistant_ids::ASSISTANT_TOGGLE_ACTIVITY_LOG => {
                Self::args::<NoArgs>(id, args)?;
                self.assistant_log_open = !self.assistant_log_open;
                self.touch();
                Ok(to_value(&self.assistant_state()))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
