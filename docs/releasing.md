# Releasing

1. Set the version in `[workspace.package]` of `Cargo.toml`, run `cargo build` so that `Cargo.lock` follows, and write `docs/releases/<version>.md`.
2. Commit, push, and wait for a green CI on Linux and Windows.
3. Optional test build: run the Release workflow by hand on main (Actions, Release, "Run workflow", or `gh workflow run release.yml`). It builds the same files with `<version>-test-<commit>` in their names, as the artifacts Linux, Windows and macOS of the run, and creates no release. Run by hand on a tag, it behaves like the tag push.
4. Tag the rosu-pp commit that `crates/lost-scores-core/Cargo.toml` pins in kz-lemon4ik/rosu-pp as `lost-scores-<version>` and push that tag. A fork branch can be deleted or rewritten; the tag keeps the commit, so the release can always be built from source. Two releases on the same commit simply give it two tags.
5. Push the tag `v<version>`. The Release workflow checks that the tag matches `Cargo.toml`, builds the files and creates a draft release with them, `SHA256SUMS` and the notes file.
6. Check the files of the draft (below), sign it with `scripts/sign-release.sh v<version>`, then publish the draft by hand. The script downloads `SHA256SUMS` of the draft, signs it with the minisign key in `~/.minisign/minisign.key` (it asks for the key's password) and attaches `SHA256SUMS.minisig`. The window installs only releases whose `SHA256SUMS` carries this signature and whose file matches its checksum; an unsigned release is shown to players as a link only.

The files:

- `lost-scores-<version>-windows-x64.exe`: the window, one portable file. It is linked with the static C runtime, and the workflow fails if it imports the Visual C++ runtime.
- `lost-scores-cli-<version>-windows-x64.zip`: the command line for Windows.
- `lost-scores-<version>-linux-x64.tar.gz`: both programs, `install.sh`, the launcher entry and the icons. Built on Ubuntu 22.04, so it needs glibc 2.35 or newer.
- `lost-scores-<version>-macos-arm64.zip`: Lost Scores.app for Apple Silicon, with an ad-hoc signature only. The workflow runs the tests on macOS first, because CI does not.
- `lost-scores-cli-<version>-macos-arm64.zip`: the command line for macOS.

While the repository is private, a macOS minute of GitHub Actions counts ten times, so macOS builds only in this workflow.

## When osu! changes the pp formula

The backend reads once a day which `ppy.osu.Game.Rulesets.Osu` version osu-queue-score-statistics deploys and sends one Telegram alert when it is not the app's formula.

1. Bring the rosu-pp fork up to the new version and compare its pp with the pp osu! shows for a top of real scores: at least 199 of 200 plays within 0.01 pp.
2. Pin the new fork commit in `crates/lost-scores-core/Cargo.toml` and set `FORMULA` in `crates/lost-scores-core/src/pp.rs` to the new version.
3. Set `FORMULA` in `src/formula.ts` of the backend to the same version and deploy it, so that the alert stops.
4. Release the app as above.

## macOS check

On a Mac with Apple Silicon, with the zip downloaded from the draft or the test build.

- [ ] The first start of the downloaded app: the wording of the system message and of "Open Anyway" in Privacy & Security, for the README.
- [ ] The Dock shows the lemon, and no second icon appears while the pictures render.
- [ ] Text, icons and pictures stay sharp on the Retina screen and on an external monitor.
- [ ] The folder button picks an osu! folder inside a Wine wrapper or a copied one; a real scan with your own keys.
- [ ] Images mode: a click opens a picture in Preview; Copy image pastes into Telegram and Discord; catbox.
- [ ] A cross on an older scan deletes it after the confirmation, and its folder is gone from disk.
- [ ] The command line from its zip in Terminal, after `xattr -d com.apple.quarantine lost-scores`.

## Windows check

On a real Windows 10 or 11 machine with osu!stable, with the exe downloaded from the draft or the test build, not a dev build.

### Start and look
- [ ] SmartScreen and the antivirus on the first start of the downloaded exe: write down the exact wording for the README.
- [ ] The window frame and title bar buttons, snapping, maximizing and restoring. The window draws no buttons of its own.
- [ ] Scale 100%, 125% and 150%, and moving the window between two monitors: the avatar, the icons and the pictures stay sharp.
- [ ] The icon in Explorer, on the taskbar and in Alt+Tab.

### Paths and settings
- [ ] osu! is found automatically in `%LOCALAPPDATA%\osu!` and can be picked in a custom folder.
- [ ] A user folder with non-Latin letters (for example `C:\Users\Пользователь`) and a path with spaces.
- [ ] The keys, the settings, the scans and `maps.json` land in `%APPDATA%\lost-scores`, the downloaded `.osu` files, covers and avatars in `%LOCALAPPDATA%\lost-scores`; Log out removes the keys.
- [ ] The exe started from another folder, and a newer exe started in place of the old one, find the same settings and scans.

### Scan and pictures
- [ ] A real scan with your own keys: the progress, the log, and the Potential Top card.
- [ ] The picture renderer is the same exe started in the background: no console window flashes and no busy cursor stays; its warnings reach the log.
- [ ] Three PNGs in the scan folder; the folder icon in the results header opens Explorer there.
- [ ] Images mode: a click opens a picture in the default viewer; a right click, Copy image, pastes into Discord, Telegram and Paint (check the transparent corners).
- [ ] catbox: the warning sheet before the first upload, the count on the button while uploading, and the links on the clipboard, one per line.

### History
- [ ] A cross on an older scan deletes it after the confirmation, and its folder is gone from disk.
- [ ] "Delete older scans" while a PNG of one of them is open in another program: the list must match what is left on disk, and the log names the folder that could not be deleted.

### Clean up
- [ ] Clear cache removes the avatars and covers only.
- [ ] Deleting the exe and the two `lost-scores` folders leaves nothing behind.

## The update key

The window checks the latest release once a day. A newer signed release is downloaded, checked and put in place of the program, which runs it from the next start. Only the release workflow builds copies that update themselves (`LOST_SCORES_OFFICIAL`); a copy built from source only names the new version and opens its page.

`scripts/sign-release.sh` signs only a draft whose tag points at the same commit as the local tag, after it has downloaded every file of the draft and checked that each carries the version of the tag and matches `SHA256SUMS`, with nothing listed that the release lacks. It cannot tell whether the build itself was honest: once the repository is public, add build provenance (GitHub artifact attestations) to the release workflow and check it in the script before signing.

The public key is `PUBLIC_KEY` in `crates/lost-scores-app/src/updates.rs`. The secret key and its password stay with the author and are kept in a second safe place; they never go into the repository or into GitHub secrets. Installed copies check a release with the key built into them. To change the key, release the version with the new `PUBLIC_KEY` still signed with the old key (sign it by hand with `minisign -Sm SHA256SUMS -s <old key>` and the same checks), and sign later releases with the new one. If the secret key is lost, make a new pair with `minisign -G`, put the new public key into `PUBLIC_KEY` and release; copies with the old key then update once by hand.

