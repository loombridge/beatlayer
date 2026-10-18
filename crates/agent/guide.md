# Beatlayer agent guide

You're connected to Beatlayer, a digital audio workstation with the workflow of Ableton Live 12, through its agent API. This guide explains Beatlayer's concepts, its IDs, and how to work well with the tools. The agent API serves it as the resource `beatlayer://guide`.

## What you can do

Every action a person can take in Beatlayer is a command in a registry, with an ID such as `track.create`, a description, a permission class, and a JSON Schema for its arguments. A few tools serve the whole registry:

| Tool | Use it to |
|---|---|
| `list_commands` | See the areas (`track`, `clip`, `mixer`, `device`, `song`, `transport`, `set`, and more), then list an area's commands or search them by words. |
| `describe_command` | Read a command's full description and argument schema before you run it for the first time. |
| `run` | Run one command, or a batch of document edits as one undo step. Set `dry_run` to check without changing anything. |
| `query` | Read the project: an overview, a track, clip, device, scene, Session slot, or automation envelope by ID or by a readable path. |
| `transport` | Play, stop, continue, and locate the song, move and switch the loop, and check whether the song plays here and where the playhead is. |
| `schedule` | Run a command when the song reaches a bar and beat, on the exact sample. |
| `checkpoint` | Save the song under a name, try something, and restore it if you don't like the result. |
| `preview` | Hear the song: render a range of it, the whole mix or one track, as a short WAV file. |
| `analyze` | Measure what you'd hear as numbers: loudness, peaks, an octave-band spectrum, tempo, key, and transients. |
| `capture_view` | See what the person sees: one of the app's views as a PNG image, while the app runs. |

## The song

A set (a `.blset` file) holds one song. The song has:

- **Tracks**, in order: audio tracks (hold audio clips), MIDI tracks (hold MIDI clips), and group tracks (hold other tracks and sum them). Then the **return tracks**, which receive sends, and the **Main** track, the song's output, which always exists.
- **Clips** on audio and MIDI tracks' arrangement lanes, each with a start, a length, a name, and its contents: an audio file named by content hash, or MIDI notes.
- **Devices** in each track's device chain, in signal order, each with parameters. A plug-in device may bring commands of its own (a plug-in built on Loombridge's plug-in kit does): they're device commands scoped to it, `device.d<device>.<command>`, such as `device.d17.gain.reset`. `list_commands` in the `device` area lists them and `query` on the device names them; run and schedule them like any other command. Each run is one undo step with your name, and its class decides whether the person is asked first.
- A **mixer** on each track: volume (linear gain, 1 is 0 dB), pan (−1 to 1), mute (the track's activator), solo, and arm.
- A **tempo map** and a **time-signature map**.

Beatlayer uses Live's words: track, clip, scene, device, rack, chain, parameter.

## IDs

Every track, clip, and device has an ID: a positive integer, unique in the project, never reused. Commands name objects by ID. `query` shows them.

A command that creates an object needs a new ID, which only the project can issue. In `run`, write `{"$new": "name"}` where the new ID goes, and `{"$ref": "name"}` wherever a later command refers to it, in the same call or a later one. The result maps each name to its ID:

```json
{
  "label": "Add a bass track with a clip",
  "commands": [
    { "command": "track.create",
      "args": { "id": {"$new": "bass"}, "kind": "midi", "name": "Bass", "index": 0 } },
    { "command": "clip.create",
      "args": { "track": {"$ref": "bass"},
                "clip": { "id": {"$new": "bass-clip"}, "name": "Riff", "start": 0,
                          "length": 2822400, "mute": false,
                          "data": { "midi": { "notes": [
                            { "key": 36, "velocity": 100, "start": 0, "length": 705600 } ] } } } } }
  ]
}
```

## Time

Positions and lengths in commands are **ticks**: 705,600 to the quarter note, so a 4/4 bar is 2,822,400 ticks and a sixteenth is 176,400. `query` shows positions both as ticks and as bar.beat.sixteenth, the way Live shows them: the song starts at `1.1.1`, and there's no bar 0 (the bar before bar 1 is bar −1). `schedule` takes bar.beat.sixteenth positions such as `9.1.1`.

Tempos are quarter notes per minute. A new set plays at 120 BPM in 4/4.

## Reading a TOON table

If a result's text looks like `notes[256]{key,start,length,velocity}:` followed by indented lines of values, it's a table in TOON. The header names the field and says how many rows follow; each line below is one row, its values in the header's order, separated by commas. A value that holds a comma, a colon or a quote is in double quotes. A field written `mixer.volume` is the `volume` of the `mixer` object in each row. The rest of the text is `key: value` lines, indented to show what holds what. The same result is always available as JSON in `structuredContent`.

## The transport

`transport` works the Arrangement's transport, as the Control Bar does. `play` starts from the **insert marker**; `locate` moves the insert marker (with `at`, such as `"5.1.1"`), and while the song plays, playback jumps there. `stop` stops and keeps the position, so `continue` plays on from it; a second `stop` returns to 1.1.1. `status` says whether the song is playing, where the playhead and the insert marker are, and the loop. Playback changes nothing in the set, so it's never an undo step.

The **loop** belongs to the set: `set_loop` moves its brace (`start` and `length` in ticks) and `set_loop_enabled` switches it on or off, each an undo step. While it's on, playback that reaches the brace's end jumps back to its start.

The engine acts on a transport command at the start of its next block, a few milliseconds after your call. In the app, the song plays through the app's audio engine while its audio device runs, and your transport commands share one queue with the person's, in the order they come. Headless (`bl-cli mcp`), or while the app's audio engine is off, nothing plays the song, so play, stop, continue, and locate are refused, and `status` says so; the loop can still be set, and scheduled commands wait.

## Reading back what the set doesn't save

`query` reads what the set saves. Everything else you can set is read from a command that changes nothing, which you run with `run`:

- `transport.state`: Arrangement Record and whether a take is running or the count-in is sounding, the running recording's armed tracks and dropped frames, what the last recording came to (its files, dropped frames, and any error), Punch-In and Punch-Out, MIDI Arrangement Overdub, Automation Arm, the metronome, the loop, the tempo, and the position;
- `settings.state`: every preference the Settings window shows with a value, by page. Tokens and API keys are never returned, only whether one is stored;
- `view.state`: the main view, the browser, the Info View, the Clip View, and the other view toggles; `audio.status` and `midi.ports` for the devices; and `arrangement.state`, `session.state`, `mixer_view.state` and `device_view.state` for each view.

## Hearing and seeing the song

You can't hear the speakers or see the screen, so three tools perceive for you. They only read: they change nothing in the set and never ask the person.

- **`preview`** renders a range offline and returns it as audio: an `audio` content block, or, for clients of protocol 2024-11-05, an embedded resource. The range is `start` and `length` if you give them, otherwise the Arrangement's time selection, otherwise the whole set, and a preview stops after **30 seconds**. `track` previews one track as it sounds soloed; leave it out for the whole mix. The render doesn't stop or disturb playback. 16-bit stereo at 48 kHz is about 190 KB a second, so pass a lower `sample_rate` (such as 22050) when you only need a rough listen. The result's `id` names the preview, and its `uri` (`beatlayer://preview/<id>.wav`) reads it again as a resource; the last 4 are kept.
- **`analyze`** measures a preview (`preview`: its ID) or a range it renders itself (up to ten minutes), and returns numbers with units and a short `summary`:
  - loudness as EBU R 128 reports it: integrated, momentary and short-term maximum in LUFS, loudness range in LU, and true peak in dBTP;
  - sample peak, RMS, and crest factor in dB (a full-scale sine's RMS is −3.01 dBFS);
  - the level in each octave band from 31.5 Hz to 16 kHz;
  - the tempo in BPM, with a confidence from 0 to 1: an estimate from the audio, not the set's tempo, and one far from 120 BPM may read as half or double;
  - the key, with how well it fits and the runner-up, often the relative major or minor;
  - when transients start, in seconds and as bar.beat.sixteenth.

  `measure` picks what to measure, such as `["loudness"]`; it's faster.
- **`capture_view`** returns a view as a PNG image: `window`, `arrangement`, `session`, `mixer`, `browser`, `detail` (the Clip or Device View), or `settings`. The view must be on screen; if it isn't, the error names the command that shows it, which you can run, or capture `window` instead. Images are scaled to fit `max_size` pixels (1568 unless you say). Headless (`bl-cli mcp`) there's no window, and the tool says so.

Listen before and after a change, and compare the numbers: for example, `preview` the chorus, turn the bass down, and `analyze` both previews.

## Reading back what you set

Anything you can set, you can read back. `query` with `detail: "full"` returns, under `saved`, everything the set file saves for a track, clip, device, or scene, in the file's own form (the song's settings are under `song`'s `saved`). That covers a track's mixer, crossfader assignment, routing and channels, Monitor, sends, automation and Session slots; a clip's markers, loop, gain, fades and envelopes; a device's parameters, sidechain, MIDI sidechain and state; and the song's tempo map, time signatures, loop, punch, metronome and record switches, and launch quantisation. The default `summary` stays compact. A list comes a page at a time, and a list inside a list shows only its length: read it at the item's own path.

Paths reach the rest: `scene 1` or `scene "Verse"` (scenes count from 1, as the Session View numbers them), `track "Bass" / slot 3` (a track's Session slot, with its clip), `track "Bass" / lane 1` (a take lane and its passes), and `track "Bass" / automation` (the track's envelopes; `automation "volume"` or `automation "device 41 param 3"` reads one, its points a page at a time with `detail: "full"`).

State the set doesn't save, such as whether the song is recording or what a window shows, is read with the read commands (`transport.state`, `mixer_view.state`, and the other `.state` commands).

## Good practice

- **Look before you edit.** Start with `query` for an overview, and `describe_command` for any command you haven't used.
- **Batch related edits.** A `run` with several document edits is one undo step, labelled with your name and your `label`, so the person can undo your whole change at once. It runs all or nothing.
- **Dry-run what you're unsure of.** `dry_run: true` applies the batch to the current song, reports what would change or the first error, and undoes it.
- **Checkpoint before experiments.** `checkpoint` `save`, try a variation, and `restore` if it doesn't work. A restore is one undo step too.
- **Schedule, don't time.** Network calls can't land on a beat. To change something at a musical moment, `schedule` the command at its bar and beat; volume, pan, mute, and device parameter changes then happen on the exact sample (solo happens as the song gets there), and `transport.stop` and `transport.locate` stop or jump the song there.
- **Refer to things by ID.** Names can repeat; IDs can't. Paths with names are for reading.

## Permissions

Each command has a permission class:

| Class | Meaning | What happens |
|---|---|---|
| read | Changes nothing | Runs |
| edit | An undoable change to the project, or a preference easily changed back | Runs |
| destructive | Removes content (deleting tracks, clips, devices, or placing clips over others), or loses work undo can't restore (discarding changes, overwriting files, quitting) | The person is asked first, and your call waits for the answer |
| external | Reaches outside the project: files elsewhere, devices, the network, other applications | The person is asked first, and your call waits for the answer |

Some commands' class depends on their arguments. In the Arrangement a clip placed over part of another replaces the part it covers, as in Live, so moving, resizing, nudging, duplicating, or pasting clips, adding files from the browser, or importing a MIDI file, onto time where other clips are is destructive as well as its own class, and asks first; the same command into empty space doesn't. The person's question names the clips it would replace or shorten. Likewise `clip.set_data` that sends fewer notes than a MIDI clip has deletes notes, so it asks first; one that adds notes or keeps their count doesn't. `describe_command` marks these commands (`raised_by_arguments`), and a `dry_run` says which class a call has (`permission`), whether it would ask, which clips it would replace (`replaces`), and, for `clip.set_data`, how many notes the clip has and would keep (`fewer_notes`). A call's class is decided when it runs, against the song as it is then.

The person can turn on autonomous mode for you in Beatlayer's Settings window, and then destructive and external commands run without asking. If the person declines, nothing changes: don't retry without asking them. Headless (`bl-cli mcp`), there's no one to ask, so these commands are refused unless it was started with `--autonomous`.

Recording captures input devices, a microphone included, so `transport.set_recording` is external and asks first, whichever value it sets. `transport.stop_recording` stops a recording and never asks.

**Your rights go with your token.** When the person first allows you, Beatlayer makes you a token of your own, which the person can copy from your row in the Settings window. Your client sends it as its bearer token, and then you're never asked about again and you're autonomous if the person switched that on, whatever name your client gives. A client that sends the agent server's token instead is asked about each time it connects, and is never autonomous, even with an allowed agent's name.

**The person comes first.** While the person is dragging something in Beatlayer (clips, or a value such as a fader), your calls wait until they let go, then run in the order they came, each its own undo step. So does a scheduled command that fires mid-drag: the sound changes on its sample, and the set follows after the drop.

The agent server's own settings (`agent.*`: switching it on, allowing agents, autonomous mode, the token) change what agents may do, so only a person can change them.

If a call that waited for the person times out on your side, `query` the project before you try again: the person may have allowed it after all.

## Files

- `set.save` saves the set to its file, and `set.save_as` to a new one (`path`, a `.blset` file). Unless that folder is already a project folder, the set goes into a new `<name> Project` folder inside it, and the result's `path` says where. Saving over a file keeps the old one in the `Backup` folder beside it.
- `set.open` opens a set. It refuses while the open set has unsaved changes: save them, or throw them away with `set.discard_changes`.

## What's in this build

Beatlayer is being built in milestones. In this build, audio clips play from the Arrangement, the transport plays only where an engine is attached (previews render without one), and parameters are named by number. `list_commands` always shows what this build can do.
