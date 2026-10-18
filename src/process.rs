use crate::error::{ErrorKind, ProbeError, Result};
use serde::Serialize;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};

#[cfg(not(windows))]
compile_error!("QuotaRail P0 currently supports native Windows only.");

mod windows {
    use super::*;
    use std::{
        mem::{size_of, zeroed},
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::*,
            ProcessStatus::{
                GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            },
            Threading::{
                GetProcessHandleCount, GetProcessTimes, OpenProcess, OpenThread,
                PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this wrapper exclusively owns a non-null kernel handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub struct Job(Handle);
    impl Job {
        pub fn new() -> Result<Self> {
            // SAFETY: null security/name select a private unnamed, non-inheritable job.
            let handle = unsafe { CreateJobObjectW(null(), null()) };
            if handle.is_null() {
                return Err(isolation_error());
            }
            let job = Self(Handle(handle));
            // SAFETY: zero initialization and cb size match the documented Windows structure.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of_val(&limits) as u32,
                )
            };
            if ok == 0 {
                return Err(isolation_error());
            }
            Ok(job)
        }

        pub fn assign(&self, child: &Child) -> Result<()> {
            let process = child.raw_handle().ok_or_else(isolation_error)?;
            // SAFETY: the borrowed child handle is valid for this call and isn't closed here.
            if unsafe { AssignProcessToJobObject(self.0.0, process.cast()) } == 0 {
                return Err(isolation_error());
            }
            Ok(())
        }

        pub fn resume(&self, child: &Child) -> Result<()> {
            let pid = child.id().ok_or_else(isolation_error)?;
            // CREATE_SUSPENDED prevents descendants from escaping before job assignment.
            let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if raw == INVALID_HANDLE_VALUE {
                return Err(isolation_error());
            }
            let snapshot = Handle(raw);
            let mut thread: THREADENTRY32 = unsafe { zeroed() };
            thread.dwSize = size_of_val(&thread) as u32;
            let mut present = unsafe { Thread32First(snapshot.0, &mut thread) };
            while present != 0 {
                if thread.th32OwnerProcessID == pid {
                    let raw_thread =
                        unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread.th32ThreadID) };
                    if raw_thread.is_null() {
                        return Err(isolation_error());
                    }
                    let handle = Handle(raw_thread);
                    // SAFETY: only our suspended child's initial thread is resumed.
                    if unsafe { ResumeThread(handle.0) } != 1 {
                        return Err(isolation_error());
                    }
                    return Ok(());
                }
                present = unsafe { Thread32Next(snapshot.0, &mut thread) };
            }
            Err(isolation_error())
        }

        pub fn sample(&self) -> Result<ResourceSample> {
            // ULONG_PTR-aligned buffer: two DWORD header fields followed by process IDs.
            let mut ids = vec![0usize; 258];
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0.0,
                    JobObjectBasicProcessIdList,
                    ids.as_mut_ptr().cast(),
                    (ids.len() * size_of::<usize>()) as u32,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(metrics_error());
            }
            // SAFETY: a successful query wrote a header into this aligned allocation.
            let count = unsafe { *ids.as_ptr().cast::<u32>().add(1) } as usize;
            let offset = 8 / size_of::<usize>();
            if count > ids.len() - offset {
                return Err(metrics_error());
            }
            let mut process_ids: Vec<u32> = ids[offset..offset + count]
                .iter()
                .map(|id| *id as u32)
                .collect();
            process_ids.push(std::process::id());
            let mut processes = Vec::new();
            let mut unavailable_processes = 0;
            for id in process_ids {
                match process_sample(id) {
                    Some(sample) => processes.push(sample),
                    None => unavailable_processes += 1,
                }
            }
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
            if unsafe {
                QueryInformationJobObject(
                    self.0.0,
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of_val(&accounting) as u32,
                    null_mut(),
                )
            } == 0
            {
                return Err(metrics_error());
            }
            let own_cpu = processes
                .iter()
                .find(|p| p.process_id == std::process::id())
                .map_or(0, |p| p.cpu_100ns);
            Ok(ResourceSample {
                elapsed_seconds: 0.0,
                total_private_bytes: processes.iter().map(|p| p.private_bytes).sum(),
                total_working_set_bytes: processes.iter().map(|p| p.working_set_bytes).sum(),
                total_cpu_100ns: (accounting.TotalKernelTime + accounting.TotalUserTime) as u64
                    + own_cpu,
                unavailable_processes,
                processes,
            })
        }
    }

    fn process_sample(id: u32) -> Option<ProcessSample> {
        let raw = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, id) };
        if raw.is_null() {
            return None;
        }
        let handle = Handle(raw);
        let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { zeroed() };
        memory.cb = size_of_val(&memory) as u32;
        let mut creation: FILETIME = unsafe { zeroed() };
        let mut exit: FILETIME = unsafe { zeroed() };
        let mut kernel: FILETIME = unsafe { zeroed() };
        let mut user: FILETIME = unsafe { zeroed() };
        let mut handles = 0;
        let ok = unsafe {
            GetProcessMemoryInfo(
                handle.0,
                (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                size_of_val(&memory) as u32,
            ) != 0
                && GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user) != 0
                && GetProcessHandleCount(handle.0, &mut handles) != 0
        };
        if !ok {
            return None;
        }
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Some(ProcessSample {
            process_id: id,
            private_bytes: memory.PrivateUsage as u64,
            working_set_bytes: memory.WorkingSetSize as u64,
            cpu_100ns: ticks(kernel) + ticks(user),
            handles,
        })
    }

    fn isolation_error() -> ProbeError {
        ProbeError::new(
            ErrorKind::ProcessIsolation,
            "Cannot isolate the owned Codex process tree.",
        )
    }
    fn metrics_error() -> ProbeError {
        ProbeError::new(
            ErrorKind::Io,
            "Cannot sample the complete owned process job.",
        )
    }
}

#[derive(Debug, Serialize)]
pub struct ProcessSample {
    pub process_id: u32,
    pub private_bytes: u64,
    pub working_set_bytes: u64,
    pub cpu_100ns: u64,
    pub handles: u32,
}

#[derive(Debug, Serialize)]
pub struct ResourceSample {
    pub elapsed_seconds: f64,
    pub total_private_bytes: u64,
    pub total_working_set_bytes: u64,
    pub total_cpu_100ns: u64,
    pub unavailable_processes: u32,
    pub processes: Vec<ProcessSample>,
}

pub struct ManagedChild {
    pub child: Child,
    job: windows::Job,
}

impl ManagedChild {
    pub fn spawn(exe: &Path, args: &[&str], codex_home: Option<&Path>) -> Result<Self> {
        let job = windows::Job::new()?;
        let mut command = Command::new(exe);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(std::env::temp_dir())
            .kill_on_drop(true)
            .creation_flags(0x08000004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
        if let Some(home) = codex_home {
            command.env("CODEX_HOME", home);
        }
        let mut child = command.spawn().map_err(|_| {
            ProbeError::new(
                ErrorKind::SpawnFailed,
                "Cannot start the selected executable.",
            )
        })?;
        if let Err(error) = job.assign(&child).and_then(|()| job.resume(&child)) {
            let _ = child.start_kill();
            return Err(error);
        }
        Ok(Self { child, job })
    }

    pub fn sample(&mut self) -> Result<ResourceSample> {
        if self
            .child
            .try_wait()
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot inspect the owned process."))?
            .is_some()
        {
            return Err(ProbeError::new(
                ErrorKind::ChildExited,
                "Codex exited during observation.",
            ));
        }
        self.job.sample()
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.child.stdin.take();
        match tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(_)) => {
                return Err(ProbeError::new(
                    ErrorKind::ChildExited,
                    "Codex exited unsuccessfully.",
                ));
            }
            Ok(Err(_)) => {
                return Err(ProbeError::new(
                    ErrorKind::Io,
                    "Cannot wait for the owned process.",
                ));
            }
            Err(_) => self
                .child
                .kill()
                .await
                .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot stop the owned process."))?,
        }
        Ok(())
    }
}

pub async fn detect_version(exe: &Path, home: Option<&Path>) -> Result<String> {
    let mut process = ManagedChild::spawn(exe, &["--version"], home)?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| ProbeError::new(ErrorKind::Io, "Version stdout is unavailable."))?;
    let mut bytes = Vec::new();
    let read = async {
        stdout
            .take(4097)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot read CLI version."))?;
        let status = process
            .child
            .wait()
            .await
            .map_err(|_| ProbeError::new(ErrorKind::Io, "Cannot wait for CLI version."))?;
        if !status.success() || bytes.len() > 4096 {
            return Err(ProbeError::new(
                ErrorKind::UnsupportedVersion,
                "Invalid CLI version response.",
            ));
        }
        let text = std::str::from_utf8(&bytes).unwrap_or_default().trim();
        let version = text
            .strip_prefix("codex-cli ")
            .filter(|v| {
                !v.is_empty()
                    && v.len() < 80
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
            })
            .ok_or_else(|| {
                ProbeError::new(
                    ErrorKind::UnsupportedVersion,
                    "The selected exe did not report a supported Codex version string.",
                )
            })?;
        Ok(version.to_owned())
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .map_err(|_| ProbeError::new(ErrorKind::Timeout, "CLI version query timed out."))?
}
