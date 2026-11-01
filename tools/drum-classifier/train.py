#!/usr/bin/env python3
"""Trains Beatlayer's drum classifier (the audio-to-MIDI spec, R21) and writes weights.txt beside this script.

The classifier tells a kick, a snare and a hi-hat apart in each onset that
crates/analysis/src/drums.rs finds, from seven numbers that describe it. Its
training set is renders of Beatlayer's own Drum Sampler playing generated
sounds (kicks, snares and hats with varied tuning, decay, level and noise),
in breaks of two bars, clean, with a generated room and with noise, from a
fixed seed: no third-party audio is used. `bl-cli drum-training-set` renders
it and writes a line for each onset: three 1s or 0s for the classes that
sounded there, then its features. training.txt beside this script is that
set for SEED and BREAKS below.

The model is a network of one hidden layer of tanh units and a logistic
output for each class (so one onset can be several drums), trained by
minibatch gradient descent on the cross-entropy. The start and the order of
the minibatches come from Python's Mersenne Twister, seeded with SEED, whose
output is the same on every computer; the arithmetic is plain IEEE doubles.
Weights are written to six decimals.

Run it from anywhere with Python 3.10 or later and no packages. With no
arguments it renders the training set (a debug build of bl-cli takes about
20 seconds) and writes weights.txt; with --features, from a training set
written earlier, such as training.txt, and the same weights come out:

    python3 tools/drum-classifier/train.py [--features FILE] [--out FILE]
"""

from __future__ import annotations

import argparse
import math
import random
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
# The seed of the training breaks and of the network's start and minibatches,
# and how many breaks there are.
SEED = 1
BREAKS = 200
HIDDEN = 16
EPOCHS = 80
BATCH = 64
RATE = 0.05
CLASSES = ["kick", "snare", "hat"]


def sigmoid(z: float) -> float:
    return 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, z))))


def hidden(w1: list[list[float]], x: list[float]) -> list[float]:
    return [math.tanh(w[0] + sum(wj * xj for wj, xj in zip(w[1:], x))) for w in w1]


def train(xs: list[list[float]], ys: list[list[int]]) -> tuple[list[list[float]], list[list[float]]]:
    """The hidden layer's and the outputs' weights, each row led by its bias."""
    rnd = random.Random(SEED)
    inputs = len(xs[0])
    w1 = [[(rnd.random() * 2 - 1) * 0.7 for _ in range(inputs + 1)] for _ in range(HIDDEN)]
    w2 = [[(rnd.random() * 2 - 1) * 0.7 for _ in range(HIDDEN + 1)] for _ in CLASSES]
    order = list(range(len(xs)))
    for _ in range(EPOCHS):
        rnd.shuffle(order)
        for start in range(0, len(order), BATCH):
            batch = order[start : start + BATCH]
            g1 = [[0.0] * (inputs + 1) for _ in range(HIDDEN)]
            g2 = [[0.0] * (HIDDEN + 1) for _ in CLASSES]
            for i in batch:
                x, h = xs[i], hidden(w1, xs[i])
                d = [sigmoid(w[0] + sum(wk * hk for wk, hk in zip(w[1:], h))) - y for w, y in zip(w2, ys[i])]
                for c, dc in enumerate(d):
                    g2[c][0] += dc
                    for k in range(HIDDEN):
                        g2[c][k + 1] += dc * h[k]
                for k in range(HIDDEN):
                    back = sum(d[c] * w2[c][k + 1] for c in range(len(CLASSES))) * (1 - h[k] * h[k])
                    g1[k][0] += back
                    for j in range(inputs):
                        g1[k][j + 1] += back * x[j]
            for w, g in zip(w1 + w2, g1 + g2):
                for j, gj in enumerate(g):
                    w[j] -= RATE * gj / len(batch)
    return w1, w2


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--features", type=Path, help="a training set written by `bl-cli drum-training-set`")
    parser.add_argument("--out", type=Path, default=HERE / "weights.txt")
    args = parser.parse_args()
    if args.features:
        text = args.features.read_text()
    else:
        text = subprocess.run(
            ["cargo", "run", "--quiet", "--package", "bl-cli", "--", "drum-training-set",
             "--seed", str(SEED), "--breaks", str(BREAKS)],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    data = [[float(w) for w in line.split()] for line in text.splitlines() if line.strip()]
    ys = [[int(v) for v in r[:3]] for r in data]
    features = [r[3:] for r in data]
    count = len(features[0])
    mean = [sum(f[j] for f in features) / len(features) for j in range(count)]
    scale = [
        math.sqrt(sum((f[j] - mean[j]) ** 2 for f in features) / len(features)) or 1.0
        for j in range(count)
    ]
    xs = [[(f[j] - mean[j]) / scale[j] for j in range(count)] for f in features]
    w1, w2 = train(xs, ys)
    row = lambda name, w: name + " " + " ".join(f"{v:.6f}" for v in w)
    lines = [
        f"# Trained by tools/drum-classifier/train.py from seed {SEED}, {BREAKS} breaks, {len(xs)} onsets.",
        "# The features' means and scales, then the hidden layer and an output for each class, each a bias and a weight for each input.",
        row("mean", mean),
        row("scale", scale),
        *[row("hidden", w) for w in w1],
        *[row(name, w) for name, w in zip(CLASSES, w2)],
    ]
    args.out.write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    sys.exit(main())
