//! The memory importing a DAWproject file with big embedded media takes
//! (the DAWproject spec, R5): the file is read from disk as the import asks
//! for it, and each medium is unpacked once, to a file, so a set of stems
//! doesn't need three times their size in memory.
//!
//! A counting allocator wraps the system allocator, so the test can read the
//! most bytes live on the heap while the import runs.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use bl_interop::dawproject::{self, Context};

/// The system allocator, counting the bytes live on the heap and the most
/// there have been since the last reset.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every call goes straight to the system allocator with the same
// arguments, so it keeps the system allocator's guarantees; the counters are
// only bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract, which we pass on.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc_zeroed`'s contract.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds `dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `realloc`'s contract.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            grow(new_size);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MEDIUM_BYTES: usize = 64 << 20;

/// The header of a 16-bit stereo 48 kHz WAV file of `data` bytes.
fn wav_header(data: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    out
}

/// Spec: dawproject R5. Importing a file with a 64 MiB medium takes far less
/// than the medium's size in memory: the medium is never held whole.
#[test]
fn a_big_embedded_medium_is_never_held_in_memory() {
    let folder = std::env::temp_dir().join(format!("bl-interop-memory-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("stems.dawproject");
    let xml = r#"<?xml version="1.0"?><Project version="1.0"><Application name="t" version="1"/>
<Transport><Tempo unit="bpm" value="120" id="tempo"/></Transport>
<Structure><Track contentType="audio" id="a" name="A"><Channel role="regular" id="ca"/></Track></Structure>
<Arrangement id="arr"><Lanes id="all"><Lanes track="a" timeUnit="seconds" id="la"><Clips id="cl">
<Clip time="0" duration="1" contentTimeUnit="seconds"><Audio><File path="audio/a.wav"/></Audio></Clip>
</Clips></Lanes></Lanes></Arrangement></Project>"#;
    {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        let stored = zip::write::SimpleFileOptions::default();
        writer.start_file("project.xml", stored).unwrap();
        writer.write_all(xml.as_bytes()).unwrap();
        writer
            .start_file("audio/a.wav", stored.large_file(false))
            .unwrap();
        writer.write_all(&wav_header(MEDIUM_BYTES)).unwrap();
        for _ in 0..MEDIUM_BYTES >> 20 {
            writer.write_all(&[0u8; 1 << 20]).unwrap();
        }
        writer.finish().unwrap();
    }

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let container = dawproject::read_container_from(
        std::io::BufReader::new(std::fs::File::open(&file).unwrap()),
        dawproject::Limits::default(),
    )
    .unwrap();
    let imported = dawproject::import(&container, &Context::new(&folder, &folder)).unwrap();
    let peak = PEAK.load(Ordering::Relaxed) - baseline;

    assert_eq!(imported.files.len(), 1, "{:?}", imported.report);
    assert!(
        peak < 16 << 20,
        "the import held {} MiB at its most",
        peak >> 20
    );
    let _ = std::fs::remove_dir_all(&folder);
}
