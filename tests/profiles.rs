#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use assert_cmd::cargo::CommandCargoExt;
use tempfile::TempDir;

#[test]
fn users_can_add_list_and_remove_account_profiles_without_modifying_codex_homes() {
    let fixture = ProfileFixture::new();
    let first_home = fixture.root.path().join("first-codex-home");
    let second_home = fixture.root.path().join("second-codex-home");
    fs::create_dir(&first_home).expect("create first Codex Home");
    fs::create_dir(&second_home).expect("create second Codex Home");
    let credential_marker = first_home.join("auth.json");
    fs::write(&credential_marker, "owned by Codex").expect("write credential marker");

    fixture.success(&["profile", "add", "personal", path_text(&first_home)]);
    fixture.success(&["profile", "add", "work", path_text(&second_home)]);

    let listed = fixture.success(&["profile", "list"]);
    assert_eq!(
        stdout(listed),
        format!(
            "personal\t{}\nwork\t{}\n",
            first_home.display(),
            second_home.display()
        )
    );
    assert_eq!(
        fs::read_to_string(fixture.config_file()).expect("read Limitr configuration"),
        format!(
            "\
[[profiles]]
label = \"personal\"
codex_home = \"{}\"

[[profiles]]
label = \"work\"
codex_home = \"{}\"
",
            first_home.display(),
            second_home.display()
        )
    );

    fixture.success(&["profile", "remove", "personal"]);

    assert_eq!(
        stdout(fixture.success(&["profile", "list"])),
        format!("work\t{}\n", second_home.display())
    );
    assert_eq!(
        fs::read_to_string(&credential_marker).expect("Codex Home remains untouched"),
        "owned by Codex"
    );
}

#[test]
fn profile_labels_must_be_unique() {
    let fixture = ProfileFixture::new();
    let first_home = fixture.root.path().join("first-codex-home");
    let second_home = fixture.root.path().join("second-codex-home");

    fixture.success(&["profile", "add", "work", path_text(&first_home)]);
    let output = fixture
        .command()
        .args(["profile", "add", "work", path_text(&second_home)])
        .output()
        .expect("run limitr");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 error output"),
        "limitr: an Account Profile labelled `work` is already configured\n"
    );
    assert_eq!(
        stdout(fixture.success(&["profile", "list"])),
        format!("work\t{}\n", first_home.display())
    );
}

#[test]
fn profile_codex_home_must_be_absolute() {
    let fixture = ProfileFixture::new();

    let output = fixture
        .command()
        .args(["profile", "add", "work", "relative/codex-home"])
        .output()
        .expect("run limitr");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 error output"),
        "limitr: Codex Home must be an absolute path: relative/codex-home\n"
    );
    assert!(!fixture.config_file().exists());
}

#[test]
fn profile_codex_homes_must_be_distinct() {
    let fixture = ProfileFixture::new();
    let codex_home = fixture.root.path().join("shared-codex-home");

    fixture.success(&["profile", "add", "personal", path_text(&codex_home)]);
    let output = fixture
        .command()
        .args(["profile", "add", "work", path_text(&codex_home)])
        .output()
        .expect("run limitr");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 error output"),
        format!(
            "limitr: Codex Home {} is already used by Account Profile `personal`\n",
            codex_home.display()
        )
    );
}

#[test]
fn human_edited_configuration_must_preserve_account_profile_invariants() {
    for (contents, expected_error) in [
        (
            "\
[[profiles]]
label = \"personal\"
codex_home = \"relative/codex-home\"
",
            "Codex Home must be an absolute path",
        ),
        (
            "\
[[profiles]]
label = \"shared\"
codex_home = \"/first\"

[[profiles]]
label = \"shared\"
codex_home = \"/second\"
",
            "an Account Profile labelled `shared` is already configured",
        ),
        (
            "\
[[profiles]]
label = \"personal\"
codex_home = \"/shared\"

[[profiles]]
label = \"work\"
codex_home = \"/shared\"
",
            "Codex Home /shared is already used by Account Profile `personal`",
        ),
    ] {
        let fixture = ProfileFixture::new();
        let config_file = fixture.config_file();
        fs::create_dir_all(config_file.parent().expect("configuration parent"))
            .expect("create configuration directory");
        fs::write(config_file, contents).expect("write hand-edited configuration");

        let output = fixture
            .command()
            .args(["profile", "list"])
            .output()
            .expect("run limitr");

        assert_eq!(output.status.code(), Some(1));
        assert!(
            String::from_utf8(output.stderr)
                .expect("UTF-8 error output")
                .contains(expected_error)
        );
    }
}

struct ProfileFixture {
    root: TempDir,
}

impl ProfileFixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("create profile fixture"),
        }
    }

    fn config_file(&self) -> std::path::PathBuf {
        self.root
            .path()
            .join("config")
            .join("limitr")
            .join("config.toml")
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("limitr").expect("locate limitr binary");
        command.env("XDG_CONFIG_HOME", self.root.path().join("config"));
        command
    }

    fn success(&self, arguments: &[&str]) -> Output {
        let output = self.command().args(arguments).output().expect("run limitr");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 fixture path")
}

fn stdout(output: Output) -> String {
    String::from_utf8(output.stdout).expect("UTF-8 output")
}
