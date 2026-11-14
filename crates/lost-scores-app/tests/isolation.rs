#[allow(dead_code)]
mod common;

#[test]
fn the_temporary_home_goes_away_with_its_last_user() {
    let first = common::isolate_environment();
    let home = common::isolated_home();
    assert!(home.is_dir());
    let second = common::isolate_environment();
    drop(first);
    assert!(common::isolated_home().is_dir());
    drop(second);
    assert!(!home.exists(), "{}", home.display());
}
