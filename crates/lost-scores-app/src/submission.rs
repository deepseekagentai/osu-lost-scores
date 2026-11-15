use anyhow::Result;
use lost_scores_core::pp::FORMULA;
use serde::Serialize;

use crate::history::SavedScan;

#[derive(Serialize)]
struct Body<'a> {
    app: &'a str,
    formula: &'a str,
    scan: &'a SavedScan,
}

pub fn body(scan: &SavedScan) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&Body {
        app: env!("CARGO_PKG_VERSION"),
        formula: FORMULA,
        scan,
    })?)
}

#[cfg(test)]
mod tests {
    use lost_scores_core::pp::FORMULA;

    use super::body;
    use crate::history::{SavedScan, VERSION};

    #[test]
    fn the_body_carries_the_scan_and_both_versions() {
        let scan = SavedScan {
            version: VERSION,
            made_at: 1_800_000_000,
            player_id: 7,
            player_name: "Lemon".into(),
            include_all: false,
            check_missing: true,
            lost: Vec::new(),
            lost_total: 3,
            lost_ranked: 2,
            potential: None,
        };
        let sent: serde_json::Value = serde_json::from_slice(&body(&scan).unwrap()).unwrap();
        assert_eq!(FORMULA, "2026.1026.0");
        assert_eq!(sent["app"], env!("CARGO_PKG_VERSION"));
        assert_eq!(sent["formula"], FORMULA);
        assert_eq!(sent["scan"], serde_json::to_value(&scan).unwrap());
        assert_eq!(sent.as_object().unwrap().len(), 3);
    }
}
