use std::fmt;

use lost_scores_app::online::Player;
use lost_scores_app::rows::thousands;
use osu_api::ApiError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Username,
    ClientId,
    ClientSecret,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Username => "Username or ID",
            Field::ClientId => "Client ID",
            Field::ClientSecret => "Client Secret",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeysError {
    Missing(Field),
    Rejected,
    Unreachable,
}

impl fmt::Display for KeysError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeysError::Missing(field) => write!(formatter, "{} is required", field.label()),
            KeysError::Rejected => formatter.write_str("User not found or API keys are invalid"),
            KeysError::Unreachable => formatter.write_str("Could not reach the osu! API"),
        }
    }
}

impl std::error::Error for KeysError {}

pub fn check_fields(
    username: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<(), Vec<Field>> {
    let missing: Vec<Field> = [
        (Field::Username, username),
        (Field::ClientId, client_id),
        (Field::ClientSecret, client_secret),
    ]
    .into_iter()
    .filter(|(_, value)| value.trim().is_empty())
    .map(|(field, _)| field)
    .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

pub fn keys_error(error: &ApiError) -> KeysError {
    match error {
        ApiError::Status { status, .. } if *status < 500 => KeysError::Rejected,
        _ => KeysError::Unreachable,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Account {
    Name(String),
    Id(u32),
}

const MODES: [&str; 4] = ["osu", "taiko", "fruits", "mania"];

pub fn parse_account(input: &str) -> Account {
    let trimmed = input.trim();
    let is_link = trimmed.contains("://") || trimmed.starts_with("osu.ppy.sh/");
    if !is_link {
        return from_part(trimmed, trimmed);
    }
    let path = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let after_marker = parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("users") || part.eq_ignore_ascii_case("u"))
        .and_then(|index| parts.get(index + 1));
    let part = after_marker.or_else(|| {
        parts
            .iter()
            .rev()
            .find(|part| !MODES.contains(&part.to_ascii_lowercase().as_str()))
    });
    match part {
        Some(part) => from_part(&decode(part), trimmed),
        None => Account::Name(trimmed.to_owned()),
    }
}

fn from_part(part: &str, fallback: &str) -> Account {
    if part.is_empty() {
        return Account::Name(fallback.to_owned());
    }
    match part.parse::<u32>() {
        Ok(id) if part.bytes().all(|byte| byte.is_ascii_digit()) => Account::Id(id),
        _ => Account::Name(part.to_owned()),
    }
}

fn decode(part: &str) -> String {
    let bytes = part.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let decoded = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match decoded {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub player_id: u32,
    pub username: String,
    pub texts: ProfileTexts,
}

impl Profile {
    pub fn from_player(player: &Player) -> Self {
        Self {
            player_id: player.id,
            username: player.names.first().cloned().unwrap_or_default(),
            texts: profile_texts(player),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileTexts {
    pub performance: String,
    pub accuracy: String,
    pub rank: String,
}

fn unknown() -> String {
    "unknown".to_owned()
}

pub fn profile_texts(player: &Player) -> ProfileTexts {
    ProfileTexts {
        performance: player.pp.map_or_else(unknown, thousands),
        accuracy: player
            .accuracy
            .map_or_else(unknown, |value| format!("{value:.2}%")),
        rank: player.global_rank.map_or_else(
            || "unranked".to_owned(),
            |rank| format!("#{}", thousands(f64::from(rank))),
        ),
    }
}

#[cfg(test)]
mod tests {
    use lost_scores_app::online::Player;

    use osu_api::ApiError;

    use super::{
        Account, Field, KeysError, Profile, check_fields, keys_error, parse_account, profile_texts,
    };

    #[test]
    fn empty_key_fields_are_reported() {
        assert_eq!(check_fields("lemon", "41234", "secret"), Ok(()));
        assert_eq!(
            check_fields("", " ", "secret"),
            Err(vec![Field::Username, Field::ClientId])
        );
        assert_eq!(check_fields("a", "b", "  "), Err(vec![Field::ClientSecret]));
    }

    #[test]
    fn a_profile_url_with_a_mode_gives_the_id() {
        for input in [
            "https://osu.ppy.sh/users/12345/osu",
            "https://osu.ppy.sh/users/12345/mania/",
            "https://osu.ppy.sh/u/12345",
            "osu.ppy.sh/users/12345?mode=osu#top",
            "  12345  ",
        ] {
            assert_eq!(parse_account(input), Account::Id(12345), "{input}");
        }
    }

    #[test]
    fn a_profile_url_or_plain_text_gives_the_name() {
        assert_eq!(
            parse_account("https://osu.ppy.sh/users/Lemon/"),
            Account::Name("Lemon".into())
        );
        assert_eq!(
            parse_account("https://osu.ppy.sh/users/Some%20Name/taiko"),
            Account::Name("Some Name".into())
        );
        assert_eq!(
            parse_account("  My Name  "),
            Account::Name("My Name".into())
        );
        assert_eq!(
            parse_account("12 Monkeys"),
            Account::Name("12 Monkeys".into())
        );
    }

    #[test]
    fn the_saved_name_is_the_api_username() {
        let player = Player {
            id: 12345,
            names: vec!["Real Name".into(), "Old Name".into()],
            pp: Some(1.0),
            accuracy: Some(99.0),
            global_rank: None,
        };
        let profile = Profile::from_player(&player);
        assert_eq!(profile.username, "Real Name");
        assert_eq!(profile.player_id, 12345);
    }

    #[test]
    fn network_errors_do_not_blame_the_keys() {
        let status = |status| ApiError::Status {
            status,
            url: "u".into(),
        };
        assert_eq!(
            keys_error(&ApiError::Network("down".into())),
            KeysError::Unreachable
        );
        assert_eq!(keys_error(&status(503)), KeysError::Unreachable);
        assert_eq!(keys_error(&status(500)), KeysError::Unreachable);
        assert_eq!(keys_error(&status(401)), KeysError::Rejected);
        assert_eq!(keys_error(&status(404)), KeysError::Rejected);
        assert_eq!(
            KeysError::Unreachable.to_string(),
            "Could not reach the osu! API"
        );
    }

    #[test]
    fn a_rejected_login_has_the_message_the_sheet_shows() {
        assert_eq!(
            KeysError::Rejected.to_string(),
            "User not found or API keys are invalid"
        );
        assert_eq!(
            KeysError::Missing(Field::ClientId).to_string(),
            "Client ID is required"
        );
    }

    #[test]
    fn profile_numbers_are_grouped_and_rounded() {
        let player = Player {
            id: 1,
            names: vec!["Lemon".into()],
            pp: Some(8556.6),
            accuracy: Some(98.3758),
            global_rank: Some(14856),
        };
        let texts = profile_texts(&player);
        assert_eq!(texts.performance, "8,557");
        assert_eq!(texts.accuracy, "98.38%");
        assert_eq!(texts.rank, "#14,856");
        let unranked = Player {
            global_rank: None,
            ..player
        };
        assert_eq!(profile_texts(&unranked).rank, "unranked");
    }

    #[test]
    fn a_profile_without_statistics_says_unknown_not_zero() {
        let player = Player {
            id: 1,
            names: vec!["Lemon".into()],
            pp: None,
            accuracy: None,
            global_rank: None,
        };
        let texts = profile_texts(&player);
        assert_eq!(texts.performance, "unknown");
        assert_eq!(texts.accuracy, "unknown");
    }
}
