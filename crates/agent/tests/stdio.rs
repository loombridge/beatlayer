//! `bl-cli mcp`'s transport polls the shell, as the app's window does, so
//! background work lands with no window: a processed file (which a stem
//! separation's tracks wait on) finishes while an agent asks for its status
//! and while no one asks anything.

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use bl_agent::Agent;
use bl_media::{BitDepth, Dither, EncodeSpec, FileType};
use bl_shell::Shell;
use bl_shell::processed::{Kind, Request};

/// A shell with a processed file being written on its worker thread, and
/// the flag its edit sets once it lands.
fn writing(name: &str) -> (Shell, Arc<AtomicBool>) {
    let mut shell = Shell::headless();
    let landed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&landed);
    let request = Request {
        kind: Kind::Stems,
        name: format!("{name}-{}.wav", std::process::id()),
        spec: EncodeSpec {
            file_type: FileType::Wav,
            bit_depth: BitDepth::Float32,
            dither: Dither::None,
            sample_rate: 48_000,
            channels: 1,
        },
        frames: 1000,
        produce: Box::new(|sink| sink.write(&[0.25; 1000])),
    };
    shell
        .start_processed(request, move |_, _| {
            flag.store(true, Relaxed);
            Ok(())
        })
        .unwrap();
    assert!(shell.is_processing());
    (shell, landed)
}

/// An agent that asks `ping` again and again until the file has landed.
struct Asking {
    landed: Arc<AtomicBool>,
    asked: Arc<AtomicUsize>,
}

impl Read for Asking {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Far more than a file takes to write; it stops the test if the
        // shell is never polled.
        if self.landed.load(Relaxed) || self.asked.load(Relaxed) >= 10_000_000 {
            return Ok(0);
        }
        let id = self.asked.fetch_add(1, Relaxed);
        let line = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n");
        buf[..line.len()].copy_from_slice(line.as_bytes());
        Ok(line.len())
    }
}

// Spec: stem-separation R14.
#[test]
fn a_file_lands_while_an_agent_keeps_asking() {
    let (mut shell, landed) = writing("asking");
    let asked = Arc::new(AtomicUsize::new(0));
    let mut output = Vec::new();
    bl_agent::stdio::serve(
        &mut Agent::headless(false),
        &mut shell,
        io::BufReader::new(Asking {
            landed: Arc::clone(&landed),
            asked: Arc::clone(&asked),
        }),
        &mut output,
    )
    .unwrap();
    assert!(landed.load(Relaxed), "the file never landed");
    assert!(!shell.is_processing());
    // One reply per request, in order, and nothing the polls wrote.
    let ids: Vec<_> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["id"].as_u64())
        .collect();
    assert_eq!(ids.len(), asked.load(Relaxed));
    assert!(ids.iter().copied().eq((0..ids.len() as u64).map(Some)));
}

/// Standard input with no one writing: it ends once `landed` is set.
struct Quiet(Receiver<()>);

impl Read for Quiet {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        let _ = self.0.recv();
        Ok(0)
    }
}

// Spec: stem-separation R14.
#[test]
fn a_file_lands_while_no_one_asks_anything() {
    let (mut shell, landed) = writing("quiet");
    let (end, input) = std::sync::mpsc::channel();
    let watching = Arc::clone(&landed);
    // Ends the input once the file has landed; far more looks than the
    // transport needs, so a shell that is never polled stops the test.
    let watcher = std::thread::spawn(move || {
        for _ in 0..50_000_000 {
            if watching.load(Relaxed) {
                break;
            }
            std::thread::yield_now();
        }
        drop(end);
    });
    let mut output = Vec::new();
    bl_agent::stdio::serve(
        &mut Agent::headless(false),
        &mut shell,
        io::BufReader::new(Quiet(input)),
        &mut output,
    )
    .unwrap();
    watcher.join().unwrap();
    assert!(landed.load(Relaxed), "the file never landed");
    assert!(output.is_empty());
}
