use std::process::Command;

fn postbode(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .output()
        .unwrap()
}

#[test]
fn rules_check_reports_bad_file_and_exits_nonzero() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(
        home.path().join("config/rules.toml"),
        "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n",
    )
    .unwrap();
    let out = postbode(home.path(), &["rules", "check"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rule 'x'"), "{stderr}");
}

#[test]
fn rules_check_passes_on_valid_file_and_lists() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(
        home.path().join("config/rules.toml"),
        "[[rules]]\nname = \"ok\"\nmatch.seen = true\nactions = [\"flag\"]\n",
    )
    .unwrap();
    assert!(postbode(home.path(), &["rules", "check"]).status.success());
    let out = postbode(home.path(), &["rules", "list"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
}

#[test]
fn list_on_unknown_account_fails_and_empty_config_lists_nothing() {
    let home = tempfile::tempdir().unwrap();
    let out = postbode(home.path(), &["list", "--account", "nope"]);
    assert!(!out.status.success());
    let out = postbode(home.path(), &["folders"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}
