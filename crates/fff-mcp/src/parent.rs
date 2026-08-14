#[cfg(unix)]
mod imp {
    pub struct ParentWatcher {
        ppid: u32,
    }

    impl ParentWatcher {
        pub fn new() -> Option<Self> {
            let ppid = std::os::unix::process::parent_id();
            // ppid <= 1 means we were spawned by init and can't detect death
            (ppid > 1).then_some(Self { ppid })
        }

        pub fn parent_pid(&self) -> u32 {
            self.ppid
        }

        // When the parent dies the kernel reparents us, so getppid() changes.
        // Race-free and immune to PID reuse, unlike kill(ppid, 0).
        pub fn parent_alive(&self) -> bool {
            std::os::unix::process::parent_id() == self.ppid
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    pub struct ParentWatcher {
        handle: HANDLE,
        ppid: u32,
    }

    // HANDLE is a raw pointer; it is only ever used via WaitForSingleObject
    // which is thread-safe, so moving/sharing the watcher across threads is fine.
    unsafe impl Send for ParentWatcher {}
    unsafe impl Sync for ParentWatcher {}

    impl ParentWatcher {
        pub fn new() -> Option<Self> {
            let ppid = parent_pid_of_current()?;
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, ppid) };
            if handle.is_null() {
                return None;
            }
            // Holding the handle pins the PID, preventing reuse for the process lifetime
            Some(Self { handle, ppid })
        }

        pub fn parent_pid(&self) -> u32 {
            self.ppid
        }

        pub fn parent_alive(&self) -> bool {
            unsafe { WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT }
        }
    }

    impl Drop for ParentWatcher {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }

    fn parent_pid_of_current() -> Option<u32> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
            let current = GetCurrentProcessId();
            let mut found = None;
            if Process32First(snapshot, &mut entry) != 0 {
                loop {
                    if entry.th32ProcessID == current {
                        found = Some(entry.th32ParentProcessID);
                        break;
                    }
                    if Process32Next(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
            found
        }
    }
}

pub use imp::ParentWatcher;
