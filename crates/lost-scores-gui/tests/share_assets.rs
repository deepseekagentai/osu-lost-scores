use std::fs;
use std::path::PathBuf;

fn png_size(path: &PathBuf) -> (u32, u32) {
    let bytes = fs::read(path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "{} is not a PNG",
        path.display()
    );
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (word(16), word(20))
}

#[test]
fn every_share_picture_has_its_exact_size() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/share");
    let mut table: Vec<(String, (u32, u32))> = Vec::new();
    for grade in ["XH", "SH", "X", "S", "A", "B", "C", "D"] {
        table.push((format!("grades/{grade}.png"), (96, 48)));
    }
    for module in [
        "NF", "EZ", "TD", "HD", "HR", "SD", "DT", "NC", "HT", "FL", "SO", "PF",
    ] {
        table.push((format!("mods/{module}.png"), (60, 42)));
    }
    table.push(("lemon-36.png".into(), (36, 36)));
    table.push(("lemon-48.png".into(), (48, 48)));
    table.push(("clock.png".into(), (22, 22)));
    table.push(("trend-down.png".into(), (24, 22)));
    table.push(("backdrop-wide.png".into(), (1960, 880)));
    table.push(("backdrop-badge.png".into(), (1040, 280)));
    for (name, expected) in table {
        assert_eq!(png_size(&root.join(&name)), expected, "{name}");
    }
}
