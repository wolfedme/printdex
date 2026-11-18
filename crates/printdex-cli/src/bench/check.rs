//! Compares a bench report with `bench/baseline.json`: hard budgets (the
//! product's performance targets) and a regression check against the
//! last recorded baseline.

use serde::{Deserialize, Serialize};

use super::run::BenchReport;

/// `bench/baseline.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Baseline {
    /// Upper limits that must always hold.
    pub budgets: Budgets,
    /// A slower result than `baseline` by more than this share fails
    /// (0.2 = 20 %).
    pub tolerance: f64,
    /// Differences under these never count, so tiny numbers don't flap.
    pub slack_ms: u64,
    pub slack_us: u64,
    /// The recorded run to compare against, if any.
    pub baseline: Option<BenchReport>,
    /// When false, regressions against `baseline` are reported but don't
    /// fail (budgets always fail).
    pub enforce_regressions: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Budgets {
    pub start_to_usable_ms: u64,
    pub search_p95_us: u64,
    pub subtree_rescan_ms: u64,
    pub rescan_fingerprinted: u64,
    pub idle_cpu_pct: f64,
    pub rss_mb: u64,
}

/// One line of the comparison.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub metric: &'static str,
    pub message: String,
    /// Whether this finding fails the check.
    pub fails: bool,
}

/// Every budget missed and every regression past the tolerance.
pub fn compare(report: &BenchReport, b: &Baseline) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut budget = |metric: &'static str, value: f64, limit: f64, unit: &str| {
        if value > limit {
            out.push(Finding {
                metric,
                message: format!("{value}{unit} is over the budget of {limit}{unit}"),
                fails: true,
            });
        }
    };
    let bu = &b.budgets;
    budget(
        "start_to_usable_ms",
        report.start_to_usable_ms as f64,
        bu.start_to_usable_ms as f64,
        " ms",
    );
    budget(
        "search_p95_us",
        report.search_p95_us as f64,
        bu.search_p95_us as f64,
        " µs",
    );
    budget(
        "subtree_rescan_ms",
        report.subtree_rescan_ms as f64,
        bu.subtree_rescan_ms as f64,
        " ms",
    );
    budget(
        "rescan_fingerprinted",
        report.rescan_fingerprinted as f64,
        bu.rescan_fingerprinted as f64,
        " files",
    );
    budget("idle_cpu_pct", report.idle_cpu_pct, bu.idle_cpu_pct, " %");
    budget("rss_mb", report.rss_mb as f64, bu.rss_mb as f64, " MB");

    let Some(base) = &b.baseline else {
        return out;
    };
    let timings: [(&'static str, u64, u64, u64); 6] = [
        (
            "initial_scan_ms",
            report.initial_scan_ms,
            base.initial_scan_ms,
            b.slack_ms,
        ),
        ("rescan_ms", report.rescan_ms, base.rescan_ms, b.slack_ms),
        (
            "subtree_rescan_ms",
            report.subtree_rescan_ms,
            base.subtree_rescan_ms,
            b.slack_ms,
        ),
        (
            "start_to_usable_ms",
            report.start_to_usable_ms,
            base.start_to_usable_ms,
            b.slack_ms,
        ),
        (
            "search_p50_us",
            report.search_p50_us,
            base.search_p50_us,
            b.slack_us,
        ),
        (
            "search_p95_us",
            report.search_p95_us,
            base.search_p95_us,
            b.slack_us,
        ),
    ];
    for (metric, now, was, slack) in timings {
        let limit = (was as f64 * (1.0 + b.tolerance)).max((was + slack) as f64);
        if now as f64 > limit {
            out.push(Finding {
                metric,
                message: format!(
                    "{now} is {:.0} % slower than the baseline {was}",
                    (now as f64 / was.max(1) as f64 - 1.0) * 100.0
                ),
                fails: b.enforce_regressions,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline(base: Option<BenchReport>, enforce: bool) -> Baseline {
        Baseline {
            budgets: Budgets {
                start_to_usable_ms: 1000,
                search_p95_us: 50_000,
                subtree_rescan_ms: 200,
                rescan_fingerprinted: 0,
                idle_cpu_pct: 1.0,
                rss_mb: 250,
            },
            tolerance: 0.2,
            slack_ms: 10,
            slack_us: 2000,
            baseline: base,
            enforce_regressions: enforce,
        }
    }

    fn report() -> BenchReport {
        BenchReport {
            initial_scan_ms: 1000,
            rescan_ms: 300,
            subtree_rescan_ms: 20,
            start_to_usable_ms: 100,
            search_p50_us: 1000,
            search_p95_us: 4000,
            idle_cpu_pct: 0.1,
            rss_mb: 80,
            ..BenchReport::default()
        }
    }

    #[test]
    fn within_budget_and_baseline_passes() {
        assert!(compare(&report(), &baseline(Some(report()), true)).is_empty());
    }

    #[test]
    fn missed_budget_fails() {
        let mut r = report();
        r.rescan_fingerprinted = 3;
        r.search_p95_us = 60_000;
        let f = compare(&r, &baseline(None, true));
        let metrics: Vec<_> = f.iter().map(|f| f.metric).collect();
        assert_eq!(metrics, ["search_p95_us", "rescan_fingerprinted"]);
        assert!(f.iter().all(|f| f.fails));
    }

    #[test]
    fn regression_past_tolerance_fails_only_when_enforced() {
        let mut r = report();
        r.rescan_ms = 400; // +33 %
        r.initial_scan_ms = 1150; // +15 %, within tolerance
        r.subtree_rescan_ms = 29; // +45 %, but under the 10 ms slack
        let f = compare(&r, &baseline(Some(report()), true));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].metric, "rescan_ms");
        assert!(f[0].fails);
        let f = compare(&r, &baseline(Some(report()), false));
        assert_eq!(f.len(), 1);
        assert!(!f[0].fails);
    }

    #[test]
    fn checked_in_baseline_parses() {
        let text = include_str!("../../../../bench/baseline.json");
        let b: Baseline = serde_json::from_str(text).unwrap();
        assert_eq!(b.budgets.rescan_fingerprinted, 0);
    }
}
