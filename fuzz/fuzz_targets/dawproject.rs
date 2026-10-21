//! Fuzzes the DAWproject reader: a container's bytes, and `project.xml`
//! alone, through the ZIP reader, the XML reader and the import.

#![no_main]

use std::collections::BTreeMap;
use std::path::Path;

use bl_core::time::Ticks;
use bl_interop::dawproject;
use libfuzzer_sys::fuzz_target;

/// The document model's rules hold whatever the import makes.
fn check(imported: &dawproject::Imported) {
    let document = &imported.project.document;
    assert!(document.check_invariants().is_ok());
    for track in document.tracks() {
        for clip in track.clips() {
            assert!(clip.start >= Ticks::ZERO);
            assert!(clip.length > Ticks::ZERO);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // The container.
    if let Ok(container) = dawproject::read_container(data) {
        let context =
            dawproject::Context::new(Path::new("/nonexistent"), Path::new("/nonexistent"));
        if let Ok(imported) = dawproject::import(&container, &context) {
            check(&imported);
        }
    }

    // The project part alone.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut report = dawproject::ReadReport::default();
    let Some(root) = dawproject::read::read_xml(text, &mut report) else {
        return;
    };
    let Some(project) = dawproject::read::project(&root, &mut report) else {
        return;
    };
    let container = dawproject::Container {
        project,
        files: BTreeMap::new(),
        metadata: None,
        report,
    };
    let context = dawproject::Context::new(Path::new("/nonexistent"), Path::new("/nonexistent"));
    if let Ok(imported) = dawproject::import(&container, &context) {
        check(&imported);
    }
});
