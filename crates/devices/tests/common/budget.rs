//! Whether a device's speed budget is judged on this machine.
//!
//! GitHub's AMD EPYC 7763 runners (Zen 3) run Wavetable's eight copies and
//! Meld's Swarm Saw short of their budgets in a debug build, while every other
//! CI processor passes (beatlayer#801). Until that gap is closed, the budget
//! tests there print their speed and don't fail (a maintainer decision, 3 November 2026).

/// Whether this processor is the EPYC 7763 the budgets aren't judged on.
fn zen3_runner() -> bool {
    // Linux names the model; Windows gives family 25 (19h) model 1 (Milan).
    let linux =
        std::fs::read_to_string("/proc/cpuinfo").is_ok_and(|info| info.contains("AMD EPYC 7763"));
    let windows = std::env::var("PROCESSOR_IDENTIFIER")
        .is_ok_and(|id| id.contains("AuthenticAMD") && id.contains("Family 25 Model 1 "));
    linux || windows
}

/// Whether to assert the speed budget here. On a Zen 3 runner it says why not.
pub fn judged() -> bool {
    if zen3_runner() {
        println!("budget not judged: AMD EPYC 7763 (Zen 3) runner, report only");
        return false;
    }
    true
}
