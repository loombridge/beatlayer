//! The memory 10,000 undo steps hold on a 100-track project (WP-0.6
//! acceptance).
//!
//! A counting allocator wraps the system allocator, so the test can read the
//! bytes live on the heap. Each scenario starts an editor on the same
//! project, records 10,000 undo steps of one kind of edit, and reports the
//! bytes the history holds afterwards. The figures are heap bytes requested,
//! without the allocator's own overhead.
//!
//! Run it with the figures printed:
//!
//! ```text
//! cargo test -p bl-model --release --test undo_memory -- --nocapture
//! ```
//!
//! On 10 October 2026 (Apple M5 Max, Rust 1.99.0) the project's document
//! held 3.3 MiB, and 10,000 steps held from 11.1 MiB (volume changes, about
//! 1.2 KB a step) to 33.2 MiB (note edits, about 3.5 KB a step). A clip
//! drag of ten moves costs the same as one move, since the moves in between
//! are freed when the gesture's transaction commits. The figures don't
//! depend on the build profile.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use bl_core::id::IdAllocator;
use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId, DeviceKind,
    Document, Editor, MidiData, Note, ParamId, TrackId, TrackKind,
};

/// The system allocator, counting the bytes live on the heap.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes straight to the system allocator with the same
// arguments, so it keeps the system allocator's guarantees; the counter is
// only bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract, which we pass on.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `dealloc`'s contract, which we pass on.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `realloc`'s contract, which we pass on.
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            LIVE.fetch_add(new_size, Ordering::Relaxed);
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

const STEPS: usize = 10_000;
const GROUPS: usize = 10;
const TRACKS_PER_GROUP: usize = 10;
const CLIPS_PER_TRACK: i64 = 32;
const NOTES_PER_CLIP: i64 = 16;
const DEVICES_PER_TRACK: usize = 4;
const PARAMS_PER_DEVICE: u32 = 16;

/// A small, fixed random number generator (SplitMix64), so every run makes
/// the same edits.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The project: 100 audio and MIDI tracks in 10 groups, 4 return tracks,
/// and Main. Each audio or MIDI track has 32 one-bar clips (MIDI clips hold
/// 16 notes each) and 4 devices with 16 parameters set. Every return and
/// Main has 2 devices.
struct Project {
    document: Document,
    ids: IdAllocator,
    tracks: Vec<TrackId>,
    midi_clips: Vec<ClipId>,
    clips: Vec<ClipId>,
    devices: Vec<DeviceId>,
}

fn device(editor: &mut Editor, track: TrackId, index: usize) -> DeviceId {
    let id = editor.new_id();
    let mut device = Device::new(
        id,
        "Effect",
        DeviceKind::Native {
            device_type: "beatlayer.test".to_owned(),
        },
    );
    for param in 0..PARAMS_PER_DEVICE {
        device.params.insert(ParamId(param), 0.5);
    }
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device,
        })
        .unwrap();
    id
}

fn notes(velocity: u8) -> MidiData {
    MidiData::new((0..NOTES_PER_CLIP).map(|n| Note {
        key: 48 + u8::try_from(n).unwrap(),
        velocity,
        start: Ticks::SIXTEENTH * n,
        length: Ticks::SIXTEENTH,
    }))
}

fn build_project() -> Project {
    let mut editor = Editor::new();
    let mut project = Project {
        document: editor.document().clone(),
        ids: editor.ids(),
        tracks: Vec::new(),
        midi_clips: Vec::new(),
        clips: Vec::new(),
        devices: Vec::new(),
    };
    let new_track = |editor: &mut Editor, kind, parent, index| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: format!("{kind} track"),
                parent,
                index,
            })
            .unwrap();
        id
    };
    for g in 0..GROUPS {
        let group = new_track(&mut editor, TrackKind::Group, None, g);
        for t in 0..TRACKS_PER_GROUP {
            let kind = if t % 2 == 0 {
                TrackKind::Audio
            } else {
                TrackKind::Midi
            };
            let track = new_track(&mut editor, kind, Some(group), t);
            project.tracks.push(track);
            for c in 0..CLIPS_PER_TRACK {
                let id = editor.new_id();
                let data = if kind == TrackKind::Audio {
                    ClipData::Audio(AudioData {
                        media: ContentHash([u8::try_from(t).unwrap(); 32]),
                    })
                } else {
                    project.midi_clips.push(id);
                    ClipData::Midi(notes(100))
                };
                let mut clip = Clip::new(id, Ticks::WHOLE * (2 * c), Ticks::WHOLE, data);
                clip.name = format!("Clip {c}");
                editor.apply(Command::CreateClip { track, clip }).unwrap();
                project.clips.push(id);
            }
            for d in 0..DEVICES_PER_TRACK {
                let id = device(&mut editor, track, d);
                project.devices.push(id);
            }
        }
    }
    let main = editor.document().main_id();
    for r in 0..4 {
        let track = new_track(&mut editor, TrackKind::Return, None, r);
        for d in 0..2 {
            project.devices.push(device(&mut editor, track, d));
        }
    }
    for d in 0..2 {
        project.devices.push(device(&mut editor, main, d));
    }
    project.document = editor.document().clone();
    project.ids = editor.ids();
    project
}

/// One kind of edit, recorded as one undo step.
#[derive(Clone, Copy, Debug)]
enum Edit {
    MoveClip,
    SetVolume,
    SetDeviceParam,
    ChangeVelocity,
    DragClip,
    Mixed,
}

impl Edit {
    fn name(self) -> &'static str {
        match self {
            Self::MoveClip => "move a clip",
            Self::SetVolume => "set a track's volume",
            Self::SetDeviceParam => "set a device parameter",
            Self::ChangeVelocity => "change one note's velocity",
            Self::DragClip => "drag a clip (10 moves, 1 step)",
            Self::Mixed => "a mix of all of these and renames",
        }
    }

    fn step(self, editor: &mut Editor, project: &Project, rng: &mut Rng) {
        let start = |rng: &mut Rng| Ticks::SIXTEENTH * i64::try_from(rng.below(4_096)).unwrap();
        let command = match self {
            Self::MoveClip => {
                let clip = *rng.pick(&project.clips);
                let track = editor.document().clip_track(clip).unwrap();
                Command::MoveClip {
                    clip,
                    track,
                    start: start(rng),
                }
            }
            Self::SetVolume => Command::SetVolume {
                track: *rng.pick(&project.tracks),
                volume: rng.unit(),
            },
            Self::SetDeviceParam => Command::SetDeviceParam {
                device: *rng.pick(&project.devices),
                param: ParamId(u32::try_from(rng.below(16)).unwrap()),
                value: rng.unit(),
            },
            Self::ChangeVelocity => {
                let clip = *rng.pick(&project.midi_clips);
                let Some(ClipData::Midi(midi)) =
                    editor.document().clip(clip).map(|c| c.data.clone())
                else {
                    unreachable!("MIDI clips hold MIDI data");
                };
                let mut midi = midi;
                let index = rng.below(midi.notes.len());
                midi.notes[index].velocity = 1 + u8::try_from(rng.below(127)).unwrap();
                Command::SetClipData {
                    clip,
                    data: ClipData::Midi(midi),
                }
            }
            Self::DragClip => {
                let clip = *rng.pick(&project.clips);
                let track = editor.document().clip_track(clip).unwrap();
                editor.begin("Drag clip");
                for _ in 0..10 {
                    let start = start(rng);
                    editor
                        .apply(Command::MoveClip { clip, track, start })
                        .unwrap();
                }
                editor.commit().unwrap();
                return;
            }
            Self::Mixed => {
                let kinds = [
                    Self::MoveClip,
                    Self::SetVolume,
                    Self::SetDeviceParam,
                    Self::ChangeVelocity,
                    Self::DragClip,
                ];
                if rng.below(6) == 0 {
                    Command::RenameClip {
                        clip: *rng.pick(&project.clips),
                        name: format!("Renamed {}", rng.below(1_000_000)),
                    }
                } else {
                    return rng.pick(&kinds).step(editor, project, rng);
                }
            }
        };
        editor.apply(command).unwrap();
    }
}

/// Records `STEPS` steps of one edit and returns the bytes the history holds.
fn measure(project: &Project, edit: Edit) -> usize {
    let mut rng = Rng(0x5eed);
    let mut editor = Editor::from_document(project.document.clone(), project.ids);
    let before = live();
    while editor.undo_history().len() < STEPS {
        edit.step(&mut editor, project, &mut rng);
    }
    let after = live();
    // Undo everything, to show the history is usable as well as held.
    while editor.can_undo() {
        editor.undo().unwrap();
    }
    assert_eq!(editor.document(), &project.document);
    after.saturating_sub(before)
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[test]
fn ten_thousand_undo_steps_on_a_hundred_tracks() {
    let empty = live();
    let project = build_project();
    let document = live() - empty;
    let doc = &project.document;
    println!(
        "Project: {} tracks ({} audio and MIDI), {} clips, {} devices; the document holds {:.1} MiB",
        doc.track_count(),
        project.tracks.len(),
        doc.clip_count(),
        doc.device_count(),
        mib(document)
    );
    assert_eq!(project.tracks.len(), 100);

    println!(
        "{:<36} {:>12} {:>12}",
        "Undo steps of 10,000 x", "total MiB", "bytes/step"
    );
    for edit in [
        Edit::MoveClip,
        Edit::SetVolume,
        Edit::SetDeviceParam,
        Edit::ChangeVelocity,
        Edit::DragClip,
        Edit::Mixed,
    ] {
        let held = measure(&project, edit);
        let per_step = held / STEPS;
        println!("{:<36} {:>12.1} {:>12}", edit.name(), mib(held), per_step);
        // A step holds only the nodes its edit replaced: a few kilobytes,
        // whatever the size of the song.
        assert!(
            per_step < 16 * 1024,
            "{} holds {per_step} bytes a step",
            edit.name()
        );
    }
}
