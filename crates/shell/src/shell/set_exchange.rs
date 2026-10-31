//! Merging sets and saving tracks or Session clips as a set (WP-6.35): the
//! commands `set.import`, `set.export_tracks` and `set.export_clips`.
//!
//! The behaviour is in the set exchange spec, R3 to R9 and R11 to R15; reading
//! a set and building a new one are [`crate::set_exchange`].

use bl_model::{Command, Device, DeviceId, DeviceRole, Input, TrackId, TrackKind};
use bl_shell_spec::set_exchange::{
    self as ids, ExportClipsArgs, ExportTracksArgs, ExportedSet, ImportArgs, Imported,
};
use bl_shell_spec::settings::CollectFiles;

use super::*;
use crate::set_exchange::{self, Built, Opened};
use crate::tracks::TrackData;

impl Shell {
    /// Runs the set exchange commands (WP-6.35).
    pub(super) fn run_set_exchange(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::IMPORT => self.run_set_import(id, args),
            ids::EXPORT_TRACKS => self.run_export_tracks(id, args),
            ids::EXPORT_CLIPS => self.run_export_clips(id, args),
            ids::CREATE_PACK | ids::INSTALL_PACK | ids::CANCEL_PACK | ids::PACK_STATUS => {
                self.run_pack(id, args)
            }
            other => Err(ShellError::UnknownCommand(other.to_owned())),
        }
    }

    fn run_set_import(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ImportArgs {
            path,
            tracks,
            devices,
            at,
            drop_area,
        } = Self::args(id, args)?;
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let invalid = |message: String| ShellError::InvalidArgs {
            command: id.to_owned(),
            message,
        };
        let file = std::path::absolute(&path).map_err(|e| invalid(e.to_string()))?;
        // A set that can't be read changes nothing (R3).
        let opened = set_exchange::open(&file).map_err(unavailable)?;
        if let Some(at) = at
            && self.editor.document().track(at).is_none()
        {
            return Err(invalid(format!("the open set has no track {}", at.0.get())));
        }
        let imported = match devices {
            Some(source) => {
                if tracks.is_some() {
                    return Err(invalid("give `tracks` or `devices`, not both".to_owned()));
                }
                self.import_devices(id, &opened, source, at, drop_area)?
            }
            None => self.import_tracks(id, &opened, tracks, at, drop_area)?,
        };
        if imported.missing_files > 0
            && let Some(message) = crate::browser::arrange::missing_message(imported.missing_files)
        {
            self.set_message(message, false);
        }
        self.dirty = true;
        self.document_changed();
        self.touch();
        Ok(to_value(&imported))
    }

    /// Adds tracks of `opened` (R3 to R7, R9): `tracks`, or all of them but
    /// its return tracks and Main.
    fn import_tracks(
        &mut self,
        id: &str,
        opened: &Opened,
        tracks: Option<Vec<TrackId>>,
        at: Option<TrackId>,
        drop_area: bool,
    ) -> Result<Imported, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let source = &opened.project.document;
        let wanted = tracks.unwrap_or_else(|| opened.top_tracks());
        for &track in &wanted {
            match source.track(track).map(bl_model::Track::kind) {
                None => {
                    return Err(ShellError::InvalidArgs {
                        command: id.to_owned(),
                        message: format!("the set has no track {}", track.0.get()),
                    });
                }
                Some(TrackKind::Return | TrackKind::Main) => {
                    return Err(unavailable(format!(
                        "track {} is a return track or Main, whose chain of devices can be \
                         added with `devices`",
                        track.0.get()
                    )));
                }
                Some(_) => {}
            }
        }
        let data: Vec<TrackData> = crate::tracks::duplicated(source, &wanted)
            .into_iter()
            .filter_map(|track| crate::tracks::snapshot(source, track))
            .collect();
        if data.is_empty() {
            return Err(unavailable("the set has no tracks to add".to_owned()));
        }
        let after = at.or_else(|| {
            if drop_area {
                None
            } else {
                self.last_selected_track()
            }
        });
        let place = self.place_after(after);
        let missing = self.register_set_media(opened, &data);
        let scenes_before = self.editor.document().scene_count();
        let copies = crate::tracks::copy_tracks(
            &mut self.editor,
            &data,
            place,
            Some(source.returns().collect()),
            ids::IMPORT_TRACKS_STEP,
        )
        .map_err(unavailable)?;
        self.select_tracks(&copies);
        Ok(Imported {
            tracks: copies,
            devices: Vec::new(),
            scenes_added: self.editor.document().scene_count() - scenes_before,
            missing_files: missing,
        })
    }

    /// Adds the chain of devices of track `source` of `opened` (R8, R9) to
    /// `at`, the selected track, or a new track.
    fn import_devices(
        &mut self,
        id: &str,
        opened: &Opened,
        source: TrackId,
        at: Option<TrackId>,
        drop_area: bool,
    ) -> Result<Imported, ShellError> {
        let unavailable = |reason: String| ShellError::Unavailable {
            command: id.to_owned(),
            reason,
        };
        let from =
            opened
                .project
                .document
                .track(source)
                .ok_or_else(|| ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("the set has no track {}", source.0.get()),
                })?;
        let chain: Vec<Device> = from.devices().cloned().collect();
        if chain.is_empty() {
            return Err(unavailable(format!("{} has no devices", from.name())));
        }
        let target = at.or_else(|| {
            if drop_area {
                None
            } else {
                self.arrangement.selected_tracks().first().copied()
            }
        });
        let doc = self.editor.document();
        let kind = match target {
            Some(track) => doc.track(track).map(bl_model::Track::kind),
            // A MIDI track for a chain with an instrument or MIDI effects,
            // an audio track otherwise (R8).
            None if chain.iter().any(|d| d.role != DeviceRole::AudioEffect) => {
                Some(TrackKind::Midi)
            }
            None => Some(TrackKind::Audio),
        };
        let kind = kind.ok_or_else(|| unavailable("the target track is gone".to_owned()))?;
        if let Some(device) = chain.iter().find(|d| !d.role.fits(kind)) {
            return Err(unavailable(format!(
                "{} can't go on a {kind} track",
                device.title()
            )));
        }
        let name = from.name().to_owned();
        let files: Vec<TrackData> = vec![TrackData {
            id: source,
            kind: from.kind(),
            name: name.clone(),
            colour: from.colour(),
            mixer: *from.mixer(),
            sends: Vec::new(),
            routing: from.routing().clone(),
            devices: chain.clone(),
            clips: Vec::new(),
            slots: Vec::new(),
            removed_stop_buttons: Vec::new(),
            take_lanes: Vec::new(),
            audition: None,
            automation: Vec::new(),
            delay: from.delay(),
            keep_monitoring_latency: from.keep_monitoring_latency(),
            tuning_bypass: false,
            tuning_layout: bl_model::TuningLayout::default(),
            info_text: String::new(),
            frozen: None,
            children: Vec::new(),
        }];
        let missing = self.register_set_media(opened, &files);
        let new_track = target.is_none();
        let label = if new_track {
            ids::IMPORT_DEVICES_AND_TRACK_STEP
        } else {
            ids::IMPORT_DEVICES_STEP
        };
        let track: TrackId = target.unwrap_or_else(|| self.editor.new_id());
        let mut added = Vec::new();
        self.editor
            .transact(label, |editor| {
                if new_track {
                    let top = editor.document().top_level().len();
                    editor.apply(Command::CreateTrack {
                        id: track,
                        kind,
                        name,
                        parent: None,
                        index: top,
                    })?;
                    crate::colours::colour_new(editor, &self.prefs.look, &[track], &[])?;
                }
                for device in &chain {
                    // New IDs for the device and everything in its racks; its
                    // mappings, sidechains and automation name the other
                    // set's objects, so they don't come (R8).
                    let mut copy = device.clone();
                    copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
                    copy.floating = device.floating;
                    copy.clear_mappings();
                    if let Some(sidechain) = copy.sidechain.as_mut() {
                        sidechain.input = Input::None;
                    }
                    if let Some(midi) = copy.midi_sidechain.as_mut() {
                        midi.source = None;
                    }
                    let index =
                        crate::devices::default_index(editor.document(), track, copy.role, None);
                    added.push(copy.id);
                    editor.apply(Command::InsertDevice {
                        track,
                        index,
                        device: copy,
                    })?;
                }
                Ok::<_, bl_model::EditError>(())
            })
            .map_err(|e| unavailable(e.to_string()))?;
        if new_track {
            self.select_tracks(&[track]);
        }
        Ok(Imported {
            tracks: vec![track],
            devices: added,
            scenes_added: 0,
            missing_files: missing,
        })
    }

    /// Registers the audio files `data` plays that the media table of
    /// `opened` names (R7): from the path it gives, relative to its project
    /// folder, or, when the file isn't there, found by its content in that
    /// folder and the browser's places. Returns how many couldn't be found.
    fn register_set_media(&mut self, opened: &Opened, data: &[TrackData]) -> usize {
        let used = crate::tracks::media_hashes(data);
        let wanted: Vec<_> = opened
            .project
            .media
            .iter()
            .filter(|file| used.contains(&file.hash))
            .cloned()
            .collect();
        let mut missing = Vec::new();
        for file in &wanted {
            let hash = bl_media::ContentHash::from_bytes(file.hash.0);
            // The store may know the file from another set this session; the
            // open set's media table must still list it.
            if let Some(entry) = self.media.entry(hash).filter(|e| e.path.is_file()) {
                self.store.add_media(file.hash, &entry.path, entry.size);
                continue;
            }
            let there = file
                .path
                .resolve(&opened.folder)
                .filter(|path| crate::clip_files::holds(path, file.size, file.hash));
            match there {
                Some(path) => self.add_set_media(file.hash, &path, file.size),
                None => missing.push((hash, file.size)),
            }
        }
        let mut folders = vec![opened.folder.clone()];
        folders.extend(self.browser_view.search_folders());
        let found = self.media.find_by_content(&missing, &folders);
        for (hash, path) in &found {
            let size = missing
                .iter()
                .find(|(wanted, _)| wanted == hash)
                .map_or(0, |(_, size)| *size);
            self.add_set_media(bl_model::ContentHash(*hash.as_bytes()), path, size);
        }
        missing.len() - found.len()
    }

    /// Records a file a set added plays, in the media table and the store.
    fn add_set_media(&mut self, hash: bl_model::ContentHash, path: &std::path::Path, size: u64) {
        self.store.add_media(hash, path, size);
        let hash = bl_media::ContentHash::from_bytes(hash.0);
        if self.media.entry(hash).is_none() {
            let _ = self.media.import(path);
        }
    }

    /// Whether saving `tracks` as a set in `folder` asks the person first:
    /// Collect Files on Export is Ask and a file the tracks play lies outside
    /// the project the set would land in (set exchange R14).
    #[must_use]
    pub fn tracks_save_asks(&self, tracks: &[TrackId], folder: &std::path::Path) -> bool {
        if self.prefs.collect_files_on_export != CollectFiles::Ask {
            return false;
        }
        let doc = self.editor.document();
        let Ok(built) = set_exchange::build_from_tracks(doc, tracks) else {
            return false;
        };
        let name = self.tracks_export_name(tracks);
        set_exchange::uses_outside(&built.data, &self.media, folder, &name)
    }

    /// As [`Shell::tracks_save_asks`], for Session clips saved as a set.
    #[must_use]
    pub fn clips_save_asks(&self, clips: &[bl_model::ClipId], folder: &std::path::Path) -> bool {
        if self.prefs.collect_files_on_export != CollectFiles::Ask {
            return false;
        }
        let Ok(built) = set_exchange::build_from_clips(self.editor.document(), clips) else {
            return false;
        };
        let name = format!("{} Clips", self.set_name());
        set_exchange::uses_outside(&built.data, &self.media, folder, &name)
    }

    /// The name a set saved from `tracks` has when none is given: the
    /// track's when there is one, the open set's otherwise (R13).
    fn tracks_export_name(&self, tracks: &[TrackId]) -> String {
        let doc = self.editor.document();
        match set_exchange::chosen_tracks(doc, tracks).as_slice() {
            [one] => doc
                .track(*one)
                .map_or_else(|| self.set_name(), |t| t.name().to_owned()),
            _ => self.set_name(),
        }
    }

    fn run_export_tracks(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ExportTracksArgs {
            tracks,
            folder,
            name,
            copy,
        } = Self::args(id, args)?;
        let tracks = tracks.unwrap_or_else(|| self.arrangement.selected_tracks().to_vec());
        let doc = self.editor.document();
        let built = set_exchange::build_from_tracks(doc, &tracks).map_err(|reason| {
            ShellError::Unavailable {
                command: id.to_owned(),
                reason,
            }
        })?;
        // The track's name for one track, the open set's otherwise (R13).
        let name = name.unwrap_or_else(|| self.tracks_export_name(&tracks));
        self.finish_export(id, &built, &folder, &name, copy)
    }

    fn run_export_clips(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ExportClipsArgs {
            clips,
            folder,
            name,
            copy,
        } = Self::args(id, args)?;
        let built =
            set_exchange::build_from_clips(self.editor.document(), &clips).map_err(|reason| {
                ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                }
            })?;
        let name = name.unwrap_or_else(|| format!("{} Clips", self.set_name()));
        self.finish_export(id, &built, &folder, &name, copy)
    }

    /// Writes `built` in `folder` (R13, R14) and shows its row, selected, with
    /// its name open for editing. The open set is not touched.
    fn finish_export(
        &mut self,
        id: &str,
        built: &Built,
        folder: &str,
        name: &str,
        copy: Option<bool>,
    ) -> Result<Value, ShellError> {
        let folder = std::path::absolute(folder).map_err(|e| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: e.to_string(),
        })?;
        if folder.exists() && !folder.is_dir() {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("{} isn't a folder", folder.display()),
            });
        }
        let copy = match (self.prefs.collect_files_on_export, copy) {
            (CollectFiles::Always, _) => true,
            (CollectFiles::Never, _) => false,
            (CollectFiles::Ask, Some(copy)) => copy,
            (CollectFiles::Ask, None) => {
                if set_exchange::uses_outside(&built.data, &self.media, &folder, name) {
                    return Err(ShellError::Unavailable {
                        command: id.to_owned(),
                        reason: "Collect Files on Export is set to Ask: ask the person whether \
                                 to copy the audio files, then run the command again with \
                                 `copy` set to true or false"
                            .to_owned(),
                    });
                }
                false
            }
        };
        let saved =
            set_exchange::write(built, &self.media, &folder, name, copy).map_err(|message| {
                ShellError::Failed {
                    command: id.to_owned(),
                    message,
                }
            })?;
        let shown = self.browser_view.file_saved(&saved.path, &self.audio);
        if shown {
            self.browser = true;
        }
        self.set_message(format!("Saved set {}.", saved.name), false);
        self.touch();
        Ok(to_value(&ExportedSet {
            path: saved.path.to_string_lossy().into_owned(),
            name: saved.name,
            copied: saved.copied,
        }))
    }
}
