# Beatlayer

Beatlayer is an open-source digital audio workstation with the workflow of Ableton Live 12: Session and Arrangement views, warping, racks, and a full set of instruments and effects. It's written in Rust and runs natively on macOS (Apple Silicon), Windows, and Linux.

Beatlayer is in planning. There's no code to run yet.

## Documents

| Document | What it covers |
|---|---|
| [Roadmap](docs/01-roadmap.md) | Principles, milestones from M0 to 1.0, scope, size, and risks |
| [Architecture](docs/02-architecture.md) | The document, engine, and UI split; crates; threads; real-time rules; testing; packaging; the decisions log |
| [Build plan](docs/03-build-plan.md) | Work packages, dependencies, acceptance criteria, human tasks, and the status table |
| [Parity matrix](docs/04-parity-matrix.md) | Every Ableton Live 12 feature and the milestone that covers it |
| [Clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code) | How Beatlayer learns from Live without using its code or content |

## Licence

GPL-3.0-or-later, pending confirmation (decision D3 in the architecture).

Ableton, Ableton Live, and Push are trademarks of Ableton AG, and Max is a trademark of Cycling '74. Beatlayer isn't affiliated with or endorsed by either.
