# Behaviour name

<!-- Copy this file to docs/specs/<topic>.md, read README.md in this folder, then replace every comment and placeholder. Keep the headings. -->

- **Status:** draft <!-- draft, partial, or implemented -->
- **Work packages:** WP-x.y
- **Parity-matrix row:** Section: Live feature
- **Checked against:** Live 12.x.y, on DD Month YYYY

## The behaviour

<!-- One or two paragraphs in your own words: what the user does, and what happens. Say what this spec covers, and what it leaves to another spec. -->

## Sources

<!-- Every source the rules rest on, from the allowed sources in CONTRIBUTING.md ("Building without Ableton's code") only. Paraphrase; never paste Ableton's text. -->

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter N, "Chapter title", section N.N | <!-- what it says, in your own words --> |
| S2 | Observation | Live 12.x.y on <!-- OS and version -->, DD Month YYYY | <!-- the steps you took, and what you saw --> |
| S3 | Measurement | Live 12.x.y on <!-- OS and version -->, DD Month YYYY | <!-- the method, input material, settings, tool, and result with its tolerance --> |

## Rules

<!-- Statements a test can check, in the present tense, each ending with the sources it rests on. Never renumber; strike a withdrawn rule through and keep its number. -->

- **R1.** When <!-- condition -->, Beatlayer <!-- does what -->. (S1, S2)
- **R2.** <!-- ... --> (S3)

## Tests

<!-- The test that encodes each rule. Mark each test in the code with a comment such as `// Spec: <topic> R1`. -->

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/<crate>/tests/<file>.rs`, `<test_name>` | <!-- unit, property, golden render, or UI script --> |
| R2 | | |
