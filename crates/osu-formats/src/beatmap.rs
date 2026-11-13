use crate::utf8_ignoring_invalid;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BeatmapMetadata {
    pub title: String,
    pub artist: String,
    pub creator: String,
    pub version: String,
    pub beatmap_id: Option<u32>,
    pub beatmapset_id: Option<u32>,
}

impl BeatmapMetadata {
    pub fn parse(data: &[u8]) -> Self {
        let mut metadata = Self::default();
        let mut in_metadata = false;
        for line in data.split(|&byte| byte == b'\n') {
            let line = utf8_ignoring_invalid(line);
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                if in_metadata {
                    break;
                }
                in_metadata = line == "[Metadata]";
                continue;
            }
            if !in_metadata {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "Title" => metadata.title = value.to_owned(),
                "Artist" => metadata.artist = value.to_owned(),
                "Creator" => metadata.creator = value.to_owned(),
                "Version" => metadata.version = value.to_owned(),
                "BeatmapID" => metadata.beatmap_id = online_id(value),
                "BeatmapSetID" => metadata.beatmapset_id = online_id(value),
                _ => {}
            }
        }
        metadata
    }
}

fn online_id(value: &str) -> Option<u32> {
    value.parse().ok().filter(|&id| id != 0)
}

#[cfg(test)]
mod tests {
    use super::BeatmapMetadata;

    const SAMPLE: &[u8] = b"osu file format v14\r\n\r\n[General]\r\nAudioFilename: audio.mp3\r\nMode: 0\r\n\r\n[Metadata]\r\nTitle:Re:Zero\r\nTitleUnicode:Re:Zero U\r\nArtist:Some Artist\r\nArtistUnicode:Some Artist U\r\nCreator: Mapper \r\nVersion:Insane\r\nSource:\r\nTags:a b c\r\nBeatmapID:402752\r\nBeatmapSetID:123456\r\n\r\n[Difficulty]\r\nHPDrainRate:5\r\n";

    #[test]
    fn reads_the_metadata_section() {
        assert_eq!(
            BeatmapMetadata::parse(SAMPLE),
            BeatmapMetadata {
                title: "Re:Zero".into(),
                artist: "Some Artist".into(),
                creator: "Mapper".into(),
                version: "Insane".into(),
                beatmap_id: Some(402752),
                beatmapset_id: Some(123456),
            }
        );
    }

    #[test]
    fn keys_outside_the_metadata_section_are_ignored() {
        let data = b"[General]\nTitle:Wrong\n[Metadata]\nTitle:Right\n[Events]\nTitle:Also wrong\n";
        assert_eq!(BeatmapMetadata::parse(data).title, "Right");
    }

    #[test]
    fn old_and_unsubmitted_beatmaps_have_no_ids() {
        for data in [
            &b"osu file format v5\r\n\r\n[Metadata]\r\nTitle:Old\r\n"[..],
            &b"[Metadata]\r\nBeatmapID:0\r\nBeatmapSetID:-1\r\n"[..],
        ] {
            let metadata = BeatmapMetadata::parse(data);
            assert_eq!((metadata.beatmap_id, metadata.beatmapset_id), (None, None));
        }
    }
}
