# osu! Lost Scores Analyzer

Finds your osu!stable plays that ScoreV1 overwrote and the pp they were worth.

Until October 2024 the osu!stable server kept one play per beatmap and mod combination: the one with the highest score. A new play with a higher score replaced the old one even when the old play was worth more pp, and a play that scored less than your record was not kept at all. That pp disappeared from your profile. Your local replays still have those plays, and this tool reads them back.

This is a Rust rewrite of the original Python desktop app. It is in early development.

## Command line

    cargo install --locked --path crates/lost-scores-cli
    lost-scores

`lost-scores` finds the osu! folder and remembers it, reads the player name and the Songs folder from the osu! config, calculates pp for your replays and prints the plays that ScoreV1 overwrote, best first. It never writes to the osu! folder.

- `--all` also shows loved and unranked maps; `--sort date`, `--limit N` and `--json` change the output.
- `--osu-dir`, `--player` and `--songs-dir` override the detection.
- Map statuses and ranked dates come from a map catalog: `--maps <file>`, or `maps.json` in the data folder (`~/.local/share/lost-scores` on Linux). Until the osu! API client lands, without a catalog every map is shown.
- `lost-scores scan --output replays.json` dumps every selected replay with its beatmap and pp.

## License

[MIT](LICENSE)
