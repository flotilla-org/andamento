//! Run separately from the independent-core check: this build intentionally
//! depends on Flotilla, while the runtime harness never does.
#[test]
fn checked_in_scenarios_match_the_real_projection() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["rail-scene", "scripted-roll"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_andamento-scenario"))
            .arg(root.join(format!("fixtures/scenarios/{name}.json")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            std::fs::read(root.join(format!("fixtures/{name}.jsonl"))).unwrap(),
            "regenerate {name}"
        );
    }
}
