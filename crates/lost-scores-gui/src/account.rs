use std::fmt;

use lost_scores_app::online::Player;
use lost_scores_app::rows::thousands;

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
}

impl fmt::Display for KeysError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeysError::Missing(field) => write!(formatter, "{} is required", field.label()),
            KeysError::Rejected => formatter.write_str("User not found or API keys are invalid"),
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

pub fn username_from_input(input: &str) -> String {
    let trimmed = input.trim();
    if !trimmed.contains("://") && !trimmed.starts_with("osu.ppy.sh/") {
        return trimmed.to_owned();
    }
    let path = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileTexts {
    pub performance: String,
    pub accuracy: String,
    pub rank: String,
}

pub fn profile_texts(player: &Player) -> ProfileTexts {
    ProfileTexts {
        performance: thousands(player.pp),
        accuracy: format!("{:.2}%", player.accuracy),
        rank: player.global_rank.map_or_else(
            || "unranked".to_owned(),
            |rank| format!("#{}", thousands(f64::from(rank))),
        ),
    }
}

#[cfg(test)]
mod tests {
    use lost_scores_app::online::Player;

    use super::{Field, KeysError, check_fields, profile_texts, username_from_input};

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
    fn a_profile_url_becomes_its_last_part() {
        assert_eq!(
            username_from_input("https://osu.ppy.sh/users/12345"),
            "12345"
        );
        assert_eq!(
            username_from_input("https://osu.ppy.sh/users/Lemon/"),
            "Lemon"
        );
        assert_eq!(
            username_from_input("osu.ppy.sh/users/12345?mode=osu#top"),
            "12345"
        );
        assert_eq!(username_from_input("  My Name  "), "My Name");
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
            pp: 8556.6,
            accuracy: 98.3758,
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
}
