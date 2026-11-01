#!/usr/bin/env python3
"""Fits the classifier that gives samples their automatic tags (WP-9.11, the
browser tags spec R53) and writes its weights to crates/analysis/src/autotag_weights.txt.

It makes 40 labelled sounds of each of five classes (Kick, Snare, Hi-Hat, Bass,
Pad) with the recipes of tests/fixtures/gen/gen_families.py, from seeds the
fixtures' sounds don't use, so the sounds the classifier is tested on are not
the ones it was fitted on. It has `bl-analysis` describe them (`cargo run -p
bl-analysis --example describe`), scales each measurement by its mean and
standard deviation over the training sounds, and fits a softmax regression to
them by batch gradient descent from zero weights, a fixed number of steps. Nothing
in it is random beyond the generator's own seeds, so the same script writes the
same file.

Run it from the repository's root with Python 3.10 or later and no packages:

    python3 tests/fixtures/gen/fit_autotags.py

and again whenever the descriptor (`bl_analysis::similarity::VERSION`) changes.
Commit the file it writes; the `autotag` tests check what it makes.
"""

from __future__ import annotations

import importlib.util
import math
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent.parent
OUT = ROOT / "crates" / "analysis" / "src" / "autotag_weights.txt"

# The Function tag each family of gen_families.py stands for.
CLASSES = {"kick": "Kick", "snare": "Snare", "hat": "Hi-Hat", "bass": "Bass", "pad": "Pad"}
PER_CLASS = 40
SEED = 0x51ED2701
STEPS = 3000
RATE = 1.0
DECAY = 0.0005


def load_families():
    spec = importlib.util.spec_from_file_location("gen_families", HERE / "gen_families.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def describe(folder: Path) -> dict[str, list[float]]:
    """The descriptor of each WAV file in `folder`, from the Rust example."""
    run = subprocess.run(
        ["cargo", "run", "-q", "-p", "bl-analysis", "--example", "describe", "--", str(folder)],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )
    found = {}
    for line in run.stdout.splitlines():
        name, numbers = line.split("\t")
        found[name] = [float(x) for x in numbers.split()]
    return found


def softmax(scores: list[float]) -> list[float]:
    top = max(scores)
    exps = [math.exp(s - top) for s in scores]
    total = sum(exps)
    return [e / total for e in exps]


def fit(rows: list[list[float]], labels: list[int], classes: int):
    """Scaling and softmax weights for `rows`, labelled `labels`."""
    n, d = len(rows), len(rows[0])
    mean = [sum(r[i] for r in rows) / n for i in range(d)]
    std = [math.sqrt(sum((r[i] - mean[i]) ** 2 for r in rows) / n) for i in range(d)]
    scale = [1.0 / s if s > 1e-9 else 1.0 for s in std]
    x = [[(r[i] - mean[i]) * scale[i] for i in range(d)] for r in rows]
    w = [[0.0] * d for _ in range(classes)]
    b = [0.0] * classes
    for _ in range(STEPS):
        gw = [[0.0] * d for _ in range(classes)]
        gb = [0.0] * classes
        for row, label in zip(x, labels):
            p = softmax([sum(wi * xi for wi, xi in zip(w[c], row)) + b[c] for c in range(classes)])
            for c in range(classes):
                err = p[c] - (1.0 if c == label else 0.0)
                gb[c] += err
                gc = gw[c]
                for i in range(d):
                    gc[i] += err * row[i]
        for c in range(classes):
            b[c] -= RATE * gb[c] / n
            for i in range(d):
                w[c][i] -= RATE * (gw[c][i] / n + DECAY * w[c][i])
    return mean, scale, w, b


def main() -> None:
    families = load_families()
    names = list(CLASSES)
    with tempfile.TemporaryDirectory() as folder:
        for index, family in enumerate(names):
            for i in range(PER_CLASS):
                noise = families.Noise(SEED + 1000 * index + i)
                families.write(
                    Path(folder) / f"{index}-{i:03}.wav", families.FAMILIES[family](noise)
                )
        described = describe(Path(folder))
    rows, labels = [], []
    for name in sorted(described):
        rows.append(described[name])
        labels.append(int(name.split("-")[0]))
    mean, scale, w, b = fit(rows, labels, len(names))

    def line(tag: str, values: list[float]) -> str:
        return tag + " " + " ".join(repr(float(v)) for v in values)

    out = ["# Written by tests/fixtures/gen/fit_autotags.py. Don't edit.",
           "classes " + " ".join(CLASSES[f] for f in names),
           line("mean", mean), line("scale", scale), line("bias", b)]
    out += [line(f"weights {CLASSES[f]}", w[c]) for c, f in enumerate(names)]
    OUT.write_text("\n".join(out) + "\n")
    right = 0
    for row, label in zip(rows, labels):
        x = [(row[i] - mean[i]) * scale[i] for i in range(len(row))]
        scores = [sum(wi * xi for wi, xi in zip(w[c], x)) + b[c] for c in range(len(names))]
        right += scores.index(max(scores)) == label
    print(f"wrote {OUT}: {right} of {len(rows)} training sounds right")


if __name__ == "__main__":
    main()
