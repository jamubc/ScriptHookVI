use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS,
    PAGE_READWRITE, VirtualAllocEx, VirtualProtectEx, VirtualQueryEx,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

static TARGET: AtomicUsize = AtomicUsize::new(0);
static TARGET_MAIN: AtomicUsize = AtomicUsize::new(0);

pub fn attach(process: HANDLE, main: usize) {
    detach();
    TARGET_MAIN.store(main, Ordering::Relaxed);
    TARGET.store(process as usize, Ordering::Relaxed);
}

pub fn detach() {
    let old = TARGET.swap(0, Ordering::Relaxed);
    if old != 0 {
        unsafe { CloseHandle(old as HANDLE) };
    }
}

pub fn remote() -> bool {
    TARGET.load(Ordering::Relaxed) != 0
}

pub fn process() -> HANDLE {
    match TARGET.load(Ordering::Relaxed) {
        0 => unsafe { GetCurrentProcess() },
        handle => handle as HANDLE,
    }
}

pub fn read(addr: usize, buf: &mut [u8]) -> bool {
    let mut done = 0;
    unsafe {
        ReadProcessMemory(process(), addr as *const c_void, buf.as_mut_ptr().cast(), buf.len(), &mut done) != 0
            && done == buf.len()
    }
}

fn write_plain(addr: usize, data: &[u8]) -> bool {
    let mut done = 0;
    unsafe {
        WriteProcessMemory(process(), addr as *const c_void, data.as_ptr().cast(), data.len(), &mut done) != 0
            && done == data.len()
    }
}

pub fn write(addr: usize, data: &[u8]) -> bool {
    if write_plain(addr, data) {
        return true;
    }
    let mut old = 0;
    unsafe {
        if VirtualProtectEx(process(), addr as *const c_void, data.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        let done = write_plain(addr, data);
        VirtualProtectEx(process(), addr as *const c_void, data.len(), old, &mut old);
        done
    }
}

pub fn alloc(len: usize) -> usize {
    let len = len.max(8);
    if remote() {
        unsafe { VirtualAllocEx(process(), std::ptr::null(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) as usize }
    } else {
        Box::leak(vec![0u64; len.div_ceil(8)].into_boxed_slice()).as_ptr() as usize
    }
}

fn query(addr: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let got = unsafe { VirtualQueryEx(process(), addr as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    (got != 0).then_some(info)
}

fn header(base: usize) -> Option<([u8; 0x1000], usize)> {
    let mut head = [0u8; 0x1000];
    if !read(base, &mut head) || &head[..2] != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(head[0x3c..0x40].try_into().ok()?) as usize;
    if pe + 0x60 > head.len() || &head[pe..pe + 4] != b"PE\0\0" || head[pe + 0x18..pe + 0x1a] != [0x0b, 0x02] {
        return None;
    }
    Some((head, pe))
}

pub fn image_size(base: usize) -> Option<usize> {
    let (head, pe) = header(base)?;
    Some(u32::from_le_bytes(head[pe + 0x50..pe + 0x54].try_into().ok()?) as usize)
}

pub fn image_stamp(base: usize) -> Option<u32> {
    let (head, pe) = header(base)?;
    Some(u32::from_le_bytes(head[pe + 8..pe + 12].try_into().ok()?))
}

pub fn main_module() -> usize {
    if remote() {
        TARGET_MAIN.load(Ordering::Relaxed)
    } else {
        unsafe { GetModuleHandleW(std::ptr::null()) as usize }
    }
}

pub fn find_title(min_size: usize) -> Option<(usize, usize)> {
    let mut best = None;
    let mut addr = 0x10000usize;
    let main = main_module();
    while addr < 0x7fff_ffff_0000 {
        let Some(info) = query(addr) else {
            break;
        };
        let region = info.BaseAddress as usize;
        if info.State == MEM_COMMIT
            && info.AllocationBase as usize == region
            && info.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0
            && region != main
        {
            if let Some(size) = image_size(region) {
                if size >= min_size && best.is_none_or(|(_, s)| size > s) {
                    best = Some((region, size));
                }
            }
        }
        addr = region + info.RegionSize.max(0x1000);
    }
    best
}

pub fn parse_pattern(text: &str) -> Option<Vec<Option<u8>>> {
    text.split_whitespace()
        .map(|t| {
            if t.starts_with('?') {
                Some(None)
            } else {
                u8::from_str_radix(t, 16).ok().map(Some)
            }
        })
        .collect()
}

pub fn scan(start: usize, len: usize, pattern: &[Option<u8>], limit: usize) -> Vec<usize> {
    const CHUNK: usize = 1 << 20;
    const PAGE: usize = 0x1000;
    let mut found = Vec::new();
    let n = pattern.len();
    if n == 0 || len < n {
        return found;
    }
    let end = start + len;
    let mut buf = vec![0u8; CHUNK + n - 1];
    let mut at = start;
    while at < end && found.len() < limit {
        let want = (end - at).min(CHUNK + n - 1);
        if read(at, &mut buf[..want]) {
            search(&buf[..want], at, pattern, &mut found);
        } else {
            let mut page = at;
            while page < (at + CHUNK).min(end) {
                let wide = (end - page).min(PAGE + n - 1);
                let narrow = (end - page).min(PAGE);
                if read(page, &mut buf[..wide]) {
                    search(&buf[..wide], page, pattern, &mut found);
                } else if read(page, &mut buf[..narrow]) {
                    search(&buf[..narrow], page, pattern, &mut found);
                }
                page += PAGE;
            }
        }
        at += CHUNK;
    }
    found.truncate(limit);
    found
}

fn search(hay: &[u8], base: usize, pattern: &[Option<u8>], found: &mut Vec<usize>) {
    if hay.len() < pattern.len() {
        return;
    }
    for i in 0..=hay.len() - pattern.len() {
        if pattern.iter().enumerate().all(|(j, p)| p.is_none_or(|b| hay[i + j] == b)) {
            found.push(base + i);
        }
    }
}
