# Interface languages

- **Status:** draft
- **Work packages:** WP-9.50 (the message catalogue, the Language chooser, menus, Settings and dialogs through it, the pseudo-language and the check: R1 to R12); WP-9.51 (the remaining views, the Info View's help texts, device and parameter names, units, value formats and context menus, and the check's allowlist emptied: R13 to R18); WP-9.52 (the languages Live offers, their fonts and text layout: R19 to R24). The Language row's place is [interface zoom](interface-zoom.md) R4's; names read by screen readers are [accessibility](accessibility.md) R14's
- **Parity-matrix row:** Views and workflow: Interface languages
- **Checked against:** the Live 12 reference manual (online edition), chapter 2 section 2.3 "Settings", read 30 October 2026; the UI parity reference row L-SET-010; Project Fluent's syntax guide

## The behaviour

Display & Input's Language chooser sets the language Beatlayer's interface speaks: its menus, Settings, dialogs, views, help texts, devices' controls and the formats of values. Every string a person reads comes from a message catalogue, one file per language, so a translation is a file to add, and a check stops a new string from bypassing it. What a set, a command or an agent sees doesn't change with the language: command IDs, the agent API and its descriptions, file formats and saved names stay as they are.

A pseudo-language, made from English with accents and 40 % more length, shows on screen any string that hasn't gone through the catalogue and any label too short for longer languages. Chinese and Japanese draw in bundled fonts, with their own line breaking.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 2 "First Steps", section 2.3 "Settings" (Display & Input) | The Display & Input page's Language chooser sets the interface's language |
| S2 | Reference | [layout.md](../ui-parity/layout.md) L-SET-010 (Display & Input: Display's Language, then Zoom) | Where Language sits |
| S3 | Standard | Project Fluent's syntax guide and the `fluent-bundle` and `unic-langid` crates (Apache-2.0 and MIT); Unicode Standard Annex #14, the Unicode line breaking algorithm; Unicode CLDR's number formats | Message files with plurals and variables; language identifiers; where lines may break in Chinese and Japanese; each language's decimal separator |
| S4 | Spec | [Interface zoom](interface-zoom.md) R4; [accessibility](accessibility.md) R1; [info view](info-view.md); [device framework](device-framework.md) (value formats); the architecture's "Agent control" (D17) | The Language row; names for assistive technology; help texts; how values are written; commands as the API |

## Rules

### The catalogue and the chooser (WP-9.50)

- **R1.** Every string a person reads in Beatlayer's interface is a message in the catalogue, by an ID of its own, never a literal in the code that draws it. English is the catalogue's first language and its fallback. (S3)
- **R2.** The catalogue is Fluent: a `.ftl` file per language under `crates/ui/locales/<language>/`, messages with variables and plural forms. A message missing in a language falls back to English. (S3)
- **R3.** Display & Input's Language chooser (L-SET-010) lists each language by its own name (English, Deutsch, Français, 日本語, 简体中文, Español, Italiano) and, in a development build, the pseudo-language. The choice is kept in the preferences; a new install takes the system's language if the catalogue has it, English otherwise. (S1, S2; the system's language is Beatlayer's own)
- **R4.** A new language applies at the next launch: choosing one says so under the chooser, with Restart Now, which quits and starts Beatlayer again after asking to save the set. (Beatlayer's own)
- **R5.** `settings.set_language` (`edit`, a kept preference) names `settings.state`'s `language` as its readback. (S4)
- **R6.** Command IDs, the agent API's tools, its commands' descriptions and schemas, set files, presets, file names Beatlayer writes, and saved names (tracks, clips, scenes) don't change with the language. A command's label in a menu is a catalogue message. (S4; keeping the agent API in English is Beatlayer's own)
- **R7.** The pseudo-language turns each English message into accented letters and makes it 40 % longer, with brackets round it, so untranslated text (no accents) and clipped text (a missing bracket) show on screen. (Beatlayer's own)
- **R8.** WP-9.50 puts the menus, every Settings page and every dialog through the catalogue. (Beatlayer's own)
- **R9.** `cargo xtask strings` fails on a string literal passed to a drawing call in `bl-ui`, outside an allowlist of files not yet converted, which only shrinks. (Beatlayer's own)
- **R10.** With the pseudo-language, UI scripts find nothing clipped and no English in every menu, Settings page and dialog. (Beatlayer's own)
- **R11.** Labels whose width is fixed by the layout (the Control Bar's, the device title bars') shrink their text to fit or end it with an ellipsis, showing the whole in their hover text; nothing is drawn outside its bounds. (Beatlayer's own)
- **R12.** Keyboard shortcuts don't change with the language. (Beatlayer's own)

### The rest of the interface (WP-9.51)

- **R13.** The remaining views, the Info View's help texts, context menus, units and value formats go through the catalogue, and the check's allowlist is empty. (Beatlayer's own)
- **R14.** Device names (Drift, Operator, Reverb) and their parameters' names stay in English in every language, as names; their help texts are translated. (S1; Beatlayer's own reading)
- **R15.** Numbers are written with the language's decimal separator (a comma in German, French, Spanish and Italian) in value displays and number fields, and a number typed with either separator is read. Units keep their symbols (dB, Hz, ms, %). (S3)
- **R16.** Note names stay as C, C#, D and so on in every language. (Beatlayer's own)
- **R17.** With the pseudo-language, every view's UI script finds no English and nothing clipped. (Beatlayer's own)
- **R18.** Assistive technology reads names and descriptions in the chosen language ([accessibility](accessibility.md) R14). (S4)

### Languages (WP-9.52)

- **R19.** The catalogue covers English, German, French, Japanese, Simplified Chinese, Spanish and Italian, each message in each; `cargo xtask strings` fails on a message missing in a shipped language. (S1; the list is Beatlayer's own reading)
- **R20.** Chinese and Japanese draw in bundled fonts under the SIL Open Font Licence 1.1, subset to the characters the catalogues use plus the common ones, and used as fallbacks after Beatlayer's typeface. (S3)
- **R21.** Text in Chinese and Japanese breaks lines by the Unicode line breaking algorithm, between characters where it allows. (S3)
- **R22.** With each language chosen, UI scripts find nothing clipped and no message missing. (Beatlayer's own)
- **R23.** Each translation names, in its file's header, the version of the English catalogue it follows. (Beatlayer's own)
- **R24.** Right-to-left languages aren't offered. (Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R12 | to be written in WP-9.50: `cargo xtask strings`; `crates/ui/tests/translations.rs` with the pseudo-language; `crates/shell/tests/settings.rs` for the preference | UI script, unit |
| R13 to R18 | to be written in WP-9.51 | UI script |
| R19 to R24 | to be written in WP-9.52 | UI script, unit |
