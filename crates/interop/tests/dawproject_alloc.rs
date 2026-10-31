//! What importing a DAWproject file allocates beyond reading it (the
//! DAWproject spec, R35): files from other applications put an `id` on the
//! Arrangement's lanes, each track's lanes, each clip list and each note
//! list, and the import must not copy the parsed model once for each `id`.
//!
//! A counting allocator wraps the system allocator, so the test can read the
//! total bytes requested between two points.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use bl_interop::dawproject::{self, Context};

/// The system allocator, summing the bytes requested.
struct Counting;

static REQUESTED: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes straight to the system allocator with the same
// arguments, so it keeps the system allocator's guarantees; the counter is
// only bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        REQUESTED.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `alloc`'s contract, which we pass on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        REQUESTED.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `alloc_zeroed`'s contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REQUESTED.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        // SAFETY: the caller upholds `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const TRACKS: usize = 50;
const NOTES: usize = 2_000;

/// A file of 50 audio-free MIDI tracks of 2,000 notes each, with an `id` on
/// every lane, clip list and note list.
fn project() -> Vec<u8> {
    let mut structure = String::new();
    let mut lanes = String::new();
    for track in 0..TRACKS {
        structure.push_str(&format!(
            r#"<Track contentType="notes" id="t{track}" name="T{track}"><Channel role="regular" id="c{track}"/></Track>"#
        ));
        let notes: String = (0..NOTES)
            .map(|note| {
                format!(
                    r#"<Note time="{}" duration="0.25" key="{}" vel="0.8"/>"#,
                    f64::from(note as u32) / 4.0,
                    36 + note % 48
                )
            })
            .collect();
        lanes.push_str(&format!(
            r#"<Lanes track="t{track}" id="l{track}"><Clips id="k{track}"><Clip time="0" duration="500" name="C"><Notes id="n{track}">{notes}</Notes></Clip></Clips></Lanes>"#
        ));
    }
    let xml = format!(
        r#"<?xml version="1.0"?><Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure>{structure}</Structure>
<Arrangement id="arr"><Lanes id="all">{lanes}</Lanes></Arrangement></Project>"#
    );
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("project.xml", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(xml.as_bytes()).unwrap();
    writer.finish().unwrap().into_inner()
}

/// Spec: dawproject R35. Importing a 100,000-note file allocates little more
/// than reading it did: the parsed project isn't copied once for each `id`.
#[test]
fn an_import_does_not_copy_the_parsed_model_for_each_id() {
    let file = project();

    REQUESTED.store(0, Ordering::Relaxed);
    let container = dawproject::read_container(&file).unwrap();
    let read = REQUESTED.load(Ordering::Relaxed);

    REQUESTED.store(0, Ordering::Relaxed);
    let folder = std::env::temp_dir();
    let imported = dawproject::import(&container, &Context::new(&folder, &folder)).unwrap();
    let imported_bytes = REQUESTED.load(Ordering::Relaxed);

    assert!(imported.report.entries.is_empty(), "{:?}", imported.report);
    println!("read {read} bytes, import {imported_bytes} bytes");
    assert!(
        imported_bytes < read + read / 2,
        "reading asked for {read} bytes and importing for {imported_bytes}"
    );
}
