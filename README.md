# osu! Lost Scores Analyzer

Finds your osu!stable plays that ScoreV1 overwrote and the pp they were worth.

Until October 2024 the osu!stable server kept one play per beatmap and mod combination: the one with the highest score. A new play with a higher score replaced the old one even when the old play was worth more pp, and a play that scored less than your record was not kept at all. That pp disappeared from your profile. Your local replays still have those plays, and this tool reads them back.

This is a Rust rewrite of the original Python desktop app.

## Download

Get the files from the [latest release](https://github.com/kz-lemon4ik/osu-lost-scores/releases/latest).

- **Windows 10 and 11:** `lost-scores-<version>-windows-x64.exe`. It is one file and needs no installation: put it anywhere and run it. The settings, the keys, the scans and the map catalog are kept in `%APPDATA%\lost-scores`, the downloaded `.osu` files, covers and avatars in `%LOCALAPPDATA%\lost-scores`, so a newer version simply replaces the file. To remove the program, delete the file and these two folders.
- **Linux:** `lost-scores-<version>-linux-x64.tar.gz`, for distributions with glibc 2.35 or newer. Unpack it and run `./lost-scores-gui`, or run `./install.sh` to copy the programs into `~/.local/bin` and add the launcher entry and the icons.
- **macOS 11 or newer on Apple Silicon:** `lost-scores-<version>-macos-arm64.zip`. Unpack it and move Lost Scores to Applications. osu!stable runs on a Mac only through Wine, so pick its osu! folder with the folder button. In that dialog, Cmd+Shift+G opens a path you type or paste, and Cmd+Shift+. shows hidden folders such as `~/Library`, where Whisky and CrossOver keep their bottles.
- On Linux and macOS the settings and the keys are kept in `~/.config/lost-scores`, the scans and the map catalog in `~/.local/share/lost-scores`, the downloaded `.osu` files, covers and avatars in `~/.cache/lost-scores`.
- The command line `lost-scores` comes in the Linux archive and, for Windows and macOS, in `lost-scores-cli-<version>-windows-x64.zip` and `lost-scores-cli-<version>-macos-arm64.zip`. Open a terminal in the unpacked folder and run `.\lost-scores.exe` on Windows or `./lost-scores` elsewhere, or put the program on your PATH to type `lost-scores` as below.
- `SHA256SUMS` has the checksums of every file.

The program is not signed yet, so the first start needs one more step:

- **Windows:** SmartScreen may say "Windows protected your PC". Choose "More info", then "Run anyway". With Smart App Control turned on in Windows 11, an unsigned program cannot start at all and cannot be allowed one by one; that ends when the builds are signed.
- **macOS 15 and newer:** the system says it cannot verify Lost Scores. Choose Done, open System Settings, Privacy & Security, and choose "Open Anyway" next to Lost Scores. On macOS 14 and older, Control-click Lost Scores, choose Open, then Open again. For the command line, run `xattr -d com.apple.quarantine lost-scores` once.

You need osu!stable with your replays and, for now, the keys of your own osu! OAuth application: create one at https://osu.ppy.sh/home/account/edit. The program only reads the osu! folder and never changes it.

## Window

The window runs the same scan as the command line. Choose "Use Custom API Keys" and the player name; the keys are checked before they are saved.

- The osu! folder is found the same way as in the command line and remembered; the folder button picks another one.
- Start Scan shows the progress and a log. Every finished scan is kept in the scan history, and the results view opens any earlier scan.
- The results open on the pictures of the scan (see Share images). The Images | Table switch in the results header shows the tables instead: Lost Scores, Online Top and Potential Top. A click on a column header sorts by it and a second click reverses the order. The search field finds text in titles, artists, difficulties, mappers, mods and dates and steps through the matches. A right click on a row copies it as text. The folder icon in the header opens the folder of the shown scan.
- Below 790 px high the log is hidden, and its newest line shows under the status instead; hover it to read it whole. From 1480 px wide the controls and the results sit side by side; in a narrower window the results open over the controls when a scan ends, See Full Results opens them again, and Back returns to the controls.
- Esc closes the top layer: a menu, a sheet, then the results. In the search field it clears the search first.
- The settings are shared with the command line. Clear cache removes the downloaded avatars and covers and keeps the scan history, the map catalog and the settings.

On Linux the launcher entry and the icons come from `install.sh`. It copies `kz.lemon4ik.LostScores.desktop` and the lemon icons into `$XDG_DATA_HOME` (`~/.local/share` by default). Under Wayland the window takes its icon from this entry, which has the same app id as the window. On Windows the icon is built into the executable.

## Share images

After every scan the window saves three pictures into the scan's folder:

- `summary_badge.png`: a small card with the main numbers;
- `lost_scores.png`: your lost plays on ranked maps, or on all maps when the scan included unranked and loved ones;
- `potential_top.png`: your top 200 as it would be with the lost plays kept, with the pp and accuracy it would give.

"Scores to show" sets how many plays the two big pictures list, up to 250. "Ensure at least one lost score is visible" extends the Potential Top down to its first lost play. The pictures are drawn at twice their size from images prepared at exactly that size, so they stay sharp when a chat shows them in full.

The Images mode shows the three pictures of the shown scan one under the other, in that order. A click on a picture opens it in your image viewer, and a right click offers Copy image and Open. The arrows in the corner jump to the previous or the next picture.

The catbox button next to the switch uploads all three pictures to [catbox.moe](https://catbox.moe) and copies their links, one per line. While it uploads, the button counts the pictures. Then it says whether the links were copied, and after a failure its tooltip shows why. A scan made before the images existed has none, and the view says so.

An upload to catbox.moe is anonymous and public: anyone with the link can see the pictures, and catbox keeps each one until nobody has opened it for two years. They cannot be deleted for free, so the window asks before the first upload, and nothing is uploaded without a click.

Covers come from assets.ppy.sh and are cached; Clear cache removes them.

## Scan history

Every finished scan is kept, and the scan list in the results view opens any earlier one. In a narrow window the results open by themselves when a scan ends.

The cross next to an older scan, or "Delete older scans" at the bottom of the list, moves scans to the system trash after a confirmation, so you can restore them from there. The newest scan always stays.

## Command line

`lost-scores` finds the osu! folder and remembers it, reads the player name and the Songs folder from the osu! config, calculates pp for your replays and prints the plays that ScoreV1 overwrote, best first. It never writes to the osu! folder. `lost-scores --help` lists the options with examples and shows where the settings, keys and map data are kept.

- `--all` also shows loved and unranked maps; `--sort date`, `--limit N` and `--json` change the output.
- `lost-scores potential` shows your top 200 as it would be if ScoreV1 had kept your best plays: the potential pp and accuracy, and which lost plays would be in it.
- `--check-missing` also finds lost plays on maps you no longer have in Songs. It downloads only the maps that can hold a lost play into the map cache; the first run takes a few minutes.
- `lost-scores login` saves the keys of your own osu! OAuth application (create one at https://osu.ppy.sh/home/account/edit). With keys, the first run asks the osu! API about the statuses and ranked dates of your maps, about 180 requests for a few thousand maps, and later runs only about new maps; statuses that can still change are asked again once a week. Long API steps show a progress bar with the time left. `--offline` skips the API, `lost-scores logout` deletes the keys. `LOST_SCORES_CLIENT_ID` and `LOST_SCORES_CLIENT_SECRET` work instead of saved keys.
- `--osu-dir`, `--player` and `--songs-dir` override the detection. Options of a command go after it: `lost-scores potential --limit 100`.
- `--timings` prints how long each step took.
- `lost-scores scan --output replays.json` dumps every replay of the player with its beatmap and pp, for checks. For checks too, the table and `lost-scores potential` take a hidden `--maps <file>` that uses another map catalog instead of the saved one.

## Privacy

The program has no telemetry. It goes online for these things and nothing else:

- saving keys, in the window or with `lost-scores login`, checks them with the osu! API;
- with saved keys, the window looks up your player through the osu! API when it starts, for the account card, and loads your avatar from a.ppy.sh;
- a scan asks the osu! API, with your keys, about your player, your online top and the statuses of your maps; the check for missing beatmap IDs also downloads the `.osu` files of the maps you no longer have;
- the results load the map covers from assets.ppy.sh;
- "Check for updates" asks GitHub for the latest release;
- the catbox button uploads the pictures of the shown scan to catbox.moe.

Your keys, settings and scans stay on your computer.

## Build from source

With the current stable Rust:

    cargo install --locked --path crates/lost-scores-gui
    cargo install --locked --path crates/lost-scores-cli

On Linux the window needs the fontconfig development files (`libfontconfig1-dev` on Debian and Ubuntu). After installing, `packaging/linux/install.sh` adds the launcher entry and the icons.

## Credits

- [Twemoji](https://github.com/jdecked/twemoji) (CC BY 4.0), the lemon.
- [Font Awesome Free](https://fontawesome.com) (CC BY 4.0), the icons.
- [Fredoka](https://fonts.google.com/specimen/Fredoka) (OFL 1.1, the licence is in `crates/lost-scores-gui/assets/OFL.txt`).
- [Slint](https://slint.dev) (GPLv3, or the royalty-free licence with attribution), the window toolkit.
- [osu-web](https://github.com/ppy/osu-web) artwork © ppy Pty Ltd (AGPL-3.0), the grade and mod icons.

## License

[AGPL-3.0-only](LICENSE)

Copyright (c) 2026 KZ_Lemon4ik
