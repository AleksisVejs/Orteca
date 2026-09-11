//! Win32 Job Object. Owning one guarantees the whole process tree dies when it
//! drops — `Child::kill()` only kills the direct child and orphans `node -> bash
//! -> npm`, which is how agent CLIs are built.

use std::io;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

pub struct Job(HANDLE);

// The handle is ours alone; nothing in it is thread-affine.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(None, None)?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;

            Ok(Job(handle))
        }
    }

    /// Adopt `pid` and everything it goes on to spawn.
    ///
    /// ponytail: assigning just after spawn leaves a sub-millisecond window in
    /// which the child could spawn a grandchild outside the job. Closing that
    /// needs CREATE_SUSPENDED + ResumeThread, which means dropping to raw
    /// CreateProcessW instead of std/tokio. Do that only if an orphan is ever
    /// actually observed.
    pub fn assign(&self, pid: u32) -> io::Result<()> {
        unsafe {
            let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
            let result = AssignProcessToJobObject(self.0, proc);
            let _ = CloseHandle(proc);
            result?;
        }
        Ok(())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE: this is the cancel.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
