# osu! Lost Scores Analyzer

Finds your osu!stable plays that ScoreV1 overwrote and the pp they were worth.

Until October 2024 the osu!stable server kept one play per beatmap and mod combination: the one with the highest score. A new play with a higher score replaced the old one even when the old play was worth more pp, and a play that scored less than your record was not kept at all. That pp disappeared from your profile. Your local replays still have those plays, and this tool reads them back.

This is a Rust rewrite of the original Python desktop app. It is in early development.

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

## License

[MIT](LICENSE)
