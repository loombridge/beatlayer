//! The browser's index and search against a large library (WP-1.10; spec:
//! browser R10).
//!
//! It makes a library of empty audio files in a temporary folder (100,000
//! by default, 200 to a folder, named from a few words and a number), gives
//! it to a headless shell as the User Library, and measures how long the
//! background thread takes to index it, then how long `browser.search`
//! takes for each keystroke of a few queries, as typing in the search field
//! runs it: the search itself and the state it returns, with its first 100
//! rows.
//!
//! ```sh
//! cargo run --release -p bl-shell --example browser_index_bench -- [--files N] [--keep]
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bl_shell::Shell;
use bl_shell::browser_commands as ids;
use serde_json::json;

const WORDS: [&str; 12] = [
    "Kick", "Snare", "Hat", "Clap", "Tom", "Ride", "Bass", "Pad", "Lead", "Vox", "Loop", "FX",
];

fn main() {
    let mut files = 100_000usize;
    let mut keep = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--files" => {
                files = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("--files takes a number");
            }
            "--keep" => keep = true,
            other => panic!("unknown option {other}"),
        }
    }
    let root = std::env::temp_dir().join(format!("bl-browser-bench-{}", std::process::id()));
    let library = root.join("User Library");
    let made = Instant::now();
    let mut folder = PathBuf::new();
    for i in 0..files {
        if i % 200 == 0 {
            folder = library
                .join(format!(
                    "{} Pack {}",
                    WORDS[(i / 200) % WORDS.len()],
                    i / 2_000
                ))
                .join(format!("Folder {}", i / 200));
            std::fs::create_dir_all(&folder).expect("made a folder");
        }
        let name = format!(
            "{} {} {:03} {}.wav",
            WORDS[i % WORDS.len()],
            WORDS[(i / 7) % WORDS.len()],
            i % 997,
            i
        );
        std::fs::File::create(folder.join(name)).expect("made a file");
    }
    println!(
        "made {files} files in {} folders in {:.1} s",
        files.div_ceil(200),
        made.elapsed().as_secs_f64()
    );

    let mut shell = Shell::headless();
    let started = Instant::now();
    shell.set_user_library(Some(library.clone()));
    while shell.browser_view().index_state().indexing {
        shell.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    shell.poll();
    let indexed = started.elapsed();
    let state = shell.browser_view().index_state();
    println!(
        "indexed {} files and {} folders in {:.0} ms on a background thread",
        state.files,
        state.folders,
        indexed.as_secs_f64() * 1000.0
    );

    let mut times = Vec::new();
    for query in ["kick bass", "loop 120", "pad 9", "vox fx 99"] {
        for end in 1..=query.len() {
            let typed = &query[..end];
            let started = Instant::now();
            let result = shell
                .run(ids::SEARCH, json!({ "query": typed, "label": "all" }))
                .expect("searched");
            let took = started.elapsed();
            times.push(took);
            if end == query.len() {
                println!(
                    "  \"{typed}\": {} results, {:.2} ms",
                    result["total_rows"],
                    took.as_secs_f64() * 1000.0
                );
            }
        }
    }
    times.sort_unstable();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "search per keystroke over {} keystrokes: median {:.2} ms, 99th percentile {:.2} ms, \
         slowest {:.2} ms (the first letters match the most)",
        times.len(),
        ms(times[times.len() / 2]),
        ms(times[times.len() * 99 / 100]),
        ms(*times.last().expect("timed"))
    );
    if keep {
        println!("kept {}", root.display());
    } else {
        let _ = std::fs::remove_dir_all(&root);
    }
}
