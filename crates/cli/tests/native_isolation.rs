#![cfg(feature = "native-isolation-validation")]
mod support;
use rune_kit_core::native_isolation::{Policy, Process, spawn_for_validation};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::Command,
    sync::mpsc,
    time::Duration,
};

struct Running {
    process: Process,
    replies: mpsc::Receiver<String>,
}
impl Running {
    fn start(root: &Path, workspace: &Path) -> Self {
        let binary = root.join(format!(
            "native-isolation-fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
        let output = support::output(
            Command::new("rustc")
                .arg(support::root().join("crates/cli/tests/support/isolation_fixture.rs"))
                .arg("-o")
                .arg(&binary),
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let policy = Policy::new(&binary, workspace).unwrap();
        let params = std::collections::HashMap::from([
            ("LD_PRELOAD".into(), "must-not-load".into()),
            ("RUNE_EXPLICIT_TEST".into(), "present".into()),
        ]);
        let mut process=spawn_for_validation(&policy,&params,&support::binary()).expect("Native isolation backend must launch successfully; unsupported hosts cannot pass validation");
        let reader = process.stdout.take().unwrap();
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self { process, replies }
    }
    fn ask(&mut self, request: &str) -> String {
        let input = self.process.stdin.as_mut().unwrap();
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        self.replies
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|e| panic!("Isolated native fixture stopped during {request}: {e}"))
    }
}
#[test]
fn kernel_boundary_confines_writes_reads_network_and_descendants() {
    let root = tempfile::tempdir().unwrap();
    let canonical_root = root.path().canonicalize().unwrap();
    let workspace = canonical_root.join("workspace with spaces");
    std::fs::create_dir(&workspace).unwrap();
    let outside = canonical_root.join("original.txt");
    std::fs::write(&outside, b"original").unwrap();
    let private = canonical_root.join("private-backup.txt");
    std::fs::write(&private, b"private").unwrap();
    let mut running = Running::start(&canonical_root, &workspace);
    if let Some(profile) = running.process.auxiliary_storage().map(Path::to_path_buf) {
        assert_eq!(
            running.ask(&format!(
                "write:{}",
                profile.join("must-not-write.txt").display()
            )),
            "denied",
            "Implicit AppContainer storage must not permit writes outside the managed workspace"
        );
    }
    let inside = workspace.join("inside.txt");
    assert_eq!(
        running.ask(&format!("write:{}", inside.display())),
        "allowed"
    );
    assert_eq!(
        running.ask(&format!("read:{}", inside.display())),
        "allowed"
    );
    for path in [&outside, &private] {
        assert_eq!(running.ask(&format!("read:{}", path.display())), "denied");
        assert_eq!(running.ask(&format!("write:{}", path.display())), "denied");
        assert_eq!(running.ask(&format!("delete:{}", path.display())), "denied");
    }
    assert_eq!(
        running.ask(&format!(
            "rename:{}|{}",
            inside.display(),
            outside.display()
        )),
        "denied"
    );
    let descendant = running.ask(&format!("child:{}", outside.display()));
    #[cfg(windows)]
    assert_eq!(
        descendant, "child-failed",
        "Windows native isolation must block subprocess creation"
    );
    #[cfg(not(windows))]
    assert_eq!(
        descendant, "denied",
        "Descendant must start and inherit restrictions"
    );
    assert_eq!(running.ask("env:LD_PRELOAD"), "absent");
    assert_eq!(running.ask("env:RUNE_EXPLICIT_TEST"), "present");
    assert_eq!(std::fs::read(outside).unwrap(), b"original");
    assert_eq!(std::fs::read(private).unwrap(), b"private");
    let moved = workspace.join("moved.txt");
    assert_eq!(
        running.ask(&format!("rename:{}|{}", inside.display(), moved.display())),
        "allowed"
    );
    assert_eq!(
        running.ask(&format!("delete:{}", moved.display())),
        "allowed"
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        running.ask(&format!("network:{}", listener.local_addr().unwrap())),
        "attempting-network"
    );
    match running.replies.recv_timeout(Duration::from_secs(5)) {
        Ok(result) => assert_eq!(result, "denied"),
        // Windows socket-provider initialization can fail when its implicit
        // profile writes are denied. Termination is also a closed boundary.
        #[cfg(windows)]
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        other => panic!("Network denial neither completed nor stopped the process: {other:?}"),
    }
    assert!(
        matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock),
        "Native network connection escaped confinement"
    );
}

#[test]
fn separate_native_identities_cannot_read_other_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let canonical_root = root.path().canonicalize().unwrap();
    let first = canonical_root.join("first");
    let second = canonical_root.join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let secret = first.join("secret.txt");
    std::fs::write(&secret, b"first-only").unwrap();
    {
        let mut one = Running::start(&canonical_root, &first);
        assert_eq!(one.ask(&format!("read:{}", secret.display())), "allowed");
    }
    let mut two = Running::start(&canonical_root, &second);
    assert_eq!(two.ask(&format!("read:{}", secret.display())), "denied");
}

#[test]
fn invalid_policy_never_launches_a_target() {
    let root = tempfile::tempdir().unwrap();
    let canonical_root = root.path().canonicalize().unwrap();
    assert!(Policy::new(&canonical_root.join("missing"), &canonical_root).is_err());
    let binary = canonical_root.join("executable");
    std::fs::write(&binary, b"fixture").unwrap();
    assert!(Policy::new(&binary, &canonical_root).is_err());
}

#[test]
fn link_inside_workspace_does_not_grant_access_to_its_external_target() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let workspace = root_path.join("workspace");
    let external = root_path.join("external");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("original.txt"), b"original").unwrap();
    let mut running = Running::start(&root_path, &workspace);
    let link = workspace.join("external-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = support::output(
            Command::new("cmd.exe")
                .creation_flags(0x08000000)
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(&external),
        );
        assert!(
            output.status.success(),
            "Cannot create junction fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        running.ask(&format!("read:{}", link.join("original.txt").display())),
        "denied"
    );
    assert_eq!(
        running.ask(&format!("write:{}", link.join("new.txt").display())),
        "denied"
    );
    assert!(!external.join("new.txt").exists());
    assert_eq!(
        std::fs::read(external.join("original.txt")).unwrap(),
        b"original"
    );
}

#[test]
fn dropping_native_process_stops_background_work() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let workspace = root_path.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let destination = workspace.join("late.txt");
    let mut running = Running::start(&root_path, &workspace);
    let response = running.ask(&format!("background:{}", destination.display()));
    #[cfg(windows)]
    assert_eq!(
        response, "child-failed",
        "The Job Object must prohibit subprocesses"
    );
    #[cfg(not(windows))]
    assert_eq!(response, "spawned");
    drop(running);
    std::thread::sleep(Duration::from_millis(1800));
    assert!(
        !destination.exists(),
        "Background child survived the native lifecycle boundary"
    );
}
