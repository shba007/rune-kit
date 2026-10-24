mod support;
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};
use zip::{ZipWriter, write::SimpleFileOptions};

struct Archives {
    _temp: tempfile::TempDir,
    canonical: PathBuf,
    legacy: PathBuf,
    tar: PathBuf,
}
fn archives() -> &'static Archives {
    static ARCHIVES: OnceLock<Archives> = OnceLock::new();
    ARCHIVES.get_or_init(|| {
        let temp = tempfile::tempdir().unwrap();
        let binary = fs::read(support::binary()).unwrap();
        let canonical = temp.path().join("fixture.zip");
        let legacy = temp.path().join("legacy.zip");
        let tar = temp.path().join("fixture.tar.gz");
        for (path, name) in [(&canonical, "rune-kit.exe"), (&legacy, "rune.exe")] {
            let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
            zip.start_file(
                name,
                SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .compression_level(Some(1)),
            )
            .unwrap();
            zip.write_all(&binary).unwrap();
            zip.finish().unwrap();
        }
        let file = fs::File::create(&tar).unwrap();
        let mut archive = tar::Builder::new(GzEncoder::new(file, Compression::fast()));
        let body = format!(
            "#!/bin/sh\nprintf 'rune-kit {}\\n'\n",
            env!("CARGO_PKG_VERSION")
        );
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "rune-kit", body.as_bytes())
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        Archives {
            _temp: temp,
            canonical,
            legacy,
            tar,
        }
    })
}
fn posix() -> Option<PathBuf> {
    if cfg!(windows) {
        let path = PathBuf::from(
            std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:/Program Files".into()),
        )
        .join("Git/bin/bash.exe");
        path.is_file().then_some(path)
    } else {
        Some(PathBuf::from("sh"))
    }
}
fn powershell() -> PathBuf {
    if Command::new("pwsh")
        .args(["-NoProfile", "-Command", "exit 0"])
        .output()
        .is_ok_and(|r| r.status.success())
    {
        "pwsh".into()
    } else {
        "powershell".into()
    }
}
struct Case {
    _temp: tempfile::TempDir,
    root: PathBuf,
    mirror: PathBuf,
    install: PathBuf,
}
impl Case {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let mirror = root.join("release files");
        fs::create_dir_all(&mirror).unwrap();
        let install = root.join("user's bin");
        Self {
            _temp: temp,
            root,
            mirror,
            install,
        }
    }
    fn package(&self, windows: bool, legacy: bool) -> PathBuf {
        let version = env!("CARGO_PKG_VERSION");
        let filename = if windows {
            format!(
                "{}-v{version}-x86_64-pc-windows-msvc.zip",
                if legacy { "rune" } else { "rune-kit" }
            )
        } else {
            format!("rune-kit-v{version}-x86_64-unknown-linux-gnu.tar.gz")
        };
        let path = self.mirror.join(filename);
        fs::copy(
            if windows {
                if legacy {
                    &archives().legacy
                } else {
                    &archives().canonical
                }
            } else {
                &archives().tar
            },
            &path,
        )
        .unwrap();
        let hash: String = Sha256::digest(fs::read(&path).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(
            path.with_extension(format!(
                "{}.sha256",
                path.extension().unwrap().to_str().unwrap()
            )),
            format!("{hash}  {}\n", path.file_name().unwrap().to_str().unwrap()),
        )
        .unwrap();
        path
    }
    fn command(&self, windows: bool, shell: Option<&Path>) -> Command {
        let mut command = if windows {
            let mut cmd = Command::new(shell.map(Path::to_owned).unwrap_or_else(powershell));
            cmd.args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(support::root().join("install.ps1"));
            cmd
        } else {
            let mut cmd = Command::new(posix().unwrap());
            cmd.arg(support::root().join("install.sh"));
            cmd
        };
        command
            .env(
                "RUNE_KIT_VERSION",
                format!("v{}", env!("CARGO_PKG_VERSION")),
            )
            .env(
                "RUNE_KIT_BASE_URL",
                self.mirror.to_str().unwrap().replace('\\', "/"),
            )
            .env(
                "RUNE_KIT_INSTALL_DIR",
                self.install.to_str().unwrap().replace('\\', "/"),
            )
            .env("RUNE_KIT_NO_MODIFY_PATH", "1")
            .env("RUNE_KIT_TARGET", "x86_64-unknown-linux-gnu");
        command
    }
    fn run(&self, windows: bool, success: bool) -> Output {
        let result = support::output(&mut self.command(windows, None));
        assert_eq!(
            result.status.success(),
            success,
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        result
    }
}
fn missing_checksum(windows: bool) {
    let case = Case::new();
    let path = case.package(windows, false);
    fs::remove_file(format!("{}.sha256", path.display())).unwrap();
    case.run(windows, false);
    assert!(!case.install.exists());
}
fn bad_checksum(windows: bool) {
    let case = Case::new();
    let path = case.package(windows, false);
    fs::write(format!("{}.sha256", path.display()), "0".repeat(64)).unwrap();
    fs::create_dir_all(&case.install).unwrap();
    let exe = case
        .install
        .join(if windows { "rune-kit.exe" } else { "rune-kit" });
    fs::write(&exe, b"previous installation").unwrap();
    let result = case.run(windows, false);
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("Checksum mismatch")
            || String::from_utf8_lossy(&result.stdout).contains("Checksum mismatch")
    );
    assert_eq!(fs::read(exe).unwrap(), b"previous installation");
}
#[cfg(windows)]
#[test]
fn windows_install_and_reinstall_canonical_binary() {
    let case = Case::new();
    case.package(true, false);
    case.run(true, true);
    case.run(true, true);
    assert_eq!(
        fs::read(case.install.join("rune-kit.exe")).unwrap(),
        fs::read(support::binary()).unwrap()
    );
    assert_eq!(fs::read_dir(&case.install).unwrap().count(), 1);
}
#[cfg(windows)]
#[test]
fn windows_powershell_51() {
    let case = Case::new();
    case.package(true, false);
    let result = support::output(&mut case.command(true, Some(Path::new("powershell"))));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(case.install.join("rune-kit.exe").is_file());
}
#[cfg(windows)]
#[test]
fn windows_legacy_archive_installs_only_new_command() {
    let case = Case::new();
    case.package(true, true);
    case.run(true, true);
    assert!(case.install.join("rune-kit.exe").is_file());
    assert!(!case.install.join("rune.exe").exists());
}
#[cfg(windows)]
#[test]
fn windows_bad_checksum_preserves_existing_install() {
    bad_checksum(true);
}
#[cfg(windows)]
#[test]
fn windows_missing_checksum_does_not_install() {
    missing_checksum(true);
}
#[test]
fn posix_bad_checksum_preserves_existing_install() {
    if posix().is_some() {
        bad_checksum(false);
    } else {
        eprintln!("SKIP: Git Bash unavailable");
    }
}
#[test]
fn posix_missing_checksum_does_not_install() {
    if posix().is_some() {
        missing_checksum(false);
    } else {
        eprintln!("SKIP: Git Bash unavailable");
    }
}
#[test]
fn posix_install_reinstall_and_quoted_path_hint() {
    let Some(shell) = posix() else {
        eprintln!("SKIP: Git Bash unavailable");
        return;
    };
    let case = Case::new();
    case.package(false, false);
    let result = case.run(false, true);
    case.run(false, true);
    assert!(case.install.join("rune-kit").is_file());
    assert!(!case.install.join("rune").exists());
    let text = String::from_utf8(result.stdout).unwrap();
    let hint = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("export PATH="))
        .unwrap();
    let result = support::output(
        Command::new(shell).args(["-c", &format!("{hint}; printf '%s' \"$PATH\"")]),
    );
    assert!(result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stdout).starts_with(&format!(
            "{}:",
            case.install.to_str().unwrap().replace('\\', "/")
        ))
    );
}
#[test]
fn posix_default_shared_root_and_path_once() {
    let Some(shell) = posix() else {
        eprintln!("SKIP: Git Bash unavailable");
        return;
    };
    let case = Case::new();
    case.package(false, false);
    let home = case.root.join("home with spaces");
    let shared = home.join(".rune");
    fs::create_dir_all(shared.join("infer")).unwrap();
    fs::write(shared.join("rune-infer"), b"infer").unwrap();
    fs::write(shared.join("infer/config"), b"configuration").unwrap();
    let mut command = case.command(false, None);
    command
        .env_remove("RUNE_KIT_INSTALL_DIR")
        .env("HOME", home.to_str().unwrap().replace('\\', "/"))
        .env("RUNE_KIT_NO_MODIFY_PATH", "0");
    for _ in 0..2 {
        let result = support::output(&mut command);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert!(shared.join("rune-kit").is_file());
    assert_eq!(fs::read(shared.join("rune-infer")).unwrap(), b"infer");
    assert_eq!(
        fs::read(shared.join("infer/config")).unwrap(),
        b"configuration"
    );
    assert_eq!(
        fs::read_to_string(home.join(".profile"))
            .unwrap()
            .matches("export PATH=")
            .count(),
        1
    );
    let result = support::output(
        Command::new(&shell)
            .args([
                "-c",
                ". \"$1\"; . \"$1\"; printf '%s' \"$PATH\"",
                "check",
                &home.join(".profile").to_str().unwrap().replace('\\', "/"),
            ])
            .env("HOME", home.to_str().unwrap().replace('\\', "/")),
    );
    assert!(result.status.success());
    let expected = support::output(
        Command::new(shell)
            .args(["-c", "printf '%s' \"$HOME/.rune\""])
            .env("HOME", home.to_str().unwrap().replace('\\', "/")),
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout)
            .matches(String::from_utf8_lossy(&expected.stdout).as_ref())
            .count(),
        1
    );
}

#[test]
#[cfg(windows)]
fn windows_default_hidden_install_deduplicates_path_and_preserves_visible_infer() {
    let case = Case::new();
    case.package(true, false);
    let home = case.root.join("home's spaces");
    let hidden = home.join(".rune");
    let visible = home.join("rune");
    fs::create_dir_all(hidden.join("infer/config")).unwrap();
    fs::create_dir_all(&visible).unwrap();
    fs::write(visible.join("rune-kit.exe"), b"original kit retained").unwrap();
    fs::write(visible.join("rune-infer.exe"), b"visible infer").unwrap();
    fs::write(hidden.join("rune-infer.exe"), b"hidden infer").unwrap();
    let path_record = case.root.join("user-path.txt");
    fs::write(
        &path_record,
        format!(
            "{};{};{};C:\\Windows",
            visible.display(),
            hidden.display(),
            hidden.display()
        ),
    )
    .unwrap();
    let ps_quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "''"));
    // Execute the real installer; isolate only OS profile/registry APIs in a copy.
    let script=fs::read_to_string(support::root().join("install.ps1")).unwrap()
        .replace("[Environment]::GetFolderPath('UserProfile')",&ps_quote(&home))
        .replace("[Environment]::GetEnvironmentVariable('Path', 'User')",&format!("([IO.File]::ReadAllText({}))",ps_quote(&path_record)))
        .replace("[Environment]::SetEnvironmentVariable('Path', ((@($installDir) + $entries) -join ';'), 'User')",&format!("[IO.File]::WriteAllText({}, ((@($installDir) + $entries) -join ';'))",ps_quote(&path_record)));
    assert!(!script.contains("[Environment]::SetEnvironmentVariable('Path'"));
    let copied = case.root.join("isolated installer.ps1");
    fs::write(&copied, script).unwrap();
    for _ in 0..2 {
        let original = case.command(true, None);
        let mut command = Command::new(powershell());
        command
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&copied);
        for (key, value) in original.get_envs() {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        command
            .env_remove("RUNE_KIT_INSTALL_DIR")
            .env("RUNE_KIT_NO_MODIFY_PATH", "0")
            .env("USERPROFILE", &home);
        let result = support::output(&mut command);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert!(hidden.join("rune-kit.exe").is_file());
    assert_eq!(
        fs::read(visible.join("rune-kit.exe")).unwrap(),
        b"original kit retained"
    );
    assert_eq!(
        fs::read(visible.join("rune-infer.exe")).unwrap(),
        b"visible infer"
    );
    assert_eq!(
        fs::read(hidden.join("rune-infer.exe")).unwrap(),
        b"hidden infer"
    );
    let entries = fs::read_to_string(path_record).unwrap();
    // Windows CI may expose TEMP as RUNNER~1 while PowerShell expands it to
    // runneradmin. Compare directory identity, retaining order/dedup assertions.
    let hidden = fs::canonicalize(&hidden).unwrap();
    let visible = fs::canonicalize(&visible).unwrap();
    let paths: Vec<_> = entries.split(';').map(Path::new).collect();
    assert_eq!(fs::canonicalize(paths[0]).unwrap(), hidden);
    assert_eq!(
        paths
            .iter()
            .filter(|path| fs::canonicalize(path).ok().as_ref() == Some(&hidden))
            .count(),
        1
    );
    assert!(
        paths
            .iter()
            .any(|path| fs::canonicalize(path).ok().as_ref() == Some(&visible))
    );
}
