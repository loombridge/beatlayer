//! Prints the sound descriptor of every WAV file in a folder, one line each:
//! the file's name, a tab, and the descriptor's numbers. The script that fits
//! the automatic tags' classifier (`tests/fixtures/gen/fit_autotags.py`)
//! reads it.
//!
//! `cargo run -p bl-analysis --example describe -- <folder>`

use bl_analysis::similarity::Descriptor;

fn main() {
    let folder = std::env::args_os().nth(1).expect("a folder of WAV files");
    let mut files: Vec<_> = std::fs::read_dir(folder)
        .expect("the folder")
        .map(|entry| entry.expect("a folder entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
        .collect();
    files.sort();
    for path in files {
        let data = bl_media::decode_file(&path).expect("a WAV file");
        let descriptor = Descriptor::of_sample(&data).expect("a sound of 60 seconds or less");
        let numbers: Vec<String> = descriptor.values().iter().map(f32::to_string).collect();
        let name = path.file_name().expect("a file name").to_string_lossy();
        println!("{name}\t{}", numbers.join(" "));
    }
}
