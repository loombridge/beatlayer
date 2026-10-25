//! The content pane's columns and sort (spec: browser-tags R27, R28).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bl_shell_spec::browser::{
    ClipFileKind, Column, ColumnsState, Item, ItemDetails, ItemKind, SetColumnsArgs,
};

use super::index::Snapshot;
use super::listing::{Entry, natural_cmp};
use super::{BrowserError, BrowserView};

/// What was read from disk about files, kept until the files change.
#[derive(Debug, Default)]
pub struct DetailsCache {
    /// When a file changed and how long it is.
    meta: HashMap<PathBuf, Option<(i64, u64)>>,
    /// An audio file's sample rate, channels and length in seconds.
    probe: HashMap<PathBuf, Option<(u32, u16, Option<f64>)>>,
}

/// What an item is, in words, as the Kind column says it.
#[must_use]
pub fn kind_text(kind: ItemKind, clip: Option<ClipFileKind>) -> &'static str {
    match kind {
        ItemKind::Folder => "Folder",
        ItemKind::Audio => "Audio File",
        ItemKind::Sfz => "SFZ Instrument",
        ItemKind::Set => "Beatlayer Set",
        ItemKind::Preset => "Device Preset",
        ItemKind::ChordBank => "Chord Bank",
        ItemKind::Groove => "Groove",
        ItemKind::Plugin => "Plug-In",
        ItemKind::Device => "Device",
        ItemKind::Clip => match clip {
            Some(ClipFileKind::Audio) => "Audio Clip File",
            Some(ClipFileKind::Midi) => "MIDI Clip File",
            None => "Clip File",
        },
        ItemKind::Track => "Track",
        ItemKind::Group => "Group Track",
        ItemKind::Return => "Return Track",
        ItemKind::Main => "Main Track",
        ItemKind::SessionClip => match clip {
            Some(ClipFileKind::Audio) => "Audio Clip",
            Some(ClipFileKind::Midi) => "MIDI Clip",
            None => "Clip",
        },
        ItemKind::Devices => "Devices",
        ItemKind::Notice => "",
    }
}

/// How a column orders items: a number, or for Kind a word.
type SortKey = (Option<i128>, &'static str);

impl BrowserView {
    /// The columns, sort and file extensions.
    #[must_use]
    pub fn columns(&self) -> &ColumnsState {
        &self.columns
    }

    /// Forgets what was read about files, as they may have changed.
    pub(super) fn forget_details(&self) {
        if let Ok(mut cache) = self.details.lock() {
            *cache = DetailsCache::default();
        }
    }

    /// When a file changed (seconds since the epoch) and its size in bytes.
    fn metadata_of(&self, path: &Path) -> Option<(i64, u64)> {
        if let Ok(cache) = self.details.lock()
            && let Some(known) = cache.meta.get(path)
        {
            return *known;
        }
        let read = std::fs::metadata(path).ok().map(|meta| {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0));
            (modified, meta.len())
        });
        if let Ok(mut cache) = self.details.lock() {
            cache.meta.insert(path.to_path_buf(), read);
        }
        read
    }

    /// An audio file's sample rate, channels and length in seconds.
    fn audio_info_of(&self, path: &Path) -> Option<(u32, u16, Option<f64>)> {
        if let Ok(cache) = self.details.lock()
            && let Some(known) = cache.probe.get(path)
        {
            return *known;
        }
        let read = bl_media::probe(path).ok().map(|info| {
            let seconds = info
                .frames
                .map(|frames| frames as f64 / f64::from(info.sample_rate.max(1)));
            (info.sample_rate, info.channels, seconds)
        });
        if let Ok(mut cache) = self.details.lock() {
            cache.probe.insert(path.to_path_buf(), read);
        }
        read
    }

    /// What the shown columns say about an item, or nothing while none
    /// shows.
    pub(super) fn details_for(&self, item: &Item) -> Option<ItemDetails> {
        if self.columns.shown.is_empty() {
            return None;
        }
        let mut details = ItemDetails::default();
        let path = Path::new(&item.path);
        let on_disk = !matches!(item.kind, ItemKind::Plugin | ItemKind::Device);
        for column in &self.columns.shown {
            match column {
                Column::DateModified | Column::Size if on_disk => {
                    if let Some((modified, size)) = self.metadata_of(path) {
                        if *column == Column::DateModified {
                            details.modified = Some(modified);
                        } else if item.kind != ItemKind::Folder {
                            details.size = Some(size);
                        }
                    }
                }
                Column::Kind => {
                    details.kind = Some(kind_text(item.kind, item.clip_kind).to_owned());
                }
                Column::Length | Column::SampleRate | Column::Channels
                    if item.kind == ItemKind::Audio =>
                {
                    if let Some((rate, channels, seconds)) = self.audio_info_of(path) {
                        match column {
                            Column::Length => details.length_seconds = seconds,
                            Column::SampleRate => details.sample_rate = Some(rate),
                            _ => details.channels = Some(channels),
                        }
                    }
                }
                _ => {}
            }
        }
        Some(details)
    }

    /// How an item sorts under a column.
    fn sort_key(
        &self,
        column: Column,
        path: &Path,
        kind: ItemKind,
        clip: Option<ClipFileKind>,
        known: Option<(i64, u64)>,
    ) -> SortKey {
        let number = |value: Option<i128>| (value, "");
        match column {
            Column::Name => (None, ""),
            Column::DateModified => number(
                known
                    .or_else(|| self.metadata_of(path))
                    .map(|(modified, _)| i128::from(modified)),
            ),
            Column::Size => number(
                known
                    .or_else(|| self.metadata_of(path))
                    .map(|(_, size)| i128::from(size)),
            ),
            Column::Kind => (None, kind_text(kind, clip)),
            Column::Length | Column::SampleRate | Column::Channels => {
                let info = (kind == ItemKind::Audio)
                    .then(|| self.audio_info_of(path))
                    .flatten();
                number(info.and_then(|(rate, channels, seconds)| match column {
                    Column::Length => seconds.map(|s| (s * 1000.0) as i128),
                    Column::SampleRate => Some(i128::from(rate)),
                    _ => Some(i128::from(channels)),
                }))
            }
        }
    }

    /// Orders the entries of a folder by the sort: folders first, then by
    /// the column, ties by name (spec: browser-tags R28).
    pub(super) fn sort_entries(&self, entries: &mut [Entry]) {
        let ColumnsState {
            sort, descending, ..
        } = self.columns;
        if sort == Column::Name && !descending {
            return;
        }
        let keys: HashMap<PathBuf, SortKey> = entries
            .iter()
            .map(|entry| {
                (
                    entry.path.clone(),
                    self.sort_key(sort, &entry.path, entry.kind, entry.clip, None),
                )
            })
            .collect();
        entries.sort_by(|a, b| {
            let folders = (a.kind != ItemKind::Folder).cmp(&(b.kind != ItemKind::Folder));
            folders.then_with(|| {
                order(
                    sort,
                    descending,
                    &keys[&a.path],
                    &keys[&b.path],
                    &a.name,
                    &b.name,
                )
            })
        });
    }

    /// Orders search results by the sort, the same way. The default order,
    /// by name, is the index's own and is kept.
    ///
    /// shortcut: Length, Sample Rate and Channels read every hit's header on
    /// the calling thread, once per file (the reads are kept); a sort by one
    /// of them over tens of thousands of audio files stalls the frame that
    /// asks for it. Size, Date Modified and Kind come from the index.
    pub(super) fn sort_hits(&self, snapshot: &Snapshot, hits: &mut [u32]) {
        let ColumnsState {
            sort, descending, ..
        } = self.columns;
        if sort == Column::Name && !descending {
            return;
        }
        let keys: HashMap<u32, SortKey> = hits
            .iter()
            .map(|&hit| {
                let entry = snapshot.entry(hit);
                let known =
                    (entry.kind != ItemKind::Folder).then_some((entry.modified, entry.size));
                let clip = None;
                let path = if matches!(
                    sort,
                    Column::DateModified
                        | Column::Size
                        | Column::Length
                        | Column::SampleRate
                        | Column::Channels
                ) {
                    snapshot.path(hit)
                } else {
                    PathBuf::new()
                };
                (hit, self.sort_key(sort, &path, entry.kind, clip, known))
            })
            .collect();
        hits.sort_by(|&a, &b| {
            let (ea, eb) = (snapshot.entry(a), snapshot.entry(b));
            let folders = (ea.kind != ItemKind::Folder).cmp(&(eb.kind != ItemKind::Folder));
            folders.then_with(|| order(sort, descending, &keys[&a], &keys[&b], &ea.name, &eb.name))
        });
    }

    /// Sets the columns, the sort and file extensions (spec: browser-tags
    /// R27, R28).
    pub(super) fn set_columns(&mut self, args: SetColumnsArgs) -> Result<(), BrowserError> {
        let mut next = self.columns.clone();
        if let Some(shown) = args.shown {
            let mut kept: Vec<Column> = Vec::new();
            for column in shown {
                if column == Column::Name {
                    return Err(BrowserError::InvalidArgs(
                        "the Name column always shows; leave it out of `shown`".to_owned(),
                    ));
                }
                if !kept.contains(&column) {
                    kept.push(column);
                }
            }
            next.shown = kept;
        }
        if let Some(sort) = args.sort {
            next.sort = sort;
        }
        if let Some(descending) = args.descending {
            next.descending = descending;
        }
        if let Some(extensions) = args.file_extensions {
            next.file_extensions = extensions;
        }
        if next.sort != Column::Name && !next.shown.contains(&next.sort) {
            if args.sort.is_some() {
                return Err(BrowserError::InvalidArgs(format!(
                    "the {} column isn't shown, so the list can't be sorted by it",
                    next.sort.title()
                )));
            }
            // Its column was hidden: the list goes back to sorting by name.
            next.sort = Column::Name;
            next.descending = false;
        }
        self.columns = next;
        Ok(())
    }
}

/// Orders two items of one kind (both folders or both not) by a column.
fn order(
    sort: Column,
    descending: bool,
    a: &SortKey,
    b: &SortKey,
    a_name: &str,
    b_name: &str,
) -> Ordering {
    let by_name = natural_cmp(a_name, b_name);
    if sort == Column::Name {
        return if descending {
            by_name.reverse()
        } else {
            by_name
        };
    }
    let mut key = a.cmp(b);
    if descending {
        key = key.reverse();
    }
    key.then(by_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: browser-tags R28.
    #[test]
    fn a_column_orders_by_its_value_and_ties_go_by_name() {
        let small = (Some(10), "");
        let large = (Some(500), "");
        assert_eq!(
            order(Column::Size, false, &small, &large, "b", "a"),
            Ordering::Less
        );
        assert_eq!(
            order(Column::Size, true, &small, &large, "b", "a"),
            Ordering::Greater
        );
        assert_eq!(
            order(Column::Size, true, &small, &small, "a", "b"),
            Ordering::Less
        );
        assert_eq!(
            order(Column::Name, true, &small, &small, "a", "b"),
            Ordering::Greater
        );
        assert_eq!(
            order(
                Column::Kind,
                false,
                &(None, "Audio File"),
                &(None, "Folder"),
                "z",
                "a"
            ),
            Ordering::Less
        );
    }

    #[test]
    fn kinds_have_words() {
        assert_eq!(kind_text(ItemKind::Folder, None), "Folder");
        assert_eq!(
            kind_text(ItemKind::Clip, Some(ClipFileKind::Midi)),
            "MIDI Clip File"
        );
    }
}
