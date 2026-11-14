use std::collections::HashMap;
use std::path::Path;

use osu_formats::{Mods, ReplayHeader};
use rayon::prelude::*;
use rosu_pp::any::DifficultyAttributes;
use rosu_pp::{Beatmap, Difficulty, Performance};

use crate::ScannedReplay;
use crate::ticker::Ticker;

const TOUCH_DEVICE: u32 = 1 << 2;

pub(crate) const fn reference_mod_bits(mods: Mods) -> u32 {
    mods.bits() & !TOUCH_DEVICE
}

pub fn fill(osu_dir: &Path, replays: &mut [ScannedReplay]) -> usize {
    fill_with(osu_dir, replays, &|_, _| {})
}

pub fn fill_with(
    osu_dir: &Path,
    replays: &mut [ScannedReplay],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> usize {
    let view: &[ScannedReplay] = replays;
    let mut by_map: HashMap<&Path, Vec<usize>> = HashMap::new();
    for (index, replay) in view.iter().enumerate() {
        if let Some(beatmap) = &replay.beatmap {
            by_map.entry(&beatmap.path).or_default().push(index);
        }
    }
    let ticker = Ticker::new(by_map.values().map(Vec::len).sum(), progress);
    let computed: Vec<Option<Vec<(usize, f64)>>> = by_map
        .into_par_iter()
        .map(|(path, indices)| {
            let Some(map) = Beatmap::from_path(osu_dir.join(path)).ok() else {
                indices.iter().for_each(|_| ticker.tick());
                return None;
            };
            let mut by_mods: HashMap<u32, DifficultyAttributes> = HashMap::new();
            Some(
                indices
                    .into_iter()
                    .map(|index| {
                        let header = &view[index].header;
                        let attrs = by_mods
                            .entry(header.mods.bits())
                            .or_insert_with(|| difficulty(&map, header.mods));
                        let pp = scored(Performance::new(attrs.clone()), header);
                        ticker.tick();
                        (index, pp)
                    })
                    .collect(),
            )
        })
        .collect();
    let mut failed = 0;
    for result in computed {
        match result {
            Some(values) => {
                for (index, pp) in values {
                    replays[index].pp = Some(pp);
                }
            }
            None => failed += 1,
        }
    }
    failed
}

pub fn accuracy(header: &ReplayHeader) -> f64 {
    let [n300, n100, n50, misses] = [
        header.count_300,
        header.count_100,
        header.count_50,
        header.count_miss,
    ]
    .map(u32::from);
    let hits = n300 + n100 + n50 + misses;
    if hits == 0 {
        return 100.0;
    }
    f64::from(300 * n300 + 100 * n100 + 50 * n50) / f64::from(300 * hits) * 100.0
}

pub fn performance(map: &Beatmap, header: &ReplayHeader) -> f64 {
    scored(Performance::new(map), header)
}

fn difficulty(map: &Beatmap, mods: Mods) -> DifficultyAttributes {
    Difficulty::new()
        .mods(mods.bits())
        .lazer(false)
        .calculate(map)
}

fn scored(performance: Performance<'_>, header: &ReplayHeader) -> f64 {
    performance
        .mods(header.mods.bits())
        .lazer(false)
        .n300(u32::from(header.count_300))
        .n100(u32::from(header.count_100))
        .n50(u32::from(header.count_50))
        .misses(u32::from(header.count_miss))
        .combo(u32::from(header.max_combo))
        .legacy_total_score(header.score)
        .calculate()
        .pp()
}

#[cfg(test)]
mod tests {
    use osu_formats::{Mods, ReplayHeader};
    use rosu_pp::Beatmap;

    use super::{accuracy, performance};

    const HD: u32 = 8;
    const HR: u32 = 16;
    const DT: u32 = 64;
    const TD: u32 = 4;

    const SAMPLE_MAP: &[u8] = include_bytes!("../tests/fixtures/sample.osu");

    fn header(counts: [u16; 4], max_combo: u16, mods: u32, score: u32) -> ReplayHeader {
        ReplayHeader {
            mode: 0,
            version: 20190906,
            beatmap_md5: String::new(),
            player_name: String::new(),
            replay_md5: String::new(),
            count_300: counts[0],
            count_100: counts[1],
            count_50: counts[2],
            count_geki: 0,
            count_katu: 0,
            count_miss: counts[3],
            score,
            max_combo,
            perfect: false,
            mods: Mods::from_bits(mods),
            timestamp_ticks: 0,
        }
    }

    #[test]
    fn accuracy_uses_the_exact_hit_counts() {
        for (counts, expected) in [
            ([557, 160, 64, 125], 186_300.0 / 271_800.0 * 100.0),
            ([0, 11, 5, 0], 28.125),
            ([300, 0, 0, 0], 100.0),
            ([0, 0, 0, 0], 100.0),
        ] {
            assert_eq!(accuracy(&header(counts, 0, 0, 0)), expected, "{counts:?}");
        }
    }

    #[test]
    fn performance_matches_the_official_calculator() {
        let map = Beatmap::from_bytes(SAMPLE_MAP).unwrap();
        for (counts, combo, mods, score, expected) in [
            ([6, 1, 0, 0], 8, HD | DT | TD, 3500, 3.1759738646211635),
            ([5, 1, 0, 1], 4, HR, 1200, 0.5514316150578498),
        ] {
            let pp = performance(&map, &header(counts, combo, mods, score));
            assert!(
                (pp - expected).abs() <= 1e-9 * expected,
                "{counts:?} {mods}: {pp} against {expected}"
            );
        }
    }
}
