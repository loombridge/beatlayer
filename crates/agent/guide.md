# Beatlayer agent guide

You're connected to Beatlayer, a digital audio workstation with the workflow of Ableton Live 12, through its agent API. This guide explains Beatlayer's concepts, its IDs, and how to work well with the tools. The agent API serves it as the resource `beatlayer://guide`.

## What you can do

Every action a person can take in Beatlayer is a command in a registry, with an ID such as `track.create`, a description, a permission class, and a JSON Schema for its arguments. A few tools serve the whole registry:

| Tool | Use it to |
|---|---|
| `list_commands` | See the areas (`track`, `clip`, `mixer`, `device`, `song`, `transport`, `set`, and more), then list an area's commands or search them by words. |
| `describe_command` | Read a command's full description and argument schema before you run it for the first time. |
| `run` | Run one command, or a batch of document edits as one undo step. Set `dry_run` to check without changing anything. |
| `query` | Read the project: an overview, a track, clip, or device by ID or by a readable path. |
| `transport` | Play, stop, continue, and locate the song, move and switch the loop, and check whether the song plays here and where the playhead is. |
| `schedule` | Run a command when the song reaches a bar and beat, on the exact sample. |
| `checkpoint` | Save the song under a name, try something, and restore it if you don't like the result. |

## The song

A set (a `.blset` file) holds one song. The song has:

- **Tracks**, in order: audio tracks (hold audio clips), MIDI tracks (hold MIDI clips), and group tracks (hold other tracks and sum them). Then the **return tracks**, which receive sends, and the **Main** track, the song's output, which always exists.
- **Clips** on audio and MIDI tracks' arrangement lanes, each with a start, a length, a name, and its contents: an audio file named by content hash, or MIDI notes.
- **Devices** in each track's device chain, in signal order, each with parameters.
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

## The transport

`transport` works the Arrangement's transport, as the Control Bar does. `play` starts from the **insert marker**; `locate` moves the insert marker (with `at`, such as `"5.1.1"`), and while the song plays, playback jumps there. `stop` stops and keeps the position, so `continue` plays on from it; a second `stop` returns to 1.1.1. `status` says whether the song is playing, where the playhead and the insert marker are, and the loop. Playback changes nothing in the set, so it's never an undo step.

The **loop** belongs to the set: `set_loop` moves its brace (`start` and `length` in ticks) and `set_loop_enabled` switches it on or off, each an undo step. While it's on, playback that reaches the brace's end jumps back to its start.

The engine acts on a transport command at the start of its next block, a few milliseconds after your call. Headless (`bl-cli mcp`), no engine plays the song, so play, stop, continue, and locate are refused, and `status` says so; the loop can still be set.

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
| destructive | Removes content (deleting tracks, clips, devices), or loses work undo can't restore (discarding changes, overwriting files, quitting) | The person is asked first, and your call waits for the answer |
| external | Reaches outside the project: files elsewhere, devices, the network, other applications | The person is asked first, and your call waits for the answer |

The person can turn on autonomous mode for you in Beatlayer's Settings window, and then destructive and external commands run without asking. If the person declines, nothing changes: don't retry without asking them. Headless (`bl-cli mcp`), there's no one to ask, so these commands are refused unless it was started with `--autonomous`.

The agent server's own settings (`agent.*`: switching it on, allowing agents, autonomous mode, the token) change what agents may do, so only a person can change them.

If a call that waited for the person times out on your side, `query` the project before you try again: the person may have allowed it after all.

## Files

- `set.save` saves the set to its file, and `set.save_as` to a new one (`path`, a `.blset` file). Unless that folder is already a project folder, the set goes into a new `<name> Project` folder inside it, and the result's `path` says where. Saving over a file keeps the old one in the `Backup` folder beside it.
- `set.open` opens a set. It refuses while the open set has unsaved changes: save them, or throw them away with `set.discard_changes`.

## What's in this build

Beatlayer is being built in milestones. In this build, audio clips are placed in the arrangement but don't play yet, the transport plays only where an engine is attached, and parameters are named by number. `list_commands` always shows what this build can do.
