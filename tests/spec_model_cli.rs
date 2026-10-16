use std::process::Command;

#[test]
fn spec_model_binary_reports_every_check_ok() {
    let out = Command::new(env!("CARGO_BIN_EXE_spec_model"))
        .output()
        .expect("run spec_model");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(stdout.matches("[ ok ]").count(), 6, "{stdout}");
    assert!(stdout.contains("all checks passed"), "{stdout}");
    assert!(!stderr.contains("[FAIL]"), "{stderr}");
}
