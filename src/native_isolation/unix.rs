use super::Policy;
use std::io;

fn close_inherited_descriptors() -> io::Result<()> {
    // Landlock does not revoke pre-existing file descriptors. The dedicated
    // worker must drop everything except its three protocol/diagnostic pipes.
    #[cfg(target_os = "linux")]
    {
        if unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let limit = unsafe { limit.assume_init() }.rlim_cur;
        if limit > 1_048_576 {
            return Err(io::Error::other(
                "Native worker descriptor limit is unsupported",
            ));
        }
        for fd in 3..limit {
            unsafe {
                libc::close(fd as i32);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn apply(policy: &Policy) -> io::Result<()> {
    close_inherited_descriptors()?;
    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
        RulesetAttr, RulesetCreatedAttr, RulesetStatus,
    };
    let error = |e: &dyn std::fmt::Display| {
        io::Error::other(format!("Native Landlock restriction failed: {e}"))
    };
    let abi = ABI::V5;
    let mut rules = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| error(&e))?
        .handle_access(AccessNet::from_all(abi))
        .map_err(|e| error(&e))?
        .create()
        .map_err(|e| error(&e))?;
    for path in [
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/etc/ld.so.cache",
    ] {
        if std::path::Path::new(path).exists() {
            // HardRequirement rejects directory-only rights on a regular file.
            let read = if std::path::Path::new(path).is_dir() {
                AccessFs::from_read(abi)
            } else {
                AccessFs::ReadFile.into()
            };
            rules = rules
                .add_rule(PathBeneath::new(
                    PathFd::new(path).map_err(|e| error(&e))?,
                    read,
                ))
                .map_err(|e| error(&e))?;
        }
    }
    for path in ["/dev/null", "/dev/urandom", "/dev/random"] {
        rules = rules
            .add_rule(PathBeneath::new(
                PathFd::new(path).map_err(|e| error(&e))?,
                AccessFs::ReadFile | AccessFs::WriteFile,
            ))
            .map_err(|e| error(&e))?;
    }
    rules = rules
        .add_rule(PathBeneath::new(
            PathFd::new(&policy.executable).map_err(|e| error(&e))?,
            AccessFs::ReadFile | AccessFs::Execute,
        ))
        .map_err(|e| error(&e))?;
    // Workspaces hold data. Executing newly written workspace payloads is denied.
    let writable = AccessFs::from_all(abi) & !AccessFs::Execute;
    rules = rules
        .add_rule(PathBeneath::new(
            PathFd::new(&policy.workspace).map_err(|e| error(&e))?,
            writable,
        ))
        .map_err(|e| error(&e))?;
    let status = rules.restrict_self().map_err(|e| error(&e))?;
    if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
        return Err(io::Error::other(
            "Native isolation requires fully enforced Landlock and no_new_privs",
        ));
    }
    seccomp()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn seccomp() -> io::Result<()> {
    // Reject foreign syscall ABIs before interpreting syscall numbers.
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xc00000b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return Err(io::Error::other(
        "Native seccomp is unsupported on this CPU",
    ));
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let statement = |code, k| libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        };
        let equal = |k, jt, jf| libc::sock_filter {
            code: 0x15,
            jt,
            jf,
            k,
        };
        let mut filter = vec![
            statement(0x20, 4),
            equal(ARCH, 1, 0),
            statement(0x06, 0x80000000),
            statement(0x20, 0),
        ];
        let denied = [
            libc::SYS_setsid,
            libc::SYS_setpgid,
            libc::SYS_socket,
            libc::SYS_socketpair,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_unshare,
            libc::SYS_setns,
            libc::SYS_bpf,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_open_by_handle_at,
            libc::SYS_pidfd_getfd,
            libc::SYS_io_uring_setup,
            libc::SYS_kill,
            libc::SYS_tkill,
            libc::SYS_tgkill,
            libc::SYS_pidfd_send_signal,
        ];
        for syscall in denied {
            filter.push(equal(syscall as u32, 0, 1));
            filter.push(statement(0x06, 0x00050000 | libc::EPERM as u32));
        }
        // x32 syscall numbers bypass the native x86_64 number table.
        #[cfg(target_arch = "x86_64")]
        {
            filter.push(libc::sock_filter {
                code: 0x35,
                jt: 0,
                jf: 1,
                k: 0x40000000,
            });
            filter.push(statement(0x06, 0x80000000));
        }
        filter.push(statement(0x06, 0x7fff0000));
        let program = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_mut_ptr(),
        };
        // SAFETY: only the single-threaded worker calls this; the BPF buffer
        // remains alive throughout prctl and the kernel copies it synchronously.
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
            || unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) }
                != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn apply(policy: &Policy) -> io::Result<()> {
    close_inherited_descriptors()?;
    use std::ffi::{CString, c_char};
    unsafe extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> i32;
        fn sandbox_free_error(error: *mut c_char);
    }
    fn quoted(path: &std::path::Path) -> String {
        format!(
            "\"{}\"",
            path.to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        )
    }
    let mut profile = String::from(
        "(version 1)(deny default)(allow process-fork)(allow signal (target self))(allow sysctl-read)",
    );
    for system in [
        "/System/Library",
        "/System/Volumes/Preboot/Cryptexes/OS/System/Library",
        "/System/Volumes/Preboot/Cryptexes/OS/usr/lib",
        "/usr/lib",
        "/Library/Apple/System/Library",
        "/dev/null",
        "/dev/urandom",
        "/dev/random",
    ] {
        profile.push_str(&format!(
            "(allow file-read* file-map-executable (subpath {}))",
            quoted(std::path::Path::new(system))
        ));
    }
    let executable = quoted(&policy.executable);
    let workspace = quoted(&policy.workspace);
    // dyld needs executable mappings in addition to ordinary file reads.
    profile.push_str(&format!(
        "(allow file-read* file-map-executable process-exec* (literal {executable}))\n\
         (allow file-read* file-write* (subpath {workspace}))"
    ));
    let profile = CString::new(profile).map_err(io::Error::other)?;
    let mut error = std::ptr::null_mut();
    // SAFETY: the worker is single-threaded, strings are NUL terminated and the
    // optional error allocation is released through Apple's matching API.
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if !error.is_null() {
        unsafe { sandbox_free_error(error) };
    }
    if result != 0 {
        return Err(io::Error::other(
            "macOS Seatbelt refused the native isolation policy",
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply(_: &Policy) -> io::Result<()> {
    Err(io::Error::other(
        "Native isolation is unavailable on this Unix platform",
    ))
}
