use std::env;
use std::fs;
use std::path::PathBuf;

use lost_scores_core::potential::{Origin, Play, potential};
use serde_json::Value;

fn plays(rows: &Value) -> Vec<Play> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| Play {
            beatmap_id: u32::try_from(row["beatmap_id"].as_u64().unwrap()).unwrap(),
            pp: row["pp"].as_f64().unwrap(),
            accuracy: row["accuracy"].as_f64().unwrap(),
        })
        .collect()
}

#[test]
#[ignore = "needs LOST_SCORES_PARITY_DIR with the old app's analysis_results.json files"]
fn matches_the_old_apps_potential_top() {
    let dir = PathBuf::from(env::var("LOST_SCORES_PARITY_DIR").unwrap());
    let mut runs: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("analysis_results.json"))
        .filter(|path| path.is_file())
        .collect();
    runs.sort();
    assert!(!runs.is_empty());
    for path in runs {
        let run = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        let data: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let summary = &data["summary_stats"];
        let result = potential(
            &plays(&data["parsed_top"]),
            &plays(&data["lost_scores"]),
            summary["current_pp"].as_f64().unwrap(),
        );
        let expected = data["top_with_lost"].as_array().unwrap();
        let ids: Vec<u64> = result
            .top
            .iter()
            .map(|ranked| u64::from(ranked.play.beatmap_id))
            .collect();
        let expected_ids: Vec<u64> = expected
            .iter()
            .map(|row| row["beatmap_id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, expected_ids, "{run}");
        let lost: Vec<bool> = result
            .top
            .iter()
            .map(|ranked| matches!(ranked.origin, Origin::Lost(_)))
            .collect();
        let expected_lost: Vec<bool> = expected
            .iter()
            .map(|row| row["score_id"] == "LOST")
            .collect();
        assert_eq!(lost, expected_lost, "{run}");
        let pp_gap = result.pp - summary["potential_pp"].as_f64().unwrap();
        let accuracy_gap = result.accuracy - summary["potential_acc"].as_f64().unwrap();
        println!("{run}: pp {pp_gap:+.4}, accuracy {accuracy_gap:+.2e}");
        assert!(pp_gap.abs() < 0.5, "{run}: pp {pp_gap}");
        assert!(accuracy_gap.abs() < 1e-6, "{run}: accuracy {accuracy_gap}");
    }
}
