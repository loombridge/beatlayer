//! The File Manager in the detail area's place (WP-6.63): the files a set,
//! a project or the User Library uses, the missing ones, the external ones,
//! and the unused ones; and the Automatic Search.
//!
//! The behaviour is in the file management spec, R10 to R21.

use egui::accesskit::Role;
use egui::{Rect, Ui};

use bl_shell::Shell;
use bl_shell::file_manager_commands::{
    ExternalGroup, FileManagerState, FileRow, Location, Scope, UnusedFiles,
};

use crate::main_window::UiEvent;
use crate::theme::Theme;
use bl_shell::Invocation;
use serde_json::json;

/// `view.toggle_file_manager`: shows and hides the File Manager.
const TOGGLE: &str = "view.toggle_file_manager";
/// `file_manager.replace_file`: replaces or relinks a file.
const REPLACE_FILE: &str = "file_manager.replace_file";
/// `file_manager.hot_swap`: arms the browser to replace a file.
const HOT_SWAP: &str = "file_manager.hot_swap";
/// `file_manager.edit_file`: opens a file in the external sample editor.
const EDIT_FILE: &str = "file_manager.edit_file";
/// `file_manager.delete_unused`: moves unused files to the system's trash.
const DELETE_UNUSED: &str = "file_manager.delete_unused";
/// `set.relink_missing`: the Automatic Search's Go button.
const RELINK_MISSING: &str = "set.relink_missing";
/// `set.collect_all_and_save`: the Collect and Save button.
const COLLECT_ALL: &str = "set.collect_all_and_save";
/// `file_manager.set_search_folder`: the Set Folder… button.
const SET_SEARCH_FOLDER: &str = "file_manager.set_search_folder";

/// What the File Manager shows that the shell doesn't keep: the switches
/// the person sets, which the commands then use.
#[derive(Clone, Debug)]
pub struct FileManagerUi {
    /// The External Files section's switches: which groups are collected.
    pub collect: Vec<(ExternalGroup, bool)>,
    /// The Automatic Search's switches, which the commands then use.
    pub search_project: bool,
    /// Whether the Automatic Search searches the User Library.
    pub search_library: bool,
    /// The unused file whose Delete was asked for, waiting for the answer.
    pub deleting: Option<String>,
    /// What the File Manager lists, as it was when the set last changed:
    /// a project's sets are read from the disk, which is too slow for
    /// every frame.
    pub state: Option<(StateKey, FileManagerState)>,
}

/// What the File Manager's state is cached on: the document's revision and
/// the shell's, so the list is built again when either changes.
type StateKey = (u64, u64);

impl Default for FileManagerUi {
    fn default() -> Self {
        Self {
            // Spec: file-management R15. The User Library is meant to be
            // shared, so it is left alone.
            collect: vec![
                (ExternalGroup::OtherProject, true),
                (ExternalGroup::UserLibrary, false),
                (ExternalGroup::Elsewhere, true),
            ],
            search_project: true,
            search_library: true,
            deleting: None,
            state: None,
        }
    }
}

/// Draws the File Manager in the detail area's place (R10).
pub fn show(
    ui: &mut Ui,
    shell: &Shell,
    theme: &Theme,
    rect: Rect,
    panel: &mut FileManagerUi,
    events: &mut Vec<UiEvent>,
) {
    let revision = (shell.document_revision(), shell.revision());
    let state = match &panel.state {
        Some((seen, state)) if *seen == revision => state.clone(),
        _ => {
            let state = shell.file_manager_state();
            panel.state = Some((revision, state.clone()));
            state
        }
    };
    let id = region(ui, "region-file-manager", rect, Role::Group, "File Manager");
    ui.painter().rect_filled(rect, 0.0, theme.colors.panel.0);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .accessibility_parent(id),
        |ui| {
            tabs(ui, &state, events);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if state.scope == Scope::Set {
                    files(ui, theme, &state.files, events);
                }
                missing(ui, theme, &state.missing, events);
                search(ui, theme, &state, panel, events);
                external(ui, theme, &state, panel, events);
                if state.scope != Scope::Set {
                    unused(ui, theme, &state.unused, panel, events);
                }
                if state.packing {
                    // Spec: file-management R16. The Packing section's
                    // content is WP-6.35b's; only the heading is drawn.
                    ui.heading("Packing");
                }
                collect_and_save(ui, &state, panel, events);
            });
        },
    );
}

/// The three buttons that choose the File Manager's view (R10).
fn tabs(ui: &mut Ui, state: &FileManagerState, events: &mut Vec<UiEvent>) {
    let id = ui.make_persistent_id("file-manager-tabs");
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::TabList);
        node.set_label("File Manager views".to_owned());
    });
    ui.horizontal(|ui| {
        for (label, view, shown) in [
            ("Manage Set", "set", state.scope == Scope::Set),
            ("Manage Project", "project", state.scope == Scope::Project),
            (
                "Manage User Library",
                "user-library",
                state.scope == Scope::UserLibrary,
            ),
        ] {
            let button = ui.selectable_label(shown, label);
            if button.clicked() {
                events.push(UiEvent::Invoke(Invocation::new(
                    TOGGLE,
                    json!({ "view": view, "open": true }),
                )));
            }
        }
        if ui
            .button("×")
            .on_hover_text("Close the File Manager")
            .clicked()
        {
            events.push(UiEvent::Invoke(Invocation::new(
                TOGGLE,
                json!({ "open": false }),
            )));
        }
    });
}

/// The set's files, each on a row (R11).
fn files(ui: &mut Ui, theme: &Theme, rows: &[FileRow], events: &mut Vec<UiEvent>) {
    section(ui, theme, "Files", rows.len());
    for row in rows {
        let rect = row_ui(ui, theme, row, "files", events);
        // Spec: file-management R11, I-DROP-012. A file dropped from the
        // browser on the row replaces it for every clip and device that
        // uses it.
        drop_file(ui, rect, &row.hash, events);
    }
}

/// The files that are missing, each on a row (R13).
fn missing(ui: &mut Ui, theme: &Theme, rows: &[FileRow], events: &mut Vec<UiEvent>) {
    section(ui, theme, "Missing Files", rows.len());
    for row in rows {
        let rect = row_ui(ui, theme, row, "missing", events);
        // Spec: file-management R13, I-DROP-012. A file dropped from the
        // browser on the row relinks it.
        drop_file(ui, rect, &row.hash, events);
    }
}

/// The Automatic Search: its switches, its folder, and Go (R14).
fn search(
    ui: &mut Ui,
    theme: &Theme,
    state: &FileManagerState,
    panel: &mut FileManagerUi,
    events: &mut Vec<UiEvent>,
) {
    section(ui, theme, "Automatic Search", 0);
    ui.horizontal(|ui| {
        if ui
            .button("Set Folder…")
            .on_hover_text("Choose the folder the search looks in")
            .clicked()
        {
            // The app asks the person for the folder.
            events.push(UiEvent::Invoke(Invocation::new(
                SET_SEARCH_FOLDER,
                json!({}),
            )));
        }
        if let Some(folder) = &state.search.folder {
            ui.label(folder);
        }
        let mut project = panel.search_project;
        ui.checkbox(&mut project, "Search Project");
        let mut library = panel.search_library;
        ui.checkbox(&mut library, "Search Library");
        panel.search_project = project;
        panel.search_library = library;
        if ui
            .button("Go")
            .on_hover_text("Search for the missing files")
            .clicked()
        {
            events.push(UiEvent::Invoke(Invocation::new(
                RELINK_MISSING,
                json!({
                    "project": panel.search_project,
                    "library": panel.search_library,
                    "folder": state.search.folder,
                }),
            )));
        }
    });
}

/// The files used from outside the project, grouped (R15).
fn external(
    ui: &mut Ui,
    theme: &Theme,
    state: &FileManagerState,
    panel: &mut FileManagerUi,
    events: &mut Vec<UiEvent>,
) {
    let count: usize = state.external.iter().map(|group| group.count).sum();
    section(ui, theme, "External Files", count);
    for group in &state.external {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} ({}, {} bytes)",
                group.name, group.count, group.size
            ));
            let switched = panel
                .collect
                .iter_mut()
                .find(|(kind, _)| *kind == group.group)
                .map(|(_, on)| *on)
                .unwrap_or(false);
            let mut on = switched;
            ui.checkbox(&mut on, "Collect Into Project");
            if let Some((_, current)) = panel
                .collect
                .iter_mut()
                .find(|(kind, _)| *kind == group.group)
            {
                *current = on;
            }
        });
        for file in &group.files {
            row_ui(ui, theme, file, "external", events);
        }
    }
}

/// The files in the project that nothing uses, grouped (R17).
fn unused(
    ui: &mut Ui,
    theme: &Theme,
    groups: &[UnusedFiles],
    panel: &mut FileManagerUi,
    events: &mut Vec<UiEvent>,
) {
    let count: usize = groups.iter().map(|group| group.count).sum();
    section(ui, theme, "Unused Files", count);
    for group in groups {
        ui.label(format!(
            "{} ({}, {} bytes)",
            group.name, group.count, group.size
        ));
        for file in &group.files {
            ui.horizontal(|ui| {
                ui.label(&file.name);
                // Spec: file-management R17. Delete asks first, warning that
                // another project may use the file.
                if panel.deleting.as_deref() == Some(file.path.as_str()) {
                    ui.label("Another project may use this file. Delete it?");
                    if ui.button("Delete it").clicked() {
                        events.push(UiEvent::Invoke(Invocation::new(
                            DELETE_UNUSED,
                            json!({ "paths": [file.path] }),
                        )));
                        panel.deleting = None;
                    }
                    if ui.button("Keep it").clicked() {
                        panel.deleting = None;
                    }
                } else if ui
                    .button("Delete")
                    .on_hover_text("Move it to the trash")
                    .clicked()
                {
                    panel.deleting = Some(file.path.clone());
                }
            });
        }
    }
}

/// The Collect and Save button, at the File Manager's foot (R15).
fn collect_and_save(
    ui: &mut Ui,
    state: &FileManagerState,
    panel: &mut FileManagerUi,
    events: &mut Vec<UiEvent>,
) {
    ui.separator();
    let groups: Vec<ExternalGroup> = panel
        .collect
        .iter()
        .filter(|(_, on)| *on)
        .map(|(kind, _)| *kind)
        .collect();
    let mut args = json!({ "groups": groups });
    if state.scope == Scope::Project
        && let Some(project) = &state.project
    {
        args["project"] = json!(project);
    }
    if ui.button("Collect and Save").clicked() {
        events.push(UiEvent::Invoke(Invocation::new(COLLECT_ALL, args)));
    }
}

/// One file's row: its name, its size, where it is, and what uses it.
fn row_ui(
    ui: &mut Ui,
    theme: &Theme,
    row: &FileRow,
    salt: &str,
    events: &mut Vec<UiEvent>,
) -> Rect {
    let id = ui.make_persistent_id(format!("file-manager-row-{salt}-{}", row.hash));
    let list = ui.make_persistent_id(format!("file-manager-rows-{salt}"));
    ui.ctx().accesskit_node_builder(list, |node| {
        node.set_role(Role::List);
        node.set_label("Files".to_owned());
    });
    ui.horizontal(|ui| {
        ui.label(format!(
            "{} — {} — {}",
            row.name,
            row.size,
            location_name(row.location)
        ));
        if !row.used_by.is_empty() {
            ui.label(format!("used by {}", row.used_by.join(", ")));
        }
        if ui
            .button("Hot-Swap")
            .on_hover_text("Choose another file for it")
            .clicked()
        {
            events.push(UiEvent::Invoke(Invocation::new(
                HOT_SWAP,
                json!({ "hash": row.hash }),
            )));
        }
        if ui
            .button("Edit")
            .on_hover_text("Open it in the external sample editor")
            .clicked()
        {
            events.push(UiEvent::Invoke(Invocation::new(
                EDIT_FILE,
                json!({ "hash": row.hash }),
            )));
        }
    });
    let rect = ui.min_rect();
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::ListItem);
        node.set_label(format!(
            "{}, {}, used by {}",
            row.name,
            location_name(row.location),
            if row.used_by.is_empty() {
                "nothing".to_owned()
            } else {
                row.used_by.join(", ")
            }
        ));
    });
    let _ = theme;
    rect
}

/// A file dropped from the browser on a row relinks it (R13, I-DROP-012).
fn drop_file(ui: &mut Ui, rect: Rect, hash: &str, events: &mut Vec<UiEvent>) {
    let ctx = ui.ctx().clone();
    let Some(payload) = egui::DragAndDrop::payload::<crate::browser::BrowserDrag>(&ctx) else {
        return;
    };
    if payload.plugin.is_some() {
        return;
    }
    let (released, pointer) = ctx.input(|i| {
        (
            i.pointer.any_released(),
            i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()),
        )
    });
    if !released {
        return;
    }
    let Some(pointer) = pointer else {
        return;
    };
    if !rect.contains(pointer) {
        return;
    }
    let Some(path) = payload.paths.first() else {
        return;
    };
    egui::DragAndDrop::clear_payload(&ctx);
    events.push(UiEvent::Invoke(Invocation::new(
        REPLACE_FILE,
        json!({ "hash": hash, "path": path }),
    )));
}

/// A section's name and how many rows it holds (R21).
fn section(ui: &mut Ui, theme: &Theme, name: &str, count: usize) {
    let id = ui.make_persistent_id(format!("file-manager-section-{name}"));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Group);
        node.set_label(format!("{name}, {count} files"));
    });
    ui.strong(format!("{name} ({count})"));
    let _ = theme;
}

/// Where a file is, as the rows say (R11).
fn location_name(location: Location) -> &'static str {
    match location {
        Location::Missing => "Missing",
        Location::UserLibrary => "User Library",
        Location::Project => "Project",
        Location::External => "External",
    }
}

/// An accessible region: a node with a role and a name that the elements
/// inside it hang from.
fn region(ui: &mut Ui, salt: &str, rect: Rect, role: Role, name: &str) -> egui::Id {
    let id = egui::Id::new(salt);
    let _ = ui.interact(rect, id, egui::Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(role);
        node.set_label(name.to_owned());
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
    id
}
