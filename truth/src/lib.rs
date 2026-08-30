//! Ground-truth assertions shared by pure tests and privileged profiler runs.

/// 01-F6.1's expected instruction attribution, in percentage points.
pub const DUTY_SPLIT_EXPECTED: [f64; 2] = [60.0, 40.0];
/// 01-F6.1's allowed error, in standard errors of the binomial sample split.
///
/// The snapshot scenario samples at 99 Hz, so a 3 s fixture yields roughly
/// 300 samples and a fixed ±3 pp bound sits inside the sampling noise. Three
/// standard errors is about ±8 pp at that size and still rejects a collector
/// that attributes samples 50/50 or swaps the two functions.
pub const DUTY_SPLIT_TOLERANCE_SIGMAS: f64 = 3.0;

/// Validates the normalized instruction attribution for the `duty_split` fixture.
///
/// The milestone name is part of every failure so CI reports point back to the
/// requirement being guarded.
pub fn assert_f6_1_duty_split(duty_60: u64, duty_40: u64) {
    let total = duty_60 + duty_40;
    assert!(
        total > 0,
        "01-F6.1 duty_split: profiler attributed no instructions to either fixture function"
    );

    let actual = [
        duty_60 as f64 * 100.0 / total as f64,
        duty_40 as f64 * 100.0 / total as f64,
    ];
    let tolerance = DUTY_SPLIT_TOLERANCE_SIGMAS * (0.6_f64 * 0.4 / total as f64).sqrt() * 100.0;
    for ((label, actual), expected) in ["duty_60", "duty_40"]
        .into_iter()
        .zip(actual)
        .zip(DUTY_SPLIT_EXPECTED)
    {
        assert!(
            (actual - expected).abs() <= tolerance,
            "01-F6.1 duty_split: {label} attribution was {actual:.2}%, expected {expected:.2}% ± {tolerance:.2} percentage points ({total} samples)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f6_1_accepts_analytic_duty_split() {
        assert_f6_1_duty_split(600, 400);
    }

    #[test]
    #[should_panic(expected = "01-F6.1 duty_split: duty_60 attribution")]
    fn f6_1_mutation_swapped_attribution_fails() {
        // Mutation evidence: a collector that assigns each sample to the wrong
        // function must make the truth suite red.
        assert_f6_1_duty_split(400, 600);
    }

    #[test]
    #[should_panic(expected = "01-F6.1 duty_split: duty_60 attribution")]
    fn f6_1_mutation_even_attribution_fails() {
        assert_f6_1_duty_split(150, 150);
    }
}
