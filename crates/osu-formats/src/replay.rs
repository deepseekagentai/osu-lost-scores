use thiserror::Error;

use crate::{Mods, utf8_ignoring_invalid};

const TICKS_AT_UNIX_EPOCH: i64 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayHeader {
    pub mode: u8,
    pub version: i32,
    pub beatmap_md5: String,
    pub player_name: String,
    pub replay_md5: String,
    pub count_300: u16,
    pub count_100: u16,
    pub count_50: u16,
    pub count_geki: u16,
    pub count_katu: u16,
    pub count_miss: u16,
    pub score: u32,
    pub max_combo: u16,
    pub perfect: bool,
    pub mods: Mods,
    pub timestamp_ticks: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("replay ends before its header is complete")]
    Truncated,
    #[error("unexpected string marker {marker:#04x} at byte {offset}")]
    InvalidString { marker: u8, offset: usize },
}

impl ReplayHeader {
    pub fn parse(data: &[u8]) -> Result<Self, ReplayError> {
        let mut r = Reader { data, pos: 0 };
        Ok(Self {
            mode: r.u8()?,
            version: r.i32()?,
            beatmap_md5: r.string()?,
            player_name: r.string()?,
            replay_md5: r.string()?,
            count_300: r.u16()?,
            count_100: r.u16()?,
            count_50: r.u16()?,
            count_geki: r.u16()?,
            count_katu: r.u16()?,
            count_miss: r.u16()?,
            score: r.u32()?,
            max_combo: r.u16()?,
            perfect: r.u8()? == 1,
            mods: Mods::from_bits(r.u32()?),
            timestamp_ticks: {
                let _life_bar_graph = r.string()?;
                r.i64()?
            },
        })
    }

    pub fn unix_time(&self) -> i64 {
        self.timestamp_ticks
            .saturating_sub(TICKS_AT_UNIX_EPOCH)
            .div_euclid(TICKS_PER_SECOND)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], ReplayError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or(ReplayError::Truncated)?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ReplayError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, ReplayError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ReplayError> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, ReplayError> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32, ReplayError> {
        self.array().map(i32::from_le_bytes)
    }

    fn i64(&mut self) -> Result<i64, ReplayError> {
        self.array().map(i64::from_le_bytes)
    }

    fn uleb128(&mut self) -> Result<u64, ReplayError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(ReplayError::Truncated)
    }

    fn string(&mut self) -> Result<String, ReplayError> {
        let offset = self.pos;
        match self.u8()? {
            0x00 => Ok(String::new()),
            0x0b => {
                let len = usize::try_from(self.uleb128()?).map_err(|_| ReplayError::Truncated)?;
                Ok(utf8_ignoring_invalid(self.take(len)?))
            }
            marker => Err(ReplayError::InvalidString { marker, offset }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplayError, ReplayHeader};
    use crate::Mods;

    const TICKS_2024_10_28_11_17_00: i64 = 638_657_110_200_000_000;
    const HEADER_LEN: usize = 112;

    struct Sample {
        beatmap_md5: Vec<u8>,
        player: Vec<u8>,
        replay_md5: Vec<u8>,
        ticks: i64,
    }

    impl Default for Sample {
        fn default() -> Self {
            Self {
                beatmap_md5: short_string("0123456789abcdef0123456789abcdef"),
                player: short_string("Lemon"),
                replay_md5: short_string("fedcba9876543210fedcba9876543210"),
                ticks: TICKS_2024_10_28_11_17_00,
            }
        }
    }

    impl Sample {
        fn bytes(&self) -> Vec<u8> {
            let mut out = vec![2];
            out.extend(20190906i32.to_le_bytes());
            out.extend(&self.beatmap_md5);
            out.extend(&self.player);
            out.extend(&self.replay_md5);
            for count in [557u16, 160, 64, 12, 34, 125] {
                out.extend(count.to_le_bytes());
            }
            out.extend(447406u32.to_le_bytes());
            out.extend(106u16.to_le_bytes());
            out.push(1);
            out.extend(10u32.to_le_bytes());
            out.push(0x00);
            out.extend(self.ticks.to_le_bytes());
            out.extend(5i32.to_le_bytes());
            out.extend([1, 2, 3, 4, 5]);
            out
        }
    }

    fn short_string(text: &str) -> Vec<u8> {
        let mut out = vec![0x0b, u8::try_from(text.len()).unwrap()];
        out.extend(text.as_bytes());
        out
    }

    fn parse(sample: Sample) -> ReplayHeader {
        ReplayHeader::parse(&sample.bytes()).unwrap()
    }

    #[test]
    fn reads_every_header_field() {
        assert_eq!(
            parse(Sample::default()),
            ReplayHeader {
                mode: 2,
                version: 20190906,
                beatmap_md5: "0123456789abcdef0123456789abcdef".into(),
                player_name: "Lemon".into(),
                replay_md5: "fedcba9876543210fedcba9876543210".into(),
                count_300: 557,
                count_100: 160,
                count_50: 64,
                count_geki: 12,
                count_katu: 34,
                count_miss: 125,
                score: 447406,
                max_combo: 106,
                perfect: true,
                mods: Mods::from_bits(10),
                timestamp_ticks: TICKS_2024_10_28_11_17_00,
            }
        );
    }

    #[test]
    fn empty_string_marker_reads_as_empty_string() {
        let header = parse(Sample {
            replay_md5: vec![0x00],
            ..Sample::default()
        });
        assert_eq!((header.replay_md5.as_str(), header.score), ("", 447406));
    }

    #[test]
    fn string_length_is_uleb128() {
        let mut player = vec![0x0b, 0xc8, 0x01];
        player.extend([b'a'; 200]);
        let header = parse(Sample {
            player,
            ..Sample::default()
        });
        assert_eq!((header.player_name.len(), header.score), (200, 447406));
    }

    #[test]
    fn invalid_utf8_in_a_string_is_dropped() {
        let header = parse(Sample {
            player: vec![0x0b, 0x06, b'L', b'e', 0xff, b'm', b'o', b'n'],
            ..Sample::default()
        });
        assert_eq!(header.player_name, "Lemon");
    }

    #[test]
    fn unknown_string_marker_is_an_error() {
        let sample = Sample {
            beatmap_md5: vec![0x05],
            ..Sample::default()
        };
        assert_eq!(
            ReplayHeader::parse(&sample.bytes()),
            Err(ReplayError::InvalidString {
                marker: 0x05,
                offset: 5
            })
        );
    }

    #[test]
    fn data_ending_inside_the_header_is_truncated() {
        let bytes = Sample::default().bytes();
        for len in 0..HEADER_LEN {
            assert_eq!(
                ReplayHeader::parse(&bytes[..len]),
                Err(ReplayError::Truncated),
                "{len}"
            );
        }
    }

    #[test]
    fn replay_frames_are_not_needed() {
        let bytes = Sample::default().bytes();
        assert!(ReplayHeader::parse(&bytes[..HEADER_LEN]).is_ok());
    }

    #[test]
    fn string_longer_than_the_data_is_truncated() {
        for player in [
            vec![0x0b, 0x7f, b'a'],
            vec![
                0x0b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
            ],
        ] {
            let sample = Sample {
                player,
                ..Sample::default()
            };
            assert_eq!(
                ReplayHeader::parse(&sample.bytes()),
                Err(ReplayError::Truncated)
            );
        }
    }

    #[test]
    fn unix_time_floors_ticks_to_whole_seconds() {
        for (ticks, expected) in [
            (TICKS_2024_10_28_11_17_00, 1_730_114_220),
            (TICKS_2024_10_28_11_17_00 + 9_999_999, 1_730_114_220),
            (TICKS_2024_10_28_11_17_00 - 1, 1_730_114_219),
        ] {
            let header = parse(Sample {
                ticks,
                ..Sample::default()
            });
            assert_eq!(header.unix_time(), expected, "{ticks}");
        }
    }
}
