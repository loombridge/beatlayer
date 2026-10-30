# Updates: the signed update manifest, Get Automatic Updates, Check for Updates, and installing an update

- **Status:** draft
- **Work packages:** WP-10.17 (the manifest, checking, Get Automatic Updates, Help ▸ Check for Updates... and the Status Bar's message: R1 to R12); WP-10.18 (downloading, verifying, installing on each platform, and rolling back: R13 to R24). The Licenses & Updates page is [the app shell](app-shell.md) R25's; the Help menu is [the Help menu](help-menu.md)'s
- **Parity-matrix row:** none (Beatlayer's own updater, in Live's places: M-HELP-010, L-SET-019's Get Automatic Updates, L-STAT-002's update messages)
- **Checked against:** the Live 12 reference manual (online edition), chapter 2 section 2.3.10, read 30 October 2026; the UI parity reference's rows L-SET-019, L-STAT-002 and M-HELP-010; the minisign signature format; [the security page](../security.md); [the architecture](../02-architecture.md), "Packaging"; [the app shell](app-shell.md) R4 and [crash recovery](crash-recovery.md) R2 and R6

## The behaviour

Each release publishes, beside its packages, an update manifest: the version, its date, a link to its release notes, and for each platform its package's address, size and SHA-256, the whole signed with the project's update key. Beatlayer trusts a manifest only when its signature checks against the public key in its own source, and a package only when its size and SHA-256 match the manifest's.

With Get Automatic Updates on (Settings ▸ Licenses & Updates, as in Live), Beatlayer checks at start, at most once a day, never while the song plays or something records. Help ▸ Check for Updates... checks at once. When a newer version is found, the Status Bar says so, and its dialog offers to download and install it. The download runs in the background; the update installs when Beatlayer quits, or at once with Restart Now, which quits as Quit does, asking about unsaved changes. On macOS the app bundle is swapped and relaunched, on Windows the installer runs after Beatlayer exits, and on Linux an AppImage replaces itself; a tarball or a distribution's package is only told about the update. The version an update replaced is kept once, so the person can go back to it.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 2 "First Steps", section 2.3.10 "Licenses & Updates" | The page manages automatic updates, among authorisation and usage data |
| S2 | Reference | [layout.md](../ui-parity/layout.md) L-SET-019 (Maintenance: Get Automatic Updates) and L-STAT-002 (the message area shows updates); [menus.md](../ui-parity/menus.md) M-HELP-010 (Check for Updates..., Beatlayer's updater) | Where the switch, the message and the menu item sit |
| S3 | Standard | The minisign signature format (Ed25519 signatures over a file, with a key ID, a trusted comment and a global signature) | How a manifest is signed and checked |
| S4 | Beatlayer | [The security page](../security.md): downloads pinned and checked before use; anything that reaches the network is `external`; [the architecture](../02-architecture.md), "Packaging" (the macOS `.app` in a DMG, the MSI and zip, the AppImage and tarball) | What a package is on each platform, and how downloads are trusted |
| S5 | Spec | [The app shell](app-shell.md) R4 (Quit with unsaved changes asks first); [crash recovery](crash-recovery.md) R2 and R6 (a session that ends without closing its set is offered for recovery; closing a set leaves nothing to offer); [crash reports](crash-reports.md) R14 | What a relaunch must and mustn't do; the Maintenance section |

## Rules

### The manifest (WP-10.17)

- **R1.** An update manifest, `update.json`, holds its `format` (`beatlayer-update`) and `version` (1); the release's `version`, `date` and `notes_url`; and `packages`, one for each platform and kind (`macos-arm64` DMG, `windows-x64` and `windows-arm64` MSI and zip, `linux-x64` and `linux-arm64` AppImage and tarball), each with its `url` (HTTPS), `size` in bytes and `sha256`. Beside it, `update.json.minisig` is its minisign signature. (S3, S4)
- **R2.** Beatlayer accepts a manifest only when its signature checks, over the manifest's exact bytes, against the public key fixed in its source. A missing or wrong signature, a key that isn't the fixed one, one changed byte, JSON that doesn't parse, or an unknown format version is refused, and the check's result is a failure with that reason. A manifest whose version isn't newer than the running one gives the result "up to date". (S3)
- **R3.** The release workflow writes the manifest from the packages it has just built, signs it with the private key it is given as a secret, and attaches both files to the release, so the latest release's assets always hold the newest; without the secret it attaches neither and says so. (S3, S4)
- **R4.** A build with no public key in its source, or whose version carries a pre-release suffix (development and nightly builds), never checks: Get Automatic Updates is disabled and Check for Updates... says that this build doesn't update itself. (S4)

### Checking (WP-10.17)

- **R5.** Get Automatic Updates is a switch, the first row of Settings ▸ Licenses & Updates ▸ Maintenance ([the app shell](app-shell.md) R25), on in a new install, as Live's is. It is kept in the preferences, set by `settings.set_automatic_updates`, and read back from `settings.state` as `licenses_updates.automatic_updates`. (S1, S2)
- **R6.** With it on, once the start-up set has opened, Beatlayer fetches the manifest in the background if its last check was more than 24 hours ago, and keeps the time of each check in the preferences. A check that would start while the transport runs or something records waits until both have stopped. (S4)
- **R7.** With it off, Beatlayer contacts nothing at start. (S4)
- **R8.** A check is one HTTPS request for the manifest and its signature, carrying nothing but a user agent of Beatlayer's name, version, operating system and architecture: no identifier, no cookie. (S4)
- **R9.** Help ▸ Check for Updates... (M-HELP-010) runs `app.check_for_updates` (`external`) at once, whatever the switch, and shows a dialog: that this version is up to date; that a newer version is available, with its version, a link to its release notes, and Download and Install and Later; or that the check failed, with the reason. (S2)
- **R10.** When an automatic check finds a newer version, the Status Bar's message area says that it is available, once a session; clicking the message opens R9's dialog. (S2)
- **R11.** `app.update_status` (`read`) reports whether automatic checks are on and allowed (R4), the last check's time and result (`never`, `up_to_date`, `available`, `failed` and the reason), the newer version and its notes link, and what R13 to R20 add. (S4)
- **R12.** `settings.set_automatic_updates` is `edit`, a kept preference, and names `settings.state` as its readback; `app.check_for_updates` names `app.update_status`. (S4)

### Downloading and installing (WP-10.18)

- **R13.** Download and Install runs `app.download_update` (`external`): the platform's package downloads in the background into the app's data folder, its progress in the Status Bar, then its size and its SHA-256 are checked against the accepted manifest. A package that fails either check is deleted and the update reports the failure; it never installs. (S4)
- **R14.** Once the package is ready, the dialog offers Install on Quit (the default) and Restart Now. Restart Now quits as Quit does: with unsaved changes it asks Save, Don't Save or Cancel first ([the app shell](app-shell.md) R4), and Cancel cancels the restart. While the transport runs or something records, nothing installs and Restart Now is disabled. (S5)
- **R15.** Before an update installs, Beatlayer closes its set and its recovery session as a normal quit does, so the relaunched version doesn't offer the set as crashed ([crash recovery](crash-recovery.md) R2, R6). (S5)
- **R16.** On macOS the DMG is mounted with the system's disk image tool, not opened in a browser, so nothing is quarantined; the new `.app` is copied beside the running one and its code signature checked: once Beatlayer is signed with a Developer ID, the new app's team must be the running app's; an ad-hoc-signed build compares its signing identifier instead, and `app.update_status` records that this check is weaker. After Beatlayer quits, the two bundles are swapped by renaming, the new version starts, and the image is unmounted and deleted. (S4)
- **R17.** On Windows, after Beatlayer exits, a small helper (Beatlayer's own executable, started to apply the update) runs the MSI with a passive interface, waits for it, and starts the new version. The MSI's upgrade code never changes between versions. (S4)
- **R18.** On Linux an AppImage is replaced in place: the new file is written beside the running one (the path in `APPIMAGE`) and renamed over it after Beatlayer quits, then started. A tarball or a distribution's package installs nothing: the dialog says a newer version is available and links its download page, and `app.update_status` reports the install as manual. (S4)
- **R19.** Where the running app's location can't be written (a mounted disk image, a folder without the rights), nothing installs: the dialog says so and links the download page. (S4)
- **R20.** The version an update replaced is kept once, in the app's data folder (the old `.app`, the old MSI, the old AppImage), and a later update replaces it. `app.rollback_update` (`destructive`) installs it as an update installs, once. (S4)
- **R21.** `app.install_update` (`destructive`: it replaces the application) and `app.rollback_update` install at the next quit, or at once with `restart`, under R14's rules; every step is a registered command, and `app.update_status` reports the download's progress, whether a verified package is ready, whether an install is pending, and whether a previous version is kept. (S4)
- **R22.** The dialogs are dialogs in the accessibility tree, their buttons in Tab order, the default focused, Esc doing what Later does; the download's progress is a progress indicator with its value. (S5)
- **R23.** A package's address must be HTTPS; its integrity comes from the signed manifest's SHA-256, not from where it is. (S4)
- **R24.** [The security page](../security.md) has a row for updates: the signed manifest, the fixed key, the checks before installing. (S4)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2 | to be written in WP-10.17 (a good manifest, a tampered one, a wrong key, an older version, malformed JSON) | unit |
| R3 | to be written in WP-10.17 (the workflow without the secret) | workflow lint |
| R4 to R8 | to be written in WP-10.17 (a local test feed: once a day, nothing when off, deferred while playing) | shell test |
| R9, R10 | to be written in WP-10.17 | UI script |
| R11, R12 | to be written in WP-10.17 | agent API, `cargo xtask commands` |
| R13 to R21 | to be written in WP-10.18 (two versions of a small stand-in package, swapped on each platform; the full app on the nightly macOS job; a tampered package; the transport rule; rollback) | integration on each CI platform |
| R22 | to be written in WP-10.18 | UI script |
| R23, R24 | to be written in WP-10.17 and WP-10.18 | unit; review |
