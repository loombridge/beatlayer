//! The set as a file: new, open, save, collect and relink.

use super::{Effect, Invocation, Message, Shell, ShellError, snapshot, to_value};
use crate::commands as ids;
use bl_commands::NoArgs;
use bl_shell_spec::dawproject as dawproject_ids;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;

use super::WindowKind;
use crate::commands::DefaultSetArgs;
use crate::commands::FileReport;
use crate::commands::PathArgs;
use crate::commands::SaveAsArgs;
use crate::commands::SetResult;
use crate::commands::SettingsArgs;
use crate::commands::TemplateArgs;
use crate::files::new_set_from;
use bl_shell_spec::devices as device_ids;
use bl_shell_spec::file_manager as file_manager_ids;

impl Shell {
    /// The bottom row of the Undo History panel: the set's start, which no
    /// step undoes. A set made new reads "New Beatlayer Set", and one
    /// opened from a file "Open Beatlayer Set".
    #[must_use]
    pub fn set_start_label(&self) -> &'static str {
        if self.set_path().is_some() {
            "Open Beatlayer Set"
        } else {
            "New Beatlayer Set"
        }
    }

    /// The set's file, if it has one.
    #[must_use]
    pub fn set_path(&self) -> Option<&Path> {
        self.set_path.as_deref()
    }

    /// The set's name: its file's name without the extension, or `Untitled`.
    #[must_use]
    pub fn set_name(&self) -> String {
        self.set_path
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(
                || {
                    self.pending_name
                        .clone()
                        .unwrap_or_else(|| "Untitled".to_owned())
                },
                |s| s.to_string_lossy().into_owned(),
            )
    }

    /// The folder the Save Set As dialog opens in: the open set's project
    /// folder, so keeping the suggestion saves into the same project
    /// (app-shell R22), or `None` for a set never saved, whose dialog opens
    /// where the system left it.
    #[must_use]
    pub fn save_dialog_folder(&self) -> Option<PathBuf> {
        self.set_path
            .as_deref()
            .map(|path| bl_project::ProjectFolder::of_set(path).root().to_owned())
    }

    /// The file name the plug-in list's Export… suggests: `<set name>
    /// Plug-Ins.txt` (the plug-in list spec, R7).
    #[must_use]
    pub fn plugin_list_file_name(&self) -> String {
        format!(
            "{} Plug-Ins.txt",
            bl_project::preset::file_safe(&self.set_name())
        )
    }

    /// Whether the set has changes that aren't saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the set has changes that aren't saved (the crash-recovery
    /// spec, R25): with Save Continuously on, a set that has a file has
    /// none, because every finished edit of it is committed within a
    /// second. A set that has never been saved still has some.
    #[must_use]
    pub fn unsaved_changes(&self) -> bool {
        self.dirty && !(self.prefs.save_continuously && self.set_path.is_some())
    }

    /// Lets go of the plug-ins of the set being replaced (WP-2.6): the new
    /// set's plug-in devices load their plug-ins from their saved state.
    pub(super) fn set_replaced(&mut self) {
        self.editor_replaced();
        // Spec: link R2. The new set takes the session's tempo.
        self.keep_tempo = true;
        self.forget_remote_values();
        // Spec: control-surfaces R9, R30. Every ring starts again.
        self.instant_reset();
        // What a DAWproject import said about the set that's gone (R11, R12).
        self.pending_name = None;
        self.import_report = None;
        self.dawproject_report_window = None;
        // A file still being written was for the set that's gone: its worker
        // deletes it when it finds nobody waiting (WP-4.20).
        self.processed.clear();
        self.pending_param_changes.clear();
        // A stem separation or autosample still running was for the set
        // that's gone: its clip, track or device would name something else
        // in this one.
        self.drop_separation();
        self.stop_autosample();
        self.autosample_dialog = None;
        self.autosample_result_value = None;
        if self.session_run.take().is_some() {
            // The recording's document is gone; its take ends unplaced.
            let _ = self
                .audio
                .send_session(bl_engine::SessionCommand::StopRecording);
            self.recording = None;
        }
        // A groove hot-swap was aimed at the set that's gone (R32).
        self.groove_pool.hot_swap = None;
        self.plugins.clear();
        self.au.clear();
        self.vst3.clear();
        self.lv2.clear();
        self.kit.clear();
    }

    /// A number that changes whenever the set is replaced (a new or opened
    /// one, or a recovered one), so what the agent API keeps about the old
    /// set, such as scheduled commands, names no objects of the new one.
    #[must_use]
    pub fn set_generation(&self) -> u64 {
        self.editor_generation
    }

    /// Notes that the editor was replaced, so the journal doesn't hold the
    /// new set as an edit of the old one (the crash-recovery spec, R2).
    pub(super) fn editor_replaced(&mut self) {
        self.editor_generation = self.editor_generation.wrapping_add(1);
    }

    /// Whether saving `clip` into `folder` (the User Library's `Clips` folder
    /// when `None`) as a clip file must ask the person first: Collect Files
    /// on Export is Ask and the clip or its track's devices play an audio
    /// file outside the project the file lands in (file management R8). The
    /// answer goes to `clip.save_file` as `copy`.
    #[must_use]
    pub fn clip_save_asks(&self, clip: bl_model::ClipId, folder: Option<&Path>) -> bool {
        let library = self.browser_view.user_library();
        let Some(folder) = folder
            .map(Path::to_path_buf)
            .or_else(|| library.map(|library| library.join("Clips")))
        else {
            return false;
        };
        self.prefs.collect_files_on_export == bl_shell_spec::settings::CollectFiles::Ask
            && crate::clip_files::uses_outside(
                self.editor.document(),
                &self.media,
                clip,
                &folder,
                library,
            )
    }

    /// Tells the browser the set's file changed, so Current Project follows
    /// it (browser R4). A set that isn't a file on disk (one a test or the
    /// benchmark holds in memory) has no project folder.
    pub(super) fn set_file_changed(&mut self) {
        let folder = self
            .set_path
            .as_deref()
            .filter(|path| path.is_file())
            .and_then(|path| std::path::absolute(path).ok())
            .and_then(|path| path.parent().map(Path::to_path_buf));
        self.browser_view.set_project(folder);
    }

    /// Where a default set goes when no file is named: the saved default
    /// set's file, the User Library's `Templates` folder, or beside the
    /// app's preferences (WP-2.25, file-management R2).
    pub(super) fn default_set_file(&self) -> Option<PathBuf> {
        if let Some(path) = &self.prefs.default_set {
            return Some(path.clone());
        }
        self.templates_folder()
            .or_else(|| self.config_dir.clone())
            .map(|folder| folder.join(format!("Default Set.{}", bl_project::SET_EXTENSION)))
    }

    /// The file `path` names for a default set, with the `.blset` extension
    /// added when it's missing (WP-2.25).
    pub(super) fn default_set_file_at(&self, path: &Path) -> PathBuf {
        if bl_project::is_set(path) {
            return path.to_owned();
        }
        let mut name = path.as_os_str().to_owned();
        name.push(".");
        name.push(bl_project::SET_EXTENSION);
        PathBuf::from(name)
    }

    /// The User Library's `Templates` folder, where templates are saved
    /// (WP-2.25, file-management R3).
    pub(super) fn templates_folder(&self) -> Option<PathBuf> {
        self.browser_view
            .user_library()
            .map(|library| library.join("Templates"))
    }

    /// The Automatic Search's switches as they are unless asked otherwise
    /// (file-management R14): the set's project folder and the User
    /// Library are searched.
    pub(super) fn default_search() -> bl_shell_spec::file_manager::Search {
        bl_shell_spec::file_manager::Search {
            folder: None,
            project: true,
            library: true,
            candidates: Vec::new(),
        }
    }

    /// Searches for the open set's missing or changed files and relinks them
    /// (WP-2.25, file-management R6 and R14): the places the search is
    /// switched on for, each with its subfolders, and the folders the files
    /// were last in. A file with one candidate is relinked to it; a file
    /// with several keeps its row, and the person chooses. Returns how many
    /// were relinked, and the candidates the others have.
    pub(super) fn relink_missing(
        &mut self,
        extra: &[PathBuf],
        search: &bl_shell_spec::file_manager::Search,
    ) -> (usize, Vec<bl_shell_spec::file_manager::Candidate>) {
        let mut folders: Vec<PathBuf> = Vec::new();
        if search.project
            && let Some(folder) = self.set_path.as_deref().and_then(Path::parent)
        {
            folders.push(folder.to_path_buf());
        }
        if search.library
            && let Some(library) = self.browser_view.user_library()
        {
            folders.push(library.to_path_buf());
        }
        if let Some(folder) = &search.folder {
            folders.push(PathBuf::from(folder));
        }
        for entry in self.media.entries() {
            if let Some(folder) = entry.path.parent() {
                folders.push(folder.to_path_buf());
            }
        }
        folders.extend(extra.iter().cloned());
        // Keep each folder once, in the order the search should try them.
        let mut seen = std::collections::HashSet::new();
        folders.retain(|folder| seen.insert(folder.clone()));
        let Ok(candidates) = self.media.relink_candidates(&folders) else {
            return (0, Vec::new());
        };
        let mut found = Vec::new();
        let mut several = Vec::new();
        for (hash, paths) in candidates {
            if paths.len() == 1 {
                self.media.relocate(hash, &paths[0]);
                found.push((bl_model::ContentHash(*hash.as_bytes()), paths[0].clone()));
            } else {
                several.push(bl_shell_spec::file_manager::Candidate {
                    hash: hash
                        .as_bytes()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                    name: paths
                        .first()
                        .and_then(|path| path.file_name())
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                    paths: paths
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                });
            }
        }
        self.store.relink(&found);
        // A relinked file's failed analysis is made again (R20).
        for (hash, _) in &found {
            let hash = bl_media::ContentHash::from_bytes(hash.0);
            if let Some(cache) = &self.analysis {
                cache.forget(hash);
            }
            self.analysed.remove(&hash);
        }
        self.start_analyses();
        (found.len(), several)
    }

    /// Runs what the save-changes dialog decided, or what follows a save it
    /// asked for: the unsaved changes go (`set.discard_changes`, a no-op
    /// after a save), then `then` runs without asking again. Opening a set
    /// asks for the file first and discards only once one is chosen, so
    /// cancelling the file dialog keeps the changes.
    ///
    /// # Errors
    ///
    /// As [`Shell::run`].
    pub fn discard_and_invoke(&mut self, then: &Invocation) -> Result<Value, ShellError> {
        if then.id == ids::SET_OPEN && then.args.get("path").is_none() {
            self.effects.push(Effect::PickOpenPath {
                discard_changes: true,
            });
            self.touch();
            return Ok(Value::Null);
        }
        self.invoke(ids::SET_DISCARD_CHANGES, json!({}))?;
        self.invoke(&then.id, then.args.clone())
    }

    /// Runs a DAWproject command (WP-6.36): opening a `.dawproject` file as
    /// a new set, or reading back the report of the last import.
    ///
    /// Spec: dawproject R10 to R12. The import is refused while the open
    /// set has unsaved changes; the reading, the extracting and the building
    /// run on a worker thread; the set changes only when the new set is
    /// whole, and a failure leaves the open set as it was.
    pub(super) fn run_dawproject(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            dawproject_ids::IMPORT_DAWPROJECT => {
                let args = Self::args::<dawproject_ids::ImportDawProjectArgs>(id, args)?;
                if self.unsaved_changes() {
                    return Err(unavailable(
                        "the set has unsaved changes; save them with `set.save` or \
                         `set.save_as`, or throw them away with `set.discard_changes`, first",
                    ));
                }
                let path = PathBuf::from(&args.path);
                let folder = crate::dawproject::import_folder(&self.temporary_folder(), &path);
                let settings = crate::dawproject::Settings {
                    default_warp_mode: crate::auto_warp::warp_mode(
                        self.prefs.warp.default_warp_mode,
                    ),
                    track_colours: self.track_palette.clone(),
                    look: self.prefs.look.clone(),
                    installed: self.installed_plugins(),
                };
                let mut job = crate::dawproject::Job::start(&path, &folder, settings)
                    .map_err(|error| failed(error.to_string()))?;
                // An agent's call returns when the new set is open (R10).
                let imported = loop {
                    if let Some(result) = job.poll() {
                        break result;
                    }
                    // The worker asks for a plug-in's parameters here, on
                    // the thread that owns the plug-in hosts (R57).
                    job.serve(|plugin| self.describe_plugin(plugin));
                    self.set_message(
                        format!("Importing {} ({}%)", path.display(), job.percent()),
                        false,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                };
                let imported = imported.map_err(|error| {
                    self.set_message(error.to_string(), true);
                    failed(error.to_string())
                })?;
                // The set changes only when the new set is whole.
                self.store.close();
                for (hash, at, size) in &imported.media {
                    self.store.add_media(*hash, at, *size);
                    let hash = bl_media::ContentHash::from_bytes(hash.0);
                    if self.media.entry(hash).is_none() {
                        let _ = self.media.import(at);
                    }
                }
                self.saved = snapshot(&imported.editor);
                self.set_replaced();
                self.editor = imported.editor;
                self.set_path = None;
                // Spec: dawproject R11. Until the set is first saved its
                // name is the file's name without `.dawproject`.
                self.pending_name = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned());
                self.dirty = false;
                self.message = None;
                self.arrangement.reset();
                self.clip_view.forget_selection();
                self.mixer.reset();
                self.session.reset();
                self.new_set_transport();
                self.document_changed();
                self.set_file_changed();
                self.import_report = Some(imported.report.clone());
                Ok(to_value(&dawproject_ids::ImportDawProjectResult {
                    name: self.set_name(),
                    report: imported.report,
                }))
            }
            dawproject_ids::EXPORT_DAWPROJECT => {
                let args = Self::args::<dawproject_ids::ExportDawProjectArgs>(id, args)?;
                if args.path.is_empty() {
                    return Err(failed("`path` is empty".to_owned()));
                }
                // Spec: dawproject R65. `.dawproject` is added when it is
                // missing, and a file already there is refused unless
                // `replace` is set.
                let mut path = PathBuf::from(&args.path);
                if path
                    .extension()
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("dawproject"))
                {
                    let mut name = path.clone().into_os_string();
                    name.push(".dawproject");
                    path = PathBuf::from(name);
                }
                if path.exists() && !args.replace {
                    return Err(failed(format!(
                        "{} is already there; pass `replace` to write over it",
                        path.display()
                    )));
                }
                // The plug-ins' states are taken as a save takes them.
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.lv2,
                    &self.kit,
                    &mut self.editor,
                );
                let document = self.editor.document().clone();
                let media = crate::dawproject::media_used(&document)
                    .into_iter()
                    .filter_map(|hash| {
                        let found = self
                            .media
                            .entry(bl_media::ContentHash::from_bytes(hash.0))?;
                        Some((hash, found.path))
                    })
                    .collect();
                let settings = crate::dawproject::ExportSettings {
                    document,
                    title: self.set_name(),
                    track_colours: self.track_palette.clone(),
                    installed: self.installed_plugins(),
                    media,
                    embed_media: args.embed_media,
                };
                let mut job = crate::dawproject::ExportJob::start(&path, settings)
                    .map_err(|error| failed(error.to_string()))?;
                let result = loop {
                    if let Some(result) = job.poll() {
                        break result;
                    }
                    // The worker asks for a plug-in's parameters here, on
                    // the thread that owns the plug-in hosts (R76).
                    job.serve(|plugin| self.describe_plugin(plugin));
                    self.set_message(
                        format!("Exporting {} ({}%)", path.display(), job.percent()),
                        false,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                };
                let result = result.map_err(|error| {
                    self.set_message(error.to_string(), true);
                    failed(error.to_string())
                })?;
                self.set_message(format!("Exported {}", path.display()), false);
                Ok(to_value(&result))
            }
            dawproject_ids::IMPORT_REPORT => {
                Self::args::<NoArgs>(id, args)?;
                let report = self.import_report.clone().unwrap_or_default();
                Ok(to_value(&report))
            }
            dawproject_ids::CLOSE_REPORT => {
                Self::args::<NoArgs>(id, args)?;
                self.dawproject_report_window = None;
                self.touch();
                Ok(to_value(&dawproject_ids::ReportWindowState { open: false }))
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }

    /// The command a person's choice of `path` runs: the import for a
    /// `.dawproject` file, the open for anything else (the DAWproject spec,
    /// R83 to R85).
    #[must_use]
    pub fn open_invocation(path: &Path) -> Invocation {
        let id = if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dawproject"))
        {
            dawproject_ids::IMPORT_DAWPROJECT
        } else {
            ids::SET_OPEN
        };
        Invocation::new(id, json!({ "path": path.to_string_lossy() }))
    }

    /// The window that lists the report of the import or export a person
    /// just ran, if it is open (R87).
    #[must_use]
    pub fn dawproject_report_window(&self) -> Option<&dawproject_ids::ReportWindow> {
        self.dawproject_report_window.as_ref()
    }

    /// The file name the Export DAWproject... dialog suggests: the set's
    /// name with `.dawproject` (R86).
    #[must_use]
    pub fn dawproject_file_name(&self) -> String {
        format!(
            "{}.dawproject",
            bl_project::preset::file_safe(&self.set_name())
        )
    }

    /// Runs a person's import or export, which has its path: a report with
    /// entries opens the report window, and one without says so in the
    /// Status Bar (R87).
    pub(super) fn run_dawproject_for_person(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let result = self.run(id, args)?;
        let export = id == dawproject_ids::EXPORT_DAWPROJECT;
        let report: dawproject_ids::ImportReport =
            serde_json::from_value(result["report"].clone()).unwrap_or_default();
        if !export {
            self.set_message(format!("Imported {}", self.set_name()), false);
        }
        if !report.entries.is_empty() {
            self.dawproject_report_window = Some(dawproject_ids::ReportWindow { export, report });
            self.touch();
        }
        Ok(result)
    }

    /// The plug-ins the scanner found, for an import to look its plug-ins up
    /// among (the DAWproject spec, R46).
    fn installed_plugins(&self) -> Vec<bl_interop::dawproject::Installed> {
        self.scanner
            .plugins()
            .into_iter()
            .map(|plugin| bl_interop::dawproject::Installed {
                format: plugin.format,
                plugin_id: plugin.plugin_id,
                path: plugin.path.to_string_lossy().into_owned(),
                name: plugin.name,
                vendor: plugin.vendor,
                version: plugin.version,
                role: plugin.role,
            })
            .collect()
    }

    /// Loads an installed plug-in and describes its parameters, for the
    /// automation an import maps through them (the DAWproject spec, R57).
    fn describe_plugin(
        &mut self,
        plugin: &bl_interop::dawproject::Installed,
    ) -> Result<Vec<bl_model::ParamDescriptor>, String> {
        use bl_model::PluginFormat;

        let path = std::path::Path::new(&plugin.path);
        let types: Vec<bl_engine::DeviceType> = match plugin.format {
            PluginFormat::Clap => self
                .plugins
                .load(path)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|t| t.device_type)
                .collect(),
            PluginFormat::Vst3 => self
                .vst3
                .load(path)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|t| t.device_type)
                .collect(),
            PluginFormat::AudioUnit => {
                let (kind, subtype, manufacturer) =
                    bl_plugin_host::au::parse_description_id(&plugin.plugin_id)
                        .ok_or("its ID doesn't read")?;
                vec![
                    self.au
                        .load(kind, subtype, manufacturer)
                        .map_err(|e| e.to_string())?
                        .device_type,
                ]
            }
            PluginFormat::Lv2 => return Err("a DAWproject file holds no LV2 plug-ins".to_owned()),
        };
        self.refresh_device_types();
        types
            .into_iter()
            .find(|t| {
                matches!(&t.kind, bl_model::DeviceKind::Plugin { plugin_id, .. }
                    if *plugin_id == plugin.plugin_id)
            })
            .map(|t| t.description.params)
            .ok_or_else(|| "the file doesn't hold that plug-in".to_owned())
    }

    /// A new or opened set starts stopped at 1.1.1
    /// (the control bar transport spec, R16), with no gesture open
    /// on the editor it replaced.
    pub(super) fn new_set_transport(&mut self) {
        self.gesture = false;
        self.key_edit = None;
        // The recordings and automation takes under way belong to the set
        // that was replaced: finishing them would put them in this one.
        self.recording = None;
        self.pending_recording = None;
        self.session_run = None;
        self.session_automation = crate::automation_record::SessionTake::default();
        self.automation_take = crate::automation_record::Take::default();
        self.automation_recording = false;
        self.audio.reset_transport();
        // Spec: launch-quantisation R1. The set's global quantisation, so
        // launching waits for the right grid (the engine defaults to it too,
        // so this only matters for a set that was saved with another).
        self.audio.set_launch_settings(self.store.launch());
    }

    /// Records a save to `path`: it's the set's file, the set is clean, and
    /// warnings from the save show in the Status Bar.
    pub(super) fn saved_as(&mut self, path: PathBuf, warnings: &[String]) {
        self.saved = snapshot(&self.editor);
        self.set_path = Some(path);
        self.set_file_changed();
        self.dirty = false;
        self.save_history.saves = self.save_history.saves.wrapping_add(1);
        // Spec: crash-recovery R6. The save holds every edit, so the
        // journal retires.
        self.journal_saved();
        self.message = (!warnings.is_empty()).then(|| Message {
            text: format!("The set was saved, but: {}", warnings.join("; ")),
            error: false,
            action: None,
        });
    }

    /// Runs the `set`, `app` and `window` commands.
    pub(super) fn run_set_files(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let failed = |message: String| ShellError::Failed {
            command: id.to_owned(),
            message,
        };
        let unavailable = |reason: &str| ShellError::Unavailable {
            command: id.to_owned(),
            reason: reason.to_owned(),
        };
        match id {
            ids::SET_NEW => {
                Self::args::<NoArgs>(id, args)?;
                self.set_replaced();
                // Spec: file-management R1. A new set starts from the saved
                // default set when there is one.
                let (editor, window_views) = new_set_from(self.prefs.default_set.as_deref());
                self.saved = snapshot(&editor);
                self.editor = editor;
                self.store.close();
                self.apply_window_views(window_views);
                self.devices.set_windows(bl_model::DeviceWindows::new());
                self.devices.set_rack_views(bl_model::RackViewMap::new());
                self.devices
                    .set_multisampler_views(bl_model::MultisamplerViews::new());
                self.clip_view.set_folds(bl_model::ClipFolds::new());
                self.clip_view.set_scale_folds(bl_model::ClipFolds::new());
                self.clip_view.restore_mpe(bl_model::MpeView::default());
                self.clip_view
                    .restore_midi_tools(&bl_project::midi_tools::MidiToolSettings::default());
                self.set_path = None;
                self.dirty = false;
                self.message = None;
                self.arrangement.reset();
                self.clip_view.forget_selection();
                self.mixer.reset();
                self.session.reset();
                self.new_set_transport();
                self.document_changed();
                self.set_file_changed();
                // Spec: crash-recovery R21. The new set is journalled from
                // the start, and the set that was open keeps its unsaved
                // edits for recovery (R7).
                self.begin_journal();
                Ok(self.set_result())
            }
            ids::SET_OPEN => {
                let PathArgs { path } = Self::args(id, args)?;
                if self.unsaved_changes() {
                    return Err(unavailable(
                        "the set has unsaved changes; save them with `set.save` or \
                         `set.save_as`, or throw them away with `set.discard_changes`, first",
                    ));
                }
                let (editor, opened) = self.store.open(Path::new(&path)).map_err(failed)?;
                // Spec: device-chains R29. Floating windows reopen where
                // they were saved.
                self.devices.set_windows(self.store.windows());
                // Spec: racks R19. So do the views each rack shows.
                self.devices.set_rack_views(self.store.rack_views());
                self.devices
                    .set_multisampler_views(self.store.multisampler_views());
                // Spec: piano-roll. Folded note editors too.
                self.clip_view.set_folds(self.store.folds());
                self.clip_view.set_scale_folds(self.store.scale_folds());
                self.clip_view.restore_mpe(self.store.mpe_view());
                self.clip_view.restore_midi_tools(&self.store.midi_tools());
                self.saved = snapshot(&editor);
                self.set_replaced();
                self.editor = editor;
                // Spec: second-window R14. The set opens with the main view
                // and the second window it was saved with.
                self.apply_window_views(self.store.window_views());
                self.set_path = Some(opened);
                self.dirty = false;
                self.message = None;
                self.arrangement.reset();
                self.arrangement.restore_lanes(self.store.lane_views());
                self.clip_view.forget_selection();
                self.mixer.reset();
                self.mixer.restore_widths(self.store.column_widths());
                self.session.reset();
                // The set's files go into the media store, so its clips'
                // waveforms build (browser R15) and the engine finds their
                // audio when the set reaches it.
                for (hash, path, size) in self.store.media_files() {
                    self.media
                        .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
                }
                // Spec: file-management R6, R14. Files that moved are found
                // again by content, so the set opens able to play them: a
                // file with one candidate is relinked to it, and a file with
                // several waits for the person to choose.
                let (relinked, candidates) = self.relink_missing(&[], &Self::default_search());
                self.file_manager.search.candidates = candidates;
                if relinked > 0 {
                    self.message = Some(Message {
                        text: format!(
                            "{relinked} missing file{} found again and relinked",
                            if relinked == 1 { " was" } else { "s were" }
                        ),
                        error: false,
                        action: None,
                    });
                }
                if let Some(notice) = self.store.open_notice() {
                    let text = match self.message.take() {
                        Some(relinked) => format!("{}; {notice}", relinked.text),
                        None => notice,
                    };
                    self.message = Some(Message {
                        text,
                        error: false,
                        action: None,
                    });
                }
                self.new_set_transport();
                self.document_changed();
                self.set_file_changed();
                // Spec: crash-recovery R21. The opened set is journalled
                // from the start.
                self.begin_journal();
                Ok(self.set_result())
            }
            ids::SET_SAVE => {
                Self::args::<NoArgs>(id, args)?;
                let path = self.set_path.clone().ok_or_else(|| {
                    unavailable("the set has never been saved; use `set.save_as` to give it a file")
                })?;
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.lv2,
                    &self.kit,
                    &mut self.editor,
                );
                self.store.set_windows(self.devices.windows().clone());
                self.store.set_rack_views(self.devices.rack_views().clone());
                self.store
                    .set_multisampler_views(self.devices.multisampler_views().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                self.store
                    .set_scale_folds(self.clip_view.scale_folds().clone());
                self.store.set_mpe_view(self.clip_view.mpe().clone());
                self.store
                    .set_lane_views(self.arrangement.lane_views().clone());
                self.store.set_column_widths(self.mixer.widths().clone());
                self.store
                    .set_midi_tools(self.clip_view.midi_tools().saved());
                let warnings = self.store.save(&path, &self.editor).map_err(failed)?;
                self.saved_as(path, &warnings);
                Ok(self.set_result())
            }
            ids::SET_SAVE_AS => {
                // Spec: app-shell R17 to R21. The store decides where in a
                // project folder the set goes; the result says where.
                let SaveAsArgs { path } = Self::args(id, args)?;
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.lv2,
                    &self.kit,
                    &mut self.editor,
                );
                self.store.set_windows(self.devices.windows().clone());
                self.store.set_rack_views(self.devices.rack_views().clone());
                self.store
                    .set_multisampler_views(self.devices.multisampler_views().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                self.store
                    .set_scale_folds(self.clip_view.scale_folds().clone());
                self.store.set_mpe_view(self.clip_view.mpe().clone());
                self.store
                    .set_lane_views(self.arrangement.lane_views().clone());
                self.store.set_column_widths(self.mixer.widths().clone());
                self.store
                    .set_midi_tools(self.clip_view.midi_tools().saved());
                self.store.set_temporary_folder(self.temporary_folder());
                let (path, warnings) = self
                    .store
                    .save_as(Path::new(&path), &self.editor)
                    .map_err(failed)?;
                // Processed files the first save moved out of the Temporary
                // Folder: the media store follows them.
                for (hash, now, _) in self.store.media_files() {
                    let hash = bl_media::ContentHash::from_bytes(hash.0);
                    if self.media.entry(hash).is_some_and(|e| !e.path.exists()) && now.is_file() {
                        let _ = self.media.import(&now);
                    }
                }
                self.saved_as(path, &warnings);
                Ok(self.set_result())
            }
            ids::SET_DISCARD_CHANGES => {
                Self::args::<NoArgs>(id, args)?;
                if self.dirty {
                    self.set_replaced();
                    self.editor = snapshot(&self.saved);
                    self.new_set_transport();
                    self.dirty = false;
                    self.document_changed();
                    // Spec: crash-recovery R6. The changes are gone, so
                    // nothing is offered at the next launch.
                    self.close_journal();
                    // The set is still open, so its later edits are
                    // journalled from now on.
                    self.begin_journal();
                }
                Ok(self.set_result())
            }
            ids::SET_LATER_RECOVERY => {
                // Spec: crash-recovery R22. The prompt closes; every set it
                // listed is still offered at the next launch.
                Self::args::<NoArgs>(id, args)?;
                self.recovery_deferred = true;
                Ok(to_value(&ids::RecoveryPrompt {
                    open: self.recovery_prompt_open(),
                }))
            }
            ids::SET_SAVE_AS_DEFAULT => {
                // Spec: file-management R2. The open set becomes the one a
                // new set starts from, saved as a copy so it keeps its own
                // file and stays open.
                let DefaultSetArgs { path } = Self::args(id, args)?;
                let path = match path {
                    Some(path) => self.default_set_file_at(Path::new(&path)),
                    None => self.default_set_file().ok_or_else(|| {
                        unavailable(
                            "there's nowhere to save the default set: give `path`, or set up a \
                             User Library",
                        )
                    })?,
                };
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.lv2,
                    &self.kit,
                    &mut self.editor,
                );
                let warnings = self.store.save_copy(&path, &self.editor).map_err(failed)?;
                self.prefs.default_set = Some(path.clone());
                self.save_preferences();
                self.message = (!warnings.is_empty()).then(|| Message {
                    text: format!("The default set was saved, but: {}", warnings.join("; ")),
                    error: false,
                    action: None,
                });
                Ok(to_value(&SetResult {
                    path: Some(path.to_string_lossy().into_owned()),
                    name: self.set_name(),
                }))
            }
            ids::SET_SAVE_AS_TEMPLATE => {
                // Spec: file-management R3. Templates live in the User
                // Library's `Templates` folder, as in Live.
                let TemplateArgs { name } = Self::args(id, args)?;
                let folder = self
                    .templates_folder()
                    .ok_or_else(|| unavailable("there's no User Library to save a template in"))?;
                let name = name.unwrap_or_else(|| self.set_name());
                if name.is_empty() || name.contains(['/', '\\']) {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: "a template's name can't be empty or hold a path separator"
                            .to_owned(),
                    });
                }
                let path = folder.join(format!("{name}.{}", bl_project::SET_EXTENSION));
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.lv2,
                    &self.kit,
                    &mut self.editor,
                );
                let warnings = self.store.save_copy(&path, &self.editor).map_err(failed)?;
                self.message = (!warnings.is_empty()).then(|| Message {
                    text: format!("The template was saved, but: {}", warnings.join("; ")),
                    error: false,
                    action: None,
                });
                Ok(to_value(&SetResult {
                    path: Some(path.to_string_lossy().into_owned()),
                    name,
                }))
            }
            ids::SET_COLLECT_ALL => {
                // Spec: file-management R4, R15, R16. The groups to collect
                // from, and the project to collect for.
                let crate::commands::CollectArgs { groups, project } = Self::args(id, args)?;
                let library = self.browser_view.user_library();
                let groups = if groups.is_empty() {
                    vec![
                        file_manager_ids::ExternalGroup::OtherProject,
                        file_manager_ids::ExternalGroup::UserLibrary,
                        file_manager_ids::ExternalGroup::Elsewhere,
                    ]
                } else {
                    groups
                };
                if let Some(project) = project {
                    let folder = PathBuf::from(project);
                    let files = self
                        .collect_project(&folder, &groups, library)
                        .map_err(failed)?;
                    return Ok(to_value(&FileReport {
                        path: Some(folder.to_string_lossy().into_owned()),
                        files,
                        candidates: Vec::new(),
                    }));
                }
                let path = self.set_path.clone().ok_or_else(|| {
                    unavailable(
                        "the set has never been saved; use `set.save_as` to give it a project \
                         folder to collect into",
                    )
                })?;
                crate::plugins::capture_states(
                    &self.plugins,
                    &self.au,
                    &self.vst3,
                    &self.lv2,
                    &self.kit,
                    &mut self.editor,
                );
                self.store.set_windows(self.devices.windows().clone());
                self.store.set_rack_views(self.devices.rack_views().clone());
                self.store
                    .set_multisampler_views(self.devices.multisampler_views().clone());
                self.store.set_folds(self.clip_view.folds().clone());
                self.store
                    .set_scale_folds(self.clip_view.scale_folds().clone());
                self.store.set_mpe_view(self.clip_view.mpe().clone());
                self.store
                    .set_lane_views(self.arrangement.lane_views().clone());
                self.store.set_column_widths(self.mixer.widths().clone());
                self.store
                    .set_midi_tools(self.clip_view.midi_tools().saved());
                let files = self
                    .store
                    .collect_groups(&groups, library)
                    .map_err(failed)?;
                let warnings = self.store.save(&path, &self.editor).map_err(failed)?;
                self.saved_as(path.clone(), &warnings);
                Ok(to_value(&FileReport {
                    path: Some(path.to_string_lossy().into_owned()),
                    files,
                    candidates: Vec::new(),
                }))
            }
            ids::SET_RELINK_MISSING => {
                // Spec: file-management R14. The places the search is
                // switched on for, and the folders it is given.
                let crate::commands::RelinkArgs {
                    folders,
                    project,
                    library,
                    folder,
                } = Self::args(id, args)?;
                let extra: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
                let search = bl_shell_spec::file_manager::Search {
                    folder: folder.clone(),
                    project: project.unwrap_or(true),
                    library: library.unwrap_or(true),
                    candidates: Vec::new(),
                };
                let (files, candidates) = self.relink_missing(&extra, &search);
                self.file_manager.search = search.clone();
                self.file_manager.search.candidates = candidates.clone();
                Ok(to_value(&FileReport {
                    path: self
                        .set_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    files,
                    candidates,
                }))
            }
            ids::APP_QUIT => {
                Self::args::<NoArgs>(id, args)?;
                self.audio.set_test_tone(false);
                self.save_preferences();
                // Spec: crash-recovery R21. The set is closed, so nothing
                // is offered at the next launch.
                self.close_journal();
                self.effects.push(Effect::Quit);
                Ok(Value::Null)
            }
            ids::APP_HIDE => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::Hide);
                Ok(Value::Null)
            }
            ids::APP_ABOUT => {
                Self::args::<NoArgs>(id, args)?;
                self.effects.push(Effect::ShowAbout);
                Ok(Value::Null)
            }
            ids::APP_SETTINGS => {
                let SettingsArgs { page } = Self::args(id, args)?;
                if let Some(page) = page {
                    self.prefs.settings_page = page;
                }
                self.settings_open = true;
                self.templates_for_page();
                self.focused_window = WindowKind::Settings;
                self.effects.push(Effect::OpenSettings);
                Ok(Value::Null)
            }
            ids::WINDOW_CLOSE => {
                Self::args::<NoArgs>(id, args)?;
                if let WindowKind::Device(device) = self.focused_window {
                    // Spec: device-chains R27. Closing a floating device's
                    // window docks the device: one undo step.
                    self.focused_window = WindowKind::Main;
                    return self.run(
                        device_ids::SET_FLOATING,
                        json!({ "device": device, "floating": false }),
                    );
                }
                if self.focused_window == WindowKind::Assistant {
                    self.focused_window = WindowKind::Main;
                    self.assistant_open = false;
                    self.touch();
                    return Ok(Value::Null);
                }
                if self.focused_window == WindowKind::Second {
                    // Spec: second-window R1. Close Window closes the second
                    // window with it in front.
                    return self.run_window_view(ids::VIEW_TOGGLE_SECOND_WINDOW, json!({}));
                }
                if !self.settings_open {
                    return Err(unavailable("no secondary window is open"));
                }
                self.close_settings();
                Ok(Value::Null)
            }
            _ => Err(ShellError::UnknownCommand(id.to_owned())),
        }
    }
}
