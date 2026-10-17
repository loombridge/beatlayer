# Beatlayer alpha release notes

Beatlayer is an early alpha of an open-source digital audio workstation. It
follows Ableton Live 12's workflow and runs natively on macOS (Apple Silicon),
Windows, and Linux.

Beatlayer is far from finished: large parts of Live's workflow are still
missing, the parts that are here are rough, and the set format can change
between builds. Treat it as a look at where the project is going, not as
something to rely on yet, and keep your own copy of anything you care about.

## What the alpha does

- Arrange audio and MIDI clips on tracks in the Arrangement View, and edit
  notes in the piano roll.
- Make sound with Beatlayer's own instruments and effects.
- Host CLAP and VST3 plug-ins on every platform, and Audio Unit plug-ins on
  macOS.
- Record audio and MIDI.
- Automate parameters, and route audio through sends, returns and sidechains.
- Export a set to WAV or MP3.
- Drive the whole app from outside through the agent API, or from the built-in
  assistant, which uses the same commands you do.

## Not there yet

- Session View is only partly there.
- There is no warping.
- There are no racks.
- The set format may change between builds, so a set saved by one build may
  not open in the next.

## The builds are unsigned

Every download here is unsigned. The project does not sign or notarise its
builds with an Apple Developer ID or a Windows code-signing certificate, so
macOS and Windows treat them as coming from an unknown developer and warn you
before they run.

The macOS build is only ad-hoc signed. That is the minimum Apple silicon needs
to run it at all, and it is not the same as a Developer ID signature, so
Gatekeeper still warns you the first time you open it.

The warning means the system cannot check who made the app. It does not mean
the app has done anything. The steps below are how you open each build past
that warning.

### macOS

1. Open the downloaded disk image and drag Beatlayer to your Applications
   folder.
2. Try to open Beatlayer from Applications by double-clicking it. macOS 15 and
   later does not open an app from an unidentified developer, and says so.
   Close that message.
3. Open **System Settings > Privacy & Security** and scroll down to the
   **Security** section. Beatlayer is named there; choose **Open Anyway**,
   then enter your login password. The button stays there for about an hour
   after your attempt to open the app; if it has gone, try to open Beatlayer
   again.
4. macOS now remembers the choice, so a plain double-click works from then on.

Apple describes these steps in its support article "Open a Mac app from an
unknown developer".

If macOS says Beatlayer **is damaged and can't be opened**, the app is not
damaged: the download carries a quarantine flag that makes macOS refuse a
build it cannot check. Run this in Terminal, which removes that flag from the
app, and open it again:

```sh
xattr -dr com.apple.quarantine /Applications/Beatlayer.app
```

### Windows

1. Run the downloaded MSI installer.
2. If a blue window says **Windows protected your PC**, choose **More info**,
   and then **Run anyway**.
3. Finish the installer, then start Beatlayer from the Start menu.

### Linux

1. Make the AppImage executable and run it:

   ```sh
   chmod +x beatlayer-*.AppImage
   ./beatlayer-*.AppImage
   ```

   Alternatively, unpack the `.tar.gz` and run the `beatlayer` binary inside it.

## Reporting problems

Beatlayer is developed in the open. If something does not work, or you want to
follow along, the project lives at https://github.com/loombridge/beatlayer.
