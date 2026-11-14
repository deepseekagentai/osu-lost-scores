use osu_formats::ReplayHeader;
use serde::{Deserialize, Serialize};

const HIDDEN: u32 = 1 << 3;
const FLASHLIGHT: u32 = 1 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    XH,
    X,
    SH,
    S,
    A,
    B,
    C,
    D,
}

impl Grade {
    pub fn from_rank(rank: &str) -> Option<Self> {
        Some(match rank {
            "XH" => Self::XH,
            "X" => Self::X,
            "SH" => Self::SH,
            "S" => Self::S,
            "A" => Self::A,
            "B" => Self::B,
            "C" => Self::C,
            "D" => Self::D,
            _ => return None,
        })
    }
}

pub fn grade(header: &ReplayHeader) -> Grade {
    let [n300, n100, n50, misses] = [
        header.count_300,
        header.count_100,
        header.count_50,
        header.count_miss,
    ]
    .map(u32::from);
    let total = n300 + n100 + n50 + misses;
    let silver = header.mods.bits() & (HIDDEN | FLASHLIGHT) != 0;
    let over = |percent: u32| n300 * 100 > percent * total;
    let top = |plain, silver_grade| if silver { silver_grade } else { plain };
    if total == 0 {
        Grade::D
    } else if n300 == total {
        top(Grade::X, Grade::XH)
    } else if over(90) && n50 * 100 <= total && misses == 0 {
        top(Grade::S, Grade::SH)
    } else if (over(80) && misses == 0) || over(90) {
        Grade::A
    } else if (over(70) && misses == 0) || over(80) {
        Grade::B
    } else if over(60) {
        Grade::C
    } else {
        Grade::D
    }
}

#[cfg(test)]
mod tests {
    use osu_formats::{Mods, ReplayHeader};

    use super::{Grade, grade};

    fn header(counts: [u16; 4], mods: u32) -> ReplayHeader {
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
            max_combo: 0,
            perfect: false,
            mods: Mods::from_bits(mods),
            timestamp_ticks: 0,
        }
    }

    #[test]
    fn grade_follows_the_stable_rules() {
        for (counts, expected) in [
            ([100, 0, 0, 0], Grade::X),
            ([95, 5, 0, 0], Grade::S),
            ([95, 3, 2, 0], Grade::A),
            ([95, 4, 1, 0], Grade::S),
            ([91, 9, 0, 0], Grade::S),
            ([95, 4, 0, 1], Grade::A),
            ([90, 10, 0, 0], Grade::A),
            ([85, 15, 0, 0], Grade::A),
            ([80, 20, 0, 0], Grade::B),
            ([85, 14, 0, 1], Grade::B),
            ([75, 25, 0, 0], Grade::B),
            ([70, 30, 0, 0], Grade::C),
            ([75, 24, 0, 1], Grade::C),
            ([65, 35, 0, 0], Grade::C),
            ([60, 40, 0, 0], Grade::D),
            ([0, 0, 0, 10], Grade::D),
            ([0, 0, 0, 0], Grade::D),
        ] {
            assert_eq!(grade(&header(counts, 0)), expected, "{counts:?}");
        }
    }

    #[test]
    fn hidden_and_flashlight_make_the_top_grades_silver() {
        for mods in [8, 1024, 8 | 1024] {
            assert_eq!(grade(&header([100, 0, 0, 0], mods)), Grade::XH, "{mods}");
            assert_eq!(grade(&header([95, 5, 0, 0], mods)), Grade::SH, "{mods}");
            assert_eq!(grade(&header([90, 10, 0, 0], mods)), Grade::A, "{mods}");
        }
        assert_eq!(grade(&header([100, 0, 0, 0], 64)), Grade::X);
    }

    #[test]
    fn grades_parse_from_the_api_rank() {
        for (text, expected) in [
            ("XH", Grade::XH),
            ("X", Grade::X),
            ("SH", Grade::SH),
            ("S", Grade::S),
            ("A", Grade::A),
            ("B", Grade::B),
            ("C", Grade::C),
            ("D", Grade::D),
        ] {
            assert_eq!(Grade::from_rank(text), Some(expected));
        }
        assert_eq!(Grade::from_rank("F"), None);
    }
}
