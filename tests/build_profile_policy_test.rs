#[test]
fn personal_profiles_disable_large_debug_artifacts() {
    let cargo = std::fs::read_to_string("Cargo.toml").unwrap();
    for name in ["dev", "test"] {
        let header = format!("[profile.{name}]");
        let start = cargo
            .find(&header)
            .unwrap_or_else(|| panic!("missing {header}"));
        let section = &cargo[start + header.len()..];
        let end = section.find("\n[").unwrap_or(section.len());
        let section = &section[..end];
        assert!(
            section.lines().any(|line| line.trim() == "debug = 0"),
            "{name}"
        );
        assert!(
            section
                .lines()
                .any(|line| line.trim() == "incremental = false"),
            "{name}"
        );
        assert!(
            section
                .lines()
                .any(|line| line.trim() == "split-debuginfo = \"off\""),
            "{name}"
        );
    }
}
