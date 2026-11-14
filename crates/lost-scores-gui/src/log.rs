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
    use super::{split, stamp};

    #[test]
    fn a_stamped_line_splits_back_into_time_and_text() {
        let line = stamp((7, 5, 9), "Logged in as Lemon");
        assert_eq!(line, "[07:05:09] Logged in as Lemon");
        assert_eq!(split(&line), ("[07:05:09]", "Logged in as Lemon"));
        assert_eq!(split("plain"), ("", "plain"));
    }
}
