//! Ancestor process ids of this process, nearest first. Windows only; empty elsewhere.

#[cfg(windows)]
pub fn ancestors() -> Vec<u32> {
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    const MAX_LEVELS: usize = 12;

    // SAFETY: plain Win32 calls. The entry struct is zeroed and dwSize is set before use,
    // and the snapshot handle is closed before returning.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut parent: HashMap<u32, u32> = HashMap::new();
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            parent.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);

        let mut out: Vec<u32> = Vec::new();
        let mut pid = std::process::id();
        while out.len() < MAX_LEVELS {
            let next = parent.get(&pid).copied().unwrap_or(0);
            // PIDs get reused, so stop on a loop as the Python bridge does.
            if next == 0 || out.contains(&next) {
                break;
            }
            out.push(next);
            pid = next;
        }
        out
    }
}

#[cfg(not(windows))]
pub fn ancestors() -> Vec<u32> {
    Vec::new()
}
