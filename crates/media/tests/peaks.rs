//! Peaks: what they hold, how they're laid out for the GPU, building them in
//! parallel, the peak file, and the background cache with its invalidation.

use crate::common;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use bl_media::peaks::{BuildOptions, BuildProgress, FRAMES_PER_PEAK, MIP_FACTOR, PeakRun};
use bl_media::{
    ContentHash, Decoder, MediaError, MediaStatus, MediaStore, PeakCache, PeakCacheOptions,
    PeakEvent, PeakStatus, Peaks, SampleData, decode_file,
};
use common::{TempDir, manifest};

/// Writes a 16-bit stereo WAV at 48 kHz from a function of the frame.
fn write_wav(path: &Path, frames: usize, sample: impl Fn(usize, usize) -> f32) {
    let data_len = frames * 4;
    let mut bytes = Vec::with_capacity(44 + data_len);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
    for n in 0..frames {
        for c in 0..2 {
            let s = (sample(n, c).clamp(-1.0, 1.0) * 32_767.0) as i16;
            bytes.extend_from_slice(&s.to_le_bytes());
        }
    }
    fs::write(path, bytes).unwrap();
}

/// Moves a file's modification time on, as an editor saving it would; set
/// explicitly because a file system's clock can be coarser than a test.
fn touch_later(path: &Path) {
    let later = SystemTime::now() + Duration::from_secs(10);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

fn tone(n: usize, c: usize) -> f32 {
    let t = n as f32 / 48_000.0;
    0.8 * (std::f32::consts::TAU * 220.0 * t + c as f32).sin() * (1.0 - t)
}

fn quieter(n: usize, c: usize) -> f32 {
    0.25 * tone(n, c)
}

/// The peaks the rules call for, computed straight from the samples at
/// every level: each window's extremes, the minimum rounded down and the
/// maximum up to 1/127 steps.
fn reference(data: &SampleData) -> Vec<Vec<Vec<[i8; 2]>>> {
    let frames = data.frames() as u64;
    let mut levels = Vec::new();
    let mut window = FRAMES_PER_PEAK;
    loop {
        let count = frames.div_ceil(window);
        let level: Vec<Vec<[i8; 2]>> = data
            .channels()
            .map(|samples| {
                (0..count)
                    .map(|i| {
                        let start = (i * window) as usize;
                        let end = ((i + 1) * window).min(frames) as usize;
                        let lo = samples[start..end]
                            .iter()
                            .copied()
                            .fold(f32::INFINITY, f32::min);
                        let hi = samples[start..end]
                            .iter()
                            .copied()
                            .fold(f32::NEG_INFINITY, f32::max);
                        [
                            (lo * 127.0).floor().clamp(-127.0, 127.0) as i8,
                            (hi * 127.0).ceil().clamp(-127.0, 127.0) as i8,
                        ]
                    })
                    .collect()
            })
            .collect();
        levels.push(level);
        if count <= 1 {
            return levels;
        }
        window *= MIP_FACTOR;
    }
}

fn levels_of(peaks: &Peaks) -> Vec<Vec<Vec<[i8; 2]>>> {
    (0..peaks.level_count())
        .map(|k| {
            (0..usize::from(peaks.channels()))
                .map(|c| peaks.level(k, c).unwrap().collect())
                .collect()
        })
        .collect()
}

fn parallel(threads: usize) -> BuildOptions {
    BuildOptions {
        threads: NonZeroUsize::new(threads),
        // Short segments, so even the short fixtures decode in several.
        segment_frames: FRAMES_PER_PEAK,
    }
}

#[test]
fn every_fixture_gives_the_peaks_of_its_decoded_samples() {
    // Spec: waveform-peaks R2, R6
    for fixture in manifest() {
        let path = fixture.path();
        let decoded = decode_file(&path).unwrap();
        let expected = reference(&decoded);

        let in_order = Peaks::build_file(
            &path,
            &BuildOptions::single_threaded(),
            &BuildProgress::new(),
        )
        .unwrap();
        assert_eq!(levels_of(&in_order), expected, "{}", fixture.file);
        assert_eq!(in_order.frames(), decoded.frames() as u64);
        assert_eq!(in_order.sample_rate(), decoded.sample_rate());
        assert_eq!(usize::from(in_order.channels()), decoded.channel_count());

        // Decoding segments in parallel, each after a seek, changes nothing.
        let progress = BuildProgress::new();
        let split = Peaks::build_file(&path, &parallel(4), &progress).unwrap();
        assert_eq!(split, in_order, "{} built in parallel", fixture.file);
        assert_eq!(progress.frames_done(), decoded.frames() as u64);
        // The header's length held, so the parallel build didn't have to
        // start again in order (which forgets the total).
        let header = Decoder::open(&path).unwrap().info().frames;
        assert_eq!(progress.frames_total(), header, "{}", fixture.file);

        assert_eq!(Peaks::from_samples(&decoded).unwrap(), in_order);
    }
}

#[test]
fn the_last_peak_covers_what_is_left() {
    // Spec: waveform-peaks R2
    let mut left = vec![0.0f32; 300];
    left[299] = -0.5;
    let data = SampleData::new(48_000, vec![left]).unwrap();
    let peaks = Peaks::from_samples(&data).unwrap();
    assert_eq!(peaks.level_count(), 2);
    assert_eq!(peaks.run(0, 0), Some(PeakRun { offset: 0, len: 2 }));
    assert_eq!(peaks.peak(0, 0, 0), Some([0, 0]));
    assert_eq!(peaks.peak(0, 0, 1), Some([-64, 0]));
    assert_eq!(peaks.peak(1, 0, 0), Some([-64, 0]));

    let empty = Peaks::from_samples(&SampleData::new(48_000, vec![vec![]]).unwrap()).unwrap();
    assert_eq!(empty.level_count(), 1);
    assert_eq!(empty.run(0, 0), Some(PeakRun { offset: 0, len: 0 }));
    assert!(empty.data().is_empty());
}

#[test]
fn only_digital_silence_draws_flat() {
    // Spec: waveform-peaks R2
    let silence = manifest()
        .into_iter()
        .find(|f| f.signal == "silence" && f.container == "wav")
        .unwrap();
    let peaks = Peaks::build_file(
        silence.path(),
        &BuildOptions::default(),
        &BuildProgress::new(),
    )
    .unwrap();
    assert!(peaks.data().iter().all(|&b| b == 0));

    // A signal 100 dB down still shows one step either side of zero.
    let quiet: Vec<f32> = (0..1_024).map(|n| 1e-5 * (n as f32 * 0.1).sin()).collect();
    let peaks = Peaks::from_samples(&SampleData::new(48_000, vec![quiet]).unwrap()).unwrap();
    assert!(peaks.level(0, 0).unwrap().all(|p| p == [-1, 1]));
}

#[test]
fn the_data_block_reads_the_way_the_waveform_shader_reads_it() {
    // Spec: waveform-peaks R8
    let fixture = manifest()
        .into_iter()
        .find(|f| f.file == "sine-48k-s24-4ch.wav")
        .unwrap();
    let a = Peaks::build_file(
        fixture.path(),
        &BuildOptions::default(),
        &BuildProgress::new(),
    )
    .unwrap();
    let data = decode_file(manifest()[0].path()).unwrap();
    let b = Peaks::from_samples(&data).unwrap();

    // Two files one after the other in one storage buffer, as the UI keeps
    // them: each block's length is a multiple of four bytes.
    assert_eq!(a.data().len() % 4, 0);
    assert_eq!(b.data().len() % 4, 0);
    let mut buffer = a.data().to_vec();
    let base_b = buffer.len();
    buffer.extend_from_slice(b.data());
    let (words, rest) = buffer.as_chunks::<4>();
    assert!(rest.is_empty());
    let words: Vec<u32> = words.iter().map(|&w| u32::from_le_bytes(w)).collect();

    // `peak` in spikes/ui-egui/src/dense.wgsl, written out in Rust:
    // extractBits on an i32 sign-extends.
    let shader_peak = |i: u32| -> [i8; 2] {
        let w = words[(i >> 1) as usize] as i32;
        let shift = (i & 1) * 16;
        let extract = |offset: u32| ((w << (24 - offset)) >> 24) as i8;
        [extract(shift), extract(shift + 8)]
    };
    for (peaks, base) in [(&a, 0), (&b, base_b)] {
        for level in 0..peaks.level_count() {
            for channel in 0..usize::from(peaks.channels()) {
                let run = peaks.run(level, channel).unwrap();
                assert_eq!(run.offset % 2, 0, "runs start on a word");
                for (i, expected) in peaks.level(level, channel).unwrap().enumerate() {
                    let index = base as u32 / 2 + run.offset + i as u32;
                    assert_eq!(shader_peak(index), expected);
                }
            }
        }
    }

    // Levels are in order, coarse ones last, and each level's channels in
    // channel order.
    let runs = a.runs();
    for pair in runs.windows(2) {
        assert!(pair[0].offset + pair[0].len <= pair[1].offset);
    }
    assert_eq!(runs.len(), a.level_count() * 4);
}

#[test]
fn a_header_that_overstates_the_length_falls_back_to_decoding_in_order() {
    // Spec: waveform-peaks R6
    // A WAV cut off half-way still claims its full length; the parallel
    // build notices and decodes what's there in order.
    let dir = TempDir::new("peaks-short");
    let path = dir.path().join("cut.wav");
    write_wav(&path, 48_000, tone);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..44 + 4 * 30_000]).unwrap();
    let decoder = Decoder::open(&path).unwrap();
    assert_eq!(decoder.info().frames, Some(48_000));

    let progress = BuildProgress::new();
    let peaks = Peaks::build_file(&path, &parallel(4), &progress).unwrap();
    assert_eq!(peaks.frames(), 30_000);
    // It started again in order, with the length unknown.
    assert_eq!(progress.frames_total(), None);
    assert_eq!(progress.frames_done(), 30_000);
    assert_eq!(
        peaks,
        Peaks::from_samples(&decode_file(&path).unwrap()).unwrap()
    );
}

#[test]
fn a_build_can_be_cancelled() {
    let dir = TempDir::new("peaks-cancel");
    let path = dir.path().join("tone.wav");
    write_wav(&path, 200_000, tone);
    for options in [BuildOptions::single_threaded(), parallel(4)] {
        let progress = BuildProgress::new();
        progress.cancel();
        assert!(matches!(
            Peaks::build_file(&path, &options, &progress),
            Err(MediaError::Cancelled)
        ));
    }
    // Cancelled part-way, from the progress observer.
    let progress = Arc::new(std::sync::OnceLock::<Arc<BuildProgress>>::new());
    let watched = Arc::clone(&progress);
    let observed = Arc::new(BuildProgress::with_observer(move |done| {
        if done > 50_000 {
            watched.get().unwrap().cancel();
        }
    }));
    progress.set(Arc::clone(&observed)).unwrap();
    assert!(matches!(
        Peaks::build_file(&path, &BuildOptions::single_threaded(), &observed),
        Err(MediaError::Cancelled)
    ));
    assert!(observed.frames_done() < 200_000);
}

/// A store, a cache over a temporary folder, and a WAV imported into both.
struct Setup {
    dir: TempDir,
    store: Arc<MediaStore>,
    cache: PeakCache,
    wav: PathBuf,
    hash: ContentHash,
}

fn setup(name: &str) -> Setup {
    let dir = TempDir::new(name);
    let wav = dir.path().join("Samples/tone.wav");
    fs::create_dir_all(wav.parent().unwrap()).unwrap();
    write_wav(&wav, 30_000, tone);
    let store = Arc::new(MediaStore::new());
    let hash = store.import(&wav).unwrap();
    let cache = PeakCache::new(Arc::clone(&store), options(dir.path())).unwrap();
    Setup {
        dir,
        store,
        cache,
        wav,
        hash,
    }
}

fn options(dir: &Path) -> PeakCacheOptions {
    PeakCacheOptions {
        threads: 2,
        ..PeakCacheOptions::in_dir(dir.join("Peaks"))
    }
}

#[test]
fn peaks_build_in_the_background_and_are_saved_by_content_hash() {
    // Spec: waveform-peaks R1, R3
    let s = setup("peaks-cache-build");
    let events = s.cache.subscribe();
    // The first request never waits.
    assert!(matches!(
        s.cache.request(s.hash),
        PeakStatus::Pending { .. }
    ));
    assert_eq!(
        events.recv_timeout(Duration::from_secs(30)).unwrap(),
        PeakEvent::Ready(s.hash)
    );
    let PeakStatus::Ready(peaks) = s.cache.request(s.hash) else {
        panic!("not ready after the ready event");
    };
    let expected = Peaks::from_samples(&decode_file(&s.wav).unwrap()).unwrap();
    assert_eq!(*peaks, expected);

    // Saved as <dir>/<first two hex digits>/<hash>.blpeaks.
    let hex = s.hash.to_string();
    let file = s
        .dir
        .path()
        .join("Peaks")
        .join(&hex[..2])
        .join(format!("{hex}.blpeaks"));
    assert_eq!(s.cache.file_path(s.hash).as_deref(), Some(file.as_path()));
    assert_eq!(Peaks::read_file(&file, s.hash).unwrap(), expected);
    // Nothing is written beside the sample.
    assert_eq!(fs::read_dir(s.wav.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn a_new_session_loads_the_peak_file_instead_of_decoding() {
    // Spec: waveform-peaks R3
    let s = setup("peaks-cache-load");
    s.cache.wait(s.hash).unwrap();
    let file = s.cache.file_path(s.hash).unwrap();
    drop(s.cache);

    // Put other (valid) peaks in the file: if the next cache returns them, it
    // read the file rather than decoding the sample.
    let other =
        Peaks::from_samples(&SampleData::new(48_000, vec![vec![0.5; 30_000]; 2]).unwrap()).unwrap();
    other.write_file(&file, s.hash).unwrap();
    let cache = PeakCache::new(Arc::clone(&s.store), options(s.dir.path())).unwrap();
    assert_eq!(*cache.wait(s.hash).unwrap(), other);

    // The same content at another path, in another store, is the same media
    // and finds the same file.
    let copy = s.dir.path().join("Elsewhere/copy.wav");
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::copy(&s.wav, &copy).unwrap();
    let store = Arc::new(MediaStore::new());
    assert_eq!(store.import(&copy).unwrap(), s.hash);
    let cache = PeakCache::new(store, options(s.dir.path())).unwrap();
    assert_eq!(*cache.wait(s.hash).unwrap(), other);
}

#[test]
fn a_damaged_or_foreign_peak_file_is_rebuilt() {
    // Spec: waveform-peaks R4
    let s = setup("peaks-cache-damaged");
    let expected = s.cache.wait(s.hash).unwrap();
    let file = s.cache.file_path(s.hash).unwrap();
    drop(s.cache);
    let good = fs::read(&file).unwrap();

    let damaged = {
        let mut bytes = good.clone();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x40;
        bytes
    };
    let other_media = Peaks::read_file(&file, s.hash)
        .unwrap()
        .to_file_bytes(ContentHash::of_bytes(b"other media"));
    let mut newer_version = good.clone();
    newer_version[8] = 2;
    for bad in [
        damaged,
        good[..good.len() / 2].to_vec(),
        other_media,
        newer_version,
    ] {
        fs::write(&file, &bad).unwrap();
        let cache = PeakCache::new(Arc::clone(&s.store), options(s.dir.path())).unwrap();
        assert_eq!(cache.wait(s.hash).unwrap(), expected);
        assert_eq!(fs::read(&file).unwrap(), good, "the file wasn't rewritten");
    }
}

#[test]
fn editing_the_source_invalidates_its_peaks() {
    // Spec: waveform-peaks R5
    let s = setup("peaks-cache-edit");
    let before = s.cache.wait(s.hash).unwrap();
    let old_file = s.cache.file_path(s.hash).unwrap();
    assert!(old_file.exists());
    let events = s.cache.subscribe();

    // An editor saves new audio over the sample: same length, new content.
    write_wav(&s.wav, 30_000, quieter);
    touch_later(&s.wav);
    let now = ContentHash::of_file(&s.wav).unwrap();

    // Checking the media drops the old peaks, from memory and disk.
    assert_eq!(s.cache.check(s.hash).unwrap(), MediaStatus::Changed { now });
    assert_eq!(
        events.try_recv().unwrap(),
        PeakEvent::Invalidated {
            hash: s.hash,
            status: MediaStatus::Changed { now }
        }
    );
    assert!(s.cache.get(s.hash).is_none());
    assert!(!old_file.exists());
    // Asking for the old content's peaks now fails rather than showing them.
    assert!(matches!(
        &*s.cache.wait(s.hash).unwrap_err(),
        MediaError::Changed { now: n, .. } if *n == now
    ));

    // The edited file is new media, and its peaks follow its new content.
    let hash = s.store.import(&s.wav).unwrap();
    assert_eq!(hash, now);
    let after = s.cache.wait(hash).unwrap();
    assert_eq!(
        *after,
        Peaks::from_samples(&decode_file(&s.wav).unwrap()).unwrap()
    );
    assert_ne!(after, before);
    assert!(s.cache.file_path(hash).unwrap().exists());
}

#[test]
fn a_later_session_never_shows_the_peaks_of_an_edited_file() {
    // Spec: waveform-peaks R5
    let s = setup("peaks-cache-edit-offline");
    s.cache.wait(s.hash).unwrap();
    let old_file = s.cache.file_path(s.hash).unwrap();
    let size = fs::metadata(&s.wav).unwrap().len();
    drop(s.cache);

    // Edited while Beatlayer wasn't running.
    write_wav(&s.wav, 30_000, quieter);
    touch_later(&s.wav);

    // The project names the old content; its peak file is still on disk.
    let store = Arc::new(MediaStore::new());
    store.add_reference(s.hash, &s.wav, size);
    let cache = PeakCache::new(store, options(s.dir.path())).unwrap();
    assert!(matches!(
        &*cache.wait(s.hash).unwrap_err(),
        MediaError::Changed { .. }
    ));
    assert!(!old_file.exists());
}

#[test]
fn a_file_edited_during_a_build_gives_no_peaks() {
    // Spec: waveform-peaks R5
    let dir = TempDir::new("peaks-edit-during");
    let wav = dir.path().join("tone.wav");
    write_wav(&wav, 100_000, tone);
    let store = MediaStore::new();
    let hash = store.import(&wav).unwrap();

    let edited = Arc::new(AtomicBool::new(false));
    let path = wav.clone();
    let flag = Arc::clone(&edited);
    let progress = BuildProgress::with_observer(move |_| {
        if !flag.swap(true, Ordering::SeqCst) {
            write_wav(&path, 100_000, quieter);
            touch_later(&path);
        }
    });
    let result = Peaks::build(&store, hash, &BuildOptions::single_threaded(), &progress);
    assert!(edited.load(Ordering::SeqCst));
    assert!(matches!(result, Err(MediaError::Changed { .. })));
}

#[test]
fn missing_media_has_no_peaks_until_it_is_found() {
    // Spec: waveform-peaks R5
    let s = setup("peaks-cache-missing");
    let expected = s.cache.wait(s.hash).unwrap();
    let file = s.cache.file_path(s.hash).unwrap();

    let moved = s.dir.path().join("Moved/tone.wav");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&s.wav, &moved).unwrap();
    assert_eq!(s.cache.check(s.hash).unwrap(), MediaStatus::Missing);
    assert!(s.cache.get(s.hash).is_none());
    // A missing file's peak file is kept, for when it's found.
    assert!(file.exists());
    assert!(matches!(
        &*s.cache.wait(s.hash).unwrap_err(),
        MediaError::Missing { .. }
    ));
    // Failures stick until the media is available again...
    assert!(matches!(s.cache.request(s.hash), PeakStatus::Failed(_)));

    // ...as it is once it's relinked.
    s.store.relink(s.hash, &[s.dir.path()]).unwrap();
    assert_eq!(s.cache.check(s.hash).unwrap(), MediaStatus::Available);
    assert_eq!(s.cache.wait(s.hash).unwrap(), expected);
}

#[test]
fn concurrent_requests_share_one_build() {
    let s = setup("peaks-cache-shared");
    let events = s.cache.subscribe();
    let cache = &s.cache;
    let results: Vec<Arc<Peaks>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(move || cache.wait(s.hash).unwrap()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for peaks in &results[1..] {
        assert!(Arc::ptr_eq(peaks, &results[0]));
    }
    assert_eq!(events.try_recv().unwrap(), PeakEvent::Ready(s.hash));
    assert!(events.try_recv().is_err(), "built more than once");
}

#[test]
fn unknown_media_fails_without_a_build() {
    let s = setup("peaks-cache-unknown");
    let unknown = ContentHash::of_bytes(b"never imported");
    assert!(matches!(
        &*s.cache.wait(unknown).unwrap_err(),
        MediaError::UnknownMedia(h) if *h == unknown
    ));
}

#[test]
fn a_memory_only_cache_writes_no_files() {
    // Spec: waveform-peaks R3
    let dir = TempDir::new("peaks-memory");
    let wav = dir.path().join("tone.wav");
    write_wav(&wav, 10_000, tone);
    let store = Arc::new(MediaStore::new());
    let hash = store.import(&wav).unwrap();
    let cache = PeakCache::new(
        store,
        PeakCacheOptions {
            threads: 2,
            ..PeakCacheOptions::memory_only()
        },
    )
    .unwrap();
    assert!(cache.dir().is_none());
    assert!(cache.file_path(hash).is_none());
    cache.wait(hash).unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn forgetting_keeps_the_file_and_removing_deletes_it() {
    let s = setup("peaks-cache-forget");
    let expected = s.cache.wait(s.hash).unwrap();
    let file = s.cache.file_path(s.hash).unwrap();
    s.cache.forget(s.hash);
    assert!(s.cache.status(s.hash).is_none());
    assert!(file.exists());
    assert_eq!(s.cache.wait(s.hash).unwrap(), expected);
    s.cache.remove(s.hash);
    assert!(s.cache.status(s.hash).is_none());
    assert!(!file.exists());
}

#[test]
fn pruning_deletes_the_least_recently_used_files_first() {
    let dir = TempDir::new("peaks-prune");
    let store = Arc::new(MediaStore::new());
    let mut hashes = Vec::new();
    for (i, gain) in [0.2f32, 0.4, 0.6].into_iter().enumerate() {
        let wav = dir.path().join(format!("{i}.wav"));
        write_wav(&wav, 20_000, |n, c| gain * tone(n, c));
        hashes.push(store.import(&wav).unwrap());
    }
    let cache = PeakCache::new(Arc::clone(&store), options(dir.path())).unwrap();
    let mut sizes = Vec::new();
    for (age, &hash) in hashes.iter().enumerate() {
        cache.wait(hash).unwrap();
        let file = cache.file_path(hash).unwrap();
        // The first is the oldest.
        let used = SystemTime::now() - Duration::from_secs(1_000 * (3 - age as u64));
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(used)
            .unwrap();
        sizes.push(fs::metadata(&file).unwrap().len());
    }
    // A temporary file left by a write that stopped, two hours ago.
    let folder = cache
        .file_path(hashes[0])
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let stale = folder.join(".x.blpeaks.1-0.tmp");
    fs::write(&stale, b"half").unwrap();
    fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(7_200))
        .unwrap();

    // Hashes held in memory are spared, so let them go first.
    for &hash in &hashes {
        cache.forget(hash);
    }
    let report = cache.prune(sizes[2]).unwrap();
    assert_eq!(report.files_removed, 2);
    assert_eq!(report.bytes_removed, sizes[0] + sizes[1]);
    assert_eq!(report.bytes_kept, sizes[2]);
    assert!(!cache.file_path(hashes[0]).unwrap().exists());
    assert!(!cache.file_path(hashes[1]).unwrap().exists());
    assert!(cache.file_path(hashes[2]).unwrap().exists());
    assert!(!stale.exists());
}

#[test]
fn dropping_the_cache_stops_its_builds() {
    let dir = TempDir::new("peaks-drop");
    let wav = dir.path().join("long.wav");
    write_wav(&wav, 2_000_000, tone);
    let store = Arc::new(MediaStore::new());
    let hash = store.import(&wav).unwrap();
    let cache = PeakCache::new(store, options(dir.path())).unwrap();
    let _ = cache.request(hash);
    let file = cache.file_path(hash).unwrap();
    // Returns once the build has stopped: whatever it got to, it's finished
    // with the folder, so removing it can't race a write.
    drop(cache);
    let leftovers: Vec<_> = file
        .parent()
        .and_then(|p| fs::read_dir(p).ok())
        .map(|d| d.filter_map(Result::ok).map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        leftovers
            .iter()
            .all(|n| n.to_string_lossy().ends_with(".blpeaks")),
        "{leftovers:?}"
    );
}
