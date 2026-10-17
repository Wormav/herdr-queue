use herdr_queue::models::unique_id;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("herdr-launcher-{}", unique_id()));
        fs::create_dir_all(root.join("tools")).unwrap();
        Self(root)
    }
    fn script(&self, name: &str, content: &str) {
        let path = self.0.join("tools").join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn shell(&self, script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .arg(script)
            .current_dir(&self.0)
            .env_clear()
            .env("HOME", &self.0)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.0.join("tools").display()),
            );
        command
    }
    fn copy_launcher(&self) {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("run.sh"),
            self.0.join("run.sh"),
        )
        .unwrap();
    }
    fn copy_installer(&self) {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"),
            self.0.join("install.sh"),
        )
        .unwrap();
    }
    fn assets(&self, good_hash: bool) {
        let assets = self.0.join("assets");
        let staging = self.0.join("staging");
        fs::create_dir_all(&assets).unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::copy(
            env!("CARGO_BIN_EXE_herdr-queue"),
            staging.join("herdr-queue"),
        )
        .unwrap();
        let name = "herdr-queue-v0.2.0-aarch64-apple-darwin.tar.gz";
        assert!(Command::new("tar")
            .args(["-czf"])
            .arg(assets.join(name))
            .arg("-C")
            .arg(staging)
            .arg("herdr-queue")
            .status()
            .unwrap()
            .success());
        let digest = if good_hash {
            format!("{:x}", Sha256::digest(fs::read(assets.join(name)).unwrap()))
        } else {
            "0".repeat(64)
        };
        fs::write(
            assets.join(format!("{name}.sha256")),
            format!("{digest}  {name}\n"),
        )
        .unwrap();
        self.script(
            "uname",
            "#!/bin/sh\ncase \"$1\" in -s) echo Darwin;; -m) echo arm64;; *) exit 2;; esac\n",
        );
        self.script("curl", "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n case \"$1\" in --output|-o) shift; destination=$1;; https://*) url=$1;; esac\n shift\ndone\ncp \"$HERDR_QUEUE_TEST_ASSETS/${url##*/}\" \"$destination\"\n");
        self.script(
            "cargo",
            "#!/bin/sh\necho 'unexpected compiler invocation' >&2\nexit 99\n",
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_version_and_invalid_context_never_contact_an_agent() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-queue"))
        .arg("--version")
        .env_clear()
        .output()
        .unwrap();
    success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Herdr Queue 0.2.0"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-queue"))
        .arg("open")
        .env_clear()
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("depuis un agent"));
}

#[test]
fn launcher_executes_the_native_binary_without_python_uv_or_cargo() {
    let fixture = Fixture::new();
    fixture.copy_launcher();
    fs::create_dir_all(fixture.0.join("bin")).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_herdr-queue"),
        fixture.0.join("bin/herdr-queue"),
    )
    .unwrap();
    for tool in ["python", "python3", "uv", "cargo"] {
        fixture.script(
            tool,
            "#!/bin/sh\necho 'unexpected runtime invocation' >&2\nexit 99\n",
        );
    }
    let output = fixture.shell("run.sh").arg("--version").output().unwrap();
    success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Herdr Queue 0.2.0"
    );
}

#[test]
fn installer_uses_the_matching_verified_release_without_a_compiler() {
    let fixture = Fixture::new();
    fixture.copy_installer();
    fixture.assets(true);
    let output = fixture
        .shell("install.sh")
        .env("HERDR_QUEUE_TEST_ASSETS", fixture.0.join("assets"))
        .output()
        .unwrap();
    success(&output);
    let output = Command::new(fixture.0.join("bin/herdr-queue"))
        .arg("--version")
        .output()
        .unwrap();
    success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Herdr Queue 0.2.0"
    );
}

#[test]
fn a_wrong_checksum_never_installs_or_runs_a_downloaded_binary() {
    let fixture = Fixture::new();
    fixture.copy_installer();
    fixture.assets(false);
    let output = fixture
        .shell("install.sh")
        .env("HERDR_QUEUE_TEST_ASSETS", fixture.0.join("assets"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.0.join("bin/herdr-queue").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("SHA-256"));
}
