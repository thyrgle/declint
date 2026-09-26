//! Integration tests for `declint check`: config discovery, language
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
/// project/.declint.yaml          languages: [markdown], rule no-sudo
/// project/notes.md              sudo          -> flagged (md -> markdown)
/// project/script.sh             sudo          -> NOT flagged (sh not in languages)
/// ```
fn project(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("declint-check-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
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
    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .arg("check")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn declint");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

#[test]
fn check_discovers_config_and_infers_language() {
    let dir = project("discover");
    // No --config: discovery finds .declint.yaml. No --language: .md
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
    // Move the hidden file into a .declint/ directory config.
    let declint_dir = dir.join(".declint");
    std::fs::create_dir_all(&declint_dir).unwrap();
    std::fs::rename(
        dir.join(".declint.yaml"),
        declint_dir.join("docs.yaml"),
    )
    .unwrap();

    // Explicit --config pointing at the directory.
    let (stdout, _stderr, code) =
        run_check(&dir, &["--config", ".declint", "notes.md"]);
    assert_eq!(code, Some(1), "stdout: {stdout}");
    assert!(stdout.contains("notes.md:1:5: error[no-sudo]"), "{stdout}");

    // A clean file: no violations, exit 0.
    write(&dir.join("clean.md"), "all good\n");
    let (stdout, stderr, code) = run_check(&dir, &["--config", ".declint", "clean.md"]);
    assert_eq!(code, Some(0));
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0 violation") || stderr.is_empty(), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn no_config_found_exits_two() {
    let dir = std::env::temp_dir().join(format!("declint-check-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write(&dir.join("x.md"), "sudo\n");
    let (_stdout, stderr, code) = run_check(&dir, &["x.md"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("no declint config found"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn check_runs_lua_callbacks() {
    let dir = std::env::temp_dir().join(format!("declint-check-cb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
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

#[test]
fn github_format_emits_workflow_commands() {
    let dir = std::env::temp_dir().join(format!("ezlint-gh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nrules:\n  - id: no-sudo\n    pattern: '\\bsudo\\b'\n    message: 'no sudo: 100% bad'\n    severity: error\n",
    );
    write(&dir.join("a.ini"), "run sudo here\n");

    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["check", "--format", "github", "a.ini"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(
        stdout.starts_with("::error file=a.ini,line=1,col=5,endLine=1::[declint/no-sudo] no sudo: 100%25 bad"),
        "{stdout}"
    );

    // A clean file in the same setup: no annotations, exit 0.
    write(&dir.join("clean.ini"), "all good\n");
    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["check", "--format", "github", "clean.ini"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn directory_arguments_are_walked() {
    let dir = std::env::temp_dir().join(format!("ezlint-walk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nrules:\n  - id: no-sudo\n    pattern: '\\bsudo\\b'\n    message: 'no sudo'\n    severity: error\n",
    );
    write(&dir.join("top.ini"), "sudo\n");
    write(&dir.join("nested").join("deep.ini"), "sudo\n");
    // Hidden directories are skipped...
    write(&dir.join(".hidden").join("h.ini"), "sudo\n");
    // ...and so are files the walker's gitignore logic excludes. The
    // walker applies .gitignore inside git repositories (require_git,
    // like git itself), so the fixture needs a .git directory.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    write(&dir.join(".gitignore"), "skipped.ini\n");
    write(&dir.join("skipped.ini"), "sudo\n");
    // Non-UTF-8 files are skipped silently, not a hard error.
    std::fs::write(dir.join("bin.ini"), b"sudo \xff\xfe\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["check", "."])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert_eq!(stdout.lines().count(), 2, "{stdout}");
    assert!(stdout.contains("top.ini:1:1:"), "{stdout}");
    assert!(stdout.contains("nested/deep.ini:1:1:"), "{stdout}");
    assert!(!stdout.contains("skipped.ini"), "{stdout}");
    assert!(!stdout.contains("h.ini"), "{stdout}");
    assert!(!stdout.contains("bin.ini"), "{stdout}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn explicit_missing_file_still_fails_hard() {
    let dir = std::env::temp_dir().join(format!("ezlint-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    let (_stdout, stderr, code) = {
        let output = Command::new(env!("CARGO_BIN_EXE_declint"))
            .args(["check", "does-not-exist.ini"])
            .current_dir(&dir)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            output.status.code(),
        )
    };
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("cannot read"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn presets_command_lists_and_shows() {
    let dir = std::env::temp_dir().join(format!("ezlint-ps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["presets"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("python"), "{stdout}");
    assert!(stdout.contains("ini"), "{stdout}");
    assert!(stdout.contains("markdown"), "{stdout}");

    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["presets", "ini"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(stdout.contains("empty-value"), "{stdout}");

    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["presets", "nope"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn init_scaffolds_and_refuses_overwrite() {
    let dir = std::env::temp_dir().join(format!("ezlint-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["init", "--lang", "python"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let config = std::fs::read_to_string(dir.join(".declint.yaml")).unwrap();
    assert!(config.contains("languages: [python]"), "{config}");
    assert!(config.contains("preset:python"), "{config}");

    // The scaffold lints: a violating file is flagged through the import.
    std::fs::write(dir.join("x.py"), "\tsoft tab\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["check", "x.py"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));

    // Second init refuses to clobber.
    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["init", "--lang", "ini"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let config = std::fs::read_to_string(dir.join(".declint.yaml")).unwrap();
    assert!(config.contains("preset:python"), "unchanged: {config}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lua_print_goes_to_stderr_not_the_output_stream() {
    let dir = std::env::temp_dir().join(format!("ezlint-print-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nrules:\n  - id: dbg\n    pattern: 'X'\n    message: hit\n    callback: |\n      return function(c)\n        print(\"DEBUG from lua callback\")\n        return nil\n      end\n",
    );
    write(&dir.join("f.txt"), "XX\n");

    // In --format github mode stdout is the annotation stream (in serve
    // mode it would be the JSON-RPC channel): a callback's print() must
    // never touch it.
    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["check", "--format", "github", "f.txt"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!stdout.contains("DEBUG"), "stdout: {stdout}");
    assert!(stderr.contains("DEBUG from lua callback"), "{stderr}");
    assert!(stdout.is_empty(), "the callback allows everything: {stdout}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fail_on_threshold_controls_the_exit_code_but_not_the_output() {
    let dir = std::env::temp_dir().join(format!("ezlint-failon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nrules:\n  - id: hint-rule\n    pattern: 'zzz'\n    message: h\n    severity: hint\n  - id: err-rule\n    pattern: 'XXX'\n    message: e\n    severity: error\n",
    );
    write(&dir.join("f.ini"), "zzz\nXXX\n");

    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_declint"))
            .arg("check")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };

    // Default: any violation fails.
    let (code, stdout, stderr) = run(&["f.ini"]);
    assert_eq!(code, Some(1));
    assert_eq!(stdout.lines().count(), 2);
    assert!(stderr.contains("2 violation(s)"), "{stderr}");

    // --fail-on error: both reported, only the error fails.
    let (code, stdout, stderr) = run(&["--fail-on", "error", "f.ini"]);
    assert_eq!(code, Some(1));
    assert_eq!(stdout.lines().count(), 2, "output unchanged: {stdout}");
    assert!(stderr.contains("1 failing violation(s) of 2"), "{stderr}");

    // Only-hint file with --fail-on error: reported, exit 0.
    write(&dir.join("g.ini"), "zzz\n");
    let (code, stdout, stderr) = run(&["--fail-on", "error", "g.ini"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("hint-rule"), "{stdout}");
    assert!(stderr.contains("all below the --fail-on threshold"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn gh_imports_in_configs_are_rejected_with_an_install_hint() {
    let dir = std::env::temp_dir().join(format!("ezlint-ghimp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - gh:thyrgle/rules\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_declint"))
        .args(["check", "x.txt"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr.contains("gh: sources are installed, not imported"),
        "{stderr}"
    );
    assert!(stderr.contains("declint install"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}
