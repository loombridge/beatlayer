# Beatlayer's library

- **Status:** draft
- **Work packages:** WP-9.46 (the manifest, the licence allowlist, the build tool, packages and their first-use download, the browser's place, tags and credits: R1 to R16); WP-9.47 (multisampled instruments from open-licensed recordings: R17 to R21); WP-9.48 (presets and racks for every native device, vocal racks and drum kits by genre: R22 to R27); WP-9.49 (loops, MIDI clips, grooves, chord banks, drones, textures and foley: R28 to R33). The Packages manager that downloads it is [stem separation](stem-separation.md) R15 to R21; the browser's places and labels are [browser](browser.md) R31's and [browser tags](browser-tags.md)'
- **Parity-matrix row:** Other: (Beatlayer's own) a library covering the same ground as Live's Packs, tagged on arrival
- **Checked against:** Beatlayer's own design, 30 October 2026; the Creative Commons licences CC0 1.0 and CC BY 4.0

## The behaviour

Beatlayer ships its own sound library, covering the same ground as Live's packs: multisampled instruments, presets and racks for every native instrument and effect, drum kits by genre, loops, MIDI clips, grooves, chord banks, drones, textures and foley. Every item is recorded, rendered from Beatlayer's own devices, or taken from open-licensed sources, never from Ableton's content, and carries its licence, its attribution and its tags from the moment it arrives.

The library isn't in the repository. A manifest in the repository names every item, where it comes from (a source recording with its licence, or a script that generates it) and its tags; `cargo xtask library` builds the library's packages from it outside the repository, and Beatlayer downloads them on first use, through the same Packages manager as its on-device models. Once installed, the library shows in the browser with its tags, found by search, with a credit for each item and a list of credits to export.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Standard | Creative Commons, CC0 1.0 Universal and Attribution 4.0 International (CC BY 4.0) | What each licence allows, and what attribution CC BY needs: the creator, the title, the licence and a link, and any changes made |
| S2 | Beatlayer | The parity matrix's library row; CONTRIBUTING.md, "Building without Ableton's code"; the build plan's rules (no third-party sample content in the repository) | What the library covers; that it's never Ableton's; that no audio enters the repository |
| S3 | Spec | [Browser](browser.md) (places and labels); [browser tags](browser-tags.md) (the database, tags and groups); [stem separation](stem-separation.md) R15 to R21 (the Packages manager); [multisampler](multisampler.md) (SFZ import); [set exchange](set-exchange.md) (`.blpack`); [clip files](clip-files.md); [grooves](grooves.md); [MIDI generators](midi-generators.md) R24 to R27 (chord banks) | Where library items show and how they're loaded |
| S4 | Standard | The SFZ format, as documented by the SFZ Format project (sfzformat.com) | How multisampled instruments map samples to keys and velocities |

## Rules

### The manifest and its packages (WP-9.46)

- **R1.** The library's manifest is a set of TOML files under `library/` in the repository. Each item has an ID, a name, its kind (instrument, preset, rack, drum kit, loop, MIDI clip, groove, chord bank, one-shot, texture), its source (a URL and the source's own version or checksum, or a script under `library/scripts/` that generates it from a seed), its licence, its attribution (creator, title, link and changes made), its tags, and the package it goes in. (S1, S2)
- **R2.** Only CC0 1.0, CC BY 4.0 and items Beatlayer made itself (which are CC0) are allowed. An item with any other licence, or no licence, attribution (for CC BY), checksum or tags, makes `cargo xtask library` refuse the whole package, naming the item. (S1, S2)
- **R3.** `cargo xtask library` builds each package outside the repository: it fetches each source, checks its checksum, converts or renders it by its script, writes Beatlayer's own files (samples, presets, racks, clips, grooves, chord banks) and a package manifest with each file's SHA-256, and archives the package. The same manifest and sources give the same package byte for byte. (S2)
- **R4.** No audio, sample or rendered file of the library is committed to the repository: only the manifest, the scripts and their seeds. (S2)
- **R5.** The library's packages are entries in the Packages manager ([stem separation](stem-separation.md) R15), in the role `library` with a package per part (Core, Instruments, Drums, Loops), each with its URL, size and SHA-256, downloaded on first use. (S3)
- **R6.** The first time the person opens the library's place in the browser without its packages, the place shows the packages, their sizes and the folder they install in, with Download and Cancel, as the package page does ([stem separation](stem-separation.md) R16). (S3; Beatlayer's own)
- **R7.** An installed package unpacks into `Library` in Beatlayer's application-data folder, read-only; a person's edits to a library item save to the User Library. (Beatlayer's own)

### In the browser (WP-9.46)

- **R8.** The library shows in the browser as the Library label Core Library, among the Library labels where Live lists its own ([browser](browser.md) R31), and its items show under the content labels (Sounds, Drums, Instruments, Audio Effects, Clips, Samples, Grooves) beside the person's own. (S3; the name is Beatlayer's own)
- **R9.** Each item carries its manifest's tags in the browser's database from install ([browser tags](browser-tags.md)), with Beatlayer's own groups and the library's: Genre, Instrument, Character, and Source (Recorded, Generated, Open-licensed). (S3; the groups are Beatlayer's own)
- **R10.** Search by name and tag finds library items as it finds the person's. (S3)
- **R11.** A library item's credit (its creator, title, licence with its link, and changes made) shows in the Info View when it's selected in the browser, and in the device's or clip's info once loaded. (S1)
- **R12.** Help ▸ Library Credits... opens a window listing every installed item's credit, grouped by package, with Export Credits..., which writes them as a text file. A set's own credits (the library items it uses) export from File ▸ Export Library Credits.... (S1; Beatlayer's own)
- **R13.** `library.state` (`read`) gives each package, installed or not, and its item count; `library.credits` (`read`) gives the credits of the installed items, or of a set's items. (S2)
- **R14.** A set that uses a library item refers to it by its library ID and version and its file's content hash; a set opened where the library isn't installed lists the missing items and offers the download. (S3)
- **R15.** A library item's previews and similarity analysis are worked out once on install, so it's analysed before it's first searched ([sound similarity](sound-similarity.md) R1). (S3)
- **R16.** The library's packages install headless, under `bl-cli mcp`, as the models do. (S3)

### What it covers (WP-9.47 to WP-9.49)

- **R17.** Multisampled instruments, each converted by a script from an open-licensed recording through SFZ import ([multisampler](multisampler.md)) into a Sampler preset or a Drum Rack: grand and upright pianos, electric pianos and organs, guitars and basses, orchestral strings, brass, woodwinds and mallets, quartets, world percussion, and acoustic drums dry and in a room. (S2, S4)
- **R18.** Every zone of a converted instrument plays; its zones are loudness-matched to within 1 LU of each other at their middle velocity. (Beatlayer's own)
- **R19.** A conversion repeats byte for byte from the same source. (S2)
- **R20.** An instrument with no open source isn't faked: it's left out until it's recorded. (S2)
- **R21.** Each instrument's preset sets its own release, round robins and velocity layers as its source allows. (S4)
- **R22.** Every native instrument has at least 24 library presets and every native effect at least 8, generated by script from data files a person can review, each loading without error and tagged. (S2; the counts are Beatlayer's own)
- **R23.** Vocal effect racks: at least 12 racks for vocals, of Beatlayer's own effects. (S2)
- **R24.** Drum kits by genre (hip-hop, trap, club, machine), as Drum Racks of one-shots rendered from Beatlayer's own drum synthesis or from open-licensed one-shots, at least 8 kits per genre; every kit's pads play and choke as its manifest says. (S2)
- **R25.** Each preset's preview render ([browser](browser.md) R27) is between −20 and −10 LUFS. (Beatlayer's own)
- **R26.** Presets for devices not built yet when a package runs wait for a later package. (S2)
- **R27.** Each kit and preset is credited as its sources are, and Beatlayer's own renders as Beatlayer's. (S1)
- **R28.** Loops carry their tempo and key, and warp to the set's tempo when added, as a loop with a known tempo does ([warping](warping.md)). (S3)
- **R29.** MIDI clips, generated by script, load as clip files ([clip files](clip-files.md)). (S3)
- **R30.** Grooves, generated by script, list under the Grooves label and load into the Groove Pool ([grooves](grooves.md)). (S3)
- **R31.** Chord banks load in Stacks ([MIDI generators](midi-generators.md) R24 to R27). (S3)
- **R32.** Drones, textures and foley come from open-licensed recordings or Beatlayer's own renders. (S2)
- **R33.** Every item in R28 to R32 is tagged and credited. (S1)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R16 | to be written in WP-9.46: `xtask` tests on a test manifest; `crates/shell/tests/library.rs` installing from a `file://` URL | unit, shell test |
| R17 to R21 | to be written in WP-9.47: conversion scripts' tests; every zone plays | unit |
| R22 to R27 | to be written in WP-9.48 | unit, shell test |
| R28 to R33 | to be written in WP-9.49 | unit, shell test |
