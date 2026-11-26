#![allow(non_snake_case)]

mod call;
mod canvas;
mod config;
pub mod detour;
pub mod engine;
mod hook;
mod mem;
mod overlay;
#[cfg(plugins)]
mod plugin;
pub mod process;

use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

static HOME: OnceLock<PathBuf> = OnceLock::new();
static LOG_FILE: AtomicBool = AtomicBool::new(true);

pub fn set_home(home: PathBuf) {
    let _ = HOME.set(home);
}

pub(crate) fn home() -> Option<&'static Path> {
    HOME.get().map(PathBuf::as_path)
}

pub(crate) fn log_to_file(on: bool) {
    LOG_FILE.store(on, Ordering::Relaxed);
}

pub(crate) fn log(text: &str) {
    let _ = writeln!(std::io::stderr(), "ScriptHookVI: {text}");
    if let Some(home) = HOME.get().filter(|_| LOG_FILE.load(Ordering::Relaxed)) {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(home.join("ScriptHookVI.log")) {
            let _ = writeln!(file, "{text}");
        }
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: *mut c_void, reason: u32, _: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let mut path = [0u16; 1024];
        let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
        let mut home = PathBuf::from(String::from_utf16_lossy(&path[..len]))
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default();
        if home.file_name().is_some_and(|n| n == "run") {
            home.pop();
        }
        let scripts = std::env::var_os("SCRIPTHOOKVI_SCRIPTS").map_or_else(|| home.join("scripts"), PathBuf::from);
        let _ = HOME.set(home);
        std::thread::spawn(move || engine::run(scripts, engine::Mode::Internal));
    }
    1
}
