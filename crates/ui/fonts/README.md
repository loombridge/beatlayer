# Bundled typefaces

Beatlayer's UI typeface is bundled into the binary, because egui's default fonts fail the licence policy and Ableton's typeface is Ableton's ([ADR-001](../../../docs/adr/ADR-001-ui-toolkit-egui.md), [clean-room policy](../../../CONTRIBUTING.md#building-without-abletons-code)). `deny.toml` allows OFL-1.1 for fonts only.

| File | Typeface | Version | Licence | Where it came from |
|---|---|---|---|---|
| `inter/Inter-Regular.ttf` | Inter Regular, by Rasmus Andersson and the Inter Project Authors | 3.019 (`git-0a5106e0b`) | SIL Open Font License 1.1, in `inter/Inter-OFL.txt` | The copy that ships in the `cosmic-text` 0.19.0 crate on crates.io (`fonts/Inter-Regular.ttf`, SHA-256 `3127f0b873387ee37e2040135a06e9e9c05030f509eb63689529becf28b50384`), unchanged |

The licence travels with the packaged app: `crates/app/Cargo.toml` lists `inter/Inter-OFL.txt` among the packaged resources. The font file isn't modified, and it isn't sold on its own, as the OFL requires.
