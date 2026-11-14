# osu! Lost Scores Analyzer

Finds your osu!stable plays that ScoreV1 overwrote and the pp they were worth.

Until October 2024 the osu!stable server kept one play per beatmap and mod combination: the one with the highest score. A new play with a higher score replaced the old one even when the old play was worth more pp, and a play that scored less than your record was not kept at all. That pp disappeared from your profile. Your local replays still have those plays, and this tool reads them back.

This is a Rust rewrite of the original Python desktop app. It is in early development.

## Window

    cargo install --locked --path crates/lost-scores-gui
    lost-scores-gui

The window runs the same scan as the command line and shows the result as a table. It needs the keys of your own osu! OAuth application for now: choose "Use Custom API Keys" and the player name, and the keys are checked before they are saved.

- The osu! folder is found the same way as in the command line and remembered; the folder button picks another one.
- Start Scan shows the progress and a log. Every finished scan is kept in the scan history, and the results view opens any earlier scan.
- The results view has three tabs: Lost Scores, Online Top and Potential Top. A click on a column header sorts by it and a second click reverses the order. The search field finds text in titles, artists, difficulties, mappers, mods and dates and steps through the matches. A right click on a row copies it as text. "Open folder" opens the folder of the shown scan.
- Below 790 px high the log is hidden. From 1480 px wide the controls and the results sit side by side; in a narrower window See Full Results shows the results over the controls, and Back returns to the controls.
- The settings are shared with the command line. Clear cache removes the downloaded avatars and covers and keeps the scan history, the map catalog and the settings.

On Linux the launcher entry and the icons come from a script. Run it after installing the program:

    packaging/linux/install.sh

It copies `kz.lemon4ik.LostScores.desktop` and the lemon icons into `$XDG_DATA_HOME` (`~/.local/share` by default). Under Wayland the window takes its icon from this entry, which has the same app id as the window. On Windows the icon is built into the executable.

## Share images

After every scan the window saves three pictures into the scan's folder:

- `lost_scores.png`: your lost plays on ranked maps, or on all maps when the scan included unranked and loved ones;
- `potential_top.png`: your top 200 as it would be with the lost plays kept, with the pp and accuracy it would give;
- `summary_badge.png`: a small card with the same numbers.

"Scores to show" sets how many plays the two big pictures list, up to 200. "Ensure at least one lost score is visible" extends the Potential Top down to its first lost play. The pictures are drawn at twice their size from images prepared at exactly that size, so they stay sharp when a chat shows them in full.

The share button in the results view lists the pictures of the shown scan:

- **Open** shows one in your image viewer;
- **Copy** puts it on the clipboard, ready to paste into a chat;
- **catbox** uploads it to [catbox.moe](https://catbox.moe) and copies the link.

An upload to catbox.moe is anonymous and public: anyone with the link can see the picture, and catbox keeps it until nobody has opened it for two years. It cannot be deleted for free, so the window asks before the first upload, and nothing is uploaded without a click.

Covers come from assets.ppy.sh and are cached; Clear cache removes them.

## Scan history

Every finished scan is kept, and the scan list in the results view opens any earlier one. In a narrow window the results open by themselves when a scan ends.

The cross next to an older scan, or "Delete older scans" at the bottom of the list, moves scans to the system trash after a confirmation, so you can restore them from there. The newest scan always stays.

## Command line

    cargo install --locked --path crates/lost-scores-cli
    lost-scores

`lost-scores` finds the osu! folder and remembers it, reads the player name and the Songs folder from the osu! config, calculates pp for your replays and prints the plays that ScoreV1 overwrote, best first. It never writes to the osu! folder. `lost-scores --help` lists every option with examples and shows where the settings, keys and map data are kept.

- `--all` also shows loved and unranked maps; `--sort date`, `--limit N` and `--json` change the output.
- `lost-scores potential` shows your top 200 as it would be if ScoreV1 had kept your best plays: the potential pp and accuracy, and which lost plays would be in it.
- `--check-missing` also finds lost plays on maps you no longer have in Songs. It downloads only the maps that can hold a lost play into the map cache; the first run takes a few minutes.
- `lost-scores login` saves the keys of your own osu! OAuth application (create one at https://osu.ppy.sh/home/account/edit). With keys, the first run asks the osu! API about the statuses and ranked dates of your maps, about 180 requests for a few thousand maps, and later runs only about new maps; statuses that can still change are asked again once a week. Long API steps show a progress bar with the time left. `--offline` skips the API, `lost-scores logout` deletes the keys. `LOST_SCORES_CLIENT_ID` and `LOST_SCORES_CLIENT_SECRET` work instead of saved keys.
- `--osu-dir`, `--player` and `--songs-dir` override the detection. Options of a command go after it: `lost-scores potential --offline`.
- `--timings` prints how long each step took.
- `lost-scores scan --output replays.json` dumps every replay of the player with its beatmap and pp, for checks. `--maps <file>` uses another map catalog instead of the saved one.

## Credits

- [Twemoji](https://github.com/jdecked/twemoji) (CC BY 4.0), the lemon.
- [Font Awesome Free](https://fontawesome.com) (CC BY 4.0), the icons.
- [Fredoka](https://fonts.google.com/specimen/Fredoka) (OFL 1.1, the licence is in `crates/lost-scores-gui/assets/OFL.txt`).
- [Slint](https://slint.dev) (GPLv3, or the royalty-free licence with attribution), the window toolkit.
- [osu-web](https://github.com/ppy/osu-web) artwork © ppy Pty Ltd (AGPL-3.0), the grade and mod icons.

## License

[AGPL-3.0-only](LICENSE)

Copyright (c) 2026 KZ_Lemon4ik
