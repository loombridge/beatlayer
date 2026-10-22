//! The Clip View's Audio Utilities panel commands through the shell
//! (WP-4.10b): opening a clip's file in the external sample editor, and
//! replacing a clip's file with another. The behaviour is in the sample
//! editor spec, R16 and R18.
//!
//! Both need the media store or the desktop, which the document doesn't
//! hold, so they are run here.

use std::path::PathBuf;

use bl_media::ContentHash;
use bl_model::{AudioData, ClipData, ClipId, Command};
use bl_shell_spec::audio_panel::{EditSampleArgs, EditedSample, ReplaceSampleArgs, ReplacedSample};
use serde_json::Value;

use super::*;

/// A file opened in the external editor, and the content it held then: the
/// shell looks at the file again when Beatlayer comes to the front.
#[derive(Clone, Debug)]
pub(super) struct Editing {
    /// The file that was opened.
    pub path: PathBuf,
    /// The content the file held when it was opened.
    pub hash: ContentHash,
}

impl Shell {
    /// The sample editor the File & Folder page names, if it names one: the
    /// command, with the clip's file as its last argument.
    #[must_use]
    pub fn sample_editor(&self) -> Option<String> {
        self.prefs.sample_editor.clone()
    }

    /// Runs the Audio Utilities panel's own commands (WP-4.10b).
    pub(super) fn run_audio_panel(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            bl_shell_spec::audio_panel::EDIT_SAMPLE => self.run_edit_sample(id, args),
            bl_shell_spec::audio_panel::REPLACE_SAMPLE => self.run_replace_sample(id, args),
            other => Err(ShellError::UnknownCommand(other.to_owned())),
        }
    }

    /// The audio clip a panel command acts on: the one given, or the one the
    /// Clip View shows.
    fn audio_clip(&self, id: &str, clip: Option<ClipId>) -> Result<ClipId, ShellError> {
        let doc = self.editor.document();
        let clip = clip
            .or_else(|| self.clip_view.shown_audio_clip(doc, &self.arrangement))
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no audio clip is shown in the Clip View".to_owned(),
            })?;
        match doc.clip(clip).map(|c| &c.data) {
            Some(ClipData::Audio(_)) => Ok(clip),
            _ => Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: format!("clip {clip} is not an audio clip"),
            }),
        }
    }

    /// The media a clip plays, as the store knows it.
    fn clip_media(&self, clip: ClipId) -> Option<ContentHash> {
        match self.editor.document().clip(clip).map(|c| &c.data) {
            Some(ClipData::Audio(audio)) => Some(ContentHash::from_bytes(audio.media.0)),
            _ => None,
        }
    }

    /// Runs `clip.edit_sample` (R16): opens the clip's file in the editor
    /// the File & Folder page names, or opens that page when it names none.
    pub(super) fn run_edit_sample(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let EditSampleArgs { clip } = Self::args(id, args)?;
        let clip = self.audio_clip(id, clip)?;
        let Some(path) = self.clip_file_path(clip) else {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: "the media store doesn't have the clip's file".to_owned(),
            });
        };
        let Some(editor) = self.sample_editor() else {
            // No editor is named: open the File & Folder page, where one
            // can be chosen.
            self.run(
                bl_shell_spec::commands::APP_SETTINGS,
                serde_json::json!({ "page": "file-folder" }),
            )?;
            return Ok(to_value(&EditedSample {
                clip,
                file: path.display().to_string(),
                editor: "File & Folder".to_owned(),
                opened: false,
            }));
        };
        let hash = self
            .clip_media(clip)
            .unwrap_or_else(|| ContentHash::from_bytes([0; 32]));
        // The editor is a command line: the program, its arguments, and the
        // file as the last one.
        let mut parts = editor.split_whitespace();
        let Some(program) = parts.next() else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "the sample editor's command is empty".to_owned(),
            });
        };
        let started = std::process::Command::new(program)
            .args(parts)
            .arg(&path)
            .spawn()
            .is_ok();
        if started {
            self.editing.push(Editing {
                path: path.clone(),
                hash,
            });
        }
        Ok(to_value(&EditedSample {
            clip,
            file: path.display().to_string(),
            editor: editor.clone(),
            opened: started,
        }))
    }

    /// Runs `clip.replace_sample` (R18): the clip plays the file at `path`
    /// from now on, keeping its gain and pitch, and its warp markers only if
    /// the new file is exactly as long as the old one (warping R27). One
    /// undo step.
    pub(super) fn run_replace_sample(
        &mut self,
        id: &str,
        args: Value,
    ) -> Result<Value, ShellError> {
        let ReplaceSampleArgs { clip, path } = Self::args(id, args)?;
        let clip = self.audio_clip(id, clip)?;
        let path = PathBuf::from(path);
        let media = self
            .media
            .import(&path)
            .map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: format!("couldn't read {}: {error}", path.display()),
            })?;
        self.replace_media(id, clip, media)?;
        Ok(to_value(&ReplacedSample {
            clip,
            file: path.display().to_string(),
            media: bl_model::ContentHash(*media.as_bytes()),
        }))
    }

    /// Makes the clip play `media`, keeping its gain and pitch, and its warp
    /// markers only if the new file is exactly as long as the old one
    /// (warping R27). One undo step.
    pub(super) fn replace_media(
        &mut self,
        id: &str,
        clip: ClipId,
        media: ContentHash,
    ) -> Result<(), ShellError> {
        let next = self.replaced_audio(id, clip, media)?;
        self.apply_replaced(id, &[(clip, next)])
    }

    /// The clip's data with `media` in place of its file: it keeps its gain
    /// and pitch, and its warp markers only if the new file is exactly as
    /// long as the old one; otherwise it takes the settings a new clip of
    /// the file gets (warping R27).
    pub(super) fn replaced_audio(
        &self,
        id: &str,
        clip: ClipId,
        media: ContentHash,
    ) -> Result<AudioData, ShellError> {
        let Some(ClipData::Audio(audio)) = self.editor.document().clip(clip).map(|c| &c.data)
        else {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: "the clip isn't an audio clip".to_owned(),
            });
        };
        let audio = audio.clone();
        let mut next = audio.clone();
        next.media = bl_model::ContentHash(*media.as_bytes());
        if !self.same_length(audio.media, next.media) {
            next = AudioData {
                warp: false,
                warp_markers: Vec::new(),
                tempo: None,
                signature: None,
                ..next.in_seconds().into_owned()
            };
        }
        Ok(next)
    }

    /// Gives the clips their replaced data in one undo step: R16 reaches
    /// every clip that played the file at once.
    fn apply_replaced(
        &mut self,
        id: &str,
        replaced: &[(ClipId, AudioData)],
    ) -> Result<(), ShellError> {
        crate::arrangement::editing::transaction(&mut self.editor, "Replace Sample", |editor| {
            for (clip, data) in replaced {
                editor.apply(Command::SetClipData {
                    clip: *clip,
                    data: ClipData::Audio(data.clone()),
                })?;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|error| match error {
            crate::arrangement::editing::EditFailure::Refused(reason)
            | crate::arrangement::editing::EditFailure::Unavailable(reason) => {
                ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                }
            }
            crate::arrangement::editing::EditFailure::InvalidArgs(message) => {
                ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                }
            }
        })
    }

    /// Whether two files are exactly as long as each other, where the store
    /// knows them.
    fn same_length(&self, a: bl_model::ContentHash, b: bl_model::ContentHash) -> bool {
        let (Ok(one), Ok(other)) = (
            self.media.info(ContentHash::from_bytes(a.0)),
            self.media.info(ContentHash::from_bytes(b.0)),
        ) else {
            return false;
        };
        let (Some(one_frames), Some(other_frames)) = (one.frames, other.frames) else {
            return false;
        };
        one_frames * u64::from(other.sample_rate) == other_frames * u64::from(one.sample_rate)
    }

    /// Looks at the files that were opened in the external editor, and
    /// replaces the media of every clip that played one of them when its
    /// content has changed (R16): the clips of one file change together, in
    /// one undo step.
    pub fn check_edited_samples(&mut self) {
        if self.editing.is_empty() {
            return;
        }
        let mut done = Vec::new();
        for editing in std::mem::take(&mut self.editing) {
            let Ok(hash) = ContentHash::of_file(&editing.path) else {
                done.push(editing);
                continue;
            };
            if hash == editing.hash {
                // The file hasn't changed yet: look again later.
                done.push(editing);
                continue;
            }
            let Ok(media) = self.media.import(&editing.path) else {
                done.push(editing);
                continue;
            };
            // R16: every clip in the set that played the old file plays the
            // new one, in one undo step.
            let replaced: Vec<(ClipId, AudioData)> = self
                .clips_of_media(editing.hash)
                .into_iter()
                .filter_map(|clip| {
                    self.replaced_audio("clip.edit_sample", clip, media)
                        .ok()
                        .map(|data| (clip, data))
                })
                .collect();
            if !replaced.is_empty() {
                let _ = self.apply_replaced("clip.edit_sample", &replaced);
            }
        }
        self.editing = done;
    }

    /// The clips of the set that play the media `hash`.
    fn clips_of_media(&self, hash: ContentHash) -> Vec<ClipId> {
        self.editor
            .document()
            .tracks()
            .flat_map(|track| track.clips().chain(track.slots().map(|(_, clip)| clip)))
            .filter(|clip| match &clip.data {
                ClipData::Audio(audio) => ContentHash::from_bytes(audio.media.0) == hash,
                ClipData::Midi(_) => false,
            })
            .map(|clip| clip.id)
            .collect()
    }
}
