# Song fixtures

The audio the M2 song test (`crates/cli/tests/m2_song.rs`) plays. Nothing here is recorded or third-party audio: `tests/fixtures/gen/gen_song.py` synthesises every file from a seeded noise generator and the standard library, so running it again writes the same bytes. Don't edit the files by hand: change the generator and run it, as its docstring describes.

| File | What it is |
|---|---|
| `kick.wav` | A kick drum, 48 kHz, 0.5 s. The song's Kick track plays it through a one-shot sampler. |
| `kit.wav` | A snare, a closed hat, an open hat and a clap, 0.25 s each, 48 kHz. One sampler cuts it into four equal slices. |
| `voice.wav` | A voice-like phrase, 24 kHz, 2 s (one bar at 120 BPM): four sung notes of an "ah" vowel made by formant synthesis. The test plays it into the null device's loopback and records it back. |
| `pluck.wav` | Four plucked-string notes, 24 kHz, 2 s (one bar at 120 BPM), which the audio parts play through the plug-ins. |
