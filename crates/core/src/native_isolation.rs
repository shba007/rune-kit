//! Native launch boundary. Production enablement requires all three OS checks.
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

// No environment variable or user configuration can bypass this release gate.
pub const VALIDATED_ON_ALL_PLATFORMS: bool = false;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub executable: PathBuf,
    pub workspace: PathBuf,
}
impl Policy {
    pub fn new(executable: &Path, workspace: &Path) -> io::Result<Self> {
        crate::sandbox::check_links(executable)?;
        crate::sandbox::check_links(workspace)?;
        let executable = executable.canonicalize()?;
        let workspace = workspace.canonicalize()?;
        if !executable.is_file()
            || !workspace.is_dir()
            || executable.starts_with(&workspace)
            || workspace.parent().is_none()
            || executable.to_string_lossy().chars().any(char::is_control)
            || workspace.to_string_lossy().chars().any(char::is_control)
        {
            return Err(io::Error::other(
                "Native isolation requires an immutable executable outside an existing workspace",
            ));
        }
        Ok(Self {
            executable,
            workspace,
        })
    }
}

pub struct Process {
    finished: bool,
    pub stdin: Option<Box<dyn Write + Send>>,
    pub stdout: Option<Box<dyn Read + Send>>,
    #[cfg(not(windows))]
    child: std::process::Child,
    #[cfg(windows)]
    child: windows::Child,
}
impl Process {
    #[cfg(feature = "native-isolation-validation")]
    pub fn auxiliary_storage(&self) -> Option<&Path> {
        #[cfg(windows)]
        {
            Some(&self.child.profile_directory)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        #[cfg(unix)]
        {
            if unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            self.child.kill()
        }
    }
    pub fn wait(&mut self) -> io::Result<()> {
        let result = self.child.wait().map(|_| ());
        if result.is_ok() {
            self.finished = true;
        }
        result
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.kill();
        let _ = self.wait();
    }
}

pub fn spawn(policy: &Policy, params: &HashMap<String, String>) -> io::Result<Process> {
    if !VALIDATED_ON_ALL_PLATFORMS {
        return Err(io::Error::other(
            "Native isolation awaits Windows, macOS and Linux validation; no process was launched",
        ));
    }
    candidate(policy, params, None)
}
#[cfg(feature = "native-isolation-validation")]
pub fn spawn_for_validation(
    policy: &Policy,
    params: &HashMap<String, String>,
    worker: &Path,
) -> io::Result<Process> {
    candidate(policy, params, Some(worker))
}

fn candidate(
    policy: &Policy,
    params: &HashMap<String, String>,
    worker: Option<&Path>,
) -> io::Result<Process> {
    let policy = Policy::new(&policy.executable, &policy.workspace)?;
    // Only explicit plugin parameters cross the boundary; host credentials,
    // dynamic-loader overrides and startup hooks never do.
    let mut clean = HashMap::new();
    for (key, value) in params {
        let upper = key.to_ascii_uppercase();
        if upper.starts_with("LD_")
            || upper.starts_with("DYLD_")
            || matches!(
                upper.as_str(),
                "PATH"
                    | "HOME"
                    | "USERPROFILE"
                    | "SYSTEMROOT"
                    | "COMSPEC"
                    | "BASH_ENV"
                    | "ENV"
                    | "NODE_OPTIONS"
                    | "PYTHONPATH"
                    | "PYTHONHOME"
                    | "RUSTC_WRAPPER"
            )
        {
            continue;
        }
        if key.contains(['\0', '=']) || value.contains('\0') {
            return Err(io::Error::other("Invalid native environment parameter"));
        }
        clean.insert(key.clone(), value.clone());
    }
    clean.insert("HOME".into(), policy.workspace.to_string_lossy().into());
    clean.insert(
        "USERPROFILE".into(),
        policy.workspace.to_string_lossy().into(),
    );
    clean.insert("TMPDIR".into(), policy.workspace.to_string_lossy().into());
    clean.insert("TEMP".into(), policy.workspace.to_string_lossy().into());
    clean.insert("TMP".into(), policy.workspace.to_string_lossy().into());
    #[cfg(windows)]
    {
        let _ = worker;
        windows::spawn(&policy, &clean)
    }
    #[cfg(unix)]
    {
        let worker = worker
            .map(Path::to_path_buf)
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)?;
        use std::os::unix::process::CommandExt;
        let mut child = Command::new(worker)
            .process_group(0)
            .arg("--native-worker")
            .arg(serde_json::to_string(&policy)?)
            .env_clear()
            .envs(clean)
            .current_dir(&policy.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(Process {
            finished: false,
            stdin: child
                .stdin
                .take()
                .map(|p| Box::new(p) as Box<dyn Write + Send>),
            stdout: child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            child,
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(io::Error::other(
            "Native isolation is unavailable on this platform",
        ))
    }
}

/// Called from the binary's synchronous entry point before a Tokio runtime exists.
pub fn worker_entry() -> Option<io::Result<()>> {
    let mut args = std::env::args_os();
    args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--native-worker")) {
        return None;
    }
    Some((|| {
        let raw = args
            .next()
            .ok_or_else(|| io::Error::other("Missing native isolation policy"))?;
        if args.next().is_some() {
            return Err(io::Error::other("Unexpected native worker arguments"));
        }
        let policy: Policy = serde_json::from_str(&raw.to_string_lossy())?;
        let policy = Policy::new(&policy.executable, &policy.workspace)?;
        #[cfg(unix)]
        {
            unix::apply(&policy)?;
            use std::os::unix::process::CommandExt;
            Err(Command::new(&policy.executable)
                .current_dir(&policy.workspace)
                .exec())
        }
        #[cfg(not(unix))]
        {
            let _ = policy;
            Err(io::Error::other(
                "This platform does not use a self-restricting worker",
            ))
        }
    })())
}

#[cfg(unix)]
#[path = "native_isolation/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "native_isolation/windows.rs"]
mod windows;

#[cfg(test)]
mod tests {
    #[test]
    fn production_gate_cannot_be_bypassed() {
        let policy = super::Policy {
            executable: "unused".into(),
            workspace: "unused".into(),
        };
        assert!(
            super::spawn(&policy, &Default::default())
                .err()
                .unwrap()
                .to_string()
                .contains("validation")
        );
    }
}
