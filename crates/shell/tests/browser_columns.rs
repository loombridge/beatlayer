//! Sorting the browser by what a file's header says (spec: browser-tags
//! R27, R28), driven headless through `Shell::run`: the sort never reads
//! files on the calling thread, and `natural_cmp` allocates nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_shell::Shell;
use bl_shell::browser::listing::natural_cmp;
use bl_shell::browser_commands::{self as ids, BrowserState, DEFAULT_ROW_LIMIT};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-columns-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of `frames` frames at 48 kHz.
fn wav(path: &Path, frames: u32) {
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    bytes.resize(bytes.len() + data as usize, 0);
    std::fs::write(path, bytes).unwrap();
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

// Spec: browser-tags R28.
#[test]
fn sorting_by_length_probes_files_in_the_background() {
    const FILES: u32 = 500;
    let scratch = Scratch::new("length");
    let library = scratch.0.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    // 37 and 500 share no factor, so every file has a length of its own, and
    // 100 frames apart is more than the millisecond the sort compares by.
    let frames = |i: u32| 1_000 + (i * 37 % FILES) * 100;
    for i in 0..FILES {
        wav(&library.join(format!("Hit {i:03}.wav")), frames(i));
    }
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell.set_user_library(Some(library));
    assert!(shell.wait_for_browser_index(Duration::from_secs(20)));
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));

    let before = bl_media::probes_on_this_thread();
    let sorted: BrowserState = run(
        &mut shell,
        ids::SET_COLUMNS,
        json!({ "shown": ["length"], "sort": "length" }),
    );
    // Only the rows the command returns are read here, not every file.
    let read_here = bl_media::probes_on_this_thread() - before;
    assert!(
        read_here <= DEFAULT_ROW_LIMIT && read_here < FILES as usize,
        "{read_here} files read on the calling thread, for {} rows of {FILES}",
        sorted.rows.len()
    );

    // The background probes arrive, and the rows end up in length order.
    let mut expected: Vec<u32> = (0..FILES).collect();
    expected.sort_by_key(|&i| frames(i));
    let expected: Vec<String> = expected
        .iter()
        .take(DEFAULT_ROW_LIMIT)
        .map(|i| format!("Hit {i:03}.wav"))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        shell.poll();
        let state: BrowserState = run(&mut shell, ids::STATE, json!({}));
        let names: Vec<String> = state.rows.iter().map(|row| row.name.clone()).collect();
        if names == expected {
            break;
        }
        assert!(Instant::now() < deadline, "the rows never reached length order: {:?} total {}", &names[..5.min(names.len())], state.total_rows);
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Spec: browser-tags R28. Sorting names allocates nothing.
#[test]
fn natural_cmp_allocates_nothing() {
    assert_eq!(natural_cmp("sample 00001.wav", "sample 00002.wav"), std::cmp::Ordering::Less);
    assert_eq!(natural_cmp("kick 808 10", "kick 808 9"), std::cmp::Ordering::Greater);
    assert_eq!(natural_cmp("Kick 2", "kick 10"), std::cmp::Ordering::Less);
    bl_rt::rt_test!({
        for _ in 0..1_000 {
            std::hint::black_box(natural_cmp(
                std::hint::black_box("sample 00001.wav"),
                std::hint::black_box("sample 00002.wav"),
            ));
            std::hint::black_box(natural_cmp(
                std::hint::black_box("kick 808 10"),
                std::hint::black_box("kick 808 9"),
            ));
        }
    });
}
