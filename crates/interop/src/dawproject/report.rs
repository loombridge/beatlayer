//! The import report (the DAWproject spec, R12): what an import left out or
//! changed, in the file's order.

/// One thing the import left out or changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// What it was: a VST2 plug-in, a send's pan, a note's channel.
    pub what: String,
    /// Where it was: the track by name, a clip by name and its place, a
    /// device by name.
    pub place: String,
    /// How many there were.
    pub count: usize,
    /// What Beatlayer did: left out, or changed to what.
    pub did: String,
}

/// What an import left out or changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The application and version the file says made it.
    pub application: Option<String>,
    /// Each thing left out or changed, in the file's order.
    pub entries: Vec<Entry>,
    /// Where each entry sits, so a repeat is found without a scan.
    positions: std::collections::HashMap<(String, String, String), usize>,
}

impl Report {
    /// Records one thing left out or changed.
    pub fn add(&mut self, what: &str, place: &str, did: &str) {
        self.add_count(what, place, did, 1);
    }

    /// Records `count` of one thing left out or changed. The same thing in
    /// the same place is counted again rather than listed twice.
    pub fn add_count(&mut self, what: &str, place: &str, did: &str, count: usize) {
        let key = (what.to_owned(), place.to_owned(), did.to_owned());
        if let Some(&at) = self.positions.get(&key) {
            self.entries[at].count += count;
            return;
        }
        self.positions.insert(key, self.entries.len());
        self.entries.push(Entry {
            what: what.to_owned(),
            place: place.to_owned(),
            count,
            did: did.to_owned(),
        });
    }
}
