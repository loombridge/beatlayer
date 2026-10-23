//! Views, widgets, GPU canvases, theming, and accessibility.
//!
//! The app shell's UI (WP-1.7), drawn with egui (ADR-001):
//!
//! - [`layout`]: the layout layer, which turns the UI parity reference's
//!   rows into rectangles, so placement isn't scattered through the code.
//! - [`theme`]: Beatlayer's colours, sizes, and bundled typeface, as tokens
//!   from theme files, dark and light.
//! - [`keymap`] and [`input`]: the default keymap (`keymaps/live12.toml`,
//!   generated from the parity reference) and the dispatcher that turns key
//!   presses into commands for the contexts that apply.
//! - [`main_window`], [`settings_window`], [`menus`], [`widgets`]: the
//!   windows and controls. Every control has a role, a name, and a value
//!   for assistive technology through AccessKit.
//! - [`control_bar`]: the Control Bar's transport section (WP-1.18): Play,
//!   Stop, the Loop switch, and the tempo, time-signature, position, and
//!   loop fields, each running a registered command.
//!
//! - [`focus`]: keyboard focus in the main window (WP-1.14): the order Tab
//!   and the neighbour keys move through, and whether the focus is
//!   outlined.
//!
//! - [`mixer`]: the Mixer view (WP-1.9): the Session View's columns and
//!   mixer, the Arrangement's mixer, and the strip controls the
//!   Arrangement's track headers share, each running a registered command.
//!
//! - [`device_view`]: the Device View (WP-2.4): the selected track's
//!   device chain in frames with Live's title bar, folding, the device
//!   context menu, dragging devices, and hot-swapping's bar in the browser.
//! - [`rack_view`]: a rack in the Device View (WP-5.2): its view column, chain
//!   list, and the selected chain's devices.
//! - [`device_panel`]: the generic device UI (WP-2.5): any described
//!   device's controls and display cells, drawn from its descriptors and
//!   its layout.
//! - [`clip_view`]: the Clip View (WP-2.3): the MIDI Note Editor, with its
//!   key track, grid, and note editing.
//!
//! Nothing here owns a window or an event loop: the app (`beatlayer`)
//! drives these from its winit loop, and tests drive them headless with an
//! `egui::Context` and synthetic input.

pub mod arrangement;
pub mod articulation_map;
pub mod assistant_panel;
pub mod audio_panel;
pub mod autosampler;
pub mod browser;
pub mod chain_strips;
pub mod clip_envelope;
pub mod clip_view;
pub mod control_bar;
pub mod detail_area;
pub mod device_display;
pub mod device_panel;
pub mod device_view;
pub mod drum_view;
pub mod export_dialog;
pub mod external;
pub mod file_manager;
pub mod focus;
pub mod follow_controls;
pub mod in_out;
pub mod input;
pub mod keymap;
pub mod launch_panel;
pub mod layout;
pub mod main_window;
pub mod mapping;
pub mod menus;
pub mod middle;
pub mod mixer;
pub mod multisampler;
pub mod note_lanes;
pub mod plugin_list;
pub mod rack_view;
pub mod realtime_dialog;
pub mod recovery_dialog;
pub mod sample_display;
pub mod sample_editor;
pub mod scene_fields;
pub mod scene_view;
pub mod search_field;
pub mod session_view;
pub mod settings_window;
pub mod sidechain;
pub mod slice_dialog;
pub mod stems_dialog;
pub mod theme;
pub mod theme_library;
pub mod theme_look;
pub mod tool_panel;
pub mod track_menu;
pub mod undo_history_panel;
pub mod widgets;
