use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub const KEEP_LINES: usize = 1000;
pub const ROTATE_BYTES: u64 = 1024 * 1024;

pub fn rotate(file: &Path) {
    if fs::metadata(file).is_ok_and(|meta| meta.len() > ROTATE_BYTES) {
        let _ = fs::rename(file, file.with_file_name("log.old.txt"));
    }
}

pub fn append(file: &Path, line: &str) {
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut out) = OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(out, "{} {line}", jiff::Zoned::now().date());
    }
}

pub fn stamp(clock: (u8, u8, u8), text: &str) -> String {
    format!("[{:02}:{:02}:{:02}] {text}", clock.0, clock.1, clock.2)
}

pub fn split(line: &str) -> (&str, &str) {
    match line.split_once("] ") {
        Some((time, text)) if line.starts_with('[') => (&line[..=time.len()], text),
        _ => ("", line),
    }
}

pub fn clock_now() -> (u8, u8, u8) {
    let now = jiff::Zoned::now();
    (now.hour() as u8, now.minute() as u8, now.second() as u8)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{ROTATE_BYTES, append, rotate, split, stamp};

    #[test]
    fn a_big_log_file_is_moved_aside_and_a_small_one_stays() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log.txt");
        fs::write(&file, "small\n").unwrap();
        rotate(&file);
        assert!(file.is_file());
        fs::write(&file, vec![b'x'; ROTATE_BYTES as usize + 1]).unwrap();
        rotate(&file);
        assert!(!file.exists());
        assert!(dir.path().join("log.old.txt").is_file());
    }

    #[test]
    fn lines_are_appended_with_the_date() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested").join("log.txt");
        append(&file, "[10:00:00] first");
        append(&file, "[10:00:01] second");
        let saved = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = saved.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" [10:00:00] first"), "{}", lines[0]);
        assert!(lines[1].ends_with(" [10:00:01] second"));
        assert_eq!(lines[0].len(), "2026-11-17 [10:00:00] first".len());
    }

    #[test]
    fn a_stamped_line_splits_back_into_time_and_text() {
        let line = stamp((7, 5, 9), "Logged in as Lemon");
        assert_eq!(line, "[07:05:09] Logged in as Lemon");
        assert_eq!(split(&line), ("[07:05:09]", "Logged in as Lemon"));
        assert_eq!(split("plain"), ("", "plain"));
    }
}
