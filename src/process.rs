use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W, Process32FirstW,
    Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_ALL_ACCESS, WaitForSingleObject};

pub fn find(name: &str) -> Option<u32> {
    if let Ok(pid) = name.parse() {
        return Some(pid);
    }
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snap, &mut entry);
        while more != 0 {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(name) {
                found = Some(entry.th32ProcessID);
            }
            more = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
        found
    }
}

pub fn open(pid: u32) -> Option<HANDLE> {
    let handle = unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, pid) };
    (!handle.is_null()).then_some(handle)
}

pub(crate) fn alive(handle: HANDLE) -> bool {
    unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT }
}

pub fn modules(pid: u32) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snap == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
        let mut more = Module32FirstW(snap, &mut entry);
        while more != 0 {
            found.push((entry.modBaseAddr as usize, entry.modBaseSize as usize));
            more = Module32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    found
}

pub fn main_base(pid: u32) -> usize {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
        let base = if Module32FirstW(snap, &mut entry) != 0 { entry.modBaseAddr as usize } else { 0 };
        CloseHandle(snap);
        base
    }
}
