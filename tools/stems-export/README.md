# Exporting the split stem separation model

Beatlayer separates stems with HTDemucs run through ONNX Runtime. On the
GPU (Core ML on macOS) the network can't run its short-time Fourier
transform, so `export.py` writes HTDemucs *without* the transform and its
inverse: Beatlayer computes them itself in `bl-analysis`
(`crates/analysis/src/stems/transform.rs`), with the settings the model
carries in its metadata.

This is a tool for people who build the model. It is outside the Cargo
workspace and continuous integration never runs it.

## The model's shape

- Inputs: `mix` (`[1, 2, 343980]`, a stereo window at 44.1 kHz) and `spec`
  (`[1, 4, 2048, 336]`: the window's spectrogram, each channel's real part
  and then its imaginary part).
- Outputs: `spec_out` (`[1, 4, 4, 2048, 336]`, the spectrogram branch's
  output for the four sources) and `time_out` (`[1, 4, 2, 343980]`, the
  waveform branch's). Beatlayer inverts `spec_out` and adds `time_out`.
- Metadata `beatlayer.stft.*`: the sample rate, window length, FFT size,
  hop, window shape, normalisation, padding and the sources in order. They
  are read from the Demucs model, and Beatlayer refuses a model whose
  settings aren't the ones its transform computes.

## Running it

```sh
python3.12 -m venv venv
venv/bin/pip install -r requirements.txt
venv/bin/python export.py htdemucs-split.onnx
```

The first run downloads Demucs's `htdemucs` weights. The script prints the
settings it wrote. Use the file with:

```sh
BL_STEMS_RUNTIME=/path/to/libonnxruntime.dylib \
BL_STEMS_MODEL=/path/to/htdemucs-split.onnx \
cargo nextest run -p bl-analysis --features stems -E 'test(/stems_model/)'
```

`BL_STEMS_WHOLE_MODEL` names the whole-graph ONNX model that the split
model's stems are compared with.

## The speed benchmark

```sh
cargo run --release -p bl-cli -- bench stems \
  --runtime /path/to/libonnxruntime.dylib --model /path/to/htdemucs-split.onnx
```

It prints JSON: the platform, the provider, the first compile's time (null
when the compiled model was cached already) and each mode's time on a
generated 3-minute mix.
