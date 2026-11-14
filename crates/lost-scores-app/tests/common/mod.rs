use std::fs;
use std::path::Path;
use std::sync::Mutex;

use tempfile::TempDir;

#[derive(Clone, Copy)]
pub struct Replay<'a> {
    pub mode: u8,
    pub player: &'a str,
    pub beatmap_md5: &'a str,
    pub counts: [u16; 4],
    pub score: u32,
    pub max_combo: u16,
    pub mods: u32,
    pub ticks: i64,
}

impl Replay<'_> {
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = vec![self.mode];
        out.extend(20190906i32.to_le_bytes());
        for text in [
            self.beatmap_md5,
            self.player,
            "fedcba9876543210fedcba9876543210",
        ] {
            out.extend([0x0b, u8::try_from(text.len()).unwrap()]);
            out.extend(text.as_bytes());
        }
        let [n300, n100, n50, misses] = self.counts;
        for count in [n300, n100, n50, 0, 0, misses] {
            out.extend(count.to_le_bytes());
        }
        out.extend(self.score.to_le_bytes());
        out.extend(self.max_combo.to_le_bytes());
        out.push(0);
        out.extend(self.mods.to_le_bytes());
        out.push(0x00);
        out.extend(self.ticks.to_le_bytes());
        out
    }
}

pub fn write(root: &Path, relative: &str, data: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

static HOME: Mutex<(usize, Option<TempDir>)> = Mutex::new((0, None));

pub struct Isolated;

pub fn isolate_environment() -> Isolated {
    let mut state = HOME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.0 == 0 {
        let home = tempfile::tempdir().unwrap();
        for (name, folder) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            unsafe { std::env::set_var(name, home.path().join(folder)) };
        }
        state.1 = Some(home);
    }
    state.0 += 1;
    Isolated
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let mut state = HOME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.0 -= 1;
        if state.0 == 0 {
            state.1 = None;
        }
    }
}

#[allow(dead_code)]
pub fn isolated_home() -> std::path::PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").unwrap();
    Path::new(&config).parent().unwrap().to_path_buf()
}
