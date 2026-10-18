//! The built-in assistant's settings and commands (WP-2.23, part 4): the
//! model provider, its base URL and model, the API key entry, and the
//! activity log.
//!
//! The assistant is Beatlayer's own (D17): Live has no assistant. Its
//! settings live on the Agents page in the Settings window, beside the
//! agent server's, because both decide which other program drives the app.
//!
//! The API key is the exception: it is never written to the preferences or
//! a set. `assistant.set_key` writes it to the OS keychain through
//! `bl-assistant`, and no command's result carries it. The rest of the
//! settings are kept in the preferences and survive the next start.
//!
//! These commands change how the assistant works, so only a person runs
//! them: the agent API refuses every command in this module, whatever the
//! agent's permissions, as it does the agent server's own.
//!
//! The assistant's panel (part 3) is Beatlayer's own too: Live has no
//! assistant, so its place is a decision of Beatlayer's (D24) rather than a
//! row of the UI parity reference. The panel is a full-height pane on the
//! right of the main window, or a window of its own; its commands are in
//! this module as well, so agents never open, resize or drive it.

use bl_commands::Context::{Global, Settings};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `assistant.set_provider`: choose the model provider.
pub const ASSISTANT_SET_PROVIDER: &str = "assistant.set_provider";
/// `assistant.set_base_url`: the base URL of an OpenAI-compatible server.
pub const ASSISTANT_SET_BASE_URL: &str = "assistant.set_base_url";
/// `assistant.set_model`: the model the provider is asked for.
pub const ASSISTANT_SET_MODEL: &str = "assistant.set_model";
/// `assistant.set_key`: store the provider's API key in the OS keychain.
pub const ASSISTANT_SET_KEY: &str = "assistant.set_key";
/// `assistant.forget_key`: remove the provider's API key from the keychain.
pub const ASSISTANT_FORGET_KEY: &str = "assistant.forget_key";
/// `assistant.toggle_activity_log`: open or close the activity log.
pub const ASSISTANT_TOGGLE_ACTIVITY_LOG: &str = "assistant.toggle_activity_log";
/// `view.toggle_assistant`: open or close the assistant's panel.
pub const VIEW_TOGGLE_ASSISTANT: &str = "view.toggle_assistant";
/// `assistant.set_panel_width`: set the docked panel's width.
pub const ASSISTANT_SET_PANEL_WIDTH: &str = "assistant.set_panel_width";
/// `assistant.toggle_floating`: float the panel into its own window, or dock
/// it back.
pub const ASSISTANT_TOGGLE_FLOATING: &str = "assistant.toggle_floating";
/// `assistant.send`: send the person's message to the assistant.
pub const ASSISTANT_SEND: &str = "assistant.send";
/// `assistant.stop`: stop the assistant's turn that is running.
pub const ASSISTANT_STOP: &str = "assistant.stop";

/// Every command [`register`] adds.
pub const IDS: &[&str] = &[
    ASSISTANT_SET_PROVIDER,
    ASSISTANT_SET_BASE_URL,
    ASSISTANT_SET_MODEL,
    ASSISTANT_SET_KEY,
    ASSISTANT_FORGET_KEY,
    ASSISTANT_TOGGLE_ACTIVITY_LOG,
    VIEW_TOGGLE_ASSISTANT,
    ASSISTANT_SET_PANEL_WIDTH,
    ASSISTANT_TOGGLE_FLOATING,
    ASSISTANT_SEND,
    ASSISTANT_STOP,
];

/// Whether `id` is one of the assistant settings' commands, which agents
/// may never run.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Which model API the assistant talks to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AssistantProvider {
    /// An OpenAI-compatible API: OpenAI itself, OpenRouter, or a local
    /// server such as Ollama, LM Studio, llama.cpp, or vLLM, reached
    /// through [`AssistantSettings::base_url`].
    #[default]
    OpenAi,
    /// Anthropic's Messages API.
    Anthropic,
    /// Google's Gemini API.
    Gemini,
}

impl AssistantProvider {
    /// Every provider, in the chooser's order.
    pub const ALL: [Self; 3] = [Self::OpenAi, Self::Anthropic, Self::Gemini];

    /// The provider's name in the chooser.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI-compatible",
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Google",
        }
    }

    /// The provider's ID in command results.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::OpenAi => "open-ai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }

    /// Whether the provider has a base URL the person may change: only the
    /// OpenAI-compatible one, which also covers local servers and
    /// OpenRouter.
    #[must_use]
    pub const fn has_base_url(self) -> bool {
        matches!(self, Self::OpenAi)
    }
}

/// The assistant's settings, kept in the preferences. The API key is not
/// here: it lives in the OS keychain ([`ASSISTANT_SET_KEY`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AssistantSettings {
    /// The provider the assistant talks to.
    pub provider: AssistantProvider,
    /// The base URL of an OpenAI-compatible server, such as
    /// `http://localhost:11434/v1` for Ollama. Empty uses the provider's
    /// own.
    pub base_url: String,
    /// The model the provider is asked for, such as `gpt-4o-mini` or
    /// `claude-sonnet-4-5`.
    pub model: String,
}

/// The panel's narrowest docked width, in points.
pub const PANEL_MIN_WIDTH: f32 = 240.0;
/// The panel's widest docked width, in points.
pub const PANEL_MAX_WIDTH: f32 = 640.0;
/// The panel's docked width until the person drags its edge, in points.
pub const PANEL_DEFAULT_WIDTH: f32 = 320.0;

/// Where the assistant's panel is, kept in the preferences so it comes
/// back the way the person left it (D24). Whether it is open is not kept:
/// the panel starts closed, as the browser's and the Info View's state
/// starts from the set's defaults.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantPanelPrefs {
    /// The docked panel's width in points, within [`PANEL_MIN_WIDTH`] and
    /// [`PANEL_MAX_WIDTH`].
    pub width: f32,
    /// Whether the panel floats in a window of its own rather than docking
    /// at the main window's right edge.
    pub floating: bool,
}

impl Default for AssistantPanelPrefs {
    fn default() -> Self {
        Self {
            width: PANEL_DEFAULT_WIDTH,
            floating: false,
        }
    }
}

/// A docked width brought within the limits.
#[must_use]
pub fn clamp_panel_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
    } else {
        PANEL_DEFAULT_WIDTH
    }
}

/// What the panel's commands return: its state afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantPanelState {
    /// Whether the panel is open.
    pub open: bool,
    /// Whether it floats in a window of its own.
    pub floating: bool,
    /// The docked width in points.
    pub width: f32,
    /// Whether an assistant turn is running.
    pub running: bool,
}

/// Arguments of `assistant.set_panel_width`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPanelWidthArgs {
    /// The width in points. A width outside the limits is brought within
    /// them.
    pub width: f32,
}

/// Arguments of `assistant.send`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendArgs {
    /// The message.
    pub text: String,
}

/// What the assistant settings' commands return: the settings afterwards,
/// without the API key, which only its presence reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssistantState {
    /// The provider.
    pub provider: AssistantProvider,
    /// The base URL, empty when the provider's own is used.
    pub base_url: String,
    /// The model.
    pub model: String,
    /// Whether an API key is stored for the provider in the keychain. The
    /// key itself never appears in a result.
    pub key_set: bool,
    /// Whether the activity log is open.
    pub activity_log_open: bool,
}

/// Arguments of `assistant.set_provider`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetProviderArgs {
    /// The provider.
    pub provider: AssistantProvider,
}

/// Arguments of `assistant.set_base_url`: the address, or none to use the
/// provider's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetBaseUrlArgs {
    /// The base URL, or empty for the provider's own.
    pub base_url: String,
}

/// Arguments of `assistant.set_model`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetModelArgs {
    /// The model.
    pub model: String,
}

/// Arguments of `assistant.set_key`: the API key to store. It goes to the
/// keychain and nowhere else, and never returns in a result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetKeyArgs {
    /// The API key.
    pub key: String,
}

/// The sentence every assistant command's description ends with.
macro_rules! person_only {
    ($text:literal) => {
        concat!(
            $text,
            " It changes how the assistant works, so only a person can run it, in the Settings \
             window's Agents page: the agent API refuses it."
        )
    };
}

/// The sentence the panel commands' descriptions end with.
macro_rules! panel_only {
    ($text:literal) => {
        concat!(
            $text,
            " It belongs to the person using the app, so the agent API refuses it."
        )
    };
}

/// Adds the built-in assistant's settings commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    let specs = [
        CommandSpec::new(ASSISTANT_SET_PROVIDER, "Assistant Provider")
            .description(person_only!(
                "Chooses the model provider the built-in assistant talks to: an \
                 OpenAI-compatible API (OpenAI itself, OpenRouter, or a local server such as \
                 Ollama, LM Studio, llama.cpp, or vLLM), Anthropic's Messages API, or Google's \
                 Gemini API. The setting is kept for the next start. Its API key is stored \
                 separately, in the OS keychain."
            ))
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SetProviderArgs>()
            .result::<AssistantState>(),
        CommandSpec::new(ASSISTANT_SET_BASE_URL, "Assistant Base URL")
            .description(person_only!(
                "Sets the base URL the assistant's OpenAI-compatible provider is reached at, \
                 such as `http://localhost:11434/v1` for a local Ollama server, or \
                 `https://openrouter.ai/api/v1` for OpenRouter. Empty uses the provider's own \
                 address. The setting is kept for the next start."
            ))
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SetBaseUrlArgs>()
            .result::<AssistantState>(),
        CommandSpec::new(ASSISTANT_SET_MODEL, "Assistant Model")
            .description(person_only!(
                "Sets the model the assistant asks its provider for, such as `gpt-4o-mini` for \
                 an OpenAI-compatible API or `claude-sonnet-4-5` for Anthropic. The setting is \
                 kept for the next start."
            ))
            .permission(Permission::Edit)
            .context(Settings)
            .args::<SetModelArgs>()
            .result::<AssistantState>(),
        CommandSpec::new(ASSISTANT_SET_KEY, "Assistant API Key")
            .description(person_only!(
                "Stores the built-in assistant's API key for its current provider in the OS \
                 keychain (macOS Keychain, Windows Credential Manager, the Secret Service on \
                 Linux), replacing any key already there. The key is never written to the \
                 preferences or a set, never returned in a result, and never shown again: the \
                 Settings window only reports whether one is stored."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<SetKeyArgs>()
            .result::<AssistantState>(),
        CommandSpec::new(ASSISTANT_FORGET_KEY, "Forget Assistant API Key")
            .description(person_only!(
                "Removes the built-in assistant's API key for its current provider from the OS \
                 keychain, so turns are refused until a key is stored again."
            ))
            .permission(Permission::External)
            .context(Settings)
            .args::<bl_commands::NoArgs>()
            .result::<AssistantState>(),
        CommandSpec::new(ASSISTANT_TOGGLE_ACTIVITY_LOG, "Assistant Activity Log")
            .description(person_only!(
                "Opens or closes the built-in assistant's activity log, which lists every \
                 command the assistant ran with the time and the result."
            ))
            .permission(Permission::Read)
            .context(Settings)
            .args::<bl_commands::NoArgs>()
            .result::<AssistantState>(),
        CommandSpec::new(VIEW_TOGGLE_ASSISTANT, "Assistant")
            .description(panel_only!(
                "Opens or closes the built-in assistant's panel: a full-height pane at the \
                 main window's right edge, beside the main view, the detail area and the \
                 Status Bar, or a window of its own while the person has floated it. The \
                 Control Bar's last control and the View menu's Assistant item do the same. \
                 A new session starts with it closed. Changes nothing in the set and isn't an \
                 undo step."
            ))
            .permission(Permission::Read)
            .context(Global)
            .args::<bl_commands::NoArgs>()
            .result::<AssistantPanelState>(),
        CommandSpec::new(ASSISTANT_SET_PANEL_WIDTH, "Assistant Panel Width")
            .description(panel_only!(
                "Sets the width in points of the assistant's panel while it is docked, as \
                 dragging its left edge does. A width outside 240 to 640 points is brought \
                 within them. The width is kept for the next start. Changes nothing in the \
                 set and isn't an undo step."
            ))
            .permission(Permission::Read)
            .context(Global)
            .args::<SetPanelWidthArgs>()
            .result::<AssistantPanelState>(),
        CommandSpec::new(ASSISTANT_TOGGLE_FLOATING, "Float Assistant")
            .description(panel_only!(
                "Floats the assistant's panel into a window of its own, or docks it back at \
                 the main window's right edge. The choice is kept for the next start. \
                 Changes nothing in the set and isn't an undo step."
            ))
            .permission(Permission::Read)
            .context(Global)
            .args::<bl_commands::NoArgs>()
            .result::<AssistantPanelState>(),
        CommandSpec::new(ASSISTANT_SEND, "Send to Assistant")
            .description(panel_only!(
                "Sends the person's message to the built-in assistant: it joins the \
                 conversation in the panel, and the assistant answers it as one turn, whose \
                 changes to the set are one undo step. Refused while a turn is running or \
                 when the message is empty."
            ))
            .permission(Permission::Read)
            .context(Global)
            .args::<SendArgs>()
            .result::<AssistantPanelState>(),
        CommandSpec::new(ASSISTANT_STOP, "Stop Assistant")
            .description(panel_only!(
                "Stops the assistant's turn that is running, between two of its commands. \
                 What it already did stays done, and is the turn's one undo step. Does \
                 nothing while no turn is running."
            ))
            .permission(Permission::Read)
            .context(Global)
            .args::<bl_commands::NoArgs>()
            .result::<AssistantPanelState>(),
    ];
    for spec in specs {
        registry.register(spec)?;
    }
    Ok(())
}
