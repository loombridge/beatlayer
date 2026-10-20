# Interactions

How Live 12 responds to the mouse, to modifier keys held during a gesture, and to drag and drop, and how keyboard focus moves between areas. Beatlayer reproduces these (D16). Keyboard shortcuts are in [shortcuts.toml](shortcuts.toml); where a gesture also appears there, the row gives its S- ID. Read the [README](README.md) for sources and IDs.

Modifier names: **Cmd** and **Option** on macOS, **Ctrl** and **Alt** on Windows. Where Live uses a different modifier on each platform, both are given (macOS first). On macOS, Ctrl-click is the same as right-click.

Source: `obs` means observed in Live 12.4.3 on macOS on 10 October 2026, unless the row gives another Live version and date; `man N` is a section of the Live 12 manual, paraphrased; `release notes N` the Live 12 release notes for version N; `Beatlayer` a gesture of Beatlayer's own.

## Clicks and double-clicks (I-CLICK)

| ID | Where | Gesture | Result | Source |
|---|---|---|---|---|
| I-CLICK-001 | Session clip | Click the launch triangle | Launches the clip (subject to launch quantisation) | man 7.1 |
| I-CLICK-002 | Session clip | Click the clip's name | Selects the clip; the Clip View shows it | man 7.1 |
| I-CLICK-003 | Session slot or track status field | Click a stop square | Stops the track's clip | man 7.1 |
| I-CLICK-004 | Empty Session slot on a MIDI track | Double-click | Creates an empty one-bar MIDI clip there and shows it in the Clip View | obs |
| I-CLICK-005 | Any clip | Double-click | Opens it in the Clip View | man 3.9, 8 |
| I-CLICK-006 | Track title bar | Double-click | Opens the track's Device View | man 3.11, 23.1 |
| I-CLICK-007 | Device title bar | Double-click | Folds or unfolds the device | man 23.1 |
| I-CLICK-008 | Clip title bar (Clip View) | Double-click | Folds or unfolds the clip panels. Beatlayer: folded, the title bar stays and the editor takes the Clip View's width ([detail views](../specs/detail-views.md) R23) | man 8.1.2 |
| I-CLICK-009 | Scene launch button (Main track) | Click | Launches every clip in the scene | man 7.2 |
| I-CLICK-010 | Scene tempo or signature field | Double-click | Turns the field off | man 7.2.1 |
| I-CLICK-011 | Arrangement background | Click | Places the insert marker; on a track lane it also selects the track. With snapping on, the marker goes to the nearest bar. Not an undo step | obs, man 6.9 |
| I-CLICK-012 | Arrangement scrub area | Click | Starts playback from that point (jump quantised to the global quantisation) | man 6.3 |
| I-CLICK-013 | Arrangement scrub area | Press and hold | Loops a short span at that point, sized by the global quantisation | man 6.1 |
| I-CLICK-014 | Arrangement scrub area or beat-time ruler | Shift-click | Scrubs even when Permanent Scrub Areas is off | man 6.3 |
| I-CLICK-015 | Overview | Double-click inside the outline | Zooms out to the whole Arrangement | man 6.1 |
| I-CLICK-016 | Beat-time ruler | Double-click | Zooms to the selection, or out to the whole Arrangement if nothing is selected | man 6.1 |
| I-CLICK-017 | Loop brace | Click | Selects everything inside the loop | man 6.9 |
| I-CLICK-018 | Locator | Click; double-click while stopped | Jumps to it; double-click also selects it and starts playback | man 6.4 |
| I-CLICK-019 | Fold button beside a track name | Click; Option/Alt-click | Folds or unfolds the track; with the modifier, all tracks | man 6.9 |
| I-CLICK-020 | Pan control | Double-click, or click the triangle above it | Resets the pan to centre | man 18.1 |
| I-CLICK-021 | Any value control | Double-click | Resets it to its default (as Delete does) | man 42.4 |
| I-CLICK-022 | Solo or Arm button | Cmd/Ctrl-click | Adds the track without releasing the others (exclusive solo and arm otherwise) | man 18.1, 3.17 |
| I-CLICK-023 | Several selected tracks | Change one track's control | Changes the same control on every selected track, keeping their differences | man 18.2 |
| I-CLICK-024 | Rack device activator | Option/Alt-click | Turns every device in the rack on or off (S-DEV-003) | man 42.4 |
| I-CLICK-025 | Plug-in window button | Cmd/Ctrl-click | Opens the window without closing the other plug-in windows (S-DEV-010) | man 42.4 |
| I-CLICK-026 | Device View selector (Status Bar) | Right-click | Lists the track's devices to jump to (S-DEV-005) | man 42.4 |
| I-CLICK-027 | Clip and Device View toggles | Click; Option/Alt-click | Shows that view; with the modifier, stacks both (S-VIEW-005) | man 42.1, 3.12 |
| I-CLICK-028 | Session and Arrangement buttons (Control Bar, right end) | Click | Switches the main view | obs |
| I-CLICK-029 | Status Bar message | Click | Acts on the message (for example the engine-off message opens Settings at the Audio page) | obs, man 5.6 |
| I-CLICK-030 | Browser item | Double-click | Loads it onto the selected track, or creates a track if none is selected. An instrument double-clicked with an audio track selected creates a new MIDI track at the end of the track list ("5 Drift"), holding the instrument, and selects it | man 23.2; obs (Live 12.4.6, 12 October 2026) |
| I-CLICK-031 | Browser filter tags | Cmd/Ctrl-click | Selects several tags in one group | man 4.4.1 |
| I-CLICK-032 | Any item | Right-click (Ctrl-click on macOS) | Opens its context menu (S-CTX-001) | man 42.29 |
| I-CLICK-033 | Session clips, tracks, scenes | Shift-click; Cmd/Ctrl-click | Extends the selection to a range; adds or removes one item | man 7.1, 42.9 |
| I-CLICK-034 | Envelope breakpoint | Option/Alt-click | Selects the breakpoint and moves the insert marker to it | man 41.5.2 |
| I-CLICK-035 | Hot-Swap Presets button (device title bar) | Click | The button turns orange and the browser shows the hot-swap bar (L-BRW-015); closing the bar ends hot-swapping | obs (Live 12.4.6, 12 October 2026) |
| I-CLICK-036 | Scene number (Main track column) | Double-click | Opens the Scene View in the detail area (L-SES-011) | obs (Live 12, 15 October 2026) |
| I-CLICK-037 | Crossfader A or B assign button | Click | Assigns the track to that side and switches the other side off; the two are exclusive | obs (Live 12, 15 October 2026) |
| I-CLICK-038 | Piano ruler key | Click; Shift-click | Selects that key track's notes; adds them to the selection or removes them | man 10.5.2, 10.5.12.1 |
| I-CLICK-039 | MIDI note | Shift-click | Adds it to the selection or removes it | man 10.5.2 |
| I-CLICK-040 | Audition Take Lane button (take lane header) | Click | Auditions the lane, or stops auditioning it (S-COMP-003) | man 21.5 |
| I-CLICK-041 | Linked-track indicator | Click; hover | Selects the tracks of its link group; highlights them | man 6.14.1 |
| I-CLICK-042 | Take lane inside a time selection on its track's main lane, Draw Mode on | Click | Copies that take lane's material in the time selection to the main lane | man 21.6 |
| I-CLICK-043 | Show/Hide Take Lanes toggle | Click | Shows or hides the track's take lanes | man 21.1 |
| I-CLICK-044 | Automation segment | Click within a few points of it but not on it; Shift-click on it | Selects the segment: its two breakpoints | man 25.5.2 |
| I-CLICK-045 | Expression envelope segment of a selected note (MPE tab) | Click | Adds a breakpoint on it | man 12.2 |
| I-CLICK-046 | Expression envelope breakpoint (MPE tab) | Click | Deletes it | man 12.2 |
| I-CLICK-047 | Expression envelope segment (MPE tab) | Click near it; Shift-click on it | Selects the segment | man 12.2 |
| I-CLICK-048 | MPE tab's lane triangle, under the lanes' selector toggles | Click; Option/Alt-click | Hides the shown lanes or shows them again; with the modifier, or with every toggle off, shows all four | man 12.1; release notes 12.0 |
| I-CLICK-049 | Piano ruler key, while the Generate panel shows Rhythm, Seed, Shape or Stacks | Option-click (Alt-click); Option-drag (Alt-drag) | Sets Rhythm's Pitch or the selected Stacks chord's root, or a one-key range for Seed and Shape; dragging sets Seed's and Shape's range to the keys dragged over | man 11.3.1 to 11.3.4; release notes 12.1 |
| I-CLICK-050 | Browser | The mouse's back and forward buttons | Browse Back and Browse Forward (S-BRW-011, S-BRW-012) | release notes 12.4 |
| I-CLICK-051 | Quick Tags tag | Double-click | Searches for the tag | release notes 12.3 |
| I-CLICK-052 | Peak level readout (mixer strip), with the extended meters shown | Click | Resets it to the current peak (L-MIX-008) | man 18.1.1 |

## Drags (I-DRAG)

| ID | Where | Gesture | Result | Source |
|---|---|---|---|---|
| I-DRAG-001 | Session clips | Drag | Moves them to other slots | man 7.1 |
| I-DRAG-002 | Session clips | Option-drag (macOS) / Ctrl-drag (Windows) | Copies them (S-SES-004) | man 42.15 |
| I-DRAG-003 | Session grid | Drag from an empty slot | Rubber-band selects slots and clips | man 7.4 |
| I-DRAG-004 | Session track title bar edges | Drag; Option/Alt-drag | Resizes the track's width; with the modifier, every track's | man 7.2 |
| I-DRAG-005 | Main track title bar, left edge | Drag left | Reveals the scene tempo and signature fields | man 7.2.1 |
| I-DRAG-006 | Scenes, tracks | Drag | Reorders them; non-adjacent selections close up when dropped | man 7.2, 18.2 |
| I-DRAG-007 | Overview, beat-time ruler | Drag sideways; drag up or down | Scrolls; zooms | man 6.1 |
| I-DRAG-008 | Time ruler | Drag sideways | Scrolls | man 6.1 |
| I-DRAG-009 | Arrangement | Cmd+Option-drag (macOS) / Ctrl+Alt-drag (Windows) | Pans the display (S-ZOOM-007) | man 6.2 |
| I-DRAG-010 | Arrangement background | Drag | Selects a span of time; Shift-drag extends it | man 6.9 |
| I-DRAG-011 | Arrangement clip bar | Drag | Moves the clip in time or to another track; snaps to the grid, clip edges, locators and signature changes | man 6.7 |
| I-DRAG-012 | Arrangement clip edge | Drag | Changes the clip's length. Trimming a MIDI clip's left edge moves its Start marker by the amount trimmed, leaving the loop brace where it was; trimmed past the loop's end, the Start marker wraps inside the brace by the loop's length. With Loop off, dragging the right edge takes the End marker with it | man 6.7; obs (Live 12.4.6, 11 October 2026) |
| I-DRAG-013 | Arrangement clip contents | Shift+Option-drag (macOS) / Shift+Alt-drag (Windows) | Slides the contents inside the clip (S-ARR-005) | man 6.7, 42.16 |
| I-DRAG-014 | Arrangement clip contents | Cmd+Option+Shift-drag / Ctrl+Alt+Shift-drag | Slides the contents without snapping (S-ARR-036) | man 6.7 |
| I-DRAG-015 | Warped clip's title bar | Shift-drag | Stretches the clip (S-ARR-006) | man 42.16 |
| I-DRAG-016 | Clip's title bar | Shift+Option-drag / Shift+Alt-drag | Selects time inside the clip (S-ARR-007) | man 42.16 |
| I-DRAG-017 | Fade handles (shown on hover) | Drag a fade handle; drag the curve handle; drag past the neighbour's edge | Sets the fade length; shapes the curve; creates a crossfade | man 6.8 |
| I-DRAG-018 | Track's lower edge (split line) | Drag; Option/Alt-drag | Sets the track's height; with the modifier, every track's | man 6.9 |
| I-DRAG-019 | Loop brace | Drag an edge; drag the bar | Moves that end; moves the whole loop | man 6.6 |
| I-DRAG-020 | Locator, time-signature marker | Drag | Moves it | man 6.4, 6.5 |
| I-DRAG-021 | Any drag on a grid | Hold Cmd (macOS) / Alt (Windows) | Ignores snapping; if the grid is off, snaps instead (S-EDIT-013) | man 6.10 |
| I-DRAG-022 | Value controls | Drag up or down; Shift-drag | Changes the value; in finer steps (S-VAL-003) | man 42.6 |
| I-DRAG-023 | Automation segment | Option/Alt-drag | Curves the segment (S-ENV-003) | man 42.7 |
| I-DRAG-024 | MIDI notes | Drag; Option-drag (macOS) / Ctrl-drag (Windows) | Moves them; copies them (S-NOTE-002) | man 42.12 |
| I-DRAG-025 | MIDI notes | Cmd-drag (macOS) / Alt-drag (Windows) up or down | Changes velocity (S-NOTE-016) | man 42.12 |
| I-DRAG-026 | MIDI notes | Hold E and click or drag | Splits notes where clicked (S-NOTE-006) | man 10.5.8 |
| I-DRAG-027 | Browser divider, right edge, bottom edge | Drag | Splits the sidebar and content pane; widens the browser; extends it down (closing the Info View and detail view) | man 4 |
| I-DRAG-028 | Clip View's top edge | Drag; drag to the bottom | Resizes the detail area; closes it. Beatlayer's limits: [detail views](../specs/detail-views.md) R10 | man 8 |
| I-DRAG-029 | Clip panels' right edge | Drag left; drag right | Stacks the panels vertically; back to side by side. Beatlayer switches past half the right group's panel width ([detail views](../specs/detail-views.md) R21) | man 8.1.2 |
| I-DRAG-030 | Mixer's top edge, or the strip section's top border | Drag up; drag down | Makes the mixer taller and adds the extended meters; shorter too, to 64 pt ([mixer view](../specs/mixer-view.md) R32) | man 18.1.1; release notes 12.0.5, 12.3 |
| I-DRAG-031 | Device title bar | Drag | Reorders the device, or moves it to another track | man 23.2 |
| I-DRAG-032 | Volume fader (mixer strip) | Drag; click away from the handle | Relative: a drag moves the handle one point per point of the pointer's movement along the meter's scale, and a click away from the handle leaves the value as it was. While it moves the Status Bar shows `Track Volume: -21.0 dB` | obs (Live 12.4.6, 11 October 2026) |
| I-DRAG-033 | Pan knob (mixer strip) | Drag up or down; Shift-drag | Moves pan linearly, 0.8 steps per point of vertical drag (62.5 points from C to 50R or 50L), up to the right; with Shift about three times finer | obs (Live 12.4.6, 11 October 2026) |
| I-DRAG-034 | Device knob | Drag | Relative, 0.8 % of the range per point (about 125 points for the whole range), the same rate as the pan knob | obs (Live 12.4.6, 12 October 2026) |
| I-DRAG-035 | Crossfader | Drag | The Status Bar shows `Crossfade: <n>A` or `Crossfade: <n>B`, from 50A at the left end to 50B at the right end, the centre between them | obs (Live 12, 15 October 2026) |
| I-DRAG-036 | Expression-lane marker | Drag; Cmd-drag (Ctrl-drag on Windows) in the Velocity lane; double-click in the Velocity lane | Changes its note's value, or every selected note's by the same amount; sets the velocity deviation; sets the deviation to 0 | man 10.5.12, 10.5.13 |
| I-DRAG-037 | Expression lane, Draw Mode on | Drag; Option-drag (Alt-drag); with Shift too | Draws the values over the grid steps passed; a straight line; a level line | man 10.5.12.1 |
| I-DRAG-038 | Split above or between the expression lanes | Drag; Option-wheel (Alt-wheel) or pinch | Resizes the lanes, hiding one dragged to nothing | man 10.1 |
| I-DRAG-039 | Selected notes | E held with Option (Ctrl on Windows), or Cmd and E, and drag up or down; with Shift too | Chops them into more or fewer equal parts; by powers of two (S-NOTE-004, S-NOTE-005) | man 10.5.8.2, 42.12; release notes 12.0 |
| I-DRAG-040 | Note stretch marker (L-CLIP-020) | Drag sideways; Cmd-drag (Alt-drag); drag the pseudo marker | Scales the notes about the other marker; without snapping; stretches the notes on either side | man 10.5.6 |
| I-DRAG-041 | Multi-clip loop bar (L-CLIP-021) | Drag a marker; Cmd-click (Ctrl-click); Shift-click; drag just above the title bar | Changes its clip's loop; adds the bar to the selection or removes it; selects the bars between; makes the bars taller or shorter | man 10.8 |
| I-DRAG-042 | Note area, with the note filters shown | Shift-drag sideways | Sets the Time filter's range | man 10.5.3 |
| I-DRAG-043 | Take lane, Draw Mode on | Drag, then release | Selects the stretch dragged over and copies it to the main lane on release | man 21.6 |
| I-DRAG-044 | Source highlight's edge, where two comp parts meet | Drag | Moves the comp boundary | man 21.7 |
| I-DRAG-045 | Take lane header | Drag up or down | Moves the selected take lanes within their track | man 21.2 |
| I-DRAG-046 | Take lane's lower edge | Drag | Sets the selected take lanes' height | man 21.2 |
| I-DRAG-047 | Automation lane background | Drag; Shift-drag | Selects time on the lanes dragged across; extends the selection | man 25.5.2 |
| I-DRAG-048 | Selected automation breakpoints | Drag any one; with Shift | Moves them all, a breakpoint passed over being removed; keeps to one axis | man 25.5.2 |
| I-DRAG-049 | Time selection's handles in an automation lane, shown on hover | Drag the top or bottom centre; the left or right centre; a corner; with Option (Alt) | Stretches the values; stretches the time (Shift takes the breakpoints beyond with it, Cmd/Alt bypasses the grid); skews; moves the opposite handle the opposite way | man 25.5.3 |
| I-DRAG-050 | Expression envelope breakpoint, or a selected segment (MPE tab) | Drag | Moves it and every selected breakpoint in time and value, a neighbour passed in time being removed; a segment moves in value | man 12.2 |
| I-DRAG-051 | Expression envelope's background (MPE tab) | Drag | Selects the breakpoints it encloses | man 12.2 |
| I-DRAG-052 | Expression envelope segment (MPE tab) | Option-drag (Alt-drag); Option-double-click (Alt-double-click) | Curves it; straightens it | man 12.2 |
| I-DRAG-053 | Pitch envelope breakpoint (MPE tab) | Cmd-drag (Alt-drag on Windows) | With the grid off, snaps the value to the nearest semitone; with it on, bypasses the grid | man 12.2, 12.3 |
| I-DRAG-054 | Outside a note, over its highlighted Slide, Pressure, Velocity or Release Velocity (MPE tab) | Drag up or down | Scales that value over the note's whole length, for every selected note | man 12.2 |
| I-DRAG-055 | Split line under an MPE lane | Drag | Resizes the lane | man 12.1 |
| I-DRAG-056 | Tag Editor tag or group, Library or saved label, nested collection's label | Drag | Moves it within its list (nested collections are Beatlayer's own, IMP-07) | release notes 12.0.5, 12.1 |
| I-DRAG-057 | Column name in the browser's content pane | Drag; click | Moves the column; sorts by it | man 4.1 |
| I-DRAG-058 | Any area that scrolls | Drag with the middle mouse button (the wheel) held | Pans it, selecting and moving nothing | release notes 12.0 |

## Scrolling and zooming (I-SCROLL)

| ID | Where | Gesture | Result | Source |
|---|---|---|---|---|
| I-SCROLL-001 | Arrangement, editors | Scroll | Scrolls vertically | man 6.2 |
| I-SCROLL-002 | Arrangement, editors | Shift-scroll | Scrolls sideways (S-ZOOM-006) | man 42.9 |
| I-SCROLL-003 | Arrangement | Cmd-scroll (macOS) / Ctrl-scroll (Windows) | Zooms around the selection | man 6.2 |
| I-SCROLL-004 | Arrangement track lane | Option/Alt-scroll; Option/Alt-pinch | Changes the track's height (every selected track's if time is selected) | man 6.2, 6.9 |
| I-SCROLL-005 | Session track columns | Scroll sideways | With more tracks than fit, the Session View scrolls sideways continuously, so a column can be partly in view; the returns and Main stay pinned at the right | obs (Live 12.4.6, 11 October 2026) |
| I-SCROLL-006 | Take lanes | Option-scroll (Alt-scroll) | Resizes the selected take lanes | man 21.2 |
| I-SCROLL-007 | Knob, slider, fader, number box, with Adjust Values with Mouse Wheel on | Scroll; Shift-scroll | Beatlayer addition (IMP-36): moves it a step a notch, as Up and Down do; in finer steps ([view options](../specs/view-options.md) R11) | Beatlayer |

## Drop targets (I-DROP)

What happens when something is dropped, by target.

| ID | Target | Dropped item | Result | Source |
|---|---|---|---|---|
| I-DROP-001 | Session slot | Audio file, MIDI file, clip | Places a clip in the slot | man 7.4 |
| I-DROP-002 | Session drop area, right of the tracks | File, device, preset, clips from another track | Creates a track of the right type holding it (clips bring their track's devices) | man 18.2 |
| I-DROP-003 | Session | Several files | Lays them down one track; hold Cmd/Ctrl while dropping to spread them across tracks | man 7.4 |
| I-DROP-004 | Session | Clips from the browser with Cmd/Ctrl held | Drops them as a scene (S-SES-017) | man 42.15 |
| I-DROP-005 | Arrangement lane | File, clip | Places a clip at the drop time | man 6.7 |
| I-DROP-006 | Arrangement drop area under the tracks | Instrument or MIDI effect; audio effect; file | Creates a MIDI track; an audio track; a track holding the file | man 6.1, 18.2 |
| I-DROP-007 | Track (title bar or lane) | Device or preset | Adds the device to the track's chain | man 3.11 |
| I-DROP-008 | Device View | Device; sample onto a MIDI track | Inserts it at the drop point; creates a sampler instrument with the sample | man 23.2 |
| I-DROP-009 | Clip View | Sample | Replaces the clip's sample | man 8.12 |
| I-DROP-010 | Groove Pool | Clip; groove file | Extracts a groove; loads the groove | man 14.1, 14.2.1 |
| I-DROP-011 | Browser, User Library or a Places folder | Session clip; Arrangement clip; several Session clips; track title bars | Saves a clip file; a clip file; a new set; a new set | man 3.21, 5.3, 5.4.4, 5.7.1, 18.2 |
| I-DROP-012 | File Manager, a missing file's row or any Manage Set row | File from the browser | Relinks or replaces the file | man 5.4.6, 5.6.1 |
| I-DROP-013 | Browser Tuning section | Tuning file (.scl, .ascl) | Loads the tuning into the set | man 15.1 |
| I-DROP-014 | Session or Arrangement view selector | Clips dragged over it | Switches views so the clips can be dropped in the other view | man 7.5 |
| I-DROP-015 | The other window (with the second window open) | Clips | Moves or copies them between Session and Arrangement. Beatlayer copies; the source stays ([second window](../specs/second-window.md) R20, R21) | man 7.5 |
| I-DROP-016 | Take lane | Samples | Inserts them as takes | man 21.4 |
| I-DROP-017 | MIDI track | Groove from the browser or Groove Pool | Creates a MIDI clip of the groove's notes for editing | man 14.2 |
| I-DROP-018 | Arrangement lane of a MIDI track | Standard MIDI File | Places the file's clip on that track at the drop position, and the track keeps its own name. If the file holds tempo or time-signature events, Live first asks "Import tempo and time signature data into Arrangement?" with No, Yes, and a "Don't show again" check box, for every such file wherever it is dropped | obs (Live 12, 15 October 2026) |
| I-DROP-019 | Arrangement drop area under the tracks | Type 1 Standard MIDI File | Makes one new MIDI track for each track chunk that has notes, all clips at the same drop position; a chunk with no notes (a conductor track of tempo and time signature, or a chunk with only a name) makes none. The new tracks get Live's default names ("5 MIDI", "6 MIDI"); only the clips carry the chunks' names | obs (Live 12, 15 October 2026) |
| I-DROP-020 | Take lane | Several samples or MIDI files, Cmd (Ctrl) held while dragging | Puts each on a track of its own, from the one dropped on down, on the take lane at the same place in each track | man 21.4 |
| I-DROP-021 | Track title bar, or either view's drop area | A set file from the browser | Adds the set's tracks except its return tracks and Main | man 5.4.3 |
| I-DROP-022 | Track, or a drop area | A track, a group or a Devices row from a set opened in the browser | Adds it; a Devices row's chain comes without its automation | man 5.4.3; release notes 12.0 |
| I-DROP-023 | Main window | A pack | Asks where to unpack it, then unpacks it | man 5.9 |
| I-DROP-024 | Nested collection's label | Browser items | Beatlayer addition (IMP-07): puts them in it ([browser tags](../specs/browser-tags.md) R46) | Beatlayer |
| I-DROP-025 | Main window | .dawproject file | Beatlayer addition: imports it as a new set, after asking whether to save changes ([dawproject](../specs/dawproject.md) R85) | Beatlayer |

## Keyboard focus (I-FOCUS)

| ID | Behaviour | Source |
|---|---|---|
| I-FOCUS-001 | Clicking in an area gives it keyboard focus; shortcuts then act in that area's context (see the contexts in shortcuts.md). | obs, man 41.4 |
| I-FOCUS-002 | Option/Alt with a digit moves focus to an area: 0 Control Bar, 1 Session, 2 Arrangement, 3 Clip View editor, 4 Device View, 5 browser, 6 Groove Pool, 7 Learn View, 8 selected clip panel (S-FOCUS-001 to 009). The Navigate menu lists the same targets with these keys, except the selected clip panel. While a value field was being edited, Option+2 typed 2 into the field instead of moving focus. | obs, man 42.2, 41.4.1 |
| I-FOCUS-003 | The Control Bar remembers its last-focused control and returns focus there. | man 41.4.1.1 |
| I-FOCUS-004 | With Use Tab Key to Move Focus on (off in a new install), Tab and Shift+Tab move between controls within the focused area, and Option+Tab / Ctrl+Tab moves to the same control on the next track; Wrap Tab Navigation (on in a new install; the Navigate menu calls the same setting Wrap Keyboard Navigation) makes Tab wrap at the area's ends, and off, Tab stays on the last control. In a Session strip Tab stays within the track. For an audio track the order is Track Title Bar, Clip Slot (the selected slot), Clip Stop, Input Type, Input Channel, Monitoring, Output Type, Output Channel, Send (A), Send (B), Peak Level, Track Pan, Track Activator, Solo/Cue, Arm Recording, Track Volume; for a MIDI track without an instrument it is Track Title Bar, Clip Slot, Clip Stop, Input Type, Input Channel, Monitoring, Output Type, Output Channel, Track Activator, Solo/Cue, Arm Recording. With Wrap on, Tab from the last control goes round to the Track Title Bar. In the Arrangement Tab runs once through the view: Set/Delete Locator, Previous Locator, Next Locator, Automation Mode, Lock Envelopes, then the selected track's header (Track Title Bar, Input Type, Input Channel, Monitoring, Output Type, Output Channel, Track Activator, Solo/Cue, Arm Recording, Track Volume, Track Pan, Send (A), Send (B)), then Waveform Vertical Zoom, Waveform Vertical Zoom Level, Optimize Arrangement Height, Optimize Arrangement Width, Loop/Punch-Recording Region (two stops, the brace's start and end), and Clip/Device Drop Area, and with Wrap on round to Set/Delete Locator. With it off, Tab switches between Session and Arrangement. | obs (12.4.6, 11 October 2026), man 42.26.1, 41.1.1 |
| I-FOCUS-005 | Session: arrow keys move between title bars, slots and scenes; Tab moves from the grid to the mixer controls; Esc returns from a track's control to its title bar. | man 41.4.1.2 |
| I-FOCUS-006 | Arrangement: Up and Down move between tracks; Option+Tab / Ctrl+Tab moves between tracks' mixer controls; Esc returns to the track's title bar. | man 41.4.1.3 |
| I-FOCUS-007 | Browser: from the search field, Tab goes to the filter toggle, the Filter View menu, then the sidebar; from the sidebar, Tab goes to the filters (if shown) or the content pane; Down from the search field goes to the content pane; Left and Right move between sidebar and content pane. | man 41.4.1.6 |
| I-FOCUS-008 | Settings window: Tab and Shift+Tab move between options whatever the Tab setting; Option+Tab / Ctrl+Tab changes page; Shift+Tab on a page's first option returns to the page chooser; Esc closes the window. | man 42.26.2 |
| I-FOCUS-009 | Outline View in Focus (Display & Input settings), on in a new install, draws a light border round the focused area and outlines the focused control itself (a white outline round a chooser, corner brackets round a clip stop button). | obs (12.4.6), man 2.3.1 |
| I-FOCUS-010 | With two windows open, the Clip or Device View opens in the focused window. | man 8 |
| I-FOCUS-011 | Renaming a track or scene: Tab moves to the next one, so a series can be renamed without the mouse. | man 18.2, 7.2 |
| I-FOCUS-012 | The Info View follows the pointer: it describes whatever is under the mouse. It also names the control that has the keyboard focus, which shows when the pointer is off the window. | obs, man 2.2.2; focus obs (Live 12.4.6, 11 October 2026) |
| I-FOCUS-013 | Navigate ▸ Mixer in the Arrangement puts the keyboard focus in the mixer at the selected track; Right then moves the selection to the next track. | obs (Live 12.4.6, 11 October 2026) |
| I-FOCUS-014 | Session, Tab with track 1 selected goes first to that track's Clip Stop. When the focus reaches a track's Clip Slot, that slot becomes the selected one. Esc from the last control (Track Volume) moves the focus to the track's Track Title Bar, and a second Esc moves the highlight to the selected clip slot. | obs (Live 12.4.6, 11 October 2026) |
| I-FOCUS-015 | Navigate ▸ Next Neighbor moves the focus to the same control on the next track (Output Type on track 1 to Output Type on track 2, then track 3). Each is disabled where there is no such neighbour (on track 1, Previous is disabled and Next enabled), whatever the Tab setting. Moving the focus this way doesn't change the selected track. | obs (Live 12.4.6, 11 October 2026) |

## Momentary (held) keys (I-HOLD)

| ID | Behaviour | Source |
|---|---|---|
| I-HOLD-001 | A, B, S, Z, F1 to F8 and Tab toggle when tapped. Held for about half a second, they act only while held and the previous state returns on release (S-ENV-001, S-GRID-001, S-TRACK-014, S-ARR-030, S-TRANS-010, S-VIEW-003). | man 42.25 |
| I-HOLD-002 | F shows the fade handles only while held over an automation lane (S-ENV-004). | man 42.7, 6.8 |
| I-HOLD-003 | N toggles Focus Mode when tapped and acts momentarily while held (S-NOTE-040). | man 10.8.1 |
| I-HOLD-004 | Option/Alt held over a Drum Rack shows the similar-sample swap controls (S-SWAP-005). | man 42.23 |
