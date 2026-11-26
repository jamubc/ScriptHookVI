use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_ALL_ACCESS, WaitForSingleObject,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else {
        eprintln!("usage: ScriptHookVI-inject <process.exe | pid> [ScriptHookVI.dll] [seconds to wait]");
        std::process::exit(2);
    };
    let dll = args.next().map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::current_exe().unwrap_or_default().with_file_name("ScriptHookVI.dll")
    });
    let wait = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let Ok(dll) = std::fs::canonicalize(&dll) else {
        eprintln!("no such file: {}", dll.display());
        std::process::exit(1);
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let run = dll.with_file_name("run");
    let copy = run.join(format!("ScriptHookVI-{stamp}.dll"));
    for old in std::fs::read_dir(&run).into_iter().flatten().flatten() {
        let _ = std::fs::remove_file(old.path());
    }
    if std::fs::create_dir_all(&run).is_err() || std::fs::copy(&dll, &copy).is_err() {
        eprintln!("cannot copy {} to {}", dll.display(), copy.display());
        std::process::exit(1);
    }
    let dll = copy;
    let text = dll.to_string_lossy();
    let path: Vec<u16> = text.trim_start_matches(r"\\?\").encode_utf16().chain([0]).collect();
    let started = Instant::now();
    let pid = loop {
        if let Some(pid) = ScriptHookVI::process::find(&target) {
            break pid;
        }
        if started.elapsed() > Duration::from_secs(wait) {
            eprintln!("no process named {target}");
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    unsafe {
        let process = OpenProcess(PROCESS_ALL_ACCESS, 0, pid);
        if process.is_null() {
            eprintln!("cannot open process {pid}");
            std::process::exit(1);
        }
        let bytes = path.len() * 2;
        let remote = VirtualAllocEx(process, std::ptr::null(), bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        let load = GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), c"LoadLibraryW".as_ptr().cast());
        if remote.is_null()
            || load.is_none()
            || WriteProcessMemory(process, remote, path.as_ptr().cast(), bytes, std::ptr::null_mut()) == 0
        {
            eprintln!("cannot write into process {pid}");
            std::process::exit(1);
        }
        let start: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(load);
        let thread = CreateRemoteThread(process, std::ptr::null(), 0, Some(start), remote, 0, std::ptr::null_mut());
        if thread.is_null() {
            eprintln!("cannot start a thread in process {pid}");
            std::process::exit(1);
        }
        if WaitForSingleObject(thread, 30_000) != WAIT_OBJECT_0 {
            eprintln!("process {pid} did not finish loading {} within 30 s", dll.display());
            std::process::exit(1);
        }
        let mut code = 0;
        GetExitCodeThread(thread, &mut code);
        if code == 0 {
            eprintln!("process {pid} did not load {}", dll.display());
            std::process::exit(1);
        }
        println!("ScriptHookVI loaded into process {pid}");
    }
}
