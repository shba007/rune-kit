use super::{Policy, Process};
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Isolation::*, *},
    Storage::FileSystem::*,
    System::{JobObjects::*, Threading::*},
};

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
#[track_caller]
fn check(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::other(format!(
            "Windows isolation step {}: {}",
            std::panic::Location::caller().line(),
            io::Error::last_os_error()
        )))
    } else {
        Ok(())
    }
}
fn handle(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the caller transfers its newly created Win32 handle exactly once.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}
struct Sid(PSID);
impl Drop for Sid {
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.0);
        }
    }
}
struct Profile(Vec<u16>);
impl Drop for Profile {
    fn drop(&mut self) {
        unsafe {
            DeleteAppContainerProfile(self.0.as_ptr());
        }
    }
}
struct Grant {
    path: Vec<u16>,
    sid: Vec<u8>,
}
impl Drop for Grant {
    fn drop(&mut self) {
        let _ = acl(&self.path, self.sid.as_mut_ptr().cast(), 0, REVOKE_ACCESS);
    }
}
fn acl(path: &[u16], sid: PSID, rights: u32, mode: ACCESS_MODE) -> io::Result<()> {
    // SAFETY: all API pointers refer to live NUL-terminated paths, SID buffers
    // and stack outputs. Windows allocations are freed with LocalFree.
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        let mut old = std::ptr::null_mut();
        let result = GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old,
            std::ptr::null_mut(),
            &mut descriptor,
        );
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: rights,
            grfAccessMode: mode,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: sid.cast(),
            },
        };
        let mut updated = std::ptr::null_mut();
        let mut result = SetEntriesInAclW(1, &entry, old, &mut updated);
        if result == 0 {
            result = SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                updated,
                std::ptr::null_mut(),
            );
        }
        if !updated.is_null() {
            LocalFree(updated.cast());
        }
        if !descriptor.is_null() {
            LocalFree(descriptor);
        }
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        Ok(())
    }
}
fn grant(path: &Path, sid: PSID, rights: u32) -> io::Result<Grant> {
    let path = wide(path);
    let mut bytes = vec![0; unsafe { GetLengthSid(sid) } as usize];
    check(unsafe { CopySid(bytes.len() as u32, bytes.as_mut_ptr().cast(), sid) })?;
    acl(&path, sid, rights, GRANT_ACCESS)?;
    Ok(Grant { path, sid: bytes })
}
fn profile_directory(sid: PSID) -> io::Result<std::path::PathBuf> {
    unsafe {
        let mut string = std::ptr::null_mut();
        check(ConvertSidToStringSidW(sid, &mut string))?;
        let mut folder = std::ptr::null_mut();
        let result = GetAppContainerFolderPath(string, &mut folder);
        LocalFree(string.cast());
        if result < 0 {
            return Err(io::Error::other(
                "Cannot resolve AppContainer private storage",
            ));
        }
        let mut length = 0;
        while *folder.add(length) != 0 {
            length += 1;
        }
        let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(
            std::slice::from_raw_parts(folder, length),
        ));
        windows_sys::Win32::System::Com::CoTaskMemFree(folder.cast());
        Ok(path)
    }
}
fn deny_writes(path: &Path) -> io::Result<Grant> {
    // Deny writes for every SID on this fresh, disposable profile.
    // Keep READ_CONTROL/WRITE_DAC available for cleanup.
    let path = wide(path);
    let mut bytes = vec![0; SECURITY_MAX_SID_SIZE as usize];
    let mut length = bytes.len() as u32;
    check(unsafe {
        CreateWellKnownSid(
            WinWorldSid,
            std::ptr::null_mut(),
            bytes.as_mut_ptr().cast(),
            &mut length,
        )
    })?;
    bytes.truncate(length as usize);
    acl(
        &path,
        bytes.as_mut_ptr().cast(),
        FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | DELETE
            | FILE_DELETE_CHILD,
        DENY_ACCESS,
    )?;
    Ok(Grant { path, sid: bytes })
}
struct Attributes {
    initialized: bool,
    buffer: Vec<usize>,
}
impl Attributes {
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.ptr());
            }
        }
    }
}

pub(super) struct Child {
    process: OwnedHandle,
    job: OwnedHandle,
    _grants: Vec<Grant>,
    _profile: Profile,
    #[cfg(feature = "native-isolation-validation")]
    pub(super) profile_directory: std::path::PathBuf,
}
impl Child {
    pub fn kill(&mut self) -> io::Result<()> {
        check(unsafe { TerminateJobObject(raw(&self.job), 1) })
    }
    pub fn wait(&mut self) -> io::Result<()> {
        if unsafe { WaitForSingleObject(raw(&self.process), INFINITE) } == WAIT_FAILED {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.kill();
        let _ = self.wait();
    }
}

pub(super) fn spawn(policy: &Policy, params: &HashMap<String, String>) -> io::Result<Process> {
    // All resources are RAII-owned, including failures before ResumeThread.
    unsafe {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = wide(format!(
            "RuneKit.{}.{}.{}",
            std::process::id(),
            nonce,
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut sid = std::ptr::null_mut();
        let result = CreateAppContainerProfile(
            name.as_ptr(),
            name.as_ptr(),
            name.as_ptr(),
            std::ptr::null(),
            0,
            &mut sid,
        );
        if result < 0 {
            return Err(io::Error::other(
                "Cannot create a fresh AppContainer identity",
            ));
        }
        let _sid_guard = Sid(sid);
        let profile = Profile(name);
        let profile_directory = profile_directory(sid)?;
        let grants = (|| {
            Ok::<_, io::Error>(vec![
                grant(
                    &policy.executable,
                    sid,
                    FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                )?,
                grant(
                    &policy.workspace,
                    sid,
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | FILE_DELETE_CHILD,
                )?,
            ])
        })();
        if grants.is_err() {
            return Err(grants.err().unwrap());
        }
        let mut grants = grants?;
        let (input_read, input_write) = std::io::pipe()?;
        let (output_read, output_write) = std::io::pipe()?;
        let stderr = File::options().write(true).open("NUL")?;
        let inherited = [
            input_read.as_raw_handle(),
            output_write.as_raw_handle(),
            stderr.as_raw_handle(),
        ];
        for item in inherited {
            check(SetHandleInformation(
                item,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            ))?;
        }
        let caps = SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            Capabilities: std::ptr::null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        let mut size = 0;
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 3, 0, &mut size);
        let mut attrs = Attributes {
            initialized: false,
            buffer: vec![0; size.div_ceil(std::mem::size_of::<usize>())],
        };
        check(InitializeProcThreadAttributeList(
            attrs.ptr(),
            3,
            0,
            &mut size,
        ))?;
        attrs.initialized = true;
        let opt_out = 1u32;
        let setup = (|| {
            check(UpdateProcThreadAttribute(
                attrs.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
                (&opt_out as *const u32).cast(),
                std::mem::size_of_val(&opt_out),
                std::ptr::null_mut(),
                std::ptr::null(),
            ))?;

            check(UpdateProcThreadAttribute(
                attrs.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (&caps as *const SECURITY_CAPABILITIES).cast(),
                std::mem::size_of_val(&caps),
                std::ptr::null_mut(),
                std::ptr::null(),
            ))?;
            check(UpdateProcThreadAttribute(
                attrs.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherited.as_ptr().cast(),
                std::mem::size_of_val(&inherited),
                std::ptr::null_mut(),
                std::ptr::null(),
            ))
        })();
        if setup.is_err() {
            return setup.map(|_| unreachable!());
        }
        let job = handle(CreateJobObjectW(std::ptr::null(), std::ptr::null()))?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        check(SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        ))?;
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = std::mem::size_of_val(&startup) as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = inherited[0];
        startup.StartupInfo.hStdOutput = inherited[1];
        startup.StartupInfo.hStdError = inherited[2];
        startup.lpAttributeList = attrs.ptr();
        let executable = wide(&policy.executable);
        let mut command = wide(format!("\"{}\"", policy.executable.to_string_lossy()));
        let cwd = wide(&policy.workspace);
        let mut environment: std::collections::BTreeMap<_, _> = params
            .iter()
            .map(|(k, v)| (k.to_ascii_uppercase(), v.clone()))
            .collect();
        environment.insert(
            "SYSTEMROOT".into(),
            std::env::var("SystemRoot").map_err(|_| io::Error::other("SystemRoot unavailable"))?,
        );
        // CreateProcess resolves the AppContainer profile using LOCALAPPDATA.
        // This is a Windows bootstrap requirement, not an inherited credential.
        environment.insert(
            "LOCALAPPDATA".into(),
            std::env::var("LOCALAPPDATA").map_err(|_| {
                io::Error::other("LOCALAPPDATA unavailable for AppContainer startup")
            })?,
        );
        let mut environment: Vec<u16> = environment
            .iter()
            .flat_map(|(k, v)| wide(format!("{k}={v}")))
            .collect();
        environment.push(0);
        let mut info: PROCESS_INFORMATION = std::mem::zeroed();
        let launched = CreateProcessW(
            executable.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_SUSPENDED
                | CREATE_NO_WINDOW,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        );
        check(launched)?;
        let process = handle(info.hProcess)?;
        let thread = handle(info.hThread)?;
        if let Err(error) = check(AssignProcessToJobObject(raw(&job), raw(&process))) {
            TerminateProcess(raw(&process), 1);
            return Err(error);
        }
        // Userenv initializes (and may replace) profile ACLs during process
        // creation. Deny its auxiliary storage after creation, while suspended.
        let denied = (|| -> io::Result<()> {
            let mut pending = vec![profile_directory.clone()];
            while let Some(path) = pending.pop() {
                crate::sandbox::check_links(&path)?;
                if path.is_dir() {
                    for entry in std::fs::read_dir(&path)? {
                        pending.push(entry?.path());
                    }
                }
                grants.push(deny_writes(&path)?);
            }
            Ok(())
        })();
        if let Err(error) = denied {
            TerminateProcess(raw(&process), 1);
            return Err(error);
        }
        let child = Child {
            process,
            job,
            _grants: grants,
            _profile: profile,
            #[cfg(feature = "native-isolation-validation")]
            profile_directory,
        };
        if ResumeThread(raw(&thread)) == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        drop(input_read);
        drop(output_write);
        drop(stderr);
        Ok(Process {
            finished: false,
            stdin: Some(Box::new(input_write)),
            stdout: Some(Box::new(output_read)),
            child,
        })
    }
}
