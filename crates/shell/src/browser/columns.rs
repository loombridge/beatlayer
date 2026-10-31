//! The content pane's columns and sort (spec: browser-tags R27, R28).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};

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
    /// What probing an audio file read.
    probe: HashMap<PathBuf, AudioInfo>,
}

/// An audio file's sample rate, channels and length in seconds, or `None`
/// when its header couldn't be read.
type AudioInfo = Option<(u32, u16, Option<f64>)>;

fn probe_audio(path: &Path) -> AudioInfo {
    bl_media::probe(path).ok().map(|info| {
        let seconds = info
            .frames
            .map(|frames| frames as f64 / f64::from(info.sample_rate.max(1)));
        (info.sample_rate, info.channels, seconds)
    })
}

/// Probes audio files on a thread of its own, so sorting by Length, Sample
/// Rate or Channels never opens a file on the browser's thread.
#[derive(Debug, Default)]
pub struct Prober(Mutex<ProberState>);

#[derive(Debug, Default)]
struct ProberState {
    requests: Option<Sender<PathBuf>>,
    results: Option<Receiver<(PathBuf, AudioInfo)>>,
    /// The files asked for whose answer hasn't been taken yet.
    pending: HashSet<PathBuf>,
}

impl Prober {
    /// Asks for `path` to be probed, unless that's already under way. The
    /// answer comes in [`Prober::poll`].
    fn request(&self, path: &Path) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        if !state.pending.insert(path.to_path_buf()) {
            return;
        }
        if state.requests.is_none() {
            let (sender, receiver) = mpsc::channel::<PathBuf>();
            let (results, results_receiver) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("bl-browser-probe".to_owned())
                .spawn(move || {
                    while let Ok(path) = receiver.recv() {
                        let info = probe_audio(&path);
                        if results.send((path, info)).is_err() {
                            return;
                        }
                    }
                });
            if spawned.is_err() {
                state.pending.remove(path);
                return;
            }
            state.requests = Some(sender);
            state.results = Some(results_receiver);
        }
        if let Some(sender) = &state.requests {
            let _ = sender.send(path.to_path_buf());
        }
    }

    /// The files probed since the last call.
    fn poll(&self) -> Vec<(PathBuf, AudioInfo)> {
        let Ok(mut state) = self.0.lock() else {
            return Vec::new();
        };
        let probed: Vec<_> = state
            .results
            .as_ref()
            .map(|results| results.try_iter().collect())
            .unwrap_or_default();
        for (path, _) in &probed {
            state.pending.remove(path);
        }
        probed
    }
}

/// What an item is, in words, as the Kind column says it.
#[must_use]
pub fn kind_text(kind: ItemKind, clip: Option<ClipFileKind>) -> &'static str {
    match kind {
        ItemKind::Folder => "Folder",
        ItemKind::Audio => "Audio File",
        ItemKind::Sfz => "SFZ Instrument",
        ItemKind::Set => "Beatlayer Set",
        ItemKind::DawProject => "DAWproject",
        ItemKind::Preset => "Device Preset",
        ItemKind::ChordBank => "Chord Bank",
        ItemKind::Tuning => "Tuning",
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

    /// An audio file's sample rate, channels and length in seconds, read
    /// now if it isn't known.
    fn audio_info_of(&self, path: &Path) -> AudioInfo {
        if let Ok(cache) = self.details.lock()
            && let Some(known) = cache.probe.get(path)
        {
            return *known;
        }
        let read = probe_audio(path);
        if let Ok(mut cache) = self.details.lock() {
            cache.probe.insert(path.to_path_buf(), read);
        }
        read
    }

    /// What probing the file read, or nothing while it hasn't been probed:
    /// then it's asked of the prober thread, and [`BrowserView::take_probed`]
    /// brings the answer.
    fn audio_info_known(&self, path: &Path) -> AudioInfo {
        let known = self
            .details
            .lock()
            .ok()
            .and_then(|cache| cache.probe.get(path).copied());
        if known.is_none() {
            self.prober.request(path);
        }
        known.flatten()
    }

    /// Keeps what the prober thread has read since the last call. Whether
    /// the rows are sorted by what it read: if so they're worked out again.
    pub(super) fn take_probed(&mut self) -> bool {
        let probed = self.prober.poll();
        if probed.is_empty() {
            return false;
        }
        if let Ok(mut cache) = self.details.lock() {
            cache.probe.extend(probed);
        }
        matches!(
            self.columns.sort,
            Column::Length | Column::SampleRate | Column::Channels
        )
    }

    /// What the shown columns say about an item, or nothing while none
    /// shows.
    pub(super) fn details_for(&self, item: &Item) -> Option<ItemDetails> {
        if self.columns.shown.is_empty() {
            return None;
        }
        let mut details = ItemDetails::default();
        let path = Path::new(&item.path);
        let on_disk = !matches!(item.kind, ItemKind::Plugin | ItemKind::Device)
            && !item.path.starts_with(super::tunings::BUNDLED_PREFIX);
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
                    .then(|| self.audio_info_known(path))
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
        let mut keyed: Vec<(SortKey, Entry)> = entries
            .iter()
            .map(|entry| {
                let key = self.sort_key(sort, &entry.path, entry.kind, entry.clip, None);
                (key, entry.clone())
            })
            .collect();
        keyed.sort_by(|(ka, a), (kb, b)| {
            let folders = (a.kind != ItemKind::Folder).cmp(&(b.kind != ItemKind::Folder));
            folders.then_with(|| order(sort, descending, ka, kb, &a.name, &b.name))
        });
        for (slot, (_, entry)) in entries.iter_mut().zip(keyed) {
            *slot = entry;
        }
    }

    /// Orders search results by the sort, the same way. The default order,
    /// by name, is the index's own and is kept.
    /// A file whose header hasn't been read sorts as having no value until
    /// the prober thread reads it. Size, Date Modified and Kind come from the
    /// index.
    pub(super) fn sort_hits(&self, snapshot: &Snapshot, hits: &mut [u32]) {
        let ColumnsState {
            sort, descending, ..
        } = self.columns;
        if sort == Column::Name && !descending {
            return;
        }
        let mut keyed: Vec<(SortKey, u32)> = hits
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
                (self.sort_key(sort, &path, entry.kind, clip, known), hit)
            })
            .collect();
        keyed.sort_by(|&(ka, a), &(kb, b)| {
            let (ea, eb) = (snapshot.entry(a), snapshot.entry(b));
            let folders = (ea.kind != ItemKind::Folder).cmp(&(eb.kind != ItemKind::Folder));
            folders.then_with(|| order(sort, descending, &ka, &kb, &ea.name, &eb.name))
        });
        for (slot, (_, hit)) in hits.iter_mut().zip(keyed) {
            *slot = hit;
        }
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
