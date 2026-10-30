//! The main window: the Control Bar, the browser, the main view, the detail
//! area, and the Status Bar, placed by the layout layer (L-WIN, L-CTRL,
//! L-STAT). The browser fills its column (WP-1.10), the main view shows the
//! Arrangement View (WP-1.8a) with its mixer, or the Session View's columns
//! and mixer (WP-1.9); views that later packages build are placeholders
//! that say so.

use std::time::Duration;

use bl_model::{CountIn, MetronomeRhythm, MetronomeSound};
use bl_shell::clip_drag::{ClipDragEvent, DragSource, DropSpot};
use bl_shell::midi_io_commands as midi_ids;
use bl_shell::remote_commands::{self as remote_ids, MapMode};
use bl_shell::settings::AutomationRecordMode;
use bl_shell::{
    Area, Effect, GesturePhase, Invocation, MainView, Shell, WindowKind, commands as ids,
    commands::DetailView, transport_commands,
};
use egui::accesskit::Role;
use egui::{
    Align2, CentralPanel, Context, Frame, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2,
};
use serde_json::json;

use crate::arrangement::ArrangementUi;
use crate::browser::BrowserUi;
use crate::control_bar::{self, TransportFields};
use crate::layout::{
    DRAWN_MENU_MARGIN, ELEMENTS, Element, Kind, ShellLayout, Strip, title_bar_height,
};
use crate::mixer::MixerUi;
use crate::theme::Theme;
use crate::widgets::{Look, element};

/// What the person did in a frame, for the host to carry out.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// Run a command, as [`Shell::invoke`] does.
    Invoke(Invocation),
    /// The person clicked in an area, which takes the keyboard focus
    /// (I-FOCUS-001).
    FocusArea(Area),
    /// The Arrangement's track lanes are laid out at a new size, in points,
    /// for the host to pass to [`Shell::set_arrangement_viewport`].
    ArrangementViewport {
        /// From the lanes' left edge to the track headers.
        width: f32,
        /// From under the rulers to the pinned return tracks.
        height: f32,
    },
    /// A step of a gesture that edits the set, such as dragging clips
    /// (WP-1.8b), for the host to pass to [`Shell::gesture`], which runs
    /// the whole gesture as one undo step.
    Gesture {
        /// Whether the gesture goes on, ends, or is abandoned.
        phase: GesturePhase,
        /// The editing command for this step, measured from where the
        /// gesture began.
        invocation: Option<Invocation>,
    },
    /// A drag on a value began: the document commands it runs until
    /// [`UiEvent::EndGesture`] are one undo step with this name, as
    /// [`Shell::begin_gesture`] makes them.
    BeginGesture(&'static str),
    /// The same, for a step whose name is made as it begins, such as a
    /// device parameter's `Change "Gain"` (WP-2.5).
    BeginNamedGesture(String),
    /// The drag ended ([`Shell::end_gesture`]).
    EndGesture,
    /// The keyboard focus is on this track's control, or on no track's,
    /// for [`Shell::set_focused_track`] (WP-1.14).
    FocusTrack(Option<bl_model::TrackId>),
    /// A device's value control gained (true) or lost (false) the keyboard
    /// focus, for [`Shell::set_value_focused`] (WP-2.5).
    FocusValue(bool),
    /// A control other than the Arrangement View, an automation lane or the
    /// Envelope Editor holds the keyboard focus (true), or none does
    /// (false), for [`Shell::set_envelope_control_focused`]: Enter, Esc and
    /// Delete are then the control's, not the envelope keys' (WP-6.26).
    FocusControl(bool),
    /// The pointer is over something with a line for the Status Bar, such
    /// as a drum rack's pad (`Some`), or left it (`None`), for
    /// [`Shell::set_hint`] (WP-5.6).
    StatusHint(Option<String>),
    /// The person chose row `n` of the search field's list (WP-6.42): the
    /// host passes it to [`Shell::search_run_row`].
    SearchRun(usize),
    /// A step of a drag of clips between the Session and the Arrangement
    /// (WP-6.29b), for the host to pass to [`Shell::clip_drag_event`] with
    /// the window it came from.
    ClipDrag(ClipDragEvent),
}

/// What the Info View is showing: the control under the pointer, the
/// registered description of the command it runs, and its key (WP-2.24).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InfoViewContent {
    /// The control's name, in Live's words, or empty while the pointer is
    /// over empty space.
    pub title: String,
    /// The command's registered description (D17), or the "not built yet"
    /// note for a control a later package brings.
    pub body: String,
    /// The key the command runs on, where the default keymap has one
    /// (L-INFO-002).
    pub shortcut: Option<String>,
}

/// The main window's UI state between frames.
#[derive(Debug)]
pub struct MainWindowUi {
    /// The window this draws: the main window, or the second window
    /// (WP-6.29), which has a Control Bar, a main view, a detail area and a
    /// Status Bar of its own.
    window: WindowKind,
    /// A focus move asked for by a command, done in the next frame.
    focus_request: Option<FocusRequest>,
    /// The Control Bar control focused last, which focus returns to
    /// (I-FOCUS-003).
    last_control: Element,
    /// The element under the pointer, for the Info View.
    hovered: Option<Element>,
    /// The element under the pointer in the other window, which the main
    /// window's Info View describes too (info-view R8).
    other_hovered: Option<Element>,
    /// A move to the next or previous control, or to a neighbour, done at
    /// the start of the next frame from the order the last frame drew
    /// (WP-1.14; the accessibility spec, R3 and R4).
    focus_move: Option<FocusMove>,
    /// The controls the last frame drew that the keyboard reaches.
    order: crate::focus::Order,
    /// A focus move waiting for its control to be drawn, with the frames
    /// left to wait.
    pending: Option<(Pending, u8)>,
    /// The Arrangement View's caches and gestures (WP-1.8a).
    arrangement: ArrangementUi,
    /// The browser's scrolling and search field (WP-1.10).
    browser: BrowserUi,
    /// The Groove Pool under the browser's list (WP-6.20).
    groove_pool: crate::groove_pool::GroovePoolUi,
    /// The Control Bar's transport fields: a value being typed, a drag
    /// (WP-1.18).
    transport: TransportFields,
    /// The mixers (WP-1.9).
    mixer: MixerUi,
    /// The Device View (WP-2.4).
    devices: crate::device_view::DeviceViewUi,
    /// The Clip View's note editor (WP-2.3): its scroll and drags.
    clip_view: crate::clip_view::ClipViewUi,
    /// The Clip View Selector's outline drag (piano-roll R36).
    clip_selector: crate::clip_selector::ClipSelectorUi,
    /// The list of the set's plug-ins (IMP-29, WP-6.41).
    plugin_list: crate::plugin_list::PluginListUi,
    /// The keymap in use, read the first time something shows a key from it
    /// (the Info View, the Mixer Config Menu's items), and read again when
    /// the person's own shortcuts change (R10).
    keymap: Option<crate::keymap::Keymap>,
    /// The shell's keymap revision the keymap was read at.
    keymap_revision: u64,
    /// What the Info View last showed, for headless reads (WP-2.24).
    info: InfoViewContent,
    /// The Info View's text for the tuning selected in the browser, read
    /// once for its path.
    tuning_info: Option<(String, InfoViewContent)>,
    /// The field Edit Info Text opened in the Info View, while it's open
    /// (the Info View spec, R10).
    info_editing: Option<crate::info_text::InfoEditing>,
    /// Whether a value was being typed into a track header's volume field
    /// when the last frame ended.
    mixer_typing: bool,
    /// Whether a menu (the drawn menu bar's, a context menu, or the Mixer
    /// Config Menu) was open when the last frame ended: its keys go to it.
    menu_open: bool,
    /// Where the last frame put the panes, in points, for view captures
    /// (WP-1.16).
    drawn: Option<ShellLayout>,
    /// The titles the last frame's Device View showed, for Rename.
    device_titles: Vec<(bl_model::DeviceId, String)>,
    /// The names of the chains the last frame's Device View listed, for
    /// Rename.
    chain_names: Vec<(bl_model::ChainId, String)>,
    /// The assistant's pane (WP-2.23, part 3): the prompt being typed and a
    /// drag on its edge.
    assistant: crate::assistant_panel::AssistantPanelUi,
    /// The Undo History panel (WP-6.28): the row the keyboard has moved to.
    undo_history: crate::undo_history_panel::UndoHistoryPanelUi,
    /// The Save History panel (WP-6.39), which shares the Undo History's
    /// place.
    save_history: crate::save_history_panel::SaveHistoryPanelUi,
    /// The detail area's top edge (detail-views R10).
    detail_edge: crate::detail_area::DetailEdge,
    browser_edge: crate::browser_edge::BrowserEdge,
    /// The File Manager's switches (WP-6.63).
    file_manager: crate::file_manager::FileManagerUi,
    /// The search field (WP-6.42).
    search_field: crate::search_field::SearchFieldUi,
    /// The view this window showed when a clip drag over its view selector
    /// switched it, so one visit to the selector switches once.
    drag_switched: Option<MainView>,
    /// The place under this window's pointer that the last frame told the
    /// shell of, while clips are dragged between the views.
    drag_over: Option<DropSpot>,
    /// Whether a drag of clips between the views was under way last frame,
    /// so the browser's drop outline ends with it, wherever it ended.
    drag_seen: bool,
}

/// A move of the keyboard focus asked for by `focus.next`,
/// `focus.previous`, or a neighbour command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusMove {
    backwards: bool,
    neighbor: bool,
}

/// A focus move to a control the last frame didn't draw.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    /// A neighbour on a track brought into view.
    Control(crate::focus::TrackControl),
    /// The first (or last) of a track's controls, Tab from the view.
    Enter {
        place: crate::mixer::Place,
        track: bl_model::TrackId,
        backwards: bool,
    },
}

/// How many frames a focus move waits for its control to be drawn.
const PENDING_FRAMES: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
enum FocusRequest {
    Element(Element),
    MainView,
}

impl Default for MainWindowUi {
    fn default() -> Self {
        Self::new()
    }
}

/// The ID of the main view's focusable area.
#[must_use]
pub fn main_view_id() -> Id {
    Id::new("main-view")
}

/// The ID of an element's widget.
#[must_use]
pub fn element_id(element: Element) -> Id {
    Id::new(("element", element))
}

impl MainWindowUi {
    /// A fresh main window.
    #[must_use]
    pub fn new() -> Self {
        Self::of(WindowKind::Main)
    }

    /// A fresh second window.
    #[must_use]
    pub fn second() -> Self {
        Self::of(WindowKind::Second)
    }

    fn of(window: WindowKind) -> Self {
        Self {
            window,
            focus_request: None,
            last_control: Element::BrowserToggle,
            hovered: None,
            other_hovered: None,
            focus_move: None,
            order: crate::focus::Order::default(),
            pending: None,
            arrangement: ArrangementUi::new(),
            browser: BrowserUi::new(),
            groove_pool: crate::groove_pool::GroovePoolUi::new(),
            transport: TransportFields::new(),
            mixer: MixerUi::new(),
            devices: crate::device_view::DeviceViewUi::new(),
            clip_view: crate::clip_view::ClipViewUi::default(),
            clip_selector: crate::clip_selector::ClipSelectorUi::default(),
            plugin_list: crate::plugin_list::PluginListUi::default(),
            keymap: None,
            file_manager: crate::file_manager::FileManagerUi::default(),
            search_field: crate::search_field::SearchFieldUi::default(),
            keymap_revision: 0,
            info: InfoViewContent::default(),
            tuning_info: None,
            info_editing: None,
            mixer_typing: false,
            menu_open: false,
            drawn: None,
            device_titles: Vec::new(),
            chain_names: Vec::new(),
            assistant: crate::assistant_panel::AssistantPanelUi::default(),
            undo_history: crate::undo_history_panel::UndoHistoryPanelUi::default(),
            save_history: crate::save_history_panel::SaveHistoryPanelUi::default(),
            detail_edge: crate::detail_area::DetailEdge::default(),
            browser_edge: crate::browser_edge::BrowserEdge::default(),
            drag_switched: None,
            drag_over: None,
            drag_seen: false,
        }
    }

    /// The window this draws.
    #[must_use]
    pub fn window(&self) -> WindowKind {
        self.window
    }

    /// The element under the pointer in this window's last frame.
    #[must_use]
    pub fn hovered(&self) -> Option<Element> {
        self.hovered
    }

    /// Tells the main window which element is under the pointer in the
    /// other window, for its Info View (info-view R8). Whether that changed
    /// what the Info View shows, so the host can draw it again.
    pub fn set_other_hovered(&mut self, hovered: Option<Element>) -> bool {
        std::mem::replace(&mut self.other_hovered, hovered) != hovered
    }

    /// Where the last frame put the panes, in points from the window's
    /// top-left corner: the agent API's view captures crop to them
    /// (WP-1.16). `None` before the first frame.
    #[must_use]
    pub fn drawn_layout(&self) -> Option<&ShellLayout> {
        self.drawn.as_ref()
    }

    /// The title a device's title bar shows, for renaming it; empty for a
    /// device the set no longer has.
    fn device_title(&self, device: bl_model::DeviceId) -> String {
        self.device_titles
            .iter()
            .find(|(id, _)| *id == device)
            .map(|(_, title)| title.clone())
            .unwrap_or_default()
    }

    /// The name a chain's row shows, for renaming it; empty for a chain the
    /// set no longer has.
    fn chain_name(&self, chain: bl_model::ChainId) -> String {
        self.chain_names
            .iter()
            .find(|(id, _)| *id == chain)
            .map(|(_, name)| name.clone())
            .unwrap_or_default()
    }

    /// The mixers, to read what they drew.
    #[must_use]
    pub fn mixer(&self) -> &MixerUi {
        &self.mixer
    }

    /// What the Info View last showed: the hovered control's name, the
    /// registered description of the command it runs, and its key
    /// (WP-2.24). Empty while the pointer is over empty space, and cleared
    /// while the Info View hides.
    #[must_use]
    pub fn info_view_content(&self) -> &InfoViewContent {
        &self.info
    }

    /// The Clip View's note editor, to read what it drew.
    #[must_use]
    pub fn clip_view(&self) -> &crate::clip_view::ClipViewUi {
        &self.clip_view
    }

    /// The browser, to read what it drew.
    #[must_use]
    pub fn browser(&self) -> &BrowserUi {
        &self.browser
    }

    /// The Device View, to read what it drew.
    #[must_use]
    pub fn devices(&self) -> &crate::device_view::DeviceViewUi {
        &self.devices
    }

    /// Whether a value is being typed into a Control Bar field: while it
    /// is, key presses go to the field rather than to shortcuts, as they do
    /// to a text field (the control bar transport spec, R10). The same while
    /// a menu is open, so its arrow keys, Enter, and Esc reach it (the
    /// accessibility spec, R5).
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.transport.is_typing()
            || self.mixer_typing
            || self.menu_open
            || self.devices.is_typing()
            || self.groove_pool.is_typing()
            || self.clip_view.is_typing()
            || self.arrangement.is_dragging_header()
            || self.search_field.is_open()
            || self.info_editing.is_some()
    }

    /// The Arrangement View, to give it a peak source or read what it drew.
    pub fn arrangement_mut(&mut self) -> &mut ArrangementUi {
        &mut self.arrangement
    }

    /// The Arrangement View.
    #[must_use]
    pub fn arrangement(&self) -> &ArrangementUi {
        &self.arrangement
    }

    /// Carries out an effect that moves the keyboard focus in this window.
    pub fn apply_effect(&mut self, ctx: &Context, effect: &Effect) {
        match effect {
            Effect::FocusArea(Area::ControlBar) | Effect::FocusSecondArea(Area::ControlBar) => {
                self.focus_request = Some(FocusRequest::Element(self.last_control));
            }
            Effect::FocusArea(Area::MainView) | Effect::FocusSecondArea(Area::MainView) => {
                self.focus_request = Some(FocusRequest::MainView);
            }
            Effect::FocusArea(Area::Assistant) => self.assistant.focus_prompt(),
            Effect::FocusClipPanels => crate::main_panel::request_focus(ctx),
            Effect::FocusSelectedClipPanel => crate::clip_panels::request_selected_focus(ctx),
            Effect::FocusTuning => crate::tuning_section::request_focus(ctx),
            Effect::RenameDevice(device) => {
                // Spec: device-chains R10. Rename types the name in the
                // title bar.
                let title = self.device_title(*device);
                self.devices.start_rename(*device, &title);
            }
            Effect::RenameChain(chain) => {
                // Spec: racks R22. Rename types the name in the chain's row.
                let name = self.chain_name(*chain);
                self.devices.start_chain_rename(*chain, &name);
            }
            Effect::RenameMacro(rack, number) => {
                // Spec: macros R14. Rename types the name in the macro's
                // slot.
                self.devices.start_macro_rename(*rack, *number);
            }
            Effect::RenameVariation(rack, index) => {
                // Spec: macros R29. Rename types the name in the variation's
                // row.
                self.devices.start_variation_rename(*rack, *index);
            }
            Effect::RenameTrack(track) => crate::track_menu::start_rename(ctx, *track),
            Effect::RenameClip(clip) => {
                crate::session_menus::start_rename(ctx, crate::session_menus::Rename::Clip(*clip))
            }
            Effect::RenameScene(scene) => {
                crate::session_menus::start_rename(ctx, crate::session_menus::Rename::Scene(*scene))
            }
            Effect::RenameTakeLane(lane) => {
                // Spec: comping R28. Rename types the name in the take
                // lane's header.
                crate::arrangement::take_lanes::start_rename(ctx, *lane);
            }
            Effect::RenameLocator(locator) => {
                // Spec: time-editing R8. Rename types the name in the
                // scrub area.
                crate::arrangement::start_rename(ctx, *locator);
            }
            Effect::EditTimeSignature(at) => {
                // Spec: time-editing R31. The signature is typed over its
                // marker in the lane.
                crate::arrangement::signatures::start_edit(ctx, *at);
            }
            Effect::EditInfoText(target) => {
                // Spec: info-view R10. The Info View becomes a field with
                // the focus.
                self.info_editing = Some(crate::info_text::InfoEditing::new(*target));
            }
            Effect::MoveFocus {
                backwards,
                neighbor,
            } => {
                self.focus_move = Some(FocusMove {
                    backwards: *backwards,
                    neighbor: *neighbor,
                });
            }
            _ => {}
        }
        ctx.request_repaint();
    }

    /// Draws the main window and returns what the person did. With
    /// `title_bar`, it draws Live's 23-point title bar along the top, for a
    /// window whose content covers the system's title bar (macOS, D22);
    /// with `menu_bar`, the menu bar under it, for platforms without a
    /// native one.
    pub fn show(
        &mut self,
        root: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        title_bar: bool,
        menu_bar: Option<DrawnMenuBar<'_>>,
    ) -> Vec<UiEvent> {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        crate::window::set(ctx, self.window);
        let second = self.window == WindowKind::Second;
        let mut events = Vec::new();
        // Spec: automation-editing R23. A value being typed goes with the
        // envelope that has the keyboard, so it can't hold the keys after
        // the keyboard leaves.
        if shell.keyboard_envelope().is_none() {
            crate::envelope_keys::set_typed(ctx, None);
        }
        // Spec: accessibility R3, R4, R7. The keyboard's moves go by the
        // order the last frame drew.
        crate::menus::menu_keys(ctx);
        // Spec: modulation R15. A lit Map button tints the parameters it can
        // map, and Esc puts it out.
        crate::mapping::set_armed(ctx, shell.armed_map());
        // Spec: macros R3. A rack's Map mode tints the parameters it can
        // map.
        crate::mapping::set_macro_map(ctx, macro_map(shell));
        // Spec: map-modes R3. A map mode tints the controls it can map.
        crate::mapping::set_remote_map(ctx, remote_map_state(shell));
        // Spec: macros R11. A parameter a macro holds is drawn disabled, and
        // the Status Bar names the macro under the pointer.
        crate::mapping::set_held(ctx, held_params(shell));
        crate::mapping::set_hover_hold(ctx, None);
        crate::mapping::rotate_hover_lock(ctx);
        crate::mapping::escape(ctx, self.is_typing(), &mut events);
        // Spec: grooves R32. Esc ends a hot-swap of a groove, as a device's
        // ends.
        if shell.groove_pool_view().hot_swap.is_some()
            && !self.is_typing()
            && !ctx.text_edit_focused()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::groove_commands::HOT_SWAP,
                json!({ "on": false }),
            )));
        }
        self.order = crate::focus::take(ctx);
        crate::focus::set_outlined(ctx, shell.outline_view_in_focus());
        if let Some(step) = self.focus_move.take() {
            self.move_focus(ctx, shell, step, &mut events);
        }
        self.resolve_pending(ctx, &mut events);
        if title_bar {
            draw_title_bar(root, theme, &window_title(shell, self.window));
        }
        if let Some(bar) = menu_bar {
            egui::Panel::top("menu-bar")
                .frame(
                    Frame::NONE
                        .fill(theme.colors.bar.0)
                        .inner_margin(DRAWN_MENU_MARGIN),
                )
                .show(root, |ui| {
                    crate::menus::menu_bar(
                        ui,
                        bar.menus,
                        shell,
                        bar.keymap,
                        bar.platform,
                        &mut events,
                    );
                });
        }
        if !second {
            // Spec: dawproject R85. A dropped DAWproject file is imported;
            // no view places it.
            crate::dawproject_window::drop_file(ctx, &mut events);
        }
        self.hovered = None;
        let frame = Frame::NONE.fill(theme.colors.window.0);
        CentralPanel::default().frame(frame).show(root, |ui| {
            let area = ui.max_rect();
            // The pane follows the pointer while its edge is dragged.
            let assistant = shell
                .assistant_docked_width()
                .map(|width| self.assistant.drag_width().unwrap_or(width));
            let undo_history = (shell.undo_history_open() || shell.save_history_open())
                .then_some(crate::layout::UNDO_HISTORY_WIDTH);
            let detail = crate::layout::DetailViews::of(shell.detail_of(self.window))
                .with_browser_width(shell.browser_view().width())
                .with_global_amount(shell.global_amount_shown())
                .with_sync(shell.sync_lamps_shown(), shell.resync_button_shown());
            let layout = if second {
                // Spec: second-window R2, R12, R17. No Info View, assistant
                // or Undo History; the Control Bar is cut off.
                ShellLayout::compute_second(area.size(), shell.browser_of(self.window), detail)
            } else {
                ShellLayout::compute_with_detail(
                    area.size(),
                    shell.browser_of(self.window),
                    shell.info_view_of(self.window),
                    assistant,
                    undo_history,
                    detail.with_full_height_browser(shell.browser_view().full_height()),
                )
            }
            .translate(area.min.to_vec2());
            self.track_area_clicks(ctx, &layout, &mut events);
            // Where the Tag Editor opens, beside the content pane.
            let mut tag_editor = None;
            let mut pool_drawn = false;
            self.control_bar(ui, shell, theme, &layout, &mut events);
            if let Some(browser) = layout.browser {
                // Spec: macros R3. A rack's Map mode puts the mappings list
                // in the browser's place.
                // Spec: map-modes R8. A map mode puts the Mapping Browser in
                // the browser's place.
                if shell.map_mode() != MapMode::Off {
                    crate::mapping_browser::show(ui, shell, theme, browser, &mut events);
                } else if shell.devices().map_mode().is_some() {
                    crate::rack_view::mappings_list(ui, shell, theme, browser, &mut events);
                } else {
                    // Spec: device-chains R19. Hot-swapping's bar tops the
                    // browser.
                    let browser =
                        crate::device_view::hot_swap_bar(ui, shell, theme, browser, &mut events);
                    // Spec: grooves R24. The Groove Pool takes the column's
                    // lower half, the browser above it shrinking to make
                    // room.
                    let (browser, pool) = if shell.groove_pool_view().shown {
                        let (browser, pool) = crate::groove_pool::split(browser);
                        (browser, Some(pool))
                    } else {
                        (browser, None)
                    };
                    self.browser.show(ui, shell, theme, browser, &mut events);
                    tag_editor = shell.browser_view().tag_editor().then_some(browser);
                    if let Some(pool) = pool {
                        self.groove_pool.show(ui, shell, theme, pool, &mut events);
                        pool_drawn = true;
                    }
                }
            }
            if !pool_drawn {
                self.groove_pool.hidden();
            }
            // Spec: browser R29. The browser's right edge, in the gap
            // between it and the main view, under the main view's own
            // controls where they reach into the gap.
            self.browser_edge
                .show(ui, theme, shell, &layout, &mut events);
            self.main_view(ui, shell, theme, layout.main_view, &mut events);
            self.clip_drag_frame(ui, shell, &layout, area, &mut events);
            // Spec: browser-tags R10. The Tag Editor opens beside the
            // content pane (L-BRW-013), over the main view, so it is drawn
            // after it.
            if let Some(browser) = tag_editor {
                let rect = crate::layout::tag_editor_rect(browser);
                self.browser.tag_editor(ui, shell, theme, rect, &mut events);
            }
            // Files dragged from the browser, or from Finder or Explorer,
            // land in the Arrangement (WP-1.10).
            // The zone editor, which sits over the foot of the Arrangement,
            // takes the files dropped on it itself.
            let device_detail = layout.device_view.map_or(layout.detail, |device| {
                Rect::from_min_max(pos2(layout.detail.left(), device.top()), layout.detail.max)
            });
            let zone_editor =
                crate::device_view::DeviceViewUi::zone_editor_rect(shell, device_detail);
            // The Clip View takes a file dropped on it: it replaces the
            // clip's sample (R18).
            let clip_view = layout.clip_view.unwrap_or(Rect::NOTHING);
            self.browser.drop_overlay(
                ui,
                shell,
                theme,
                (layout.main_view, zone_editor, clip_view),
                &mut events,
            );
            // The Status Bar draws before the Info View, so the elements
            // under the pointer are known by the time it reads them.
            self.status_bar(ui, shell, theme, &layout, &mut events);
            let scene = shell.scene_view_scene_in(self.window);
            // The File Manager and the list of the set's plug-ins show in the
            // main window only.
            if !second && shell.file_manager_open() {
                // Spec: file-management R10. The File Manager takes the
                // detail area's place, in front of the Clip or Device View,
                // and the list of the set's plug-ins gives way to it.
                crate::file_manager::show(
                    ui,
                    shell,
                    theme,
                    layout.views(),
                    &mut self.file_manager,
                    &mut events,
                );
            } else if !second && shell.plugin_list().visible() {
                // Spec: plugin-list R1. The list of the set's plug-ins takes
                // the detail area's place, where the File Manager opens
                // (L-OTH-002), in front of the Clip or Device View.
                self.plugin_list
                    .show(ui, shell, theme, layout.views(), &mut events);
            } else {
                // Spec: detail-views R13. A selected scene's Scene View
                // (session-view R21) takes the place of the top view that
                // shows, and waits while neither does.
                let top = layout.clip_view.or(layout.device_view);
                if let (Some(scene), Some(rect)) = (scene, top) {
                    crate::scene_view::show(ui, shell, theme, rect, scene, &mut events);
                }
                if let Some(rect) = layout.clip_view
                    && scene.is_none()
                {
                    // Spec: detail-views R7, R8. The Clip View takes its own
                    // place, alone or above the Device View.
                    let arrangement = &mut self.arrangement;
                    self.clip_view
                        .show(ui, shell, theme, rect, &mut events, &mut |media| {
                            arrangement.peaks_of(media)
                        });
                }
                if let Some(rect) = layout.device_view
                    && (scene.is_none() || layout.clip_view.is_some())
                {
                    self.devices.show(ui, shell, theme, rect, &mut events);
                    // Spec: multisampler R1. The zone editor sits above the
                    // Device View.
                    self.devices
                        .show_zone_editor(ui, shell, theme, device_detail, &mut events);
                }
            }
            self.detail_edge
                .show(ui, theme, shell, &layout, &mut events);
            let shown = shell.device_view_state().devices;
            self.chain_names.clear();
            for device in &shown {
                collect_chain_names(device, &mut self.chain_names);
            }
            self.device_titles.clear();
            for device in &shown {
                collect_titles(device, &mut self.device_titles);
            }
            // The views draw before the Info View reads the item the pointer
            // is over (info-view R11).
            self.info_view(ui, theme, shell, layout.info_view, &mut events);
            if let Some(pane) = layout.assistant {
                self.assistant
                    .show(ui, shell, theme, pane, true, &mut events);
            }
            match layout.undo_history {
                Some(pane) if shell.save_history_open() => {
                    self.undo_history.hide(&mut events);
                    self.save_history.show(ui, shell, theme, pane, &mut events);
                }
                Some(pane) => {
                    self.save_history.hide(&mut events);
                    self.undo_history.show(ui, shell, theme, pane, &mut events);
                }
                None if !second => {
                    self.undo_history.hide(&mut events);
                    self.save_history.hide(&mut events);
                }
                None => {}
            }
            outline_focused_area(ui, shell, theme, &layout, self.window);
            // Spec: command-palette R2. The search field sits above every
            // pane, at the main view's top.
            // The field is one for the set; the window in front draws it.
            if !shell.search().open || shell.acting_window() == self.window {
                self.search_field
                    .show(ui, shell, theme, layout.main_view, &mut events);
            }
            if let Some(track) = self.search_field.revealed(shell) {
                self.mixer.reveal(shell.front_mixer(), track);
            }
            self.drawn = Some(layout);
        });
        // The dialogs sit over the main window.
        if !second {
            // Spec: stem-separation R1. Separate Stems' dialog sits over the
            // window while it's open.
            crate::stems_dialog::show(ctx, shell, theme, &mut events);
            // Spec: dawproject R87. The report of a person's import or export
            // sits over the window while it is open.
            crate::dawproject_window::show(ctx, shell, theme, &mut events);
            // Spec: external-devices R23. The real-time render's dialog sits
            // over the window while a render that runs through external
            // hardware is under way.
            crate::realtime_dialog::show(ctx, shell, theme, &mut events);
            // Spec: export R28, R41. The Export Audio/Video dialog sits over
            // the window while it's open, and the progress window while an
            // export runs.
            crate::export_dialog::show(ctx, shell, theme, &mut events);
            // Spec: set-exchange R20, R23. A pack dropped on the window is
            // unpacked, and a pack or unpack under way shows its progress.
            crate::file_manager::drop_pack(ctx, &mut events);
            crate::file_manager::pack_progress(ctx, shell, theme, &mut events);
            // Spec: multisampler R64. The autosampler's dialog sits over the
            // window while it's open.
            crate::autosampler::show(ctx, shell, theme, &mut events);
            crate::slice_dialog::show(ctx, shell, theme, &mut events);
            // Spec: mpe R18. The MPE/Multi-channel Settings dialog sits over
            // the window while it's open.
            crate::mpe_settings_dialog::show(ctx, shell, theme, &mut events);
            // Spec: tunings R23. The Configure MIDI Layout dialog does the
            // same.
            crate::tuning_layout_dialog::show(ctx, shell, theme, &mut events);
            // Spec: crash-recovery R22. The recovery prompt sits over the
            // window while there are sets a crash left to offer.
            crate::recovery_dialog::show(ctx, shell, theme, &mut events);
        }
        self.mixer_typing = crate::mixer::is_typing(ctx)
            || crate::track_menu::is_renaming(ctx)
            || crate::session_menus::is_renaming(ctx)
            || crate::session_menus::field_has_focus(ctx)
            || crate::arrangement::take_lanes::is_renaming(ctx)
            || crate::envelope_keys::is_typing(ctx);
        // An open menu, or a dialog, keeps the keys.
        self.menu_open = egui::Popup::is_any_open(ctx)
            || (!second
                && (shell.stems_dialog().is_some()
                    || shell.dawproject_report_window().is_some()
                    || shell.export_dialog().is_some()
                    || shell.export_running()
                    || shell.pack_status().job.is_some()
                    || shell.autosampler_dialog().is_some()
                    || shell.slice_dialog().is_some()
                    || shell.mpe_settings_dialog().open
                    || crate::tuning_layout_dialog::is_open(shell)));
        // The track whose control has the focus, for the neighbour items'
        // state (the accessibility spec, R4).
        let order = crate::focus::drawn(ctx);
        let track = ctx
            .memory(egui::Memory::focused)
            .and_then(|id| order.find(id))
            .and_then(|entry| entry.control)
            .map(|control| control.track);
        // Only the window in front knows which control has the focus.
        if track != shell.focused_track() && shell.acting_window() == self.window {
            events.push(UiEvent::FocusTrack(track));
        }
        // Spec: automation-editing R20. The envelope keys are the view's,
        // the lane's and the Envelope Editor's own: a button or chooser with
        // the focus keeps Enter, Esc and Delete.
        let doc = shell.editor().document();
        let editor = shell
            .clip_view()
            .shown_any_clip(doc, shell.arrangement())
            .map(crate::clip_envelope::lane_id);
        let control = ctx.memory(egui::Memory::focused).is_some_and(|id| {
            id != main_view_id() && id != crate::arrangement::lanes_id() && Some(id) != editor
        });
        if control != shell.envelope_control_focused() && shell.acting_window() == self.window {
            events.push(UiEvent::FocusControl(control));
        }
        let metering = shell.meters().iter().any(|m| {
            m.peak.iter().chain(&m.rms).any(|level| *level > 0.0) || m.gain_reduction < 0.0
        });
        if shell.playing() || metering || shell.chain_meters_lit() {
            // The playhead and the Arrangement Position fields move with
            // the engine every frame (control-bar-transport R14), and the
            // meters and the gain-reduction bars until they're silent
            // (mixer-view R10, R40).
            ctx.request_repaint();
        } else if shell.audio().engine_on() {
            // The CPU meter updates a few times a second.
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if second {
            // The commands this window's clicks run act on this window,
            // whichever has the focus when the host runs them.
            for event in &mut events {
                if let UiEvent::Invoke(invocation) = event
                    && ids::acts_on_a_window(&invocation.id)
                    && invocation.args.get("window").is_none()
                {
                    if !invocation.args.is_object() {
                        invocation.args = json!({});
                    }
                    invocation.args["window"] = json!("second");
                }
            }
        }
        crate::mixer::end_orphaned_drag(ctx, &mut events);
        self.devices.end_undrawn_rename();
        events
    }

    /// Clips dragged between the Session and the Arrangement, from this
    /// window's side (second-window R18, R19, R23): over the view selector
    /// they switch this window to the other view, in the view they aren't from
    /// they tell the shell which place is under the pointer, and the window
    /// they began in ends the drag where the pointer is let go. Esc ends it
    /// with nothing dropped.
    fn clip_drag_frame(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        layout: &ShellLayout,
        area: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(drag) = shell.clip_drag() else {
            self.drag_switched = None;
            self.drag_over = None;
            // Spec: clip files R3. A drag ended by Esc, or let go where the
            // view it began in isn't drawn, leaves no outline in the browser.
            if std::mem::take(&mut self.drag_seen) {
                crate::browser::end_clip_drag(ui.ctx());
            }
            return;
        };
        self.drag_seen = true;
        let ctx = ui.ctx();
        let (pos, released, escaped, modifiers) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.any_released(),
                i.key_pressed(egui::Key::Escape),
                i.modifiers,
            )
        });
        let tell = |events: &mut Vec<UiEvent>, event| events.push(UiEvent::ClipDrag(event));
        if escaped {
            self.drag_switched = None;
            self.drag_over = None;
            tell(events, ClipDragEvent::End { drop: false });
            return;
        }
        let pos = pos.filter(|pos| area.contains(*pos));
        let shown = shell.main_view_of(self.window);
        // The selector of the view this window doesn't show switches it.
        let selector = layout.rect(match shown {
            MainView::Session => Element::ArrangementView,
            MainView::Arrangement => Element::SessionView,
        });
        if pos.is_some_and(|pos| selector.contains(pos)) {
            if self.drag_switched != Some(shown) {
                self.drag_switched = Some(shown);
                if matches!(drag.source, DragSource::Arrangement(_)) {
                    self.arrangement.abandon_drag(ctx, events);
                }
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::VIEW_TOGGLE_SESSION_ARRANGEMENT,
                    json!({}),
                )));
            }
        } else {
            self.drag_switched = None;
        }
        // A live move of Arrangement clips can't go on into another window:
        // it goes back, and the clips are carried instead.
        if pos.is_none() && shell.second_window_open() && drag.window == self.window {
            self.arrangement.abandon_drag(ctx, events);
        }
        let spot = pos.and_then(|pos| match (&drag.source, shown) {
            (DragSource::Slots(_), MainView::Arrangement) => {
                self.arrangement.drop_spot(shell, pos, modifiers)
            }
            (DragSource::Arrangement(_), MainView::Session) => self.mixer.drop_spot(),
            _ => None,
        });
        if spot != self.drag_over {
            self.drag_over = spot;
            tell(events, ClipDragEvent::Over(spot));
        }
        if drag.window == self.window {
            // A drag whose button is up without a release having come ends too:
            // the window lost it.
            let down = ctx.input(|i| i.pointer.any_down());
            if released || !down {
                self.drag_switched = None;
                self.drag_over = None;
                tell(events, ClipDragEvent::End { drop: released });
            }
        }
    }

    /// Carries out a focus move from the last frame's order (the
    /// accessibility spec, R3 and R4).
    fn move_focus(
        &mut self,
        ctx: &Context,
        shell: &Shell,
        step: FocusMove,
        events: &mut Vec<UiEvent>,
    ) {
        use crate::focus::Neighbor;
        if egui::Popup::is_any_open(ctx) {
            // An open menu keeps the keyboard.
            return;
        }
        if self.search_field.is_open() {
            // Spec: command-palette R13. Tab moves round the search field's
            // controls and stays in the field.
            if let Some(id) = self.search_field.tab(ctx, step.backwards) {
                focus_on(ctx, id, events);
            }
            return;
        }
        let focused = ctx.memory(egui::Memory::focused);
        let tracks = track_order(shell);
        let selected = shell
            .arrangement()
            .selected_tracks()
            .iter()
            .find(|t| tracks.contains(t))
            .copied();
        let place = match shell.main_view_of(self.window) {
            MainView::Arrangement => crate::mixer::Place::Header,
            MainView::Session => crate::mixer::Place::Mixer(bl_shell::mixer_view::MixerOf::Session),
        };
        if step.neighbor {
            // From a track's control, or from the view at the selected
            // track's title bar (Next Neighbor is enabled whenever there's a
            // neighbour, observed in Live 12.4.6).
            let current = focused
                .and_then(|id| self.order.find(id))
                .and_then(|entry| entry.control)
                .or_else(|| {
                    let track = selected.or(tracks.first().copied())?;
                    Some(crate::focus::TrackControl {
                        place,
                        track,
                        slot: match place {
                            crate::mixer::Place::Header => crate::focus::Slot::Title,
                            crate::mixer::Place::Mixer(_) => crate::focus::Slot::Column,
                        },
                    })
                });
            let Some(current) = current else {
                return;
            };
            match self.order.neighbor(current, &tracks, step.backwards) {
                Some(Neighbor::Focus(id)) => focus_on(ctx, id, events),
                Some(Neighbor::Reveal(target)) => {
                    self.reveal(target.place, target.track, events);
                    self.pending = Some((Pending::Control(target), PENDING_FRAMES));
                }
                None => {}
            }
            return;
        }
        let in_view = focused == Some(main_view_id())
            || (focused.is_none() && shell.focused_area_of(self.window) == Area::MainView);
        if in_view && shell.main_view_of(self.window) == MainView::Session {
            // From the Session View, Tab goes into the selected track's strip
            // (the first track's), after its title bar.
            let Some(track) = selected.or(tracks.first().copied()) else {
                return;
            };
            match self.order.enter_track(place, track, step.backwards) {
                Some(id) => focus_on(ctx, id, events),
                None if !self.order.drew_track(place, track) => {
                    self.reveal(place, track, events);
                    self.pending = Some((
                        Pending::Enter {
                            place,
                            track,
                            backwards: step.backwards,
                        },
                        PENDING_FRAMES,
                    ));
                }
                None => {}
            }
            return;
        }
        // The Arrangement runs through the view with the selected track's
        // header in it (the first track's with none selected).
        if let Some(id) = self.order.tab(
            focused.filter(|id| *id != main_view_id()),
            shell.focused_area_of(self.window),
            selected.or(tracks.first().copied()),
            step.backwards,
            shell.wrap_navigation(),
        ) {
            focus_on(ctx, id, events);
        }
    }

    /// Brings a track into view in the headers or a mixer, for a focus
    /// move to one of its controls.
    fn reveal(
        &mut self,
        place: crate::mixer::Place,
        track: bl_model::TrackId,
        events: &mut Vec<UiEvent>,
    ) {
        match place {
            crate::mixer::Place::Header => events.push(UiEvent::Invoke(Invocation::new(
                bl_shell::arrangement_commands::SHOW_TRACK,
                json!({ "track": track }),
            ))),
            crate::mixer::Place::Mixer(of) => self.mixer.reveal(of, track),
        }
    }

    /// Focuses a control a focus move waits for, once it's drawn.
    fn resolve_pending(&mut self, ctx: &Context, events: &mut Vec<UiEvent>) {
        let Some((pending, frames)) = self.pending.take() else {
            return;
        };
        let found = match pending {
            Pending::Control(control) => self.order.control(control),
            Pending::Enter {
                place,
                track,
                backwards,
            } => self.order.enter_track(place, track, backwards),
        };
        match found {
            Some(id) => focus_on(ctx, id, events),
            None if frames > 1 => self.pending = Some((pending, frames - 1)),
            None => {}
        }
    }

    /// The node a screen reader reads as the Arrangement View's focus while
    /// the view itself has the keyboard: the selected clip, or the selected
    /// track's title (the accessibility spec, R2).
    fn active_descendant(&self, ctx: &Context, shell: &Shell) -> Option<Id> {
        if shell.main_view_of(self.window) != MainView::Arrangement {
            return None;
        }
        let view = shell.arrangement();
        // Spec: automation-editing R28. A selected automation lane is the
        // focus: its parameter and value are read.
        if let Some(lane) = self.arrangement.keyboard_lane_node(view) {
            return Some(lane);
        }
        if let Some(clip) = self.arrangement.selected_clip_node(view) {
            return Some(clip);
        }
        let drawn = crate::focus::drawn(ctx);
        view.selected_tracks().iter().find_map(|track| {
            drawn.control(crate::focus::TrackControl {
                place: crate::mixer::Place::Header,
                track: *track,
                slot: crate::focus::Slot::Title,
            })
        })
    }

    fn track_area_clicks(&self, ctx: &Context, layout: &ShellLayout, events: &mut Vec<UiEvent>) {
        let Some(pos) = ctx.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.press_origin())
                .flatten()
        }) else {
            return;
        };
        // A click in the search field isn't a click in the pane under it.
        if self.search_field.contains(pos) {
            return;
        }
        let areas = [
            (Area::ControlBar, Some(layout.control_bar)),
            (Area::Browser, layout.browser),
            (Area::MainView, Some(layout.main_view)),
            (Area::Detail, Some(layout.detail)),
            (Area::StatusBar, Some(layout.status_bar)),
            (Area::Assistant, layout.assistant),
            (Area::UndoHistory, layout.undo_history),
        ];
        if let Some((area, _)) = areas
            .iter()
            .find(|(_, rect)| rect.is_some_and(|r| r.contains(pos)))
        {
            events.push(UiEvent::FocusArea(*area));
            // Spec: detail-views R14 (I-FOCUS-001). With both views showing,
            // a click in one gives that view the focus.
            if *area == Area::Detail
                && let (Some(clip), Some(device)) = (layout.clip_view, layout.device_view)
            {
                let view = if clip.contains(pos) {
                    ids::FOCUS_CLIP_VIEW
                } else if device.contains(pos) {
                    ids::FOCUS_DEVICE_VIEW
                } else {
                    return;
                };
                events.push(UiEvent::Invoke(Invocation::new(view, json!({}))));
            }
        }
    }

    fn strip(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &ShellLayout,
        strip: Strip,
        events: &mut Vec<UiEvent>,
    ) {
        for (placement, (el, rect)) in ELEMENTS.iter().zip(&layout.elements) {
            if placement.strip != strip {
                continue;
            }
            // The assistant's toggle stays the main window's alone (D24).
            if self.window == WindowKind::Second && *el == Element::AssistantToggle {
                continue;
            }
            // Spec: tunings R16. A tuning of other than twelve notes hides
            // the scale controls.
            if matches!(
                el,
                Element::ScaleMode | Element::RootNote | Element::ScaleName
            ) && shell.scales_hidden()
            {
                continue;
            }
            // Spec: grooves R30. The Global Amount shows while the pool holds
            // a groove, and the layout has room for it.
            if *el == Element::GlobalAmount && rect.width() <= 0.0 {
                continue;
            }
            // Spec: midi-clock R10, R15. EXT and the sync lamps show while an
            // input has Sync on, the Resync button while Show Resync Button
            // is on.
            if matches!(
                el,
                Element::ExternalSync
                    | Element::SyncInput
                    | Element::SyncOutput
                    | Element::ResyncExternalHardware
            ) && rect.width() <= 0.0
            {
                continue;
            }
            let mut look = self
                .transport
                .look(shell, *el)
                .unwrap_or_else(|| look(*el, shell, self.window));
            if *el == Element::Message
                && let Some(readout) = crate::mixer::take_readout(ui.ctx())
            {
                // Spec: mixer-view R6. While a fader is dragged, the Status
                // Bar shows its volume.
                look = Look {
                    text: Some(readout),
                    ..Look::default()
                };
            }
            let response = element(ui, theme, placement, *rect, &look);
            if is_live(*el) && response.sense.is_focusable() {
                let area = match strip {
                    Strip::ControlBar => Area::ControlBar,
                    Strip::StatusBar => Area::StatusBar,
                };
                crate::focus::register(ui.ctx(), response.id, area, None);
            }
            if self.focus_request == Some(FocusRequest::Element(*el)) {
                response.request_focus();
                self.focus_request = None;
            }
            if placement.pending.is_none()
                && let Some(field) = control_bar::field(*el)
            {
                self.transport.interact(ui, shell, field, &response, events);
            }
            if placement.pending.is_none() {
                self.transport.nudge(ui, shell, *el, &response, events);
            }
            if response.has_focus() && strip == Strip::ControlBar {
                self.last_control = *el;
            }
            if response.hovered() {
                self.hovered = Some(*el);
            }
            // Spec: control-bar-transport R18, R23. Tap Tempo taps on the
            // press; a key's or a screen reader's press, which has no
            // pointer, taps too.
            let activated = if *el == Element::TapTempo {
                let (hovered, clicked) = (response.hovered(), response.clicked());
                !look.disabled
                    && ui.input(|input| {
                        (input.pointer.primary_pressed() && hovered)
                            || (clicked && !input.pointer.any_released())
                    })
            } else {
                response.clicked()
            };
            if activated {
                let option = ui.input(|input| input.modifiers.alt);
                let invocations = detail_click(*el, shell, self.window, option)
                    .unwrap_or_else(|| click(*el, shell, self.window).into_iter().collect());
                events.extend(invocations.into_iter().map(UiEvent::Invoke));
            }
            if *el == Element::ClipSelector {
                self.clip_selector
                    .show(ui, shell, theme, *rect, &response, events);
            }
            if *el == Element::Metronome {
                response.context_menu(|ui| {
                    Self::metronome_menu_content(ui, shell, events);
                });
            }
            if *el == Element::AutomationArm {
                // Spec: automation R18. The Automation Arm button's context
                // menu chooses the recording's mode.
                response.context_menu(|ui| {
                    Self::automation_arm_menu_content(ui, shell, events);
                });
            }
            if *el == Element::MetronomeMenu {
                Self::metronome_menu(&response, shell, events);
            }
            if *el == Element::Tempo {
                // Spec: warping R46. The tempo field's context menu holds
                // Unfollow Tempo Automation.
                response.context_menu(|ui| {
                    Self::tempo_menu_content(ui, shell, events);
                });
            }
            if *el == Element::MixerMenu {
                self.mixer_menu(&response, shell, events);
            }
            if *el == Element::BrowserMenu {
                self.browser_menu(&response, shell, events);
            }
            if *el == Element::RootNote {
                // Spec: scales R9. The Root Note chooser lists the twelve
                // pitch classes.
                egui::Popup::menu(&response).show(|ui| {
                    root_note_menu(ui, shell, events);
                });
            }
            if *el == Element::ScaleName {
                // Spec: scales R9. The Scale Name chooser lists the scale
                // list.
                egui::Popup::menu(&response).show(|ui| {
                    scale_name_menu(ui, shell, events);
                });
            }
            if *el == Element::DeviceSelector {
                // Spec: racks R25 (S-DEV-005). A right-click lists every
                // device on the track.
                response.context_menu(|ui| device_list(ui, shell, events));
            }
        }
    }

    /// The Metronome Settings menu (L-CTRL-009).
    fn metronome_menu(response: &egui::Response, shell: &Shell, events: &mut Vec<UiEvent>) {
        egui::Popup::menu(response).show(|ui| {
            Self::metronome_menu_content(ui, shell, events);
        });
    }

    /// The tempo field's context menu (the warping spec, R46): Unfollow
    /// Tempo Automation, which makes every leading clip a follower and
    /// writes the tempo they set into the Song Tempo envelope.
    fn tempo_menu_content(ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let leading = shell.editor().document().any_tempo_leader();
        let mut item = ui.button("Unfollow Tempo Automation");
        if !leading {
            item = item.on_hover_text("no clip leads the tempo");
        }
        if item.clicked() && leading {
            events.push(UiEvent::Invoke(Invocation::new(
                control_bar::UNFOLLOW_TEMPO_AUTOMATION,
                serde_json::json!({}),
            )));
        }
    }

    /// The Automation Arm button's context menu (automation R18): the
    /// recording's mode, the current one ticked.
    fn automation_arm_menu_content(ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let mode = shell.automation_record_mode();
        for choice in AutomationRecordMode::ALL {
            let mut text = choice.label().to_owned();
            if choice == mode {
                text = format!("✔ {text}");
            }
            if ui.button(text).clicked() {
                events.push(UiEvent::Invoke(Invocation::new(
                    ids::SETTINGS_SET_AUTOMATION_RECORD_MODE,
                    json!({ "mode": choice }),
                )));
                ui.close();
            }
        }
    }

    fn metronome_menu_content(ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let document = shell.editor().document();
        let settings = document.settings().metronome;
        // A rhythm that doesn't fit one bar of the current time signature
        // shows disabled, as Live's menu does (manual 19.4.1).
        let signature = document
            .settings()
            .time_signatures
            .signature_at(shell.arrangement().transport().position);

        // Count-In
        ui.label(egui::RichText::new("Count-In").strong());
        for (label, val) in [
            ("None", CountIn::None),
            ("1 Bar", CountIn::Bars1),
            ("2 Bars", CountIn::Bars2),
            ("4 Bars", CountIn::Bars4),
        ] {
            let checked = settings.count_in == val;
            let mut text = label.to_string();
            if checked {
                text = format!("✔ {text}");
            }
            if ui.button(text).clicked() {
                events.push(UiEvent::Invoke(Invocation::new(
                    transport_commands::SET_METRONOME_COUNT_IN,
                    json!({ "count_in": val }),
                )));
                ui.close();
            }
        }

        ui.separator();

        // Sound
        ui.label(egui::RichText::new("Sound").strong());
        for (label, val) in [
            ("Classic", MetronomeSound::Classic),
            ("Click", MetronomeSound::Click),
            ("Wood", MetronomeSound::Wood),
        ] {
            let checked = settings.sound == val;
            let mut text = label.to_string();
            if checked {
                text = format!("✔ {text}");
            }
            if ui.button(text).clicked() {
                events.push(UiEvent::Invoke(Invocation::new(
                    transport_commands::SET_METRONOME_SOUND,
                    json!({ "sound": val }),
                )));
                ui.close();
            }
        }

        ui.separator();

        // Rhythm
        ui.label(egui::RichText::new("Rhythm").strong());
        for (label, val) in [
            ("Auto", MetronomeRhythm::Auto),
            ("1 Bar", MetronomeRhythm::Bar1),
            ("1/2", MetronomeRhythm::Half),
            ("1/2T", MetronomeRhythm::HalfTriplet),
            ("1/4", MetronomeRhythm::Quarter),
            ("1/4T", MetronomeRhythm::QuarterTriplet),
            ("1/8", MetronomeRhythm::Eighth),
            ("1/8T", MetronomeRhythm::EighthTriplet),
            ("1/16", MetronomeRhythm::Sixteenth),
            ("1/16T", MetronomeRhythm::SixteenthTriplet),
        ] {
            let checked = settings.rhythm == val;
            let mut text = label.to_string();
            if checked {
                text = format!("✔ {text}");
            }
            let fits = val.fits_in_bar(signature);
            if ui.add_enabled(fits, egui::Button::new(text)).clicked() {
                events.push(UiEvent::Invoke(Invocation::new(
                    transport_commands::SET_METRONOME_RHYTHM,
                    json!({ "rhythm": val }),
                )));
                ui.close();
            }
        }

        ui.separator();

        // Enable Only While Recording
        let checked = settings.only_while_recording;
        let mut text = "Enable Only While Recording".to_string();
        if checked {
            text = format!("✔ {text}");
        }
        if ui.button(text).clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                transport_commands::SET_METRONOME_ONLY_WHILE_RECORDING,
                json!({ "only_while_recording": !checked }),
            )));
            ui.close();
        }
    }

    /// The Mixer Config Menu (L-STAT-008): the Mixer Controls choices, as
    /// View ▸ Mixer Controls has them (the mixer view spec, R12).
    fn mixer_menu(&mut self, response: &egui::Response, shell: &Shell, events: &mut Vec<UiEvent>) {
        let keymap =
            crate::keymap::keymap_in_use(&mut self.keymap, &mut self.keymap_revision, shell);
        let platform = crate::keymap::Platform::current();
        let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
        let Some(controls) = menus
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .find(|item| item.row == "M-VIEW-032")
        else {
            return;
        };
        egui::Popup::menu(response).show(|ui| {
            for item in bl_shell::menus::shown(&controls.items) {
                let _ = crate::menus::item(ui, item, shell, keymap, platform, events);
            }
        });
    }

    /// The Browser Config Menu (L-CTRL-001): Full-Height Browser, Groove
    /// Pool and Tuning, as the View menu's rows give them (spec: browser
    /// R26).
    fn browser_menu(
        &mut self,
        response: &egui::Response,
        shell: &Shell,
        events: &mut Vec<UiEvent>,
    ) {
        let keymap =
            crate::keymap::keymap_in_use(&mut self.keymap, &mut self.keymap_revision, shell);
        let platform = crate::keymap::Platform::current();
        let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
        let rows: Vec<&bl_shell::menus::MenuItem> = ["M-VIEW-024", "M-VIEW-022", "M-VIEW-023"]
            .iter()
            .filter_map(|row| {
                menus
                    .iter()
                    .flat_map(bl_shell::menus::Menu::walk)
                    .find(|item| item.row == *row)
            })
            .collect();
        egui::Popup::menu(response).show(|ui| {
            for item in rows {
                let _ = crate::menus::item(ui, item, shell, keymap, platform, events);
            }
        });
    }

    fn control_bar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &ShellLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let id = region(
            ui,
            "region-control-bar",
            layout.control_bar,
            Role::Toolbar,
            "Control Bar",
        );
        ui.painter()
            .rect_filled(layout.control_bar, 0.0, theme.colors.bar.0);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.control_bar)
                .accessibility_parent(id),
            |ui| self.strip(ui, shell, theme, layout, Strip::ControlBar, events),
        );
        self.transport.end_frame(ui);
    }

    fn status_bar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &ShellLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let id = region(
            ui,
            "region-status-bar",
            layout.status_bar,
            Role::Group,
            "Status Bar",
        );
        ui.painter()
            .rect_filled(layout.status_bar, 0.0, theme.colors.bar.0);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(layout.status_bar)
                .accessibility_parent(id),
            |ui| self.strip(ui, shell, theme, layout, Strip::StatusBar, events),
        );
        // Spec: map-modes R10 (L-STAT-009). While the selected control has a
        // controller mapping, the mode chooser sits at the right of the
        // message area.
        let message = layout
            .elements
            .iter()
            .find(|(element, _)| *element == Element::Message)
            .map(|(_, rect)| *rect);
        if let (Some(mapping), Some(message)) = (shell.map_controller_mapping(), message) {
            let choices: Vec<crate::widgets::Choice<bl_model::RemoteMode>> =
                bl_model::RemoteMode::choices(&mapping.source)
                    .into_iter()
                    .map(|mode| crate::widgets::Choice {
                        label: mode.label(),
                        value: mode,
                        leds: crate::widgets::NO_LEDS,
                    })
                    .collect();
            let rect = Rect::from_min_size(
                pos2(message.right() - 152.0, message.top() + 2.0),
                egui::vec2(148.0, message.height() - 4.0),
            );
            if let Some(mode) = crate::widgets::chooser(
                ui,
                theme,
                "mapping-mode",
                "Mapping Mode",
                rect,
                &choices,
                Some(&mapping.mode),
                true,
            ) {
                events.push(UiEvent::Invoke(Invocation::new(
                    remote_ids::SET_MODE,
                    json!({ "source": mapping.source, "mode": mode }),
                )));
            }
        }
        // Spec: macros R11. The macro that holds the parameter under the
        // pointer is named here.
        let hint = crate::mapping::hover_hold(ui.ctx())
            .map(|name| format!("Held by macro \"{name}\""))
            // Spec: device-chains R42. The surfaces a locked device's hand
            // icon names.
            .or_else(|| crate::mapping::hover_lock(ui.ctx()));
        if let Some(text) = hint {
            ui.painter().text(
                pos2(
                    layout.status_bar.right() - 8.0,
                    layout.status_bar.center().y,
                ),
                Align2::RIGHT_CENTER,
                text,
                theme.small_font(),
                theme.colors.text.0,
            );
        }
    }

    fn main_view(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let (name, note) = match shell.main_view_of(self.window) {
            MainView::Arrangement => (
                "Arrangement View",
                "Tracks and their clips along the song's timeline, with the rulers above and \
                 the track headers at the right.",
            ),
            MainView::Session => (
                "Session View",
                "The tracks' columns and their mixer. The clip slots and scenes arrive in \
                 milestone M3.",
            ),
        };
        let response = ui.interact(rect, main_view_id(), Sense::click());
        let pressed_here = ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|p| rect.contains(p))
        });
        if self.focus_request == Some(FocusRequest::MainView) || pressed_here {
            // A click in the view gives it the keyboard (I-FOCUS-001).
            response.request_focus();
            self.focus_request = None;
        }
        match shell.main_view_of(self.window) {
            MainView::Arrangement => {
                self.arrangement.show(ui, shell, theme, rect, events);
                if let Some(mixer) = self.arrangement.mixer_rect() {
                    self.mixer.show_mixer(
                        ui,
                        shell,
                        theme,
                        mixer,
                        rect,
                        bl_shell::mixer_view::MixerOf::Arrangement,
                        events,
                    );
                }
            }
            MainView::Session => self.mixer.show_session(ui, shell, theme, rect, events),
        }
        // Spec: accessibility R2. With the view focused, the clip or track
        // the keys select is its active descendant, so a screen reader reads
        // it as the selection moves.
        let active = self.active_descendant(ui.ctx(), shell);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Panel, true, name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Pane);
            node.set_description(note.to_owned());
            if let Some(id) = active {
                node.set_active_descendant(id.accesskit_id());
            }
        });
    }

    fn info_view(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        rect: Option<Rect>,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(rect) = rect else {
            // Hidden: nothing to show, and nothing left over from before. A
            // field open in it goes with it, so the keys don't stay with a
            // field nobody sees.
            self.info = InfoViewContent::default();
            self.info_editing = None;
            return;
        };
        let id = region(ui, "region-info-view", rect, Role::Group, "Info View");
        let _ = id;
        ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
        // Like Live's Info View, it reads whatever the pointer is over
        // (L-INFO-002, I-FOCUS-012): the control's name, the help text of
        // the command it runs, and its key on a separate line; nothing
        // while the pointer is over empty space.
        let keymap =
            crate::keymap::keymap_in_use(&mut self.keymap, &mut self.keymap_revision, shell);
        if self.info_editing.is_some() {
            crate::info_text::edit(ui, theme, shell, rect, &mut self.info_editing, events);
            self.info = InfoViewContent::default();
            return;
        }
        // An item with info text of the user's own shows it in place of the
        // registered description (info-view R11).
        let item = crate::info_text::take_hovered(ui.ctx(), shell);
        // A MIDI Tool's control under the pointer says its own name and
        // description (the MIDI Tools spec, R7).
        self.info = item
            .or_else(|| crate::search_field::info(shell))
            .or_else(|| crate::tool_panel::take_info(ui.ctx()))
            .or_else(|| crate::tuning_section::take_info(ui.ctx()))
            .unwrap_or_else(|| info_content(self.hovered.or(self.other_hovered), shell, keymap));
        // Spec: tunings R26. With the pointer over nothing, a tuning
        // selected in the browser shows its description.
        if self.info.title.is_empty()
            && let Some(path) = shell.browser_view().selected_tuning()
        {
            if self
                .tuning_info
                .as_ref()
                .is_none_or(|(known, _)| *known != path)
            {
                let (title, body) = bl_shell::browser::tunings::info(&path);
                self.tuning_info = Some((
                    path,
                    InfoViewContent {
                        title,
                        body,
                        shortcut: None,
                    },
                ));
            }
            if let Some((_, content)) = &self.tuning_info {
                self.info = content.clone();
            }
        }
        if self.info.title.is_empty() {
            return;
        }
        let painter = ui.painter_at(rect);
        let mut y = rect.top() + 8.0;
        painter.text(
            egui::pos2(rect.left() + 8.0, y),
            Align2::LEFT_TOP,
            &self.info.title,
            egui::FontId::proportional(theme.sizes.heading),
            theme.colors.text.0,
        );
        y += 20.0;
        if !self.info.body.is_empty() {
            let galley = painter.layout(
                self.info.body.clone(),
                theme.small_font(),
                theme.colors.text_dim.0,
                rect.width() - 16.0,
            );
            let height = galley.size().y;
            painter.galley(
                egui::pos2(rect.left() + 8.0, y),
                galley,
                theme.colors.text_dim.0,
            );
            y += height + 4.0;
        }
        if let Some(shortcut) = &self.info.shortcut {
            painter.text(
                egui::pos2(rect.left() + 8.0, y),
                Align2::LEFT_TOP,
                shortcut,
                theme.small_font(),
                theme.colors.text.0,
            );
        }
    }
}

/// What the Info View shows for the element under the pointer
/// (L-INFO-002): the control's name, the registered description of the
/// command it runs (D17), or the note for a control a later package
/// brings, and the command's key where the default keymap has one.
/// The rack whose Map mode is on and the devices inside it, for the
/// controls to tint (macros spec, R3).
fn macro_map(shell: &Shell) -> Option<crate::mapping::MacroMap> {
    let rack = shell.devices().map_mode()?;
    let doc = shell.editor().document();
    let device = doc.device(rack)?;
    let devices = device.walk().map(|d| d.id).collect();
    Some(crate::mapping::MacroMap { rack, devices })
}

/// The map mode that is on, with the source of each mapped control.
fn remote_map_state(shell: &Shell) -> Option<crate::mapping::RemoteMap> {
    let mode = shell.map_mode();
    if mode == MapMode::Off {
        return None;
    }
    let keys = mode == MapMode::Key;
    let labels = shell
        .editor()
        .document()
        .settings()
        .remote
        .iter()
        .filter(|m| m.source.is_key() == keys)
        .map(|m| (m.target, m.source.label()))
        .collect();
    Some(crate::mapping::RemoteMap {
        mode,
        selected: shell.map_selection(),
        labels,
    })
}

/// Every parameter a rack's macro holds, with the macro's name (macros spec,
/// R11).
fn held_params(
    shell: &Shell,
) -> std::collections::BTreeMap<(bl_model::DeviceId, bl_model::ParamId), String> {
    let doc = shell.editor().document();
    let mut held = std::collections::BTreeMap::new();
    for rack in doc
        .tracks()
        .flat_map(|track| track.devices_deep())
        .filter(|device| device.rack_kind().is_some())
    {
        let count = shell.devices().views_of(rack.id).macro_count;
        let Some(state) = shell.devices().macros_of(doc, rack.id, count) else {
            continue;
        };
        for info in state.macros {
            for mapping in info.mappings {
                if let bl_model::MacroTarget::Device { device, param } = mapping.target {
                    held.insert((device, param), info.name.clone());
                }
            }
        }
    }
    held
}

fn info_content(
    hovered: Option<Element>,
    shell: &Shell,
    keymap: &crate::keymap::Keymap,
) -> InfoViewContent {
    let Some(element) = hovered else {
        return InfoViewContent::default();
    };
    let placement = crate::layout::placement(element);
    let command = command_of(element);
    let body = match placement.pending {
        Some(package) => format!("Not built yet: arrives with {package}."),
        None => command
            .and_then(|command| shell.registry().get(command))
            .map(|spec| spec.description.to_string())
            .unwrap_or_default(),
    };
    let shortcut = command.and_then(|command| {
        let platform = crate::keymap::Platform::current();
        keymap
            .chord_for_command(command, platform)
            .map(|chord| crate::keymap::chord_text(chord, platform))
    });
    InfoViewContent {
        title: placement.name.to_owned(),
        body,
        shortcut,
    }
}

/// Moves the keyboard focus to a control: a part of the browser through
/// `focus.browser`, so the shell knows which part's keys apply, and any
/// other control directly.
fn focus_on(ctx: &Context, id: Id, events: &mut Vec<UiEvent>) {
    match crate::browser::part_of(id) {
        Some(part) => events.push(UiEvent::Invoke(Invocation::new(
            bl_shell::browser_commands::FOCUS_BROWSER,
            json!({ "part": part }),
        ))),
        None => ctx.memory_mut(|m| m.request_focus(id)),
    }
}

/// The tracks in the order the neighbour keys move through them: the
/// mixer's columns (the Arrangement's track order), then the returns and
/// Main.
fn track_order(shell: &Shell) -> Vec<bl_model::TrackId> {
    shell.neighbor_order()
}

/// The title a window shows, and reaches assistive technology as its name:
/// the set's, and "<set name>, Second Window" for the second window
/// (second-window R16).
#[must_use]
pub fn window_title(shell: &Shell, window: WindowKind) -> String {
    if window == WindowKind::Second {
        format!("{}, Second Window", shell.set_name())
    } else {
        shell.set_name()
    }
}

/// Draws a border round the area with the keyboard focus while Outline View
/// in Focus is on (the accessibility spec, R7; I-FOCUS-009). It's off by
/// default, as Live's is, and then nothing shows where the focus is.
fn outline_focused_area(
    ui: &Ui,
    shell: &Shell,
    theme: &Theme,
    layout: &ShellLayout,
    window: WindowKind,
) {
    // Only the window in front outlines its focused area (second-window
    // R16).
    if !shell.outline_view_in_focus() || shell.focused_window() != window {
        return;
    }
    let rect = match shell.focused_area_of(window) {
        Area::ControlBar => Some(layout.control_bar),
        Area::Browser => layout.browser,
        Area::MainView => Some(layout.main_view),
        // Spec: detail-views R15. With both views showing, the outline goes
        // round the one that has the focus.
        Area::Detail => match (layout.clip_view, layout.device_view) {
            (Some(clip), Some(_)) if shell.detail_of(window).focus() == DetailView::Clip => {
                Some(clip)
            }
            (Some(_), Some(device)) => Some(device),
            _ => Some(layout.detail),
        },
        Area::StatusBar => Some(layout.status_bar),
        Area::Assistant => layout.assistant,
        Area::UndoHistory => layout.undo_history,
    };
    if let Some(rect) = rect {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(1.0, theme.colors.focus.0),
            StrokeKind::Inside,
        );
    }
}

/// Draws Live's compact title bar across the top of a window (L-WIN-002,
/// D22): [`crate::layout::TITLE_BAR`] points tall, the title centred. The
/// system's window buttons sit over its left end, and the system's title bar
/// view over all of it handles dragging and double-clicks, so this only
/// paints. The title reaches assistive technology as the window's own title.
///
/// At every interface zoom it stays that many window points tall, with its
/// title at its 100% size, since the system's buttons in it don't scale
/// (interface-zoom R9).
pub fn draw_title_bar(root: &mut Ui, theme: &Theme, title: &str) {
    let zoom = root.ctx().zoom_factor();
    let font = theme.font();
    egui::Panel::top("title-bar")
        .exact_size(title_bar_height(zoom))
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(theme.colors.bar.0))
        .show(root, |ui| {
            let rect = ui.max_rect();
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                title,
                egui::FontId::new(font.size / zoom, font.family),
                theme.colors.text_dim.0,
            );
            ui.painter().line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                Stroke::new(1.0 / zoom, theme.colors.separator.0),
            );
        });
}

/// The focus direction for a move to the next or previous control.
#[must_use]
pub fn direction(backwards: bool) -> egui::FocusDirection {
    if backwards {
        egui::FocusDirection::Previous
    } else {
        egui::FocusDirection::Next
    }
}

/// An accessible region: a node with a role and a name that the elements
/// inside it hang from. Returns its ID.
fn region(ui: &mut Ui, salt: &str, rect: Rect, role: Role, name: &str) -> Id {
    let id = Id::new(salt);
    let _ = ui.interact(rect, id, Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(role);
        node.set_label(name.to_owned());
        // Its place, so a screen reader can show where the region is (and
        // the zoom tests can find it).
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    id
}

/// What an element shows, from the shell's state.
fn look(element: Element, shell: &Shell, window: WindowKind) -> Look {
    let text = |t: &str| Look {
        text: Some(t.to_owned()),
        ..Look::default()
    };
    match element {
        // Spec: racks R25 (L-STAT-007). The selected track's name.
        Element::DeviceSelector => {
            let doc = shell.editor().document();
            shell
                .device_view_state()
                .track
                .and_then(|t| doc.track(t))
                .map_or_else(Look::default, |t| text(t.name()))
        }
        Element::BrowserToggle => Look {
            on: shell.browser_of(window),
            value: Some(
                if shell.browser_of(window) {
                    "shown"
                } else {
                    "hidden"
                }
                .to_owned(),
            ),
            ..Look::default()
        },
        Element::InfoToggle => Look {
            on: shell.info_view_visible(),
            value: Some(
                if shell.info_view_visible() {
                    "shown"
                } else {
                    "hidden"
                }
                .to_owned(),
            ),
            ..Look::default()
        },
        Element::AssistantToggle => {
            let open = shell.assistant_panel_state().open;
            Look {
                on: open,
                value: Some(if open { "shown" } else { "hidden" }.to_owned()),
                ..Look::default()
            }
        }
        // Spec: detail-views R3 (L-STAT-004 to L-STAT-007). The toggles are
        // on while their view shows; the Clip View's selector reads the name
        // of the clip it shows.
        Element::ClipViewToggle | Element::DeviceViewToggle => {
            let on = if element == Element::ClipViewToggle {
                shell.detail_of(window).clip_shown()
            } else {
                shell.detail_of(window).device_shown()
            };
            Look {
                on,
                value: Some(if on { "on" } else { "off" }.to_owned()),
                ..Look::default()
            }
        }
        Element::ClipSelector => {
            let doc = shell.editor().document();
            shell
                .clip_view()
                .shown_any_clip(doc, shell.arrangement())
                .and_then(|clip| doc.clip(clip))
                .map_or_else(Look::default, |clip| text(&clip.name))
        }
        Element::DrawMode => Look {
            on: shell.clip_view().draw_mode(),
            value: Some(
                if shell.clip_view().draw_mode() {
                    "on"
                } else {
                    "off"
                }
                .to_owned(),
            ),
            ..Look::default()
        },
        Element::ComputerKeyboard => {
            let on = shell.computer_keyboard_on();
            Look {
                on,
                value: Some(if on { "on" } else { "off" }.to_owned()),
                ..Look::default()
            }
        }
        // The transport section's values come from `control_bar`.
        Element::Quantization => text("1 Bar"),
        // Spec: scales R6, R9, R14. The scale controls show the selected
        // clips' scale, and an asterisk where they differ.
        Element::ScaleMode => {
            let display = shell.scale_display();
            Look {
                on: display.enabled == Some(true),
                mixed: display.enabled.is_none(),
                value: Some(
                    match display.enabled {
                        Some(true) => "on",
                        Some(false) => "off",
                        None => "mixed",
                    }
                    .to_owned(),
                ),
                description: Some(
                    "Scale Mode: whether the clips carry a scale, and which keys belong to \
                     it."
                    .to_owned(),
                ),
                ..Look::default()
            }
        }
        Element::RootNote => {
            let display = shell.scale_display();
            let name = display
                .root
                .map_or_else(|| "*".to_owned(), |root| shell.root_name(root));
            Look {
                text: Some(name.clone()),
                value: Some(display.root.map_or_else(|| "mixed".to_owned(), |_| name)),
                description: Some("The root note of the scale.".to_owned()),
                ..Look::default()
            }
        }
        Element::ScaleName => {
            let display = shell.scale_display();
            let name = display.name.map_or("*", |name| name.label());
            Look {
                text: Some(name.to_owned()),
                value: Some(
                    display
                        .name
                        .map_or_else(|| "mixed".to_owned(), |name| name.label().to_owned()),
                ),
                description: Some("The scale the keys belong to.".to_owned()),
                ..Look::default()
            }
        }
        // Spec: map-modes R1. The switches are lit while their mode is on.
        Element::KeyMap | Element::MidiMap => {
            let mode = if element == Element::KeyMap {
                MapMode::Key
            } else {
                MapMode::Midi
            };
            let on = shell.map_mode() == mode;
            Look {
                text: Some(if mode == MapMode::Key { "KEY" } else { "MIDI" }.to_owned()),
                on,
                value: Some(if on { "on" } else { "off" }.to_owned()),
                ..Look::default()
            }
        }
        Element::SampleRate => {
            let rate = shell.audio().status().sample_rate;
            let shown = rate.map_or_else(|| "—".to_owned(), sample_rate_text);
            Look {
                value: Some(
                    rate.map_or_else(|| "no audio device open".to_owned(), sample_rate_text),
                ),
                text: Some(shown),
                ..Look::default()
            }
        }
        Element::Cpu => {
            let status = shell.audio().status();
            match status.cpu_load.filter(|_| status.engine_on) {
                Some(load) => Look {
                    text: Some(format!("{:.0} %", load.ceil())),
                    value: Some(format!("{load:.1} percent")),
                    number: Some(f64::from(load)),
                    ..Look::default()
                },
                None => Look {
                    text: Some("OFF".to_owned()),
                    value: Some("audio engine off".to_owned()),
                    ..Look::default()
                },
            }
        }
        Element::Follow => {
            let view = shell.arrangement();
            Look {
                on: view.follow(),
                value: Some(
                    match (view.follow(), view.follow_paused()) {
                        (true, true) => "on, paused",
                        (true, false) => "on",
                        (false, _) => "off",
                    }
                    .to_owned(),
                ),
                ..Look::default()
            }
        }
        Element::MixerToggle => {
            let shown = shell.mixer_view().of(shell.mixer_of(window)).shown;
            Look {
                on: shown,
                value: Some(if shown { "shown" } else { "hidden" }.to_owned()),
                ..Look::default()
            }
        }
        Element::SessionView => Look {
            on: shell.main_view_of(window) == MainView::Session,
            ..Look::default()
        },
        Element::ArrangementView => Look {
            on: shell.main_view_of(window) == MainView::Arrangement,
            ..Look::default()
        },
        Element::MidiIndicators => {
            // Spec: midi-io R25; external-devices R7. The number holds the
            // two lamps: 1 for the input's, 2 for the output's.
            let (input, output) = shell.midi_indicators();
            let lit = |on: bool, what: &str| if on { what.to_owned() } else { String::new() };
            Look {
                number: Some(f64::from(u8::from(input) | u8::from(output) << 1)),
                value: Some(
                    [lit(input, "receiving"), lit(output, "sending")]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                description: Some(
                    "The upper light flashes when MIDI reaches a track's input, the lower \
                     when a track sends MIDI to a port."
                        .to_owned(),
                ),
                ..Look::default()
            }
        }
        Element::RemoteIndicators => {
            // Spec: remote-mapping R41. The number holds the two lamps: 1 for
            // the upper (a message taken), 2 for the lower (feedback sent).
            let (taken, sent) = shell.remote_indicators();
            Look {
                number: Some(f64::from(u8::from(taken) | u8::from(sent) << 1)),
                value: Some(
                    [(taken, "receiving"), (sent, "sending")]
                        .into_iter()
                        .filter_map(|(on, what)| on.then_some(what))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                description: Some(
                    "The upper light flashes when a mapping or a control surface takes a MIDI \
                     message, or one is taken for learning, the lower when feedback or a \
                     surface's message leaves for a port."
                        .to_owned(),
                ),
                ..Look::default()
            }
        }
        Element::KeyIndicators => {
            // Spec: remote-mapping R41. The upper lamp flashes for a key a key
            // mapping takes; the lower stays dark.
            let (taken, _) = shell.key_indicators();
            Look {
                number: Some(f64::from(u8::from(taken))),
                value: Some(if taken { "receiving" } else { "" }.to_owned()),
                description: Some(
                    "The upper light flashes when a key mapping takes a key.".to_owned(),
                ),
                ..Look::default()
            }
        }
        Element::SyncInput | Element::SyncOutput => {
            // Spec: midi-clock R12, R16, R27. Each lamp is lit or dark.
            let (lit, what) = if element == Element::SyncInput {
                (shell.sync_input_lit(), "arrives at an input with Sync on")
            } else {
                (shell.sync_output_lit(), "is sent to an output with Sync on")
            };
            Look {
                number: Some(f64::from(u8::from(lit))),
                value: Some(if lit { "lit" } else { "dark" }.to_owned()),
                description: Some(format!("Flashes when MIDI clock {what}.")),
                ..Look::default()
            }
        }
        Element::ResyncExternalHardware => text("RESYNC"),
        Element::Message => match (shell.message(), shell.hint()) {
            (Some(message), _) => Look {
                text: Some(message.text.clone()),
                on: message.error,
                ..Look::default()
            },
            (None, Some(hint)) => Look {
                text: Some(hint.to_owned()),
                ..Look::default()
            },
            // Spec: map-modes R5, R10. A selected control with nowhere for a
            // message to come from, and a mode that is only a guess.
            (None, None) if map_message(shell).is_some() => Look {
                text: map_message(shell),
                ..Look::default()
            },
            // Spec: file-management R12. The Status Bar says how many of
            // the set's files are missing, until none is.
            (None, None) if shell.note_group_status().is_some() => Look {
                text: shell.note_group_status(),
                ..Look::default()
            },
            (None, None) => {
                let missing = shell.missing_files();
                Look {
                    text: Some(if missing == 0 {
                        String::new()
                    } else {
                        format!(
                            "{missing} file{} missing: click to open the File Manager",
                            if missing == 1 { " is" } else { "s are" }
                        )
                    }),
                    on: missing > 0,
                    ..Look::default()
                }
            }
        },
        _ => Look::default(),
    }
}

/// What the Status Bar says in MIDI Map Mode: that no input has its Remote
/// switch on, while a control waits for a message, and that a controller's
/// mode is a guess (map modes R5, R10).
fn map_message(shell: &Shell) -> Option<String> {
    if shell.map_mode() != MapMode::Midi || shell.map_selection().is_none() {
        return None;
    }
    if !shell.remote_input_on() {
        return Some("No MIDI input has its Remote switch on".to_owned());
    }
    shell
        .map_guess()
        .map(|mode| format!("Mapping mode guessed from the controller: {}", mode.label()))
}

/// A sample rate as the Control Bar shows it: `48 kHz`, `44.1 kHz`.
#[must_use]
pub fn sample_rate_text(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}

/// The registered command a control of the main window runs, for the Info
/// View to read its description from (WP-2.24): the transport section's
/// command, then the panel toggles'.
fn command_of(element: Element) -> Option<&'static str> {
    match element {
        Element::BrowserToggle => Some(ids::VIEW_TOGGLE_BROWSER),
        Element::InfoToggle => Some(ids::VIEW_TOGGLE_INFO_VIEW),
        Element::AssistantToggle => Some(bl_shell::assistant::VIEW_TOGGLE_ASSISTANT),
        Element::ClipViewToggle => Some(ids::VIEW_TOGGLE_CLIP_VIEW),
        Element::DeviceViewToggle => Some(ids::VIEW_TOGGLE_DEVICE_VIEW),
        Element::DrawMode => Some(ids::VIEW_DRAW_MODE),
        Element::ComputerKeyboard => Some(midi_ids::TOGGLE_COMPUTER_KEYBOARD),
        Element::KeyMap | Element::MidiMap => Some(remote_ids::MAP_MODE),
        Element::ResyncExternalHardware => Some("transport.resync_external_hardware"),
        Element::SessionView | Element::ArrangementView => {
            Some(ids::VIEW_TOGGLE_SESSION_ARRANGEMENT)
        }
        Element::MixerToggle => Some(bl_shell::mixer_view_commands::TOGGLE),
        _ => control_bar::command(element),
    }
}

/// What double-clicking a clip, a slot or a title bar, or a click on the
/// Session View's status field, runs (detail-views R4): what a click on
/// `view`'s selector runs, except that with both views showing neither is
/// hidden.
pub(crate) fn open_view(shell: &Shell, view: DetailView) -> Vec<Invocation> {
    let window = shell.acting_window();
    let detail = shell.detail_of(window);
    if detail.clip_shown() && detail.device_shown() {
        Vec::new()
    } else {
        show_only(shell, window, view)
    }
}

/// What a click on a view's selector runs (detail-views R3): the commands
/// that show `view` and hide the other.
fn show_only(shell: &Shell, window: WindowKind, view: DetailView) -> Vec<Invocation> {
    let detail = shell.detail_of(window);
    let command = |toggle| Invocation::new(toggle, json!({}));
    let (own, own_shown, other, other_shown) = match view {
        DetailView::Clip => (
            ids::VIEW_TOGGLE_CLIP_VIEW,
            detail.clip_shown(),
            ids::VIEW_TOGGLE_DEVICE_VIEW,
            detail.device_shown(),
        ),
        DetailView::Device => (
            ids::VIEW_TOGGLE_DEVICE_VIEW,
            detail.device_shown(),
            ids::VIEW_TOGGLE_CLIP_VIEW,
            detail.clip_shown(),
        ),
    };
    let mut commands = Vec::new();
    if !own_shown {
        commands.push(command(own));
    }
    if other_shown {
        commands.push(command(other));
    }
    commands
}

/// What clicking one of the detail area's toggles or selectors runs, or
/// `None` for another element (detail-views R1 to R3): a toggle shows or
/// hides its view, and Option-click on either shows both; a selector shows
/// its view and hides the other.
fn detail_click(
    element: Element,
    shell: &Shell,
    window: WindowKind,
    option: bool,
) -> Option<Vec<Invocation>> {
    let toggle = |id| vec![Invocation::new(id, json!({}))];
    Some(match element {
        Element::ClipViewToggle | Element::DeviceViewToggle if option => {
            toggle(ids::VIEW_SHOW_DETAIL_VIEWS)
        }
        Element::ClipViewToggle => toggle(ids::VIEW_TOGGLE_CLIP_VIEW),
        Element::DeviceViewToggle => toggle(ids::VIEW_TOGGLE_DEVICE_VIEW),
        Element::ClipSelector => show_only(shell, window, DetailView::Clip),
        Element::DeviceSelector => show_only(shell, window, DetailView::Device),
        _ => return None,
    })
}

/// What clicking a live element runs.
fn click(element: Element, shell: &Shell, window: WindowKind) -> Option<Invocation> {
    match element {
        Element::BrowserToggle => Some(Invocation::new(ids::VIEW_TOGGLE_BROWSER, json!({}))),
        Element::InfoToggle => Some(Invocation::new(ids::VIEW_TOGGLE_INFO_VIEW, json!({}))),
        Element::AssistantToggle => Some(Invocation::new(
            bl_shell::assistant::VIEW_TOGGLE_ASSISTANT,
            json!({}),
        )),
        Element::DrawMode => Some(Invocation::new(ids::VIEW_DRAW_MODE, json!({}))),
        Element::ComputerKeyboard => Some(Invocation::new(
            midi_ids::TOGGLE_COMPUTER_KEYBOARD,
            json!({}),
        )),
        Element::KeyMap => Some(Invocation::new(
            remote_ids::MAP_MODE,
            json!({ "mode": "key", "toggle": true }),
        )),
        Element::MidiMap => Some(Invocation::new(
            remote_ids::MAP_MODE,
            json!({ "mode": "midi", "toggle": true }),
        )),
        Element::ResyncExternalHardware => Some(Invocation::new(
            "transport.resync_external_hardware",
            json!({}),
        )),
        Element::Follow => Some(Invocation::new(
            bl_shell::arrangement_commands::FOLLOW,
            json!({}),
        )),
        // Spec: mixer-view R11.
        Element::MixerToggle => Some(Invocation::new(
            bl_shell::mixer_view_commands::TOGGLE,
            json!({}),
        )),
        Element::SampleRate => Some(Invocation::new(
            ids::APP_SETTINGS,
            json!({ "page": "audio" }),
        )),
        Element::SessionView if shell.main_view_of(window) != MainView::Session => Some(
            Invocation::new(ids::VIEW_TOGGLE_SESSION_ARRANGEMENT, json!({})),
        ),
        Element::ArrangementView if shell.main_view_of(window) != MainView::Arrangement => Some(
            Invocation::new(ids::VIEW_TOGGLE_SESSION_ARRANGEMENT, json!({})),
        ),
        // Spec: file-management R12. A click on the missing-files message
        // shows the File Manager's Manage Set view.
        Element::Message => shell.message().and_then(|m| m.action.clone()).or_else(|| {
            (shell.missing_files() > 0).then(|| {
                Invocation::new(
                    "view.toggle_file_manager",
                    json!({ "view": "set", "open": true }),
                )
            })
        }),
        // Spec: scales R12. The Scale Mode switch changes the selected
        // clips' scales, or the set's current scale when no clip is
        // selected.
        Element::ScaleMode => {
            let on = shell.scale_display().enabled == Some(true);
            Some(scale_command(shell, &[("enabled", json!(!on))]))
        }
        // Play, Stop, and the Loop switch (WP-1.18).
        _ => control_bar::click(element, shell),
    }
}

/// The command that changes the scale the Control Bar shows: the selected
/// clips' own scales, or the set's current scale when no clip is selected
/// (the scales spec, R12).
fn scale_command(shell: &Shell, change: &[(&str, serde_json::Value)]) -> Invocation {
    let targets = shell.scale_targets();
    let mut args = serde_json::Map::new();
    if !targets.is_empty() {
        args.insert("clips".to_owned(), json!(targets));
    }
    for (key, value) in change {
        args.insert((*key).to_owned(), value.clone());
    }
    let id = if targets.is_empty() {
        bl_shell::scales_commands::SET_SCALE
    } else {
        bl_shell::scales_commands::SET_CLIP_SCALE
    };
    Invocation::new(id, serde_json::Value::Object(args))
}

/// The Root Note chooser's list: the twelve pitch classes, from C (the
/// scales spec, R9).
fn root_note_menu(ui: &mut egui::Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
    let display = shell.scale_display();
    for root in 0..12u8 {
        let item = ui.selectable_label(display.root == Some(root), shell.root_name(root));
        if item.clicked() {
            events.push(UiEvent::Invoke(scale_command(
                shell,
                &[("root", json!(root))],
            )));
        }
    }
}

/// The Scale Name chooser's list: the scale list, in its order (the scales
/// spec, R1, R9).
fn scale_name_menu(ui: &mut egui::Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
    let display = shell.scale_display();
    for name in bl_model::ScaleName::ALL {
        let item = ui.selectable_label(display.name == Some(name), name.label());
        if item.clicked() {
            events.push(UiEvent::Invoke(scale_command(
                shell,
                &[("name", json!(name.id()))],
            )));
        }
    }
}

/// Whether an element is one the person can operate now (not pending).
#[must_use]
pub fn is_live(element: Element) -> bool {
    let placement = crate::layout::placement(element);
    placement.pending.is_none() && !matches!(placement.kind, Kind::Indicator | Kind::Meter)
}

/// The menu bar to draw, where there's no native one.
#[derive(Clone, Copy, Debug)]
pub struct DrawnMenuBar<'a> {
    /// The menus.
    pub menus: &'a [bl_shell::menus::Menu],
    /// The keymap, for the keys each item shows.
    pub keymap: &'a crate::keymap::Keymap,
    /// Whose keys to show.
    pub platform: crate::keymap::Platform,
}

/// The names of the chains in `device`'s rack and the racks inside it.
fn collect_chain_names(
    device: &bl_shell::device_commands::DeviceFrame,
    names: &mut Vec<(bl_model::ChainId, String)>,
) {
    if let Some(rack) = &device.rack {
        names.extend(rack.chains.iter().map(|c| (c.id, c.name.clone())));
        for child in &rack.devices {
            collect_chain_names(child, names);
        }
    }
}

/// The titles of `device` and the devices inside its rack, at any depth.
fn collect_titles(
    device: &bl_shell::device_commands::DeviceFrame,
    titles: &mut Vec<(bl_model::DeviceId, String)>,
) {
    titles.push((device.id, device.title.clone()));
    for child in device.rack.iter().flat_map(|rack| &rack.devices) {
        collect_titles(child, titles);
    }
}

/// The Device View selector's list: every device on the selected track as a
/// hierarchy, each rack's devices under it chain by chain; choosing one
/// selects it (racks spec, R25).
fn device_list(ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
    let list = shell.device_view_state().device_list;
    if list.is_empty() {
        ui.add_enabled(false, egui::Button::new("No devices"));
    }
    for entry in list {
        ui.horizontal(|ui| {
            ui.add_space(entry.depth as f32 * 12.0);
            let response = ui.button(&entry.label);
            crate::menus::register_item(ui.ctx(), &response);
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description(format!("Level {}", entry.depth + 1));
            });
            if response.clicked() {
                let invocation = match (entry.device, entry.chain) {
                    (Some(device), _) => {
                        Invocation::new("device.select", json!({ "device": device }))
                    }
                    (None, Some(chain)) => Invocation::new(
                        "chain.select",
                        json!({ "chain": chain, "mode": "replace" }),
                    ),
                    (None, None) => return,
                };
                events.push(UiEvent::Invoke(invocation));
                ui.close();
            }
        });
    }
}
