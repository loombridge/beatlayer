# Beatlayer alpha release notes

Beatlayer is an early alpha of an open-source digital audio workstation. It
follows Ableton Live 12's workflow and runs natively on macOS (Apple Silicon),
Windows, and Linux.

The alpha is the first of ten planned phases of work, so it is far from
finished: large parts of Live's workflow are still missing, the parts that are
here are rough, and the set format can change between builds. Treat it as a
look at where the project is going, not as something to rely on yet, and keep
your own copy of anything you care about.

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
2. In Applications, right-click (or Control-click) Beatlayer and choose
   **Open**, then choose **Open** again in the dialog that appears. macOS
   remembers the choice, so a plain double-click works from then on.
3. If macOS still refuses to open it, open **System Settings > Privacy &
   Security**, find the message about Beatlayer near the bottom of the window,
   and choose **Open Anyway**.

### Windows

1. Run the downloaded MSI installer.
2. If a blue window says **Windows protected your PC**, choose **More info**,
   and then **Run anyway**.
3. Finish the installer, then start Beatlayer from the Start menu.

### Linux

1. Make the AppImage executable and run it:

   ```sh
   chmod +x Beatlayer-*.AppImage
   ./Beatlayer-*.AppImage
   ```

   Alternatively, unpack the `.tar.gz` and run the `beatlayer` binary inside it.

## Reporting problems

Beatlayer is developed in the open. If something does not work, or you want to
follow along, the project lives at https://github.com/loombridge/beatlayer.
