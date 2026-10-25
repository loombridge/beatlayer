#!/usr/bin/env python3
"""Writes Beatlayer's own tunings, the `.ascl` files in crates/shell/tunings/ (WP-6.10, the tunings spec R31).

Every file is written from its mathematical definition alone, so no
third-party tuning file is involved and the same script always writes the same
bytes. A file holds the Scala body (a description, the number of notes and one
pitch per note, the last the pseudo-octave); the extra comments an `.ascl`
file may carry (note names, a reference pitch, a range) are not written yet.

- N Equal Steps: equal divisions of the octave, in cents;
- Just Intonation: 5-limit and 7-limit twelve-note scales and the harmonics
  8 to 16, in ratios;
- Pythagorean, Quarter-Comma Meantone, Werckmeister III, Kirnberger III,
  Vallotti and Young: twelve-note temperaments, each a circle of fifths whose
  sizes are set below, in cents;
- Bohlen-Pierce: 13 equal steps of 3/1.

Run it from anywhere with Python 3.10 or later and no packages. With a
directory it writes the files there; with none it writes them into
crates/shell/tunings/:

    python3 tests/fixtures/gen/gen_bundled_tunings.py [directory]
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parents[3] / "crates" / "shell" / "tunings"

OCTAVE = 1200.0
FIFTH = OCTAVE * math.log2(3 / 2)
# The Pythagorean comma (twelve fifths against seven octaves) and the
# syntonic comma (four fifths against two octaves and a major third).
PYTHAGOREAN_COMMA = 12 * FIFTH - 7 * OCTAVE
SYNTONIC_COMMA = 4 * FIFTH - 2 * OCTAVE - OCTAVE * math.log2(5 / 4)

# The circle of fifths from C: fifth k leads from CHAIN[k] to CHAIN[k + 1].
CHAIN = ["C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#", "F"]


def cents(value: float) -> str:
    return f"{value:.6f}"


def body(description: str, pitches: list[str]) -> str:
    return "\n".join([description, str(len(pitches)), *pitches]) + "\n"


def equal(steps: int) -> tuple[str, str]:
    pitches = [cents(OCTAVE * i / steps) for i in range(1, steps + 1)]
    return f"{steps} Equal Steps", body(f"{steps} equal steps to the octave", pitches)


def ratios(name: str, description: str, ratio_list: list[str]) -> tuple[str, str]:
    return name, body(description, ratio_list)


def temperament(
    name: str, description: str, fifths: list[float | None]
) -> tuple[str, str]:
    """Twelve notes from a circle of fifths.

    `fifths` gives the size in cents of each of the twelve fifths, in the
    order of CHAIN. At most one is None, the wolf: it takes whatever is left
    of the seven octaves the circle spans. A circle with none must close.
    """
    wolf = fifths.index(None) if None in fifths else len(fifths) - 1
    position = [0.0] * 13
    for k in range(1, wolf + 1):
        position[k] = position[k - 1] + fifths[k - 1]
    for k in range(len(fifths) - 1, wolf, -1):
        position[k] = position[k + 1] - fifths[k]
    if None not in fifths:
        assert abs(sum(fifths) - 7 * OCTAVE) < 1e-9, name
    pitch_classes = sorted(position[k] % OCTAVE for k in range(1, 12))
    return name, body(description, [cents(c) for c in pitch_classes] + [cents(OCTAVE)])


def pure(narrowed: dict[int, float]) -> list[float | None]:
    """Twelve pure fifths, the ones in `narrowed` made smaller by a size."""
    return [FIFTH - narrowed.get(k, 0.0) for k in range(12)]


def tunings() -> list[tuple[str, str]]:
    quarter_comma = OCTAVE * math.log2(5) / 4
    wolf = [FIFTH] * 12
    wolf[CHAIN.index("G#")] = None
    meantone = [quarter_comma] * 12
    meantone[CHAIN.index("G#")] = None
    kirnberger = pure({k: SYNTONIC_COMMA / 4 for k in range(4)})
    kirnberger[CHAIN.index("F#")] = None
    list_ = [equal(n) for n in (5, 7, 10, 12, 15, 16, 17, 19, 22, 24, 31, 41, 53)]
    list_ += [
        ratios(
            "Just Intonation 5-Limit",
            "A twelve-note scale of 5-limit ratios",
            ["16/15", "9/8", "6/5", "5/4", "4/3", "45/32", "3/2", "8/5", "5/3", "9/5", "15/8", "2/1"],
        ),
        ratios(
            "Just Intonation 7-Limit",
            "A twelve-note scale of 7-limit ratios",
            ["15/14", "8/7", "6/5", "5/4", "4/3", "7/5", "3/2", "8/5", "5/3", "7/4", "15/8", "2/1"],
        ),
        ratios(
            "Harmonics 8 to 16",
            "The harmonics 8 to 16 of a fundamental, as ratios",
            [f"{n}/8" for n in range(9, 17)],
        ),
        temperament("Pythagorean", "Twelve pure fifths in a circle, the wolf between G# and D#", wolf),
        temperament(
            "Quarter-Comma Meantone",
            "Fifths narrowed by a quarter of the syntonic comma, the wolf between G# and D#",
            meantone,
        ),
        temperament(
            "Werckmeister III",
            "Four fifths narrowed by a quarter of the Pythagorean comma, the rest pure",
            pure({k: PYTHAGOREAN_COMMA / 4 for k in (0, 1, 2, 5)}),
        ),
        temperament(
            "Kirnberger III",
            "Four fifths from C narrowed by a quarter of the syntonic comma, the F# fifth taking the rest",
            kirnberger,
        ),
        temperament(
            "Vallotti",
            "Six fifths narrowed by a sixth of the Pythagorean comma, the rest pure",
            pure({k: PYTHAGOREAN_COMMA / 6 for k in (11, 0, 1, 2, 3, 4)}),
        ),
        temperament(
            "Young",
            "Six fifths from C narrowed by a sixth of the Pythagorean comma, the rest pure",
            pure({k: PYTHAGOREAN_COMMA / 6 for k in range(6)}),
        ),
        (
            "Bohlen-Pierce",
            body(
                "13 equal steps of 3/1",
                [cents(OCTAVE * math.log2(3) * i / 13) for i in range(1, 13)] + ["3/1"],
            ),
        ),
    ]
    return list_


def main() -> None:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else OUT
    out.mkdir(parents=True, exist_ok=True)
    for name, text in tunings():
        (out / f"{name}.ascl").write_text(text, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
