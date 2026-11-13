# osu! Lost Scores Analyzer

Finds your osu!stable plays that ScoreV1 overwrote and the pp they were worth.

Until October 2024 the osu!stable server kept one play per beatmap and mod combination: the one with the highest score. A new play with a higher score replaced the old one even when the old play was worth more pp, and that pp disappeared from your profile. Your local replays still have those plays, and this tool reads them back.

This is a Rust rewrite of the original Python desktop app. It is in early development and not usable yet.

## Command line

So far the command line tool only lists the replays the analysis will use, with their local beatmaps:

    cargo run --release -- scan --osu-dir <osu! folder> --player <name> --output replays.json

It reads `Data/r` and `Songs` and never writes to the osu! folder.

## License

[MIT](LICENSE)
