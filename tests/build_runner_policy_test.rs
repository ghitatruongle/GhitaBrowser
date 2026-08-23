fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn runner_has_three_tiers_without_redundant_all_target_check() {
    let script = read("tools/test.ps1");
    for tier in ["fast", "release", "full"] {
        assert!(script.contains(tier), "missing tier {tier}");
    }
    assert!(!script.contains("cargo check --all-targets"));
    assert_eq!(script.matches("cargo test --all-targets").count(), 1);
    assert!(script.contains("dist\\build-metrics"));
}

#[test]
fn wrappers_and_packaging_reuse_the_central_runner_and_build() {
    let release_gate = read("tools/release-gate.ps1");
    assert!(!release_gate.contains("cargo check --all-targets"));
    assert!(release_gate.contains("test.ps1"));

    let personal = read("tools/personal-release-gate.ps1");
    assert!(personal.contains("test.ps1"));
    assert!(personal.contains("-Tier release"));

    let package = read("packaging/package.ps1");
    assert!(package.contains("SkipBuild"));
}
