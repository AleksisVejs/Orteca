//! Win32 Job Object. Owning one guarantees the whole process tree dies when it
//! drops — `Child::kill()` only kills the direct child and orphans `node -> bash
//! -> npm`, which is how agent CLIs are built.

use std::io;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenThread, ResumeThread, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    THREAD_SUSPEND_RESUME,
};

pub struct Job(HANDLE);

// The handle is ours alone; nothing in it is thread-affine.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> io::Result<Self> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        Self::with_info(&info)
    }

    fn with_info(info: &JOBOBJECT_EXTENDED_LIMIT_INFORMATION) -> io::Result<Self> {
        unsafe {
            let job = Job(CreateJobObjectW(None, None)?);

            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;

            Ok(job)
        }
    }

    /// Adopt `pid` and everything it goes on to spawn.
    /// The caller keeps the process suspended until adoption succeeds.
    pub fn assign(&self, pid: u32) -> io::Result<()> {
        unsafe {
            let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
            let result = AssignProcessToJobObject(self.0, proc);
            let _ = CloseHandle(proc);
            result?;
        }
        Ok(())
    }

    pub fn terminate(&self) -> io::Result<()> {
        unsafe {
            TerminateJobObject(self.0, 1)?;
        }
        Ok(())
    }
}

pub fn resume(pid: u32) -> io::Result<()> {
    unsafe {
        // std closes the primary thread handle, so find that still-suspended thread.
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let _snapshot = OwnedHandle::from_raw_handle(snapshot.0);
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        Thread32First(snapshot, &mut entry)?;
        loop {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)?;
                let _thread = OwnedHandle::from_raw_handle(thread.0);
                if ResumeThread(thread) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            if Thread32Next(snapshot, &mut entry).is_err() {
                return Err(io::Error::other("suspended process thread was not found"));
            }
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE also covers failures before a Run is returned.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::JobObjects::JOB_OBJECT_LIMIT;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

    #[test]
    fn failed_configuration_closes_job_handle() {
        if std::env::var_os("ORTECA_TEST_JOB_HANDLES").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "proc::job::tests::failed_configuration_closes_job_handle",
                ])
                .env("ORTECA_TEST_JOB_HANDLES", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        let handles = || unsafe {
            let mut count = 0;
            GetProcessHandleCount(GetCurrentProcess(), &mut count).unwrap();
            count
        };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT(u32::MAX);
        let before = handles();
        for _ in 0..64 {
            assert!(Job::with_info(&info).is_err());
        }
        assert!(
            handles() <= before + 1,
            "failed configuration leaked job handles"
        );
    }
}
