//! The browser (WP-1.10): the search row, the sidebar's Collections,
//! Library, and Places, the content pane's list, and the preview strip,
//! placed by the layout layer's [`BrowserLayout`] (layout.md, L-BRW rows);
//! and dragging files from it into the Arrangement.
//!
//! Like the Arrangement View, it holds no state of its own beyond scrolling
//! and the search field's text while it's typed: what it shows comes from
//! the shell's `bl_shell::browser::BrowserView`, and every click, key, and
//! drop becomes one of the browser's commands, which the host runs. The
//! list builds only the rows on screen, so a library of 100,000 files draws
//! as quickly as a short one.
//!
//! The behaviour is in the browser spec.

use std::path::Path;

use bl_core::time::Ticks;
use bl_model::ClipId;
use bl_shell::browser::drop::{DropTarget, drop_target_on};
use bl_shell::browser::listing::file_kind;
use bl_shell::browser_commands::{
    self as ids, ClipFileKind, Item, ItemKind, Label, LabelInfo, Part, Section, TagEditorRow,
    TagEditorRowKind, TagState,
};
use bl_shell::{Area, Invocation, MainView, Shell, WindowKind};
use egui::accesskit::Role;
use egui::{
    Align2, Color32, CursorIcon, DragAndDrop, EventFilter, Id, Key, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::layout::{ArrangementLayout, BROWSER_ROW, BrowserLayout};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The part of the browser a focus target is, as `focus.browser` names
/// it: the keyboard moves between the parts through that command, so the
/// shell knows which part's keys apply (the accessibility spec, R3).
#[must_use]
pub fn part_of(id: Id) -> Option<&'static str> {
    if id == search_id() {
        Some("search")
    } else if id == sidebar_id() {
        Some("sidebar")
    } else if id == list_id() {
        Some("content")
    } else {
        None
    }
}

/// The search field's ID.
#[must_use]
pub fn search_id() -> Id {
    Id::new("browser-search")
}

/// The content pane's list, which takes the keyboard focus.
#[must_use]
pub fn list_id() -> Id {
    Id::new("browser-list")
}

/// The sidebar, which takes the keyboard focus.
#[must_use]
pub fn sidebar_id() -> Id {
    Id::new("browser-sidebar")
}

/// The Preview switch.
#[must_use]
pub fn preview_toggle_id() -> Id {
    Id::new("browser-preview-toggle")
}

/// The Raw switch in the preview strip (WP-4.18; spec: browser R18).
#[must_use]
pub fn raw_toggle_id() -> Id {
    Id::new("browser-raw-toggle")
}

/// The preview strip.
#[must_use]
pub fn preview_strip_id() -> Id {
    Id::new("browser-preview-strip")
}

/// A row of the content pane, by its item's path.
#[must_use]
pub fn row_id(path: &str) -> Id {
    Id::new(("browser-row", path))
}

/// A sidebar label, by its label.
#[must_use]
pub fn label_id(label: &Label) -> Id {
    Id::new(("browser-label", label))
}

/// Add Folder..., the sidebar's last item.
#[must_use]
pub fn add_folder_id() -> Id {
    Id::new("browser-add-folder")
}

/// The Quick Tags section, at the content pane's bottom (L-BRW-012).
#[must_use]
pub fn quick_tags_id() -> Id {
    Id::new("browser-quick-tags")
}

/// Quick Tags' Add field.
#[must_use]
pub fn quick_tags_add_id() -> Id {
    Id::new("browser-quick-tags-add")
}

/// The Tag Editor, beside the content pane (L-BRW-013).
#[must_use]
pub fn tag_editor_id() -> Id {
    Id::new("browser-tag-editor")
}

/// The Tag Editor's field, where a name is typed.
#[must_use]
pub fn tag_editor_field_id() -> Id {
    Id::new("browser-tag-editor-field")
}

/// The Tag Editor's list, whose background has the context menu.
#[must_use]
pub fn tag_editor_list_id() -> Id {
    Id::new("browser-tag-editor-list")
}

/// A row of the Tag Editor, by the tag's full name.
#[must_use]
pub fn tag_editor_row_id(full_name: &str) -> Id {
    Id::new(("browser-tag-row", full_name))
}

/// A tag in Quick Tags, by its full name.
#[must_use]
pub fn quick_tag_id(full_name: &str) -> Id {
    Id::new(("browser-quick-tag", full_name))
}

/// The X that takes a tag off the selected items.
#[must_use]
pub fn quick_tag_x_id(full_name: &str) -> Id {
    Id::new(("browser-quick-tag-x", full_name))
}

/// What a drag from the browser carries: the files' paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserDrag {
    /// The audio files dragged, in the list's order.
    pub paths: Vec<String>,
    /// The plug-in dragged, by its Plug-Ins row's key (WP-2.9b): dropped
    /// on a track, it loads there.
    pub plugin: Option<String>,
}

/// Where a clip dragged out of the Arrangement or the Session View is saved
/// if it is released over the browser (clip files R3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipDropTarget {
    /// A folder's row, or a place's label (its folder).
    Folder(String),
    /// The Clips label: the User Library's `Clips` folder.
    Clips,
}

/// A save that waits for the person's answer to Collect Files on Export's
/// Ask (file management R8).
#[derive(Clone, Debug, PartialEq, Eq)]
struct AskCopy {
    clip: ClipId,
    folder: Option<String>,
}

fn clip_drag_id() -> Id {
    Id::new("browser-clip-drag")
}

fn clip_target_id() -> Id {
    Id::new("browser-clip-target")
}

fn ask_copy_id() -> Id {
    Id::new("browser-ask-copy")
}

/// A clip is being dragged out of the Arrangement or the Session View: the
/// browser notes which folder row or label is under the pointer, for
/// [`end_clip_drag`] to read.
pub fn begin_clip_drag(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.insert_temp(clip_drag_id(), true);
        data.insert_temp(clip_target_id(), None::<ClipDropTarget>);
    });
}

/// The clip drag ended: where the pointer was over the browser, if it was.
pub fn end_clip_drag(ctx: &egui::Context) -> Option<ClipDropTarget> {
    ctx.data_mut(|data| {
        data.remove::<bool>(clip_drag_id());
        data.get_temp::<Option<ClipDropTarget>>(clip_target_id())
            .flatten()
    })
}

/// Saves `clip` as a clip file at `target` (clip files R3): at once, or, with
/// Collect Files on Export on Ask and audio to copy, once the person has
/// answered.
pub fn save_clip(
    ctx: &egui::Context,
    shell: &Shell,
    clip: ClipId,
    target: &ClipDropTarget,
    events: &mut Vec<UiEvent>,
) {
    let folder = match target {
        ClipDropTarget::Folder(path) => Some(path.clone()),
        ClipDropTarget::Clips => None,
    };
    if shell.clip_save_asks(clip, folder.as_deref().map(Path::new)) {
        ctx.data_mut(|data| data.insert_temp(ask_copy_id(), AskCopy { clip, folder }));
        return;
    }
    let mut args = json!({ "clip": clip });
    if let Some(folder) = folder {
        args["folder"] = json!(folder);
    }
    invoke(events, "clip.save_file", args);
}

/// While a clip is dragged, outlines `rect` and notes `target` when the
/// pointer is over it.
fn note_clip_target(ui: &Ui, theme: &Theme, rect: Rect, target: ClipDropTarget) {
    let ctx = ui.ctx();
    if !ctx
        .data(|data| data.get_temp::<bool>(clip_drag_id()))
        .unwrap_or(false)
    {
        return;
    }
    let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    if !rect.contains(pointer) || !ui.clip_rect().contains(pointer) {
        return;
    }
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(2.0, theme.colors.accent.0),
        StrokeKind::Inside,
    );
    ctx.data_mut(|data| data.insert_temp(clip_target_id(), Some(target)));
}

/// What a drag of presets or devices from the browser carries (the racks
/// spec, R43): preset files' paths and the IDs of devices the labels list,
/// for the Device View to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicesDrag {
    /// The presets and devices dragged, in the list's order.
    pub paths: Vec<String>,
}

/// What the last frame built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserStats {
    /// Rows of the content pane drawn.
    pub rows_built: usize,
    /// Rows the content pane has in all.
    pub rows_total: usize,
}

/// One row of the sidebar.
#[derive(Clone, Debug, PartialEq)]
enum SideRow {
    Heading(&'static str),
    /// A collection label (M6): its name and colour's palette index, or
    /// `None` for Favorites.
    Collection(&'static str, Option<usize>),
    Label(LabelInfo),
    AddFolder,
}

/// The collection labels, in Live's order (L-BRW-003), with Beatlayer's own
/// colours from the track palette.
const COLLECTIONS: [(&str, Option<usize>); 7] = [
    ("Favorites", None),
    ("Orange", Some(1)),
    ("Yellow", Some(2)),
    ("Green", Some(4)),
    ("Blue", Some(7)),
    ("Purple", Some(9)),
    ("Gray", Some(12)),
];

/// What a drag in the Tag Editor carries: a tag or a group, by its name
/// (spec: browser-tags R12).
#[derive(Clone, Debug)]
enum TagDrag {
    /// A tag, by its full name.
    Tag(String),
    /// A group, by its name.
    Group(String),
}

/// What the Tag Editor is asking the person to type.
#[derive(Clone, Debug, Default, PartialEq)]
enum TagPrompt {
    /// Nothing.
    #[default]
    None,
    /// A new tag in a group.
    AddTag { group: String },
    /// A new group.
    AddGroup,
    /// A new name for a tag.
    RenameTag { tag: String },
    /// A new name for a group.
    RenameGroup { group: String },
    /// A new subtag under a tag.
    AddSubtag { parent: String },
}

/// The browser's state between frames: scrolling and the search field's
/// text, never browser state. See the [module docs](self).
#[derive(Debug, Default)]
pub struct BrowserUi {
    list_scroll: f32,
    sidebar_scroll: f32,
    search: String,
    /// The shell's search text as this last saw or sent it.
    search_seen: String,
    seen_focus_requests: u64,
    seen_cursor: Option<usize>,
    /// Where a Shift-extended selection started.
    anchor: Option<usize>,
    stats: BrowserStats,
    /// The Tag Editor's scroll.
    tag_scroll: f32,
    /// The tag groups that are folded in the Tag Editor.
    folded_groups: std::collections::BTreeSet<String>,
    /// The tags that are folded in the Tag Editor, by their full names.
    folded_tags: std::collections::BTreeSet<String>,
    /// What the Tag Editor is asking the person to type.
    tag_prompt: TagPrompt,
    /// The text in the Tag Editor's field.
    tag_text: String,
    /// The Tag Editor's row the keyboard is on (spec: browser-tags R17).
    tag_row: usize,
    /// Quick Tags' text in the Add field.
    quick_add: String,
    /// Whether Quick Tags is choosing where a new tag goes.
    quick_adding: bool,
    /// The group the Status Bar shows for the Quick Tag under the pointer
    /// (spec: browser-tags R13).
    quick_tag_hint: Option<String>,
    /// The text of the name being edited after a clip was saved (clip files
    /// R4), and the file it is for.
    rename: String,
    rename_for: Option<String>,
}

/// The text after the last `#` in the search field, when a `#` term is
/// being typed (spec: browser-tags R15).
fn search_tag_prefix(search: &str) -> Option<&str> {
    let hash = search.rfind('#')?;
    let rest = &search[hash + 1..];
    if rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

/// Whether the platform's command key is held: Cmd on macOS, Ctrl
/// elsewhere.
fn command(modifiers: egui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.mac_cmd
    } else {
        modifiers.ctrl
    }
}

impl BrowserUi {
    /// A browser scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What the last frame built.
    #[must_use]
    pub fn stats(&self) -> BrowserStats {
        self.stats
    }

    /// Draws the browser in its column and adds what the person did to
    /// `events`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        column: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        // Spec: clip files R3. While a clip is dragged, the rows and labels
        // under the pointer say where it would be saved.
        if ui
            .ctx()
            .data(|data| data.get_temp::<bool>(clip_drag_id()))
            .unwrap_or(false)
        {
            ui.ctx()
                .data_mut(|data| data.insert_temp(clip_target_id(), None::<ClipDropTarget>));
        }
        self.ask_copy_dialog(ui.ctx(), theme, events);
        let layout = BrowserLayout::compute_with(column, view.quick_tags_shown());
        let region = Id::new("region-browser");
        let _ = ui.interact(column, region, Sense::hover());
        ui.ctx().accesskit_node_builder(region, |node| {
            node.set_role(Role::Group);
            node.set_label("Browser".to_owned());
            node.set_bounds(bounds(column));
        });
        ui.painter().rect_filled(column, 0.0, theme.colors.panel.0);

        // A command moved the focus to a part of the browser.
        if view.focus_requests() != self.seen_focus_requests {
            self.seen_focus_requests = view.focus_requests();
            let target = match view.focus() {
                Part::Search => search_id(),
                Part::Sidebar => sidebar_id(),
                Part::Content => list_id(),
            };
            // Asking again would reset the arrows' lock filter.
            ui.memory_mut(|m| {
                if !m.has_focus(target) {
                    m.request_focus(target);
                }
            });
        }

        // Everything inside hangs from the browser's node for assistive
        // technology.
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(column)
                .accessibility_parent(region),
            |ui| {
                self.top_row(ui, shell, theme, &layout, events);
                self.sidebar(ui, shell, theme, &layout, events);
                self.content(ui, shell, theme, &layout, events);
                if view.quick_tags_shown() {
                    self.quick_tags(ui, shell, theme, layout.quick_tags, events);
                }
                self.preview_strip(ui, shell, theme, &layout, events);
                self.keys(ui, shell, events);
            },
        );
        if view.index_state().indexing || view.preview_active() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn top_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        // Back, Forward, and the filters arrive with the browser's history
        // and tags (M6).
        for (rect, name, kind) in [
            (layout.back, "Browse Back", Glyph::Left),
            (layout.forward, "Browse Forward", Glyph::Right),
            (layout.filter_toggle, "Show Filter View", Glyph::Filter),
            (layout.filter_menu, "Filter View Menu", Glyph::Menu),
        ] {
            let id = Id::new(("browser-control", name));
            let response = ui.interact(rect, id, Sense::click());
            let painter = ui.painter_at(rect.expand(1.0));
            painter.rect_filled(
                rect,
                theme.sizes.corner,
                theme.colors.control.0.gamma_multiply(0.6),
            );
            glyph(&painter, rect, kind, theme.colors.pending.0);
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, false, name));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_description("Not built yet: arrives in milestone M6.".to_owned());
            });
            // The Filter View Menu (C-FILTER): the Tag Editor and Quick
            // Tags for now; the Filter View's groups come with WP-6.32.
            if name == "Filter View Menu" && response.clicked() {
                egui::Popup::menu(&response).show(|ui| {
                    self.filter_view_menu(ui, shell, events);
                });
            }
        }

        // The search field (S-BRW-006, S-BRW-007).
        let view = shell.browser_view();
        // The shell's search changed by other means (an agent, Cmd-F's All):
        // the field shows it.
        if view.search_text() != self.search_seen {
            self.search_seen = view.search_text().to_owned();
            self.search.clone_from(&self.search_seen);
        }
        let (down, enter, escape) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        let edit = egui::TextEdit::singleline(&mut self.search)
            .id(search_id())
            .hint_text("Search")
            .font(theme.small_font())
            .margin(vec2(4.0, 2.0));
        let response = ui.put(layout.search, edit);
        // Tab's order in the browser (the accessibility spec, R3;
        // I-FOCUS-007): the search field, the sidebar, then the list.
        crate::focus::register(ui.ctx(), response.id, bl_shell::Area::Browser, None);
        // Live's Info View calls it the Search Field (observed in Live
        // 12.4.6; the accessibility spec, R1).
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Search Field".to_owned())
        });
        if response.gained_focus() && view.focus() != Part::Search {
            invoke(events, ids::FOCUS_BROWSER, json!({ "part": "search" }));
        }
        if response.changed() {
            invoke(events, ids::SEARCH, json!({ "query": self.search }));
            self.search_seen.clone_from(&self.search);
        }
        // A `#` term suggests the tags it names (spec: browser-tags R15).
        if let Some(prefix) = search_tag_prefix(&self.search) {
            let suggestions = view.tag_suggestions(prefix);
            if !suggestions.is_empty() {
                let rect = Rect::from_min_size(
                    pos2(layout.search.left(), layout.search.bottom() + 2.0),
                    vec2(
                        layout.search.width(),
                        suggestions.len() as f32 * BROWSER_ROW,
                    ),
                );
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, theme.colors.panel.0);
                painter.line_segment(
                    [rect.left_top(), rect.right_top()],
                    Stroke::new(1.0, theme.colors.separator.0),
                );
                for (index, suggestion) in suggestions.iter().enumerate() {
                    let row = Rect::from_min_size(
                        pos2(rect.left(), rect.top() + index as f32 * BROWSER_ROW),
                        vec2(rect.width(), BROWSER_ROW),
                    );
                    let id = Id::new(("browser-search-suggestion", suggestion.clone()));
                    let response = ui.interact(row, id, Sense::click());
                    painter.text(
                        pos2(row.left() + 6.0, row.center().y),
                        Align2::LEFT_CENTER,
                        suggestion,
                        theme.small_font(),
                        theme.colors.text.0,
                    );
                    if response.clicked() {
                        self.search = format!("#{suggestion}");
                        self.search_seen.clone_from(&self.search);
                        invoke(events, ids::SEARCH, json!({ "query": self.search }));
                    }
                }
            }
        }
        if response.has_focus() && down {
            // Down moves to the results.
            response.surrender_focus();
            self.go_to_results(shell, events);
        } else if response.lost_focus() && enter {
            self.go_to_results(shell, events);
        } else if response.lost_focus() && escape && !self.search.is_empty() {
            self.search.clear();
            self.search_seen.clear();
            invoke(events, ids::SEARCH, json!({ "query": "" }));
        }
    }

    /// The Filter View Menu (C-FILTER): the Tag Editor and Quick Tags for
    /// now; the Filter View's groups come with WP-6.32.
    fn filter_view_menu(&mut self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let view = shell.browser_view();
        if ui
            .selectable_label(view.tag_editor(), "Tag Editor")
            .clicked()
        {
            invoke(
                events,
                ids::SHOW_TAG_EDITOR,
                json!({ "show": !view.tag_editor() }),
            );
            ui.close();
        }
        if ui
            .selectable_label(view.quick_tags_shown(), "Show Quick Tags")
            .clicked()
        {
            invoke(
                events,
                ids::SHOW_QUICK_TAGS,
                json!({ "show": !view.quick_tags_shown() }),
            );
            ui.close();
        }
    }

    /// Moves the focus from the search field to the first result.
    fn go_to_results(&mut self, shell: &Shell, events: &mut Vec<UiEvent>) {
        invoke(events, ids::FOCUS_BROWSER, json!({ "part": "content" }));
        if let Some(first) = shell.browser_view().row(0) {
            self.anchor = Some(0);
            invoke(events, ids::SELECT, json!({ "paths": [first.path] }));
        }
    }

    fn side_rows(shell: &Shell) -> Vec<SideRow> {
        let view = shell.browser_view();
        let labels = view.labels();
        let mut rows = vec![SideRow::Heading("Collections")];
        rows.extend(
            COLLECTIONS
                .iter()
                .map(|(name, colour)| SideRow::Collection(name, *colour)),
        );
        rows.push(SideRow::Heading("Library"));
        rows.extend(
            labels
                .iter()
                .filter(|l| l.section == Section::Library)
                .cloned()
                .map(SideRow::Label),
        );
        rows.push(SideRow::Heading("Places"));
        rows.extend(
            labels
                .into_iter()
                .filter(|l| l.section == Section::Places)
                .map(SideRow::Label),
        );
        rows.push(SideRow::AddFolder);
        rows
    }

    fn sidebar(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let area = layout.sidebar;
        let view = shell.browser_view();
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, theme.colors.bar.0);
        let region = ui.interact(area, sidebar_id(), Sense::click());
        crate::focus::register(ui.ctx(), sidebar_id(), bl_shell::Area::Browser, None);
        ui.ctx().accesskit_node_builder(sidebar_id(), |node| {
            node.set_role(Role::List);
            node.set_label("Browser Sidebar".to_owned());
        });
        if region.clicked() {
            region.request_focus();
        }
        let rows = Self::side_rows(shell);
        let content = rows.len() as f32 * BROWSER_ROW;
        self.sidebar_scroll = scrolled(ui, area, self.sidebar_scroll, content);
        // Spec: view-options R1. A middle-button drag pans the list, the
        // content following the pointer.
        if let Some(delta) = crate::middle::pan(ui, area) {
            self.sidebar_scroll =
                (self.sidebar_scroll - delta.y).clamp(0.0, (content - area.height()).max(0.0));
        }
        let first = (self.sidebar_scroll / BROWSER_ROW).floor() as usize;
        let mut sidebar_ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
        sidebar_ui.set_clip_rect(area);
        for (index, row) in rows.iter().enumerate().skip(first) {
            let top = area.top() + index as f32 * BROWSER_ROW - self.sidebar_scroll;
            if top > area.bottom() {
                break;
            }
            let rect = Rect::from_min_size(pos2(area.left(), top), vec2(area.width(), BROWSER_ROW));
            match row {
                SideRow::Heading(name) => {
                    let mut text = (*name).to_owned();
                    if *name == "Places" && view.index_state().indexing {
                        text.push_str(" \u{b7} indexing");
                    }
                    painter.text(
                        pos2(rect.left() + 4.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        text,
                        theme.small_font(),
                        theme.colors.text_dim.0,
                    );
                    let id = Id::new(("browser-heading", *name));
                    let _ = sidebar_ui.interact(rect, id, Sense::hover());
                    ui.ctx().accesskit_node_builder(id, |node| {
                        node.set_role(Role::Heading);
                        node.set_label((*name).to_owned());
                        if *name == "Places" && view.index_state().indexing {
                            node.set_description("Indexing the places' files".to_owned());
                        }
                    });
                }
                SideRow::Collection(name, colour) => {
                    let id = Id::new(("browser-collection", *name));
                    let response = sidebar_ui.interact(rect, id, Sense::hover());
                    let dot = colour.map_or(theme.colors.accent.0, |i| {
                        theme.colors.track_palette[i % theme.colors.track_palette.len()].0
                    });
                    painter.circle_filled(pos2(rect.left() + 10.0, rect.center().y), 3.5, dot);
                    painter.text(
                        pos2(rect.left() + 18.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        *name,
                        theme.small_font(),
                        theme.colors.pending.0,
                    );
                    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, false, *name));
                    ui.ctx().accesskit_node_builder(id, |node| {
                        node.set_role(Role::ListItem);
                        node.set_description("Collections arrive in milestone M6.".to_owned());
                    });
                }
                SideRow::Label(info) => {
                    self.label_row(&mut sidebar_ui, shell, theme, rect, info, events);
                }
                SideRow::AddFolder => {
                    let response = sidebar_ui.interact(rect, add_folder_id(), Sense::click());
                    if response.hovered() {
                        painter.rect_filled(rect, 0.0, theme.colors.control_hover.0);
                    }
                    painter.text(
                        pos2(rect.left() + 8.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        "Add Folder\u{2026}",
                        theme.small_font(),
                        theme.colors.text.0,
                    );
                    if response.clicked() {
                        invoke(events, ids::ADD_FOLDER, json!({}));
                    }
                    response.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::Button, true, "Add Folder\u{2026}")
                    });
                }
            }
        }
        // Spec: accessibility R7: outlined only with Outline View in Focus.
        if ui.memory(|m| m.has_focus(sidebar_id())) && crate::focus::outlined(ui.ctx()) {
            ui.painter().rect_stroke(
                area.shrink(1.0),
                0.0,
                Stroke::new(1.0, theme.colors.focus.0),
                StrokeKind::Inside,
            );
        }
    }

    fn label_row(
        &self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        info: &LabelInfo,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let id = label_id(&info.label);
        let response = ui.interact(rect, id, Sense::CLICK);
        let chosen = *view.label() == info.label;
        let painter = ui.painter_at(rect);
        if chosen {
            painter.rect_filled(rect, 0.0, theme.colors.lane_selected.0);
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, theme.colors.control_hover.0);
        }
        let colour = if info.available {
            theme.colors.text.0
        } else {
            theme.colors.pending.0
        };
        painter.text(
            pos2(rect.left() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            &info.name,
            theme.small_font(),
            colour,
        );
        if response.clicked() {
            if view.focus() != Part::Sidebar || shell.focused_area() != Area::Browser {
                invoke(events, ids::FOCUS_BROWSER, json!({ "part": "sidebar" }));
            }
            invoke(events, ids::OPEN_PLACE, json!({ "label": info.label }));
        }
        // Spec: racks R46. A chain dropped on a place is saved there; on the
        // User Library, in its folder for the rack's presets.
        match (&info.label, &info.path) {
            (Label::UserLibrary, _) => drop_chains(ui, theme, rect, "", events),
            (label, Some(path)) if label.is_place() => {
                drop_chains(ui, theme, rect, path, events);
            }
            _ => {}
        }
        // Spec: clip files R3. A clip dropped on a place's label is saved in
        // its folder, and on the Clips label in the User Library's `Clips`.
        match (&info.label, &info.path) {
            (Label::Clips, _) => note_clip_target(ui, theme, rect, ClipDropTarget::Clips),
            (label, Some(path)) if label.is_place() => {
                note_clip_target(ui, theme, rect, ClipDropTarget::Folder(path.clone()));
            }
            _ => {}
        }
        if let Label::Folder(path) = &info.label {
            // C-BROWSER-017: a folder the person added.
            let menu = |ui: &mut Ui, events: &mut Vec<UiEvent>| {
                let response = ui.button("Remove from Sidebar");
                crate::menus::register_item(ui.ctx(), &response);
                crate::menus::take_focus_first(ui, &response);
                if response.clicked() {
                    invoke(events, ids::REMOVE_FOLDER, json!({ "path": path }));
                    ui.close();
                }
            };
            response.context_menu(|ui| menu(ui, events));
            // S-CTX-002: the keys open the chosen place's menu while the
            // sidebar has the keyboard (the accessibility spec, R8).
            if chosen
                && ui.memory(|m| m.has_focus(sidebar_id()))
                && crate::menus::context_menu_keys(ui)
            {
                crate::menus::open_keyboard_context_menu(ui.ctx(), id, rect.left_bottom());
            }
            crate::menus::keyboard_context_menu(&response, |ui| menu(ui, events));
        }
        response.widget_info(|| WidgetInfo::selected(WidgetType::Other, true, chosen, &info.name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::ListItem);
            node.set_label(info.name.clone());
            node.set_selected(chosen);
            if let Some(path) = &info.path {
                node.set_description(path.clone());
            } else if let Some(note) = info.label.pending() {
                node.set_description(note.to_owned());
            }
        });
    }

    fn content(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        // The column header row (L-BRW-009): Name.
        let header = layout.header;
        let painter = ui.painter_at(layout.content);
        painter.rect_filled(header, 0.0, theme.colors.ruler.0);
        painter.text(
            pos2(header.left() + 6.0, header.center().y),
            Align2::LEFT_CENTER,
            "Name",
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        painter.line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        painter.line_segment(
            [layout.content.left_top(), layout.content.left_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        let header_id = Id::new("browser-column-name");
        let _ = ui.interact(header, header_id, Sense::hover());
        ui.ctx().accesskit_node_builder(header_id, |node| {
            node.set_role(Role::ColumnHeader);
            node.set_label("Name".to_owned());
            node.set_bounds(bounds(header));
        });

        let list = layout.list;
        let region = ui.interact(list, list_id(), Sense::click());
        crate::focus::register(ui.ctx(), list_id(), bl_shell::Area::Browser, None);
        ui.ctx().accesskit_node_builder(list_id(), |node| {
            node.set_role(Role::List);
            node.set_label("Browser Items".to_owned());
            if let Some(note) = view.note() {
                node.set_description(note.to_owned());
            }
        });
        if region.clicked() {
            region.request_focus();
            invoke(events, ids::SELECT, json!({ "paths": [] }));
        }
        let total = view.row_count();
        // Keep the row the keys moved to in view.
        if view.cursor() != self.seen_cursor {
            self.seen_cursor = view.cursor();
            if let Some(cursor) = view.cursor() {
                let top = cursor as f32 * BROWSER_ROW;
                if top < self.list_scroll {
                    self.list_scroll = top;
                } else if top + BROWSER_ROW > self.list_scroll + list.height() {
                    self.list_scroll = top + BROWSER_ROW - list.height();
                }
            }
        }
        self.list_scroll = scrolled(ui, list, self.list_scroll, total as f32 * BROWSER_ROW);
        // Spec: view-options R1. A middle-button drag pans the list, the
        // content following the pointer.
        if let Some(delta) = crate::middle::pan(ui, list) {
            self.list_scroll = (self.list_scroll - delta.y)
                .clamp(0.0, (total as f32 * BROWSER_ROW - list.height()).max(0.0));
        }
        let first = (self.list_scroll / BROWSER_ROW).floor() as usize;
        let shown =
            ((list.height() / BROWSER_ROW).ceil() as usize + 1).min(total.saturating_sub(first));
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        list_ui.set_clip_rect(list);
        let modifiers = ui.input(|i| i.modifiers);
        for index in first..first + shown {
            let Some(item) = view.row(index) else {
                break;
            };
            let top = list.top() + index as f32 * BROWSER_ROW - self.list_scroll;
            let rect = Rect::from_min_size(pos2(list.left(), top), vec2(list.width(), BROWSER_ROW));
            self.item_row(
                &mut list_ui,
                shell,
                theme,
                rect,
                index,
                &item,
                modifiers,
                events,
            );
        }
        self.stats = BrowserStats {
            rows_built: shown,
            rows_total: total,
        };
        if total == 0
            && let Some(note) = view.note()
        {
            let galley = painter.layout(
                note.to_owned(),
                theme.small_font(),
                theme.colors.pending.0,
                list.width() - 16.0,
            );
            painter.galley(
                pos2(list.left() + 8.0, list.top() + 8.0),
                galley,
                theme.colors.pending.0,
            );
        }
        // Spec: accessibility R7: outlined only with Outline View in Focus.
        if ui.memory(|m| m.has_focus(list_id())) && crate::focus::outlined(ui.ctx()) {
            ui.painter().rect_stroke(
                list.shrink(1.0),
                0.0,
                Stroke::new(1.0, theme.colors.focus.0),
                StrokeKind::Inside,
            );
        }
    }

    /// The Tag Editor (L-BRW-013; spec: browser-tags R10 to R12): the tag
    /// groups, their tags, and the check boxes that put them on the
    /// selected items.
    pub fn tag_editor(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.colors.panel.0);
        let _ = ui.interact(rect, tag_editor_id(), Sense::hover());
        ui.ctx().accesskit_node_builder(tag_editor_id(), |node| {
            node.set_role(Role::Group);
            node.set_label("Tag Editor".to_owned());
            node.set_bounds(bounds(rect));
        });
        // The close button at the top right (L-BRW-013).
        let close = Rect::from_center_size(
            pos2(rect.right() - 12.0, rect.top() + 12.0),
            vec2(14.0, 14.0),
        );
        let response = ui.interact(close, Id::new("browser-tag-editor-close"), Sense::click());
        painter.text(
            pos2(close.center().x, close.center().y),
            Align2::CENTER_CENTER,
            "\u{d7}",
            theme.small_font(),
            theme.colors.text.0,
        );
        if response.clicked() {
            invoke(events, ids::SHOW_TAG_EDITOR, json!({ "show": false }));
        }
        let selected: Vec<String> = view
            .selected()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let rows = view.tag_editor_rows();
        let list = Rect::from_min_max(
            pos2(rect.left(), rect.top() + 22.0),
            pos2(rect.right(), rect.bottom() - 24.0),
        );
        let visible = self.visible_tag_rows(&rows);
        // The list's own background takes the context menu (Reset Order).
        // Register it before the rows, so a row on top of it takes its own
        // clicks (spec: browser-tags R11, R12).
        let list_response = ui.interact(list, tag_editor_list_id(), Sense::click());
        crate::focus::register(
            ui.ctx(),
            tag_editor_list_id(),
            bl_shell::Area::Browser,
            None,
        );
        if list_response.clicked() {
            ui.memory_mut(|m| m.request_focus(tag_editor_list_id()));
        }
        self.tag_scroll = scrolled(
            ui,
            list,
            self.tag_scroll,
            visible.len() as f32 * BROWSER_ROW,
        );
        // Spec: view-options R1. A middle-button drag pans the list, the
        // content following the pointer.
        if let Some(delta) = crate::middle::pan(ui, list) {
            self.tag_scroll = (self.tag_scroll - delta.y).clamp(
                0.0,
                (visible.len() as f32 * BROWSER_ROW - list.height()).max(0.0),
            );
        }
        let first = (self.tag_scroll / BROWSER_ROW).floor() as usize;
        let shown = ((list.height() / BROWSER_ROW).ceil() as usize + 1)
            .min(visible.len().saturating_sub(first));
        // The keys the Tag Editor reads itself (spec: browser-tags R17):
        // Up and Down move through its rows, and Left and Right fold and
        // unfold them. (R17's Space, which ticks and unticks a tag, can't
        // be reached: Space runs Play/Stop in every context.)
        let tag_focused = ui.memory(|m| m.has_focus(tag_editor_list_id()));
        if tag_focused && !visible.is_empty() {
            self.tag_row = self.tag_row.min(visible.len() - 1);
            let (up, down, left, right) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowUp),
                    i.key_pressed(Key::ArrowDown),
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::ArrowRight),
                )
            });
            if up {
                self.tag_row = self.tag_row.saturating_sub(1);
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            if down {
                self.tag_row = (self.tag_row + 1).min(visible.len() - 1);
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            if left || right {
                // The arrows act here; egui mustn't move the focus with
                // them.
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            if let Some(row) = visible.get(self.tag_row) {
                let fold = |folded: &mut std::collections::BTreeSet<String>| {
                    if left {
                        folded.insert(row.name.clone());
                    } else {
                        folded.remove(&row.name);
                    }
                };
                match row.kind {
                    TagEditorRowKind::Group if left || right => fold(&mut self.folded_groups),
                    TagEditorRowKind::Tag if (left || right) && row.has_subtags => {
                        if let Some(name) = &row.full_name {
                            if left {
                                self.folded_tags.insert(name.clone());
                            } else {
                                self.folded_tags.remove(name);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // A drag moves a tag or a group to a new place (spec: browser-tags
        // R12).
        let dragging =
            egui::DragAndDrop::payload::<TagDrag>(ui.ctx()).map(|payload| (*payload).clone());
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or_default();
        let released = ui.input(|i| i.pointer.any_released());
        let mut menu_taken = false;
        for (index, row) in visible.iter().enumerate().skip(first).take(shown) {
            let row_rect = Rect::from_min_size(
                pos2(
                    list.left(),
                    list.top() + (index - first) as f32 * BROWSER_ROW - self.tag_scroll,
                ),
                vec2(list.width(), BROWSER_ROW),
            );
            let over = dragging.is_some() && row_rect.contains(pointer);
            if over {
                ui.painter().rect_stroke(
                    row_rect,
                    0.0,
                    Stroke::new(1.0, theme.colors.focus.0),
                    StrokeKind::Inside,
                );
            }
            // The row the keyboard is on shows an outline (spec:
            // browser-tags R17).
            if tag_focused && index == self.tag_row {
                ui.painter().rect_stroke(
                    row_rect,
                    0.0,
                    Stroke::new(1.0, theme.colors.focus.0),
                    StrokeKind::Inside,
                );
            }
            let clicked = self.tag_editor_row(
                ui,
                shell,
                theme,
                row_rect,
                row,
                &selected,
                &mut menu_taken,
                events,
            );
            // A click puts the keyboard's row where the pointer is (spec:
            // browser-tags R17).
            if clicked {
                self.tag_row = index;
            }
            if over
                && released
                && let Some(drag) = &dragging
            {
                let position = Self::tag_position(&visible, index);
                match drag {
                    TagDrag::Tag(name) => {
                        invoke(
                            events,
                            ids::MOVE_TAG,
                            json!({ "tag": name, "position": position }),
                        );
                    }
                    TagDrag::Group(name) => {
                        invoke(
                            events,
                            ids::MOVE_TAG,
                            json!({ "group": name, "position": position }),
                        );
                    }
                }
                egui::DragAndDrop::clear_payload(ui.ctx());
            }
        }
        // Reset Order, in the list's context menu (C-TAG-004).
        if list_response.secondary_clicked() && !menu_taken {
            egui::Popup::context_menu(&list_response).show(|ui| {
                let response = ui.button("Reset Order");
                crate::menus::register_item(ui.ctx(), &response);
                crate::menus::take_focus_first(ui, &response);
                if response.clicked() {
                    invoke(events, ids::RESET_TAG_ORDER, json!({}));
                    ui.close();
                }
            });
        }
        // The field, where a name is typed.
        let field = Rect::from_min_size(
            pos2(rect.left() + 2.0, rect.bottom() - 20.0),
            vec2(rect.width() - 4.0, 16.0),
        );
        let hint = match &self.tag_prompt {
            TagPrompt::None => "",
            TagPrompt::AddTag { .. } => "Tag name\u{2026}",
            TagPrompt::AddGroup => "Group name\u{2026}",
            TagPrompt::RenameTag { .. } | TagPrompt::RenameGroup { .. } => "New name\u{2026}",
            TagPrompt::AddSubtag { .. } => "Subtag name\u{2026}",
        };
        let edit = egui::TextEdit::singleline(&mut self.tag_text)
            .id(tag_editor_field_id())
            .hint_text(hint)
            .font(theme.small_font());
        let response = ui.put(field, edit);
        crate::focus::register(ui.ctx(), response.id, bl_shell::Area::Browser, None);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Tag Editor Field".to_owned())
        });
        if response.has_focus() {
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            let escape = ui.input(|i| i.key_pressed(Key::Escape));
            if enter {
                self.tag_prompt_apply(&selected, events);
            } else if escape {
                self.tag_prompt = TagPrompt::None;
                self.tag_text.clear();
            }
        }
    }

    /// Where a row of the Tag Editor sits in its group, so a tag dragged
    /// there goes to that place (spec: browser-tags R12).
    fn tag_position(rows: &[TagEditorRow], index: usize) -> i32 {
        let mut position = 0;
        for row in &rows[..index] {
            match row.kind {
                TagEditorRowKind::Group => position = 0,
                TagEditorRowKind::Tag if row.depth == 1 => position += 1,
                _ => {}
            }
        }
        position
    }

    /// The rows of the Tag Editor that are on screen: a folded group hides
    /// its tags, and a folded tag its subtags.
    fn visible_tag_rows(&self, rows: &[TagEditorRow]) -> Vec<TagEditorRow> {
        let mut visible: Vec<TagEditorRow> = Vec::new();
        let mut hidden_depth: Option<u16> = None;
        for row in rows {
            if let Some(depth) = hidden_depth {
                if row.depth > depth {
                    continue;
                }
                hidden_depth = None;
            }
            let folded = match row.kind {
                TagEditorRowKind::Group => self.folded_groups.contains(&row.name),
                TagEditorRowKind::Tag => row
                    .full_name
                    .as_ref()
                    .is_some_and(|name| self.folded_tags.contains(name)),
                _ => false,
            };
            visible.push(row.clone());
            if folded {
                hidden_depth = Some(row.depth);
            }
        }
        visible
    }

    /// One row of the Tag Editor.
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn tag_editor_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        row: &TagEditorRow,
        selected: &[String],
        menu_taken: &mut bool,
        events: &mut Vec<UiEvent>,
    ) -> bool {
        let painter = ui.painter_at(rect);
        let indent = rect.left() + 4.0 + f32::from(row.depth) * 12.0;
        let id = tag_editor_row_id(row.full_name.as_deref().unwrap_or(&row.name));
        let response = ui.interact(rect, id, Sense::click() | Sense::drag());
        let clicked = response.clicked();
        // A click puts the keyboard on the Tag Editor's rows (spec:
        // browser-tags R17).
        if clicked {
            ui.memory_mut(|m| m.request_focus(tag_editor_list_id()));
        }
        if response.drag_started() {
            let payload = match row.kind {
                TagEditorRowKind::Group => Some(TagDrag::Group(row.name.clone())),
                TagEditorRowKind::Tag => row.full_name.clone().map(TagDrag::Tag),
                _ => None,
            };
            if let Some(payload) = payload {
                egui::DragAndDrop::set_payload(ui.ctx(), payload);
            }
        }
        match row.kind {
            TagEditorRowKind::Group | TagEditorRowKind::Tag => {
                // The fold triangle.
                let fold =
                    Rect::from_center_size(pos2(indent + 4.0, rect.center().y), vec2(8.0, 8.0));
                let folded = match row.kind {
                    TagEditorRowKind::Group => self.folded_groups.contains(&row.name),
                    _ => row
                        .full_name
                        .as_ref()
                        .is_some_and(|name| self.folded_tags.contains(name)),
                };
                if row.kind == TagEditorRowKind::Group || row.has_subtags {
                    glyph(
                        &painter,
                        fold,
                        if folded { Glyph::Right } else { Glyph::Down },
                        theme.colors.text_dim.0,
                    );
                }
                let mut name_left = indent + 12.0;
                if row.kind == TagEditorRowKind::Tag {
                    // The check box (spec: browser-tags R11).
                    let box_rect =
                        Rect::from_center_size(pos2(indent + 4.0, rect.center().y), vec2(9.0, 9.0));
                    let mark = match row.state {
                        TagState::All => "\u{2713}",
                        TagState::Some => "\u{2013}",
                        TagState::Fixed => "\u{25cf}",
                        TagState::None => "",
                    };
                    if !mark.is_empty() {
                        painter.text(
                            pos2(box_rect.center().x, box_rect.center().y),
                            Align2::CENTER_CENTER,
                            mark,
                            theme.small_font(),
                            theme.colors.text.0,
                        );
                    }
                    painter.rect_stroke(
                        box_rect,
                        0.0,
                        Stroke::new(1.0, theme.colors.text_dim.0),
                        StrokeKind::Inside,
                    );
                    if response.clicked() {
                        let attach = row.state != TagState::All;
                        if let Some(name) = &row.full_name {
                            invoke(
                                events,
                                ids::TAG_ITEMS,
                                json!({
                                    "items": selected,
                                    "tags": [name],
                                    "attach": attach,
                                }),
                            );
                        }
                    }
                    name_left = indent + 12.0;
                }
                let colour = if row.marked && row.builtin {
                    theme.colors.text.0
                } else {
                    theme.colors.text.0.gamma_multiply(0.92)
                };
                painter.text(
                    pos2(name_left, rect.center().y),
                    Align2::LEFT_CENTER,
                    &row.name,
                    theme.small_font(),
                    colour,
                );
                if row.kind == TagEditorRowKind::Group {
                    // The group folds and unfolds on a click.
                    if response.clicked() {
                        if folded {
                            self.folded_groups.remove(&row.name);
                        } else {
                            self.folded_groups.insert(row.name.clone());
                        }
                    }
                }
                // The context menu: the person's own tags and groups can be
                // renamed and deleted (C-TAG).
                if !row.builtin {
                    let mut menu = |ui: &mut Ui, events: &mut Vec<UiEvent>| {
                        if row.kind == TagEditorRowKind::Tag
                            && let Some(name) = &row.full_name
                        {
                            let response = ui.button("Add Subtag In\u{2026}");
                            crate::menus::register_item(ui.ctx(), &response);
                            crate::menus::take_focus_first(ui, &response);
                            if response.clicked() {
                                self.tag_prompt = TagPrompt::AddSubtag {
                                    parent: name.clone(),
                                };
                                self.tag_text.clear();
                                ui.close();
                            }
                        }
                        let response = ui.button("Rename");
                        crate::menus::register_item(ui.ctx(), &response);
                        if response.clicked() {
                            self.tag_prompt = match row.kind {
                                TagEditorRowKind::Group => TagPrompt::RenameGroup {
                                    group: row.name.clone(),
                                },
                                _ => TagPrompt::RenameTag {
                                    tag: row.full_name.clone().unwrap_or_default(),
                                },
                            };
                            self.tag_text = row.name.clone();
                            ui.close();
                        }
                        let response = ui.button("Delete");
                        crate::menus::register_item(ui.ctx(), &response);
                        if response.clicked() {
                            invoke(
                                events,
                                ids::DELETE_TAG,
                                match row.kind {
                                    TagEditorRowKind::Group => {
                                        json!({ "group": row.name })
                                    }
                                    _ => json!({
                                        "tag": row.full_name.clone().unwrap_or_default()
                                    }),
                                },
                            );
                            ui.close();
                        }
                    };
                    response.context_menu(|ui| {
                        *menu_taken = true;
                        menu(ui, events);
                    });
                }
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(if row.kind == TagEditorRowKind::Group {
                        Role::Group
                    } else {
                        Role::CheckBox
                    });
                    node.set_label(row.full_name.clone().unwrap_or_else(|| row.name.clone()));
                    node.set_value(match row.state {
                        TagState::All => "ticked",
                        TagState::Some => "partly ticked",
                        TagState::Fixed => "fixed",
                        TagState::None => "empty",
                    });
                    node.set_description(if row.marked {
                        "the selection carries one of its tags".to_owned()
                    } else {
                        String::new()
                    });
                });
            }
            TagEditorRowKind::AddTag | TagEditorRowKind::AddGroup => {
                painter.text(
                    pos2(indent, rect.center().y),
                    Align2::LEFT_CENTER,
                    &row.name,
                    theme.small_font(),
                    theme.colors.text_dim.0,
                );
                if response.clicked() {
                    self.tag_prompt = match row.kind {
                        TagEditorRowKind::AddTag => {
                            // The group the row belongs to is the one
                            // above it.
                            let group = shell
                                .browser_view()
                                .tag_editor_rows()
                                .iter()
                                .rev()
                                .find(|other| {
                                    other.kind == TagEditorRowKind::Group && other.depth < row.depth
                                })
                                .map_or_else(String::new, |other| other.name.clone());
                            TagPrompt::AddTag { group }
                        }
                        _ => TagPrompt::AddGroup,
                    };
                    self.tag_text.clear();
                }
                ui.ctx().accesskit_node_builder(id, |node| {
                    node.set_role(Role::Button);
                    node.set_label(row.name.clone());
                });
            }
        }
        let _ = shell;
        clicked
    }

    /// What the Tag Editor's field asks for, done.
    fn tag_prompt_apply(&mut self, selected: &[String], events: &mut Vec<UiEvent>) {
        let name = self.tag_text.trim().to_owned();
        let prompt = self.tag_prompt.clone();
        self.tag_prompt = TagPrompt::None;
        self.tag_text.clear();
        if name.is_empty() {
            return;
        }
        match prompt {
            TagPrompt::None => {}
            TagPrompt::AddTag { group } => {
                invoke(
                    events,
                    ids::ADD_TAG,
                    json!({ "name": name, "group": group }),
                );
                if !group.is_empty() {
                    invoke(
                        events,
                        ids::TAG_ITEMS,
                        json!({
                            "items": selected,
                            "tags": [format!("{group}|{name}")],
                            "attach": true,
                        }),
                    );
                }
            }
            TagPrompt::AddGroup => {
                invoke(events, ids::ADD_TAG_GROUP, json!({ "name": name }));
            }
            TagPrompt::RenameTag { tag } => {
                invoke(events, ids::RENAME_TAG, json!({ "tag": tag, "name": name }));
            }
            TagPrompt::RenameGroup { group } => {
                invoke(
                    events,
                    ids::RENAME_TAG,
                    json!({ "group": group, "name": name }),
                );
            }
            TagPrompt::AddSubtag { parent } => {
                invoke(
                    events,
                    ids::ADD_TAG,
                    json!({ "name": name, "parent": parent }),
                );
                invoke(
                    events,
                    ids::TAG_ITEMS,
                    json!({
                        "items": selected,
                        "tags": [format!("{parent}|{name}")],
                        "attach": true,
                    }),
                );
            }
        }
    }

    /// Quick Tags (L-BRW-012; spec: browser-tags R13 and R14): the selected
    /// items' tags, and the field that adds one.
    fn quick_tags(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.colors.panel.0);
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        let _ = ui.interact(rect, quick_tags_id(), Sense::hover());
        ui.ctx().accesskit_node_builder(quick_tags_id(), |node| {
            node.set_role(Role::List);
            node.set_label("Quick Tags".to_owned());
            node.set_bounds(bounds(rect));
        });
        let selected: Vec<String> = view
            .selected()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let add_height = 20.0;
        let tags = view.quick_tags();
        let rows = rect.height() - add_height;
        let shown = ((rows / BROWSER_ROW).floor() as usize).min(tags.len());
        let mut hint: Option<String> = None;
        for (index, tag) in tags.iter().take(shown).enumerate() {
            let row = Rect::from_min_size(
                pos2(rect.left(), rect.top() + index as f32 * BROWSER_ROW),
                vec2(rect.width(), BROWSER_ROW),
            );
            let id = quick_tag_id(&tag.full_name);
            let response = ui.interact(row, id, Sense::click());
            // The pointer over a tag shows its group in the Status Bar
            // (spec: browser-tags R13).
            if response.hovered() {
                hint = Some(tag.group.clone());
            }
            // A tag not every selected item carries shows an asterisk.
            let text = if tag.all {
                tag.name.clone()
            } else {
                format!("{}*", tag.name)
            };
            painter.text(
                pos2(row.left() + 6.0, row.center().y),
                Align2::LEFT_CENTER,
                &text,
                theme.small_font(),
                theme.colors.text.0,
            );
            // A person's own tag shows an X that takes it off.
            if tag.removable {
                let cross = Rect::from_center_size(
                    pos2(row.right() - 10.0, row.center().y),
                    vec2(10.0, 10.0),
                );
                let crossed = ui.interact(cross, quick_tag_x_id(&tag.full_name), Sense::click());
                painter.text(
                    pos2(cross.center().x, cross.center().y),
                    Align2::CENTER_CENTER,
                    "\u{d7}",
                    theme.small_font(),
                    theme.colors.text_dim.0,
                );
                if crossed.clicked() {
                    invoke(
                        events,
                        ids::TAG_ITEMS,
                        json!({
                            "items": selected,
                            "tags": [tag.full_name],
                            "attach": false,
                        }),
                    );
                }
            }
            // Double-clicking searches for the tag (spec: browser-tags R13).
            if response.double_clicked() {
                invoke(
                    events,
                    ids::SEARCH,
                    json!({ "query": format!("#{}", tag.full_name) }),
                );
            }
            // The context menu: Show Tag in Tag Editor (C-QTAG-001).
            let menu = |ui: &mut Ui, events: &mut Vec<UiEvent>| {
                let response = ui.button("Show Tag in Tag Editor");
                crate::menus::register_item(ui.ctx(), &response);
                crate::menus::take_focus_first(ui, &response);
                if response.clicked() {
                    invoke(events, ids::SHOW_TAG_EDITOR, json!({ "show": true }));
                    ui.close();
                }
            };
            response.context_menu(|ui| menu(ui, events));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::ListItem);
                node.set_label(tag.full_name.clone());
                node.set_value(if tag.all {
                    "on every selected item".to_owned()
                } else {
                    "on some selected items".to_owned()
                });
            });
        }

        if hint != self.quick_tag_hint {
            self.quick_tag_hint.clone_from(&hint);
            events.push(UiEvent::StatusHint(hint));
        }

        // The Add field, with the tags it suggests.
        let add = Rect::from_min_size(
            pos2(rect.left() + 2.0, rect.bottom() - add_height),
            vec2(rect.width() - 4.0, add_height - 4.0),
        );
        let hint = if self.quick_adding {
            "Group or tag\u{2026}"
        } else {
            "Add\u{2026}"
        };
        let edit = egui::TextEdit::singleline(&mut self.quick_add)
            .id(quick_tags_add_id())
            .hint_text(hint)
            .font(theme.small_font());
        let response = ui.put(add, edit);
        crate::focus::register(ui.ctx(), response.id, bl_shell::Area::Browser, None);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Quick Tags Add Field".to_owned())
        });
        if !self.quick_add.trim().is_empty() {
            self.quick_suggestions(ui, shell, &response, events);
        }
    }

    /// The tags the Add field suggests, and the choice of where a new tag
    /// goes (spec: browser-tags R14).
    fn quick_suggestions(
        &mut self,
        _ui: &mut Ui,
        shell: &Shell,
        field: &egui::Response,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let typed = self.quick_add.trim().to_owned();
        let selected: Vec<String> = view
            .selected()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        egui::Popup::from_response(field).show(|ui| {
            if self.quick_adding {
                // The group or the tag to put the new tag in.
                for group in view.tag_groups() {
                    let response = ui.button(format!("In {group}"));
                    crate::menus::register_item(ui.ctx(), &response);
                    if response.clicked() {
                        self.make_tag(&typed, Some(&group), None, &selected, events);
                        ui.close();
                    }
                }
                for tag in view.tag_names() {
                    let response = ui.button(format!("Under {tag}"));
                    crate::menus::register_item(ui.ctx(), &response);
                    if response.clicked() {
                        self.make_tag(&typed, None, Some(&tag), &selected, events);
                        ui.close();
                    }
                }
                let response = ui.button("New Group\u{2026}");
                crate::menus::register_item(ui.ctx(), &response);
                if response.clicked() {
                    invoke(events, ids::ADD_TAG_GROUP, json!({ "name": typed }));
                    self.make_tag(&typed, Some(&typed), None, &selected, events);
                    ui.close();
                }
                return;
            }
            let suggestions = view.tag_suggestions(&typed);
            let mut chosen = None;
            for suggestion in suggestions.iter().take(8) {
                let response = ui.button(suggestion);
                crate::menus::register_item(ui.ctx(), &response);
                if response.clicked() {
                    chosen = Some(suggestion.clone());
                }
            }
            if let Some(tag) = chosen {
                invoke(
                    events,
                    ids::TAG_ITEMS,
                    json!({ "items": selected, "tags": [tag], "attach": true }),
                );
                self.quick_add.clear();
                ui.close();
                return;
            }
            // A name no tag has: Create New Tag\u{2026}. The popup stays
            // open and shows where the new tag goes.
            let response = ui.button("Create New Tag\u{2026}");
            crate::menus::register_item(ui.ctx(), &response);
            if response.clicked() {
                self.quick_adding = true;
            }
        });
    }

    /// Makes a tag and puts it on the selected items.
    fn make_tag(
        &mut self,
        name: &str,
        group: Option<&str>,
        parent: Option<&str>,
        selected: &[String],
        events: &mut Vec<UiEvent>,
    ) {
        if name.is_empty() {
            return;
        }
        invoke(
            events,
            ids::ADD_TAG,
            json!({ "name": name, "group": group, "parent": parent }),
        );
        self.quick_add.clear();
        self.quick_adding = false;
        let full = match (group, parent) {
            (Some(group), _) => format!("{group}|{name}"),
            (None, Some(parent)) => format!("{parent}|{name}"),
            (None, None) => name.to_owned(),
        };
        invoke(
            events,
            ids::TAG_ITEMS,
            json!({ "items": selected, "tags": [full], "attach": true }),
        );
    }

    /// Collect Files on Export's Ask (file management R8): Copy, Don't Copy
    /// and Cancel for a clip being saved.
    fn ask_copy_dialog(&mut self, ctx: &egui::Context, theme: &Theme, events: &mut Vec<UiEvent>) {
        let Some(ask) = ctx.data(|data| data.get_temp::<AskCopy>(ask_copy_id())) else {
            return;
        };
        let mut answer = None;
        egui::Window::new("Collect Files on Export")
            .id(Id::new("browser-ask-copy-window"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    "Copy the audio files this clip uses into the project the clip file is saved in?",
                );
                ui.horizontal(|ui| {
                    for (label, copy) in [("Copy", Some(true)), ("Don't Copy", Some(false)), ("Cancel", None)] {
                        let button = ui.button(label);
                        if button.clicked() {
                            answer = Some(copy);
                        }
                    }
                });
            });
        let _ = theme;
        if let Some(copy) = answer {
            ctx.data_mut(|data| data.remove::<AskCopy>(ask_copy_id()));
            if let Some(copy) = copy {
                let mut args = json!({ "clip": ask.clip, "copy": copy });
                if let Some(folder) = ask.folder {
                    args["folder"] = json!(folder);
                }
                invoke(events, "clip.save_file", args);
            }
        }
    }

    /// The text field a saved clip's name is edited in (clip files R4):
    /// Enter renames the file, Esc keeps its name. Returns the field's
    /// rectangle.
    fn rename_field(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        row: Rect,
        left: f32,
        item: &Item,
        events: &mut Vec<UiEvent>,
    ) -> Rect {
        let first = self.rename_for.as_deref() != Some(item.path.as_str());
        if first {
            self.rename_for = Some(item.path.clone());
            self.rename = Path::new(&item.path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        let rect = Rect::from_min_max(
            pos2(left, row.top() + 1.0),
            pos2((row.right() - 4.0).max(left + 40.0), row.bottom() - 1.0),
        );
        let id = Id::new("browser-rename-field");
        let edit = egui::TextEdit::singleline(&mut self.rename)
            .id(id)
            .font(theme.small_font())
            .margin(vec2(2.0, 0.0));
        let response = ui.put(rect, edit);
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_label("Name".to_owned());
        });
        if first {
            response.request_focus();
            let len = self.rename.chars().count();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(len),
                    )));
                state.store(ui.ctx(), id);
            }
        }
        // Enter and Esc both end the edit and take the field's focus.
        if response.lost_focus() && !first {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                invoke(events, ids::RENAME_FILE, json!({ "name": self.rename }));
            } else {
                invoke(events, ids::RENAME_FILE, json!({}));
            }
            self.rename_for = None;
        }
        rect
    }

    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    fn item_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        rect: Rect,
        index: usize,
        item: &Item,
        modifiers: egui::Modifiers,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let id = row_id(&item.path);
        let response = ui.interact(rect, id, Sense::CLICK | Sense::DRAG);
        let selected = view.is_selected(Path::new(&item.path));
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, 0.0, theme.colors.accent.0.gamma_multiply(0.45));
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, theme.colors.control_hover.0);
        }
        let indent = rect.left() + 4.0 + f32::from(item.depth) * 12.0;
        let mark = Rect::from_center_size(pos2(indent + 4.0, rect.center().y), vec2(8.0, 8.0));
        let ink = theme.colors.text_dim.0;
        match item.kind {
            ItemKind::Folder => glyph(
                &painter,
                mark,
                if item.open { Glyph::Down } else { Glyph::Right },
                ink,
            ),
            ItemKind::Audio | ItemKind::Sfz => glyph(&painter, mark, Glyph::Wave, ink),
            ItemKind::Clip => glyph(&painter, mark, Glyph::Clip, ink),
            ItemKind::Set => glyph(&painter, mark, Glyph::Set, ink),
            ItemKind::Preset => glyph(&painter, mark, Glyph::Preset, ink),
            ItemKind::Plugin | ItemKind::Device => glyph(&painter, mark, Glyph::Plugin, ink),
        }
        let text_colour = if selected {
            theme.colors.text.0
        } else {
            theme.colors.text.0.gamma_multiply(0.92)
        };
        // Spec: plugin-scanning R11. A plug-in's row shows its format as a
        // badge before its name, as Live's does.
        let mut name_left = indent + 12.0;
        if let Some(format) = item.format {
            let badge = painter.text(
                pos2(name_left + 2.0, rect.center().y),
                Align2::LEFT_CENTER,
                format.short_name(),
                theme.small_font(),
                ink,
            );
            painter.rect_stroke(
                badge.expand2(vec2(2.0, 0.0)),
                2.0,
                Stroke::new(1.0, ink),
                StrokeKind::Inside,
            );
            name_left = badge.right() + 6.0;
        }
        // Spec: clip files R4. A saved clip's name is open for editing.
        let renaming = view.renaming() == Some(Path::new(&item.path));
        let name = if renaming {
            self.rename_field(ui, theme, rect, name_left, item, events)
        } else {
            painter.text(
                pos2(name_left, rect.center().y),
                Align2::LEFT_CENTER,
                &item.name,
                theme.small_font(),
                text_colour,
            )
        };
        if let Some(folder) = &item.folder {
            // Search results name their folder at the right, in the room
            // the name leaves, showing the folder's end when it's long.
            let room = Rect::from_min_max(pos2(name.right() + 8.0, rect.top()), rect.max);
            if room.width() > 12.0 {
                painter.with_clip_rect(room).text(
                    pos2(rect.right() - 4.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    folder,
                    theme.small_font(),
                    theme.colors.pending.0,
                );
            }
        }

        if item.kind == ItemKind::Folder {
            drop_chains(ui, theme, rect, &item.path, events);
            note_clip_target(ui, theme, rect, ClipDropTarget::Folder(item.path.clone()));
            // Spec: file-management R10, C-BROWSER-014. A project folder's
            // context menu has Manage Project, which shows the File
            // Manager at that project.
            if shell.is_project_folder(Path::new(&item.path)) {
                response.context_menu(|ui| {
                    let button = ui.button("Manage Project");
                    crate::menus::register_item(ui.ctx(), &button);
                    if button.clicked() {
                        invoke(
                            events,
                            "view.toggle_file_manager",
                            json!({
                                "view": "project",
                                "project": item.path,
                                "open": true,
                            }),
                        );
                        ui.close();
                    }
                });
            }
        }
        if response.double_clicked() {
            match item.kind {
                ItemKind::Folder => invoke(
                    events,
                    ids::OPEN_FOLDER,
                    json!({ "path": item.path, "keep_others": command(modifiers) }),
                ),
                ItemKind::Audio => invoke(events, ids::LOAD_SELECTED, json!({})),
                // Spec: clip files R10. A clip file places its clip.
                ItemKind::Clip => {
                    if !selected {
                        invoke(events, ids::SELECT, json!({ "paths": [item.path] }));
                    }
                    invoke(events, ids::LOAD_SELECTED, json!({}));
                }
                // Spec: multisampler R47. An SFZ instrument loads as a
                // multisampler.
                ItemKind::Sfz => {
                    if !selected {
                        invoke(events, ids::SELECT, json!({ "paths": [item.path] }));
                    }
                    invoke(events, ids::LOAD_SELECTED, json!({}));
                }
                // Spec: device-chains R19. While a device is hot-swapped, a
                // preset loads into it.
                ItemKind::Preset => {
                    if !selected {
                        invoke(events, ids::SELECT, json!({ "paths": [item.path] }));
                    }
                    invoke(events, ids::LOAD_SELECTED, json!({}));
                }
                // Spec: modulation R12, plugin-scanning R11. A device or a
                // plug-in loads onto the selected track.
                ItemKind::Device | ItemKind::Plugin => {
                    if !selected {
                        invoke(events, ids::SELECT, json!({ "paths": [item.path] }));
                    }
                    invoke(events, ids::LOAD_SELECTED, json!({}));
                }
                ItemKind::Set => {}
            }
        } else if response.clicked() {
            ui.memory_mut(|m| {
                if !m.has_focus(list_id()) {
                    m.request_focus(list_id());
                }
            });
            if view.focus() != Part::Content || shell.focused_area() != Area::Browser {
                invoke(events, ids::FOCUS_BROWSER, json!({ "part": "content" }));
            }
            self.click(shell, index, item, modifiers, events);
        }
        if response.drag_started() {
            // The selection if the row is in it, otherwise the row.
            let mut paths: Vec<String> = if selected {
                view.selected()
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            } else {
                vec![item.path.clone()]
            };
            paths.sort_by_key(|p| view.position_of(Path::new(p)));
            if item.kind == ItemKind::Plugin {
                DragAndDrop::set_payload(
                    ui.ctx(),
                    BrowserDrag {
                        paths: Vec::new(),
                        plugin: Some(item.path.clone()),
                    },
                );
            } else {
                let (loadable, mut audio): (Vec<String>, Vec<String>) =
                    paths.into_iter().partition(|p| is_loadable(p));
                audio.retain(|p| is_audio(p) || is_sfz(p) || is_clip(p));
                // Clip files go together or not at all (clip files R7).
                if audio.iter().any(|p| is_clip(p)) {
                    audio.retain(|p| is_clip(p));
                }
                if !audio.is_empty() {
                    DragAndDrop::set_payload(
                        ui.ctx(),
                        BrowserDrag {
                            paths: audio,
                            plugin: None,
                        },
                    );
                } else if !loadable.is_empty() {
                    DragAndDrop::set_payload(ui.ctx(), DevicesDrag { paths: loadable });
                }
            }
        }
        let dragged = DragAndDrop::payload::<BrowserDrag>(ui.ctx())
            .map(|d| (d.paths.clone(), d.plugin.is_some()))
            .or_else(|| {
                DragAndDrop::payload::<DevicesDrag>(ui.ctx()).map(|d| (d.paths.clone(), false))
            });
        if response.dragged()
            && let Some((drag, plugin)) = dragged
            && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
        {
            let text = match drag.as_slice() {
                _ if plugin => item.name.clone(),
                [one] if bl_shell::browser::device_by_id(one).is_some() => {
                    bl_shell::browser::device_by_id(one).map_or_else(String::new, |d| d.name)
                }
                [one] => Path::new(one)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                many => format!("{} files", many.len()),
            };
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                Id::new("browser-drag"),
            ));
            let galley = painter.layout_no_wrap(text, theme.small_font(), theme.colors.text.0);
            let at = pointer + vec2(12.0, 4.0);
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand(3.0),
                theme.sizes.corner,
                theme.colors.control.0,
            );
            painter.galley(at, galley, theme.colors.text.0);
        }
        let kind = match item.kind {
            ItemKind::Folder => {
                if item.open {
                    "folder, open"
                } else {
                    "folder"
                }
            }
            ItemKind::Audio => "audio file",
            ItemKind::Clip => match item.clip_kind {
                Some(ClipFileKind::Audio) => "audio clip file",
                Some(ClipFileKind::Midi) => "MIDI clip file",
                None => "clip file",
            },
            ItemKind::Sfz => "SFZ instrument",
            ItemKind::Set => "Beatlayer set",
            ItemKind::Preset => "Beatlayer device preset",
            ItemKind::Plugin => match item.format {
                Some(bl_model::PluginFormat::Clap) => "CLAP plug-in",
                Some(bl_model::PluginFormat::Vst3) => "VST3 plug-in",
                Some(bl_model::PluginFormat::AudioUnit) => "Audio Unit plug-in",
                Some(bl_model::PluginFormat::Lv2) => "LV2 plug-in",
                None => "plug-in",
            },
            ItemKind::Device => "device",
        };
        response
            .widget_info(|| WidgetInfo::selected(WidgetType::Other, true, selected, &item.name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::ListItem);
            node.set_label(item.name.clone());
            node.set_value(kind.to_owned());
            node.set_selected(selected);
            if let Some(folder) = &item.folder {
                node.set_description(format!("in {folder}"));
            }
        });
    }

    /// A click on row `index`: select it, with Shift the range to it, with
    /// Cmd (Ctrl) add or remove it (spec: browser R7).
    fn click(
        &mut self,
        shell: &Shell,
        index: usize,
        item: &Item,
        modifiers: egui::Modifiers,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        if modifiers.shift
            && let Some(anchor) = self.anchor.or(view.cursor())
        {
            let paths = range_paths(shell, anchor, index);
            invoke(events, ids::SELECT, json!({ "paths": paths }));
        } else if command(modifiers) {
            let mut paths: Vec<String> = view
                .selected()
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| *p != item.path)
                .collect();
            if !view.is_selected(Path::new(&item.path)) {
                paths.push(item.path.clone());
            }
            self.anchor = Some(index);
            invoke(events, ids::SELECT, json!({ "paths": paths }));
        } else {
            self.anchor = Some(index);
            invoke(events, ids::SELECT, json!({ "paths": [item.path] }));
        }
    }

    fn preview_strip(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let strip = layout.strip;
        let painter = ui.painter_at(strip);
        painter.rect_filled(strip, 0.0, theme.colors.bar.0);
        painter.line_segment(
            [strip.left_top(), strip.right_top()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        let preview = shell.browser_preview();
        // The Preview switch (L-BRW-011, M-OPT-006).
        let toggle = layout.preview_toggle;
        let response = ui.interact(toggle, preview_toggle_id(), Sense::click());
        let fill = if preview.on {
            theme.colors.accent.0
        } else if response.hovered() {
            theme.colors.control_hover.0
        } else {
            theme.colors.control.0
        };
        painter.rect_filled(toggle, theme.sizes.corner, fill);
        let ink = if preview.on {
            theme.colors.accent_text.0
        } else {
            theme.colors.text_dim.0
        };
        glyph(&painter, toggle.shrink(2.0), Glyph::Speaker, ink);
        if response.clicked() {
            invoke(events, ids::SET_PREVIEW, json!({}));
        }
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Checkbox, true, preview.on, "Preview")
        });

        // The file previewed: its waveform and how far it has played.
        let wave = layout.preview_wave;
        let id = preview_strip_id();
        let wave_response = ui.interact(wave, id, Sense::click());
        if wave_response.clicked()
            && let Some(pos) = wave_response.interact_pointer_pos()
        {
            let fraction = ((pos.x - wave.left()) / wave.width()).clamp(0.0, 1.0) as f64;
            invoke(events, ids::SCRUB_PREVIEW, json!({ "fraction": fraction }));
        }
        let view = shell.browser_view();
        let shown = preview
            .playing
            .clone()
            .or_else(|| preview.loading.clone())
            .or_else(|| {
                view.preview_overview()
                    .map(|(p, _)| p.to_string_lossy().into_owned())
            });
        let name = shown
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some((path, overview)) = view.preview_overview()
            && shown.as_deref() == Some(&*path.to_string_lossy())
            && !overview.is_empty()
        {
            let middle = wave.center().y;
            // Scaled to the file's peak, so a quiet file still shows its
            // shape.
            let peak = overview
                .iter()
                .fold(0.0f32, |peak, [low, high]| peak.max(-low).max(*high))
                .max(1e-3);
            let half = wave.height() / 2.0 / peak.min(1.0);
            let step = wave.width() / overview.len() as f32;
            for (i, [low, high]) in overview.iter().enumerate() {
                let x = wave.left() + (i as f32 + 0.5) * step;
                painter.line_segment(
                    [
                        pos2(x, middle - high.clamp(-1.0, 1.0) * half),
                        pos2(x, middle - low.clamp(-1.0, 1.0) * half + 0.5),
                    ],
                    Stroke::new(step.max(1.0), theme.colors.text_dim.0.gamma_multiply(0.6)),
                );
            }
        }
        if preview.playing.is_some() && preview.length_seconds > 0.0 {
            let done = (preview.position_seconds / preview.length_seconds).clamp(0.0, 1.0) as f32;
            let x = wave.left() + wave.width() * done;
            painter.rect_filled(
                Rect::from_min_max(wave.min, pos2(x, wave.bottom())),
                0.0,
                theme.colors.accent.0.gamma_multiply(0.25),
            );
            painter.line_segment(
                [pos2(x, wave.top()), pos2(x, wave.bottom())],
                Stroke::new(1.0, theme.colors.playhead.0),
            );
        }
        painter.text(
            pos2(wave.left() + 3.0, wave.center().y),
            Align2::LEFT_CENTER,
            &name,
            theme.small_font(),
            theme.colors.text.0,
        );
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::ProgressIndicator);
            node.set_label("Preview Strip".to_owned());
            let value = if let Some(loading) = &preview.loading {
                format!("loading {}", file_name(loading))
            } else if preview.playing.is_some() {
                format!(
                    "{name}, {:.1} of {:.1} seconds",
                    preview.position_seconds, preview.length_seconds
                )
            } else if name.is_empty() {
                "nothing previewed".to_owned()
            } else {
                format!("{name}, stopped")
            };
            node.set_value(value);
        });

        // The Raw switch at the strip's right (WP-4.18; spec: browser R18).
        let raw_toggle = layout.raw_toggle;
        let raw_id = raw_toggle_id();
        let raw_response = ui.interact(raw_toggle, raw_id, Sense::click());
        let fill = if preview.raw {
            theme.colors.accent.0
        } else if raw_response.hovered() {
            theme.colors.control_hover.0
        } else {
            theme.colors.control.0
        };
        painter.rect_filled(raw_toggle, theme.sizes.corner, fill);
        let ink = if preview.raw {
            theme.colors.accent_text.0
        } else {
            theme.colors.text_dim.0
        };
        painter.text(
            raw_toggle.center(),
            Align2::CENTER_CENTER,
            "Raw",
            theme.small_font(),
            ink,
        );
        if raw_response.clicked() {
            invoke(events, ids::SET_RAW, json!({}));
        }
        raw_response
            .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, preview.raw, "Raw"));
        ui.ctx().accesskit_node_builder(raw_id, |node| {
            node.set_role(Role::CheckBox);
            node.set_label("Raw".to_owned());
            node.set_selected(preview.raw);
        });
    }

    /// Keys the browser reads itself, because one shortcut's keys act
    /// differently (the keymap gives each shortcut one command and
    /// arguments): Up and Down (S-BRW-001, S-A11Y-015, S-A11Y-016), Left and
    /// Right (S-BRW-003, S-BRW-005, S-A11Y-017), and Shift-Enter (S-BRW-005).
    fn keys(&mut self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let view = shell.browser_view();
        let part = view.focus();
        let focused = shell.focused_area() == Area::Browser
            && shell.focused_window() == WindowKind::Main
            && part != Part::Search
            && !ui.ctx().text_edit_focused()
            && ui.memory(|m| {
                m.focused()
                    .is_none_or(|id| id == list_id() || id == sidebar_id())
            });
        if !focused {
            return;
        }
        let region = if part == Part::Sidebar {
            sidebar_id()
        } else {
            list_id()
        };
        if ui.memory(|m| m.has_focus(region)) {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    region,
                    EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..EventFilter::default()
                    },
                );
            });
        }
        // Each press with the modifiers held as it went down.
        let presses: Vec<(Key, egui::Modifiers)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        let find = |key: Key, shift: bool, cmd_ok: bool| {
            presses.iter().find(|(k, m)| {
                *k == key
                    && m.shift == shift
                    && !m.alt
                    && (cmd_ok || !command(*m))
                    && (!m.ctrl || (cmd_ok && command(*m)))
            })
        };
        let up = find(Key::ArrowUp, false, false).or(find(Key::ArrowUp, true, false));
        let down = find(Key::ArrowDown, false, false).or(find(Key::ArrowDown, true, false));
        let shift = up.or(down).is_some_and(|(_, m)| m.shift);
        let right = find(Key::ArrowRight, false, true);
        let cmd = right.is_some_and(|(_, m)| command(*m));
        let (up, down, right) = (up.is_some(), down.is_some(), right.is_some());
        let left = find(Key::ArrowLeft, false, false).is_some();
        let shift_enter = find(Key::Enter, true, false).is_some();
        let enter = find(Key::Enter, false, false).is_some();
        // S-BRW-015: Cmd-E (Ctrl-E) moves the keyboard focus to Quick
        // Tags' Add field.
        if view.quick_tags_shown()
            && let Some((_, modifiers)) = find(Key::E, false, true)
            && command(*modifiers)
        {
            ui.memory_mut(|m| m.request_focus(quick_tags_add_id()));
            return;
        }
        if up || down || left || right {
            // The arrows act here; egui mustn't also move the focus with
            // them, which it decides before this runs when the lock filter
            // isn't in place yet.
            ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        }
        match part {
            Part::Sidebar => Self::sidebar_keys(shell, up, down, right, events),
            Part::Content | Part::Search => {
                self.list_keys(shell, up, down, left, right, shift, cmd, events);
                if shift_enter && view.can_run(ids::PREVIEW, &json!({})) {
                    invoke(events, ids::PREVIEW, json!({}));
                }
                // Enter on a folder (no file to load) opens or closes it.
                if enter
                    && let Some(item) = view.cursor().and_then(|c| view.row(c))
                    && item.kind == ItemKind::Folder
                {
                    invoke(events, ids::OPEN_FOLDER, json!({ "path": item.path }));
                }
            }
        }
    }

    fn sidebar_keys(shell: &Shell, up: bool, down: bool, right: bool, events: &mut Vec<UiEvent>) {
        let view = shell.browser_view();
        if up || down {
            let labels: Vec<Label> = view.labels().into_iter().map(|l| l.label).collect();
            let at = labels.iter().position(|l| l == view.label()).unwrap_or(0);
            let next = if up {
                at.checked_sub(1)
            } else {
                (at + 1 < labels.len()).then_some(at + 1)
            };
            if let Some(next) = next {
                invoke(events, ids::OPEN_PLACE, json!({ "label": labels[next] }));
            }
        }
        if right {
            invoke(events, ids::FOCUS_BROWSER, json!({ "part": "content" }));
            if view.cursor().is_none()
                && let Some(first) = view.row(0)
            {
                invoke(events, ids::SELECT, json!({ "paths": [first.path] }));
            }
        }
    }

    #[expect(clippy::too_many_arguments, reason = "each key the list reads")]
    fn list_keys(
        &mut self,
        shell: &Shell,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        shift: bool,
        cmd: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let total = view.row_count();
        if (up || down) && total > 0 {
            let next = match (view.cursor(), up) {
                (Some(c), true) => c.saturating_sub(1),
                (Some(c), false) => (c + 1).min(total - 1),
                (None, _) => 0,
            };
            if shift {
                let anchor = *self.anchor.get_or_insert(view.cursor().unwrap_or(next));
                let paths = range_paths(shell, anchor, next);
                invoke(events, ids::SELECT, json!({ "paths": paths }));
            } else if let Some(item) = view.row(next) {
                self.anchor = Some(next);
                invoke(events, ids::SELECT, json!({ "paths": [item.path] }));
            }
        }
        let Some(cursor) = view.cursor() else {
            return;
        };
        let Some(item) = view.row(cursor) else {
            return;
        };
        if right {
            match item.kind {
                ItemKind::Folder if !item.open => invoke(
                    events,
                    ids::OPEN_FOLDER,
                    json!({ "path": item.path, "open": true, "keep_others": cmd }),
                ),
                ItemKind::Folder => {
                    if let Some(child) = view.row(cursor + 1).filter(|c| c.depth > item.depth) {
                        self.anchor = Some(cursor + 1);
                        invoke(events, ids::SELECT, json!({ "paths": [child.path] }));
                    }
                }
                ItemKind::Audio | ItemKind::Clip => {
                    invoke(events, ids::PREVIEW, json!({ "path": item.path }));
                }
                ItemKind::Sfz
                | ItemKind::Set
                | ItemKind::Preset
                | ItemKind::Plugin
                | ItemKind::Device => {}
            }
        }
        if left {
            if item.kind == ItemKind::Folder && item.open {
                invoke(
                    events,
                    ids::OPEN_FOLDER,
                    json!({ "path": item.path, "open": false }),
                );
            } else if item.depth > 0 {
                let parent = (0..cursor)
                    .rev()
                    .find_map(|i| view.row(i).filter(|row| row.depth < item.depth));
                if let Some(parent) = parent {
                    self.anchor = None;
                    invoke(events, ids::SELECT, json!({ "paths": [parent.path] }));
                }
            } else {
                invoke(events, ids::FOCUS_BROWSER, json!({ "part": "sidebar" }));
            }
        }
    }

    /// Shows where files dragged from the browser (or from Finder or
    /// Explorer) would land in the Arrangement, and drops them there
    /// (spec: browser R13). `main_view` is the main view's rectangle.
    pub fn drop_overlay(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        (main_view, zone_editor, clip_view): (Rect, Option<Rect>, Rect),
        events: &mut Vec<UiEvent>,
    ) {
        let ctx = ui.ctx().clone();
        let payload = DragAndDrop::payload::<BrowserDrag>(&ctx);
        let (dropped, hovering, released, pointer, modifiers) = ctx.input(|i| {
            let dropped: Vec<String> = i
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_string_lossy().into_owned())
                .filter(|path| is_audio(path) || is_sfz(path) || is_clip(path))
                .collect();
            (
                dropped,
                !i.raw.hovered_files.is_empty(),
                i.pointer.any_released(),
                i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()),
                i.modifiers,
            )
        });
        if payload.is_none() && dropped.is_empty() && !hovering {
            return;
        }
        if shell.main_view() != MainView::Arrangement {
            return;
        }
        if pointer.is_some_and(|p| zone_editor.is_some_and(|editor| editor.contains(p))) {
            return;
        }
        if pointer.is_some_and(|p| clip_view.contains(p)) {
            // Spec: sample-editor R18. A file dropped on the Clip View
            // replaces the clip's sample, which the Clip View does.
            return;
        }
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let pinned = view.pinned_tracks(doc).count();
        let lanes = ArrangementLayout::compute(main_view, view.overview(), pinned).lanes;
        // Cmd (macOS) or Alt (Windows) ignores the grid (I-DRAG-021).
        let ignore_snap = if cfg!(target_os = "macos") {
            modifiers.mac_cmd
        } else {
            modifiers.alt
        };
        // An SFZ instrument goes on a MIDI track, where audio goes on an
        // audio track (multisampler R47).
        let sfz_drag = payload.as_ref().map_or_else(
            || !dropped.is_empty() && dropped.iter().all(|path| is_sfz(path)),
            |drag| drag.paths.iter().all(|path| is_sfz(path)),
        );
        // A clip file goes on a track of its clip's kind (clip files R7).
        let first_clip = payload
            .as_ref()
            .map(|drag| drag.paths.as_slice())
            .unwrap_or(&dropped)
            .iter()
            .find(|path| is_clip(path))
            .and_then(|path| bl_shell::browser::listing::clip_kind(Path::new(path)));
        let kind = if sfz_drag || first_clip == Some(ClipFileKind::Midi) {
            bl_model::TrackKind::Midi
        } else {
            bl_model::TrackKind::Audio
        };
        let target = pointer.filter(|p| lanes.contains(*p)).and_then(|p| {
            drop_target_on(
                kind,
                view,
                doc,
                lanes.height(),
                p.x - lanes.left(),
                p.y - lanes.top(),
                ignore_snap,
            )
        });
        let plugin = payload.as_ref().and_then(|p| p.plugin.clone());
        if let Some(key) = plugin {
            // Spec: plugin-scanning R11. A plug-in dropped on a track loads
            // onto it.
            let track = target.as_ref().and_then(|t| t.track);
            if track.is_none() && pointer.is_some_and(|p| main_view.contains(p)) {
                ctx.set_cursor_icon(CursorIcon::NoDrop);
            }
            if released {
                DragAndDrop::clear_payload(&ctx);
                if let Some(track) = track
                    && shell.browser_view().plugin(Path::new(&key)).is_some()
                {
                    invoke(events, ids::SELECT, json!({ "paths": [key] }));
                    invoke(
                        events,
                        bl_shell::arrangement_commands::SELECT_TRACKS,
                        json!({ "tracks": [track] }),
                    );
                    invoke(events, ids::LOAD_SELECTED, json!({}));
                }
            }
            return;
        }
        if let Some(target) = target {
            draw_target(ui, theme, lanes, view, doc, &target, kind);
        } else if pointer.is_some_and(|p| main_view.contains(p)) {
            ctx.set_cursor_icon(CursorIcon::NoDrop);
        }
        // Several files spread across tracks with Cmd (Ctrl) held
        // (I-DROP-003).
        let spread = command(modifiers);
        let add = |events: &mut Vec<UiEvent>, paths: &[String], target: Option<DropTarget>| {
            if sfz_drag {
                // One multisampler for each instrument, on the track the
                // pointer is over or on a new one.
                for path in paths {
                    let mut args = json!({ "path": path });
                    if let Some(track) = target.and_then(|t| t.track) {
                        args["track"] = json!(track);
                    }
                    invoke(events, bl_shell::multisampler_commands::IMPORT_SFZ, args);
                }
                return;
            }
            let clips = paths.iter().any(|p| is_clip(p));
            let paths: Vec<String> = paths
                .iter()
                .filter(|p| if clips { is_clip(p) } else { is_audio(p) })
                .cloned()
                .collect();
            if paths.is_empty() {
                return;
            }
            let mut args = json!({ "paths": paths, "spread": spread });
            match target {
                Some(target) => {
                    args["at"] = json!(target.at);
                    if let Some(track) = target.track {
                        args["track"] = json!(track);
                    }
                }
                // From Finder without a pointer position: a new track at the
                // insert marker.
                None => args["at"] = json!(view.insert_marker().max(Ticks::ZERO)),
            }
            invoke(events, ids::ADD_TO_ARRANGEMENT, args);
        };
        if let Some(payload) = payload
            && released
        {
            DragAndDrop::clear_payload(&ctx);
            if target.is_some() {
                add(events, &payload.paths, target);
            }
        }
        if !dropped.is_empty() {
            match (pointer.filter(|p| lanes.contains(*p)), target) {
                (Some(_), Some(target)) => add(events, &dropped, Some(target)),
                (Some(_), None) => {}
                // An instrument dropped outside the main view, on the
                // Device View or the zone editor, is theirs to take.
                (None, _) if sfz_drag && pointer.is_some_and(|p| !main_view.contains(p)) => {}
                (None, _) => add(events, &dropped, None),
            }
        }
    }
}

/// Highlights where a drop lands: the track's lane (or the drop area) and
/// a line at the position.
fn draw_target(
    ui: &Ui,
    theme: &Theme,
    lanes: Rect,
    view: &bl_shell::arrangement::ArrangementView,
    doc: &bl_model::Document,
    target: &DropTarget,
    kind: bl_model::TrackKind,
) {
    let painter = ui.painter_at(lanes);
    let top = lanes.top() + target.top;
    let rect = Rect::from_min_max(
        pos2(lanes.left(), top),
        pos2(lanes.right(), (top + target.height).min(lanes.bottom())),
    );
    painter.rect_filled(rect, 0.0, theme.colors.focus.0.gamma_multiply(0.18));
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, theme.colors.focus.0),
        StrokeKind::Inside,
    );
    let x = lanes.left() + view.horizontal(doc).x(target.at) as f32;
    if x >= lanes.left() && x <= lanes.right() {
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, theme.colors.insert_marker.0),
        );
    }
    if target.track.is_none() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            if kind == bl_model::TrackKind::Midi {
                "Drop to make a new MIDI track"
            } else {
                "Drop to make a new audio track"
            },
            theme.small_font(),
            theme.colors.text.0,
        );
    }
}

/// A rectangle as AccessKit places it, in points: a node made only to name
/// a part (a hover-only one) doesn't get its place by itself. The tree's
/// root carries the interface zoom (WP-1.17), so its nodes' bounds are in
/// points, as the main window's regions are.
fn bounds(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.left().into(),
        y0: rect.top().into(),
        x1: rect.right().into(),
        y1: rect.bottom().into(),
    }
}

/// The paths of rows `from` to `to`, in that order, so the last is `to`.
fn range_paths(shell: &Shell, from: usize, to: usize) -> Vec<String> {
    let view = shell.browser_view();
    let rows: Vec<usize> = if from <= to {
        (from..=to).collect()
    } else {
        (to..=from).rev().collect()
    };
    rows.into_iter()
        .filter_map(|i| view.row(i).map(|item| item.path))
        .collect()
}

/// Chain rows dragged over `rect` are saved as chain presets when released
/// there (the racks spec, R46): in `folder`, or with an empty one in the User
/// Library's folder for the rack's presets.
fn drop_chains(ui: &Ui, theme: &Theme, rect: Rect, folder: &str, events: &mut Vec<UiEvent>) {
    let Some(drag) = DragAndDrop::payload::<crate::rack_view::ChainDrag>(ui.ctx()) else {
        return;
    };
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    if !rect.contains(pointer) || !ui.clip_rect().contains(pointer) {
        return;
    }
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(2.0, theme.colors.accent.0),
        StrokeKind::Inside,
    );
    if ui.input(|i| i.pointer.any_released()) {
        DragAndDrop::clear_payload(ui.ctx());
        for chain in &drag.chains {
            let args = if folder.is_empty() {
                json!({ "chain": chain })
            } else {
                json!({ "chain": chain, "folder": folder })
            };
            invoke(events, "chain.save_preset", args);
        }
    }
}

/// Whether a dragged path is a preset file or the ID of a device a label
/// lists: what the Device View loads.
fn is_loadable(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| file_kind(&n.to_string_lossy()))
        == Some(ItemKind::Preset)
        || bl_shell::browser::device_by_id(path).is_some()
}

fn is_audio(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| file_kind(&n.to_string_lossy()))
        == Some(ItemKind::Audio)
}

/// Whether `path` names a clip file.
pub(crate) fn is_clip(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| file_kind(&n.to_string_lossy()))
        == Some(ItemKind::Clip)
}

/// Whether `path` names an SFZ instrument, which a MIDI track or the drop
/// area takes (multisampler R47).
pub(crate) fn is_sfz(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| file_kind(&n.to_string_lossy()))
        == Some(ItemKind::Sfz)
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Scrolls an area with the wheel while the pointer is over it; returns
/// the new offset, kept within the content.
fn scrolled(ui: &Ui, area: Rect, offset: f32, content: f32) -> f32 {
    let over = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| area.contains(p));
    let delta = if over {
        ui.input(|i| i.smooth_scroll_delta().y)
    } else {
        0.0
    };
    (offset - delta).clamp(0.0, (content - area.height()).max(0.0))
}

/// Beatlayer's own small marks for the browser.
#[derive(Clone, Copy)]
enum Glyph {
    Left,
    Right,
    Down,
    Filter,
    Menu,
    Wave,
    Set,
    Preset,
    Plugin,
    Clip,
    Speaker,
}

fn glyph(painter: &egui::Painter, rect: Rect, glyph: Glyph, colour: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.3;
    let triangle = |points: [Pos2; 3]| {
        painter.add(Shape::convex_polygon(points.to_vec(), colour, Stroke::NONE));
    };
    match glyph {
        Glyph::Left => triangle([c + vec2(s, -s), c + vec2(s, s), c + vec2(-s, 0.0)]),
        Glyph::Right => triangle([
            c + vec2(-s * 0.7, -s),
            c + vec2(-s * 0.7, s),
            c + vec2(s, 0.0),
        ]),
        Glyph::Down => triangle([
            c + vec2(-s, -s * 0.7),
            c + vec2(s, -s * 0.7),
            c + vec2(0.0, s),
        ]),
        Glyph::Menu => triangle([
            c + vec2(-s, -s * 0.5),
            c + vec2(s, -s * 0.5),
            c + vec2(0.0, s * 0.6),
        ]),
        Glyph::Filter => {
            for (i, width) in [1.0f32, 0.66, 0.33].iter().enumerate() {
                let y = c.y - s + i as f32 * s;
                painter.line_segment(
                    [pos2(c.x - s * width, y), pos2(c.x + s * width, y)],
                    Stroke::new(1.0, colour),
                );
            }
        }
        Glyph::Wave => {
            for (i, height) in [0.5f32, 1.0, 0.7, 0.35].iter().enumerate() {
                let x = c.x - s + i as f32 * s * 0.66;
                painter.line_segment(
                    [pos2(x, c.y - s * height), pos2(x, c.y + s * height)],
                    Stroke::new(1.0, colour),
                );
            }
        }
        Glyph::Preset => {
            painter.circle_stroke(c, s * 0.9, Stroke::new(1.0, colour));
        }
        Glyph::Plugin => {
            // Beatlayer's own mark for a hosted plug-in: a small square, as
            // a plug-in is a box in the chain.
            painter.rect_filled(
                Rect::from_center_size(c, vec2(s * 1.4, s * 1.4)),
                1.0,
                colour,
            );
        }
        Glyph::Clip => {
            // Beatlayer's own mark for a clip file: a clip's body and its
            // title bar.
            let body = Rect::from_center_size(c, vec2(s * 2.0, s * 1.5));
            painter.rect_stroke(body, 1.0, Stroke::new(1.0, colour), StrokeKind::Inside);
            painter.rect_filled(
                Rect::from_min_size(body.min, vec2(body.width(), s * 0.5)),
                1.0,
                colour,
            );
        }
        Glyph::Set => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(s * 1.8, s * 1.8)),
                1.0,
                Stroke::new(1.0, colour),
                StrokeKind::Inside,
            );
        }
        Glyph::Speaker => {
            let w = rect.width();
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(rect.left() + w * 0.1, c.y - w * 0.15),
                    pos2(rect.left() + w * 0.3, c.y - w * 0.15),
                    pos2(rect.left() + w * 0.55, c.y - w * 0.4),
                    pos2(rect.left() + w * 0.55, c.y + w * 0.4),
                    pos2(rect.left() + w * 0.3, c.y + w * 0.15),
                    pos2(rect.left() + w * 0.1, c.y + w * 0.15),
                ],
                colour,
                Stroke::NONE,
            ));
            painter.line_segment(
                [
                    pos2(rect.left() + w * 0.72, c.y - w * 0.2),
                    pos2(rect.left() + w * 0.72, c.y + w * 0.2),
                ],
                Stroke::new(1.0, colour),
            );
            painter.line_segment(
                [
                    pos2(rect.left() + w * 0.88, c.y - w * 0.35),
                    pos2(rect.left() + w * 0.88, c.y + w * 0.35),
                ],
                Stroke::new(1.0, colour),
            );
        }
    }
}
