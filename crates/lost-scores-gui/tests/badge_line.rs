use lost_scores_gui::ShareBadge;
use lost_scores_gui::share::render::use_offscreen_platform;

#[test]
fn the_summary_line_fits_with_the_longest_values() {
    use_offscreen_platform().unwrap();
    let badge = ShareBadge::new().unwrap();
    badge.set_rank_date("#123,456 \u{b7} 17 Sep 2026".into());
    for lost in [
        "1239 lost plays on ranked maps",
        "1339 lost plays, 1239 ranked",
    ] {
        badge.set_lost(lost.into());
        assert!(badge.get_lost_fits(), "{lost}");
    }
}
