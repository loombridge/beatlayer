#!/usr/bin/env python3
"""Generates the tuning files in tests/fixtures/tunings/ (WP-6.9, the tunings spec R3).

Every file is written here from its mathematical definition, so no third-party
tuning file is involved and the same script always writes the same bytes:

- equal-5.scl, equal-12.scl, equal-19.scl, equal-31.scl: equal divisions of
  the octave, written in cents;
- bohlen-pierce.scl: 13 equal steps of 3/1, written as the ratio 3/1 at the
  end;
- just-12.scl: twelve pitches written as ratios, the last 2/1;
- well-12.scl: twelve pitches in cents, a well temperament made of equal steps
  and a comma spread over them;
- scrambled.scl: pitches that do not rise in order;
- no-pitches.scl: a count of 0;
- falling.scl: a pseudo-octave below the unison;
- short.scl: a count of 4 over three pitches;
- long.scl: a count of 2 over three pitches;
- ratio-zero.scl: a ratio of 0;
- empty-description.scl: an empty description line, comments and blank lines,
  and text after the pitches on their lines.

The reader's tests in crates/interop/tests/scala.rs read them. Run it from
anywhere with Python 3.10 or later and no packages:

    python3 tests/fixtures/gen/gen_tunings.py
"""

from __future__ import annotations

import math
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "tunings"


def cents(value: float) -> str:
    return f"{value:.6f}"


def equal(steps: int, span: float = 1200.0) -> str:
    lines = [f"! equal-{steps}.scl", f"{steps} equal steps to {span:g} cents", f"{steps}"]
    lines += [cents(span * i / steps) for i in range(1, steps + 1)]
    return "\n".join(lines) + "\n"


def just() -> str:
    ratios = ["16/15", "9/8", "6/5", "5/4", "4/3", "45/32", "3/2", "8/5", "5/3", "9/5", "15/8", "2/1"]
    lines = ["! just-12.scl", "A twelve-note scale of ratios", "12"] + ratios
    return "\n".join(lines) + "\n"


def well() -> str:
    # Equal steps, with the Pythagorean comma taken up by the six steps on
    # one side of the circle of fifths: every step is within 12 cents of equal.
    comma = 1200.0 * math.log2(3**12 / 2**19)
    steps = [100.0 - (comma / 12.0) * (1 if i % 2 == 0 else -1) / 6.0 for i in range(12)]
    total = 0.0
    lines = ["! well-12.scl", "A twelve-note well temperament", "12"]
    for i, step in enumerate(steps):
        total += step
        lines.append(cents(1200.0 if i == 11 else total))
    return "\n".join(lines) + "\n"


FILES = {
    "equal-5.scl": equal(5),
    "equal-12.scl": equal(12),
    "equal-19.scl": equal(19),
    "equal-31.scl": equal(31),
    "bohlen-pierce.scl": "! bohlen-pierce.scl\n13 equal steps of 3/1\n13\n"
    + "".join(f"{cents(1200.0 * math.log2(3) * i / 13)}\n" for i in range(1, 13))
    + "3/1\n",
    "just-12.scl": just(),
    "well-12.scl": well(),
    "scrambled.scl": "! scrambled.scl\nSteps that do not rise in order\n4\n300.0\n100.0\n700.0\n1200.0\n",
    "no-pitches.scl": "! no-pitches.scl\nNothing\n0\n",
    "falling.scl": "! falling.scl\nA pseudo-octave below the unison\n2\n100.0\n-1200.0\n",
    "short.scl": "! short.scl\nOne pitch short\n4\n100.0\n200.0\n1200.0\n",
    "long.scl": "! long.scl\nOne pitch too many\n2\n100.0\n1200.0\n1300.0\n",
    "ratio-zero.scl": "! ratio-zero.scl\nA ratio of zero\n2\n0/1\n2/1\n",
    "empty-description.scl": "! empty-description.scl\n! another comment\n\n3\n! between the pitches\n"
    "400.0  the third\n\n800.0 the sixth\n2/1 the octave\n\n",
}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, text in FILES.items():
        (OUT / name).write_text(text, newline="\n")


if __name__ == "__main__":
    main()
