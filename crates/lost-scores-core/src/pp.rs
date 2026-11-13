use std::collections::HashMap;
use std::path::Path;

use osu_formats::{Mods, ReplayHeader};
use rayon::prelude::*;
use rosu_pp::any::HitResultPriority;
use rosu_pp::{Beatmap, Difficulty, Performance};

use crate::ScannedReplay;

const TOUCH_DEVICE: u32 = 1 << 2;

pub(crate) const fn reference_mod_bits(mods: Mods) -> u32 {
    mods.bits() & !TOUCH_DEVICE
}

pub fn fill(osu_dir: &Path, replays: &mut [ScannedReplay]) -> usize {
    let view: &[ScannedReplay] = replays;
    let mut by_map: HashMap<&Path, Vec<usize>> = HashMap::new();
    for (index, replay) in view.iter().enumerate() {
        if let Some(beatmap) = &replay.beatmap {
            by_map.entry(&beatmap.path).or_default().push(index);
        }
    }
    let computed: Vec<Option<Vec<(usize, f64)>>> = by_map
        .into_par_iter()
        .map(|(path, indices)| {
            let map = Beatmap::from_path(osu_dir.join(path)).ok()?;
            Some(
                indices
                    .into_iter()
                    .map(|index| (index, performance(&map, &view[index].header)))
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

pub fn rounded_accuracy(header: &ReplayHeader) -> f64 {
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
    let accuracy = f64::from(300 * n300 + 100 * n100 + 50 * n50) / f64::from(300 * hits) * 100.0;
    format!("{accuracy:.2}").parse().unwrap_or(accuracy)
}

pub fn performance(map: &Beatmap, header: &ReplayHeader) -> f64 {
    Performance::new(map)
        .accuracy(rounded_accuracy(header))
        .combo(u32::from(header.max_combo))
        .misses(u32::from(header.count_miss))
        .hitresult_priority(HitResultPriority::BestCase)
        .difficulty(
            Difficulty::new()
                .mods(reference_mod_bits(header.mods))
                .lazer(false),
        )
        .calculate()
        .pp()
}

#[cfg(test)]
mod tests {
    use osu_formats::{Mods, ReplayHeader};
    use rosu_pp::Beatmap;

    use super::{performance, rounded_accuracy};

    const HD: u32 = 8;
    const HR: u32 = 16;
    const DT: u32 = 64;
    const TD: u32 = 4;

    const SAMPLE_MAP: &[u8] = include_bytes!("../tests/fixtures/sample.osu");

    fn header(counts: [u16; 4], max_combo: u16, mods: u32) -> ReplayHeader {
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
            score: 0,
            max_combo,
            perfect: false,
            mods: Mods::from_bits(mods),
            timestamp_ticks: 0,
        }
    }

    #[test]
    fn accuracy_is_rounded_to_hundredths_like_python() {
        for (counts, expected) in [
            ([557, 160, 64, 125], 68.54),
            ([0, 11, 5, 0], 28.12),
            ([300, 0, 0, 0], 100.0),
            ([0, 0, 0, 0], 100.0),
        ] {
            assert_eq!(
                rounded_accuracy(&header(counts, 0, 0)),
                expected,
                "{counts:?}"
            );
        }
    }

    #[test]
    fn performance_matches_the_reference_calculator() {
        let map = Beatmap::from_bytes(SAMPLE_MAP).unwrap();
        for (counts, combo, mods, expected) in [
            ([6, 1, 0, 0], 8, HD | DT | TD, 4.8957636184902515),
            ([5, 1, 0, 1], 4, HR, 0.8099663083271696),
        ] {
            let pp = performance(&map, &header(counts, combo, mods));
            assert_eq!(pp, expected, "{counts:?} {mods}");
        }
    }
}
