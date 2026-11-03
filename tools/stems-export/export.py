"""Exports HTDemucs to ONNX with its STFT and inverse STFT left out.

The graph takes the mix (`mix`, [1, 2, L]) and its spectrogram as the
network sees it (`spec`, [1, 4, F, T]: left real, left imaginary, right
real, right imaginary), and gives back the spectrogram branch's output
(`spec_out`, [1, S, 4, F, T]) and the waveform branch's (`time_out`,
[1, S, 2, L]). Beatlayer turns the first back into a waveform with its own
inverse STFT and adds the two. The transform's settings travel in the
file's metadata, so Beatlayer reads them from the model.

Run it with tools/stems-export/README.md's steps; CI never does.
"""

import argparse
import json
import math

import onnx
import torch
from demucs.pretrained import get_model


class Split(torch.nn.Module):
    """HTDemucs with `_spec`, `_magnitude`, `_mask` and `_ispec` replaced
    so the transforms stay outside and the two branches come out apart."""

    def __init__(self, model):
        super().__init__()
        self.model = model
        self.held = []

    def forward(self, mix, spec):
        model = self.model
        model._spec = lambda _x: spec
        model._magnitude = lambda z: z

        def mask(_z, m):
            self.held.append(m)
            return m

        model._mask = mask
        model._ispec = lambda _z, length, scale=0: torch.zeros_like(mix)[:, None].expand(
            -1, len(model.sources), -1, -1
        )[..., :length]
        time_out = model(mix)
        return self.held.pop(), time_out


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", help="where to write the .onnx file")
    parser.add_argument("--name", default="htdemucs")
    args = parser.parse_args()

    model = get_model(args.name)
    inner = model.models[0] if hasattr(model, "models") else model
    inner.eval()
    assert inner.cac, "only complex-as-channels models are handled"
    nfft, hop = inner.nfft, inner.hop_length
    frames = int(inner.segment * inner.samplerate)
    le = math.ceil(frames / hop)
    bins = nfft // 2
    sources = list(inner.sources)

    mix = torch.randn(1, inner.audio_channels, frames)
    spec = torch.randn(1, inner.audio_channels * 2, bins, le)
    wrapper = Split(inner).eval()
    torch.onnx.export(
        wrapper,
        (mix, spec),
        args.output,
        input_names=["mix", "spec"],
        output_names=["spec_out", "time_out"],
        opset_version=17,
        dynamo=False,
    )

    proto = onnx.load(args.output)
    settings = {
        "samplerate": inner.samplerate,
        "channels": inner.audio_channels,
        "window_frames": frames,
        "nfft": nfft,
        "hop": hop,
        "window": "hann",
        "normalized": True,
        "center_pad": "reflect",
        "sources": ",".join(sources),
    }
    for key, value in settings.items():
        entry = proto.metadata_props.add()
        entry.key = f"beatlayer.stft.{key}"
        entry.value = str(value).lower() if isinstance(value, bool) else str(value)
    onnx.save(proto, args.output, save_as_external_data=False)
    print(json.dumps(settings))


if __name__ == "__main__":
    main()
