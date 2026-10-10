# Media decode fixtures

Short generated audio files in every format Beatlayer imports, for the decode tests in `crates/media/tests/`. Nothing here is recorded or third-party audio: `tests/fixtures/gen/gen_media.py` synthesises the signals (sines, sweeps, white noise, silence, and clipping edge cases), writes them as WAV byte by byte, and encodes them with outside encoders.

`manifest.json` lists every file with its source WAV, signal, container, codec, rate, channels, length, the encoder and its version, and the command that made it. The tests read it, so a new fixture needs only a new line in the generator. Don't edit the files or the manifest by hand: rerun the generator, as its docstring describes.

The encoders, as of 10 October 2026:

| Formats | Encoder |
|---|---|
| WAV sources, including WAVE_FORMAT_EXTENSIBLE and Broadcast Wave; BW64 | `gen_media.py` itself |
| RF64 | ffmpeg 8.1.1 (`-rf64 always`) |
| FLAC | ffmpeg 8.1.1 |
| MP3 | LAME 3.100 |
| Ogg Vorbis | libvorbis, through libsndfile 1.2.2 (`soundfile` 0.14.0) |
| AIFF, AIFF-C, AAC (M4A and ADTS), ALAC, and RIFF with a reserved ds64 JUNK chunk | afconvert, macOS 26.6 |
