//! Integration tests for `ezlint check`: config discovery, language
//! filtering, `--language` override, and exit codes.

use std::path::Path;
use std::process::Command;

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Sets up:
/// ```text
/// project/.ezlint.yaml          languages: [markdown], rule no-sudo
/// project/notes.md              sudo          -> flagged (md -> markdown)
/// project/script.sh             sudo          -> NOT flagged (sh not in languages)
/// ```
fn project(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ezlint-check-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".ezlint.yaml"),
        "\
version: 1
languages: [markdown]
rules:
  - id: no-sudo
    pattern: '\\bsudo\\b'
    message: 'no sudo in docs'
    severity: error
",
    );
    write(&dir.join("notes.md"), "run sudo here\n");
    write(&dir.join("script.sh"), "run sudo here\n");
    dir
}

fn run_check(dir: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_ezlint"))
        .arg("check")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn ezlint");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

#[test]
fn check_discovers_config_and_infers_language() {
    let dir = project("discover");
    // No --config: discovery finds .ezlint.yaml. No --language: .md
    // infers markdown, which the config targets; .sh does not.
    let (stdout, _stderr, code) = run_check(&dir, &["notes.md", "script.sh"]);
    assert_eq!(code, Some(1), "stdout: {stdout}");
    assert!(stdout.contains("notes.md:1:5: error[no-sudo]"), "{stdout}");
    assert!(!stdout.contains("script.sh"), "sh is filtered out: {stdout}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn language_flag_overrides_inference() {
    let dir = project("flag");
    // Asking for markdown on the .sh file flags it, because the flag
    // wins over the .sh extension.
    let (stdout, _stderr, code) = run_check(&dir, &["--language", "markdown", "script.sh"]);
    assert_eq!(code, Some(1), "stdout: {stdout}");
    assert!(stdout.contains("script.sh:1:5: error[no-sudo]"), "{stdout}");

    // A language no config targets: nothing runs, exit 0.
    let (stdout, _stderr, code) = run_check(&dir, &["--language", "rust", "notes.md"]);
    assert_eq!(code, Some(0), "stdout: {stdout}");
    assert!(stdout.is_empty(), "{stdout}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn explicit_config_directory_and_clean_exit() {
    let dir = project("dir");
    // Move the hidden file into a .ezlint/ directory config.
    let ezlint_dir = dir.join(".ezlint");
    std::fs::create_dir_all(&ezlint_dir).unwrap();
    std::fs::rename(
        dir.join(".ezlint.yaml"),
        ezlint_dir.join("docs.yaml"),
    )
    .unwrap();

    // Explicit --config pointing at the directory.
    let (stdout, _stderr, code) =
        run_check(&dir, &["--config", ".ezlint", "notes.md"]);
    assert_eq!(code, Some(1), "stdout: {stdout}");
    assert!(stdout.contains("notes.md:1:5: error[no-sudo]"), "{stdout}");

    // A clean file: no violations, exit 0.
    write(&dir.join("clean.md"), "all good\n");
    let (stdout, stderr, code) = run_check(&dir, &["--config", ".ezlint", "clean.md"]);
    assert_eq!(code, Some(0));
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0 violation") || stderr.is_empty(), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn no_config_found_exits_two() {
    let dir = std::env::temp_dir().join(format!("ezlint-check-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write(&dir.join("x.md"), "sudo\n");
    let (_stdout, stderr, code) = run_check(&dir, &["x.md"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("no ezlint config found"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_runs_lua_callbacks() {
    let dir = std::env::temp_dir().join(format!("ezlint-check-cb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".ezlint.yaml"),
        "\
version: 1
rules:
  - id: todo-ticket
    pattern: 'TODO:[ \t]*(?<ticket>\\S*)'
    callback: |
      return function(c)
        if c.captures.ticket == \"\" then
          return { severity = \"warning\", message = \"TODO without a ticket (\" .. c.path .. \":\" .. c.line .. \")\" }
        end
        return nil
      end
",
    );
    write(&dir.join("a.md"), "TODO:\nTODO: EZ-123\n");

    // No --config: discovery. The TODO with a ticket is allowed; the
    // bare TODO is flagged with the callback's message (path + line).
    let (stdout, _stderr, code) = run_check(&dir, &["a.md"]);
    assert_eq!(code, Some(1), "{stdout}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(
        stdout.contains("a.md:1:1: warning[todo-ticket]: TODO without a ticket (a.md:1)"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
