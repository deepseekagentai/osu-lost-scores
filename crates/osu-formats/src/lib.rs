mod beatmap;
mod lzma;
mod mods;
mod replay;

pub use beatmap::BeatmapMetadata;
pub use mods::Mods;
pub use replay::{ReplayError, ReplayHeader};

fn utf8_ignoring_invalid(bytes: &[u8]) -> String {
    bytes.utf8_chunks().map(|chunk| chunk.valid()).collect()
}
