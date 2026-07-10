use std::process::Command;

const DUTY_SPLIT_FIXTURES: [&str; 2] =
    [env!("TRUTH_DUTY_SPLIT_FP"), env!("TRUTH_DUTY_SPLIT_NO_FP")];
const KNOWN_SLEEPER_FIXTURES: [&str; 2] = [
    env!("TRUTH_KNOWN_SLEEPER_FP"),
    env!("TRUTH_KNOWN_SLEEPER_NO_FP"),
];
const TMA_FIXTURES: [&str; 2] = [
    env!("TRUTH_POINTER_CHASE_FP"),
    env!("TRUTH_BRANCH_HEAVY_FP"),
];

#[test]
fn f6_1_fixture_variants_are_executable() {
    for fixture in DUTY_SPLIT_FIXTURES {
        let status = Command::new(fixture)
            .arg("0.01")
            .status()
            .unwrap_or_else(|error| panic!("01-F6.1: failed to run {fixture}: {error}"));
        assert!(
            status.success(),
            "01-F6.1: controlled fixture {fixture} failed"
        );
    }

    // This fixture is activated as a database truth assertion by 08-M1. Until
    // then this smoke test keeps both controlled build variants viable.
    for fixture in KNOWN_SLEEPER_FIXTURES {
        let status = Command::new(fixture).status().unwrap_or_else(|error| {
            panic!("08-M1 known_sleeper: failed to run {fixture}: {error}")
        });
        assert!(
            status.success(),
            "08-M1 known_sleeper: controlled fixture {fixture} failed"
        );
    }
    for fixture in TMA_FIXTURES {
        assert!(
            Command::new(fixture).status().unwrap().success(),
            "TMA fixture {fixture} failed"
        );
    }
}
