pub fn ago(seconds: f64) -> String {
    let seconds = seconds.abs();
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    let years = days / 365.0;
    let round = |value: f64| (value + 0.5).floor() as i64;
    let words = if seconds < 45.0 {
        "less than a minute".to_owned()
    } else if seconds < 90.0 {
        "about a minute".to_owned()
    } else if minutes < 45.0 {
        format!("{} minutes", round(minutes))
    } else if minutes < 90.0 {
        "about an hour".to_owned()
    } else if hours < 24.0 {
        format!("about {} hours", round(hours))
    } else if hours < 42.0 {
        "a day".to_owned()
    } else if days < 30.0 {
        format!("{} days", round(days))
    } else if days < 45.0 {
        "about a month".to_owned()
    } else if days < 365.0 {
        format!("{} months", round(days / 30.0))
    } else if years < 1.5 {
        "about a year".to_owned()
    } else {
        format!("{} years", round(years))
    };
    format!("{words} ago")
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn ago_matches_timeago_1_6_7() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/timeago-1.6.7.json")).unwrap();
        let cases = data["cases"].as_array().unwrap();
        assert!(cases.len() > 500);
        for case in cases {
            let seconds = case[0].as_f64().unwrap();
            assert_eq!(ago(seconds), case[1].as_str().unwrap(), "{seconds} s");
        }
    }

    #[test]
    fn ago_reads_like_the_website() {
        assert_eq!(ago(30.0), "less than a minute ago");
        assert_eq!(ago(40.0 * 86_400.0), "about a month ago");
        assert_eq!(ago(800.0 * 86_400.0), "2 years ago");
    }
}
