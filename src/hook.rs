use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::mem;

static BLOCKED: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static ASYNC_STATE: AtomicUsize = AtomicUsize::new(0);
static KEY_STATE: AtomicUsize = AtomicUsize::new(0);

pub fn block(keys: &[u8]) {
    let mut bits = [0u64; 4];
    for &vk in keys {
        bits[usize::from(vk) / 64] |= 1 << (vk % 64);
    }
    for (slot, value) in BLOCKED.iter().zip(bits) {
        slot.store(value, Ordering::Relaxed);
    }
}

fn blocked(vk: i32) -> bool {
    (0..256).contains(&vk) && BLOCKED[vk as usize / 64].load(Ordering::Relaxed) & (1 << (vk % 64)) != 0
}

unsafe extern "system" fn async_state(vk: i32) -> i16 {
    if blocked(vk) {
        return 0;
    }
    let original: unsafe extern "system" fn(i32) -> i16 = unsafe { std::mem::transmute(ASYNC_STATE.load(Ordering::Relaxed)) };
    unsafe { original(vk) }
}

unsafe extern "system" fn key_state(vk: i32) -> i16 {
    if blocked(vk) {
        return 0;
    }
    let original: unsafe extern "system" fn(i32) -> i16 = unsafe { std::mem::transmute(KEY_STATE.load(Ordering::Relaxed)) };
    unsafe { original(vk) }
}

fn u32_at(addr: usize) -> usize {
    let mut b = [0u8; 4];
    if mem::read(addr, &mut b) { u32::from_le_bytes(b) as usize } else { 0 }
}

fn u64_at(addr: usize) -> u64 {
    let mut b = [0u8; 8];
    if mem::read(addr, &mut b) { u64::from_le_bytes(b) } else { 0 }
}

fn name_at(addr: usize) -> String {
    let mut b = [0u8; 64];
    mem::read(addr, &mut b);
    let len = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..len]).to_ascii_lowercase()
}

pub fn install() -> usize {
    let base = mem::main_module();
    if mem::image_size(base).is_none() {
        return 0;
    }
    let nt = base + u32_at(base + 0x3c);
    let mut descriptor = base + u32_at(nt + 0x90);
    let mut patched = 0;
    if descriptor == base {
        return 0;
    }
    while u32_at(descriptor + 12) != 0 {
        if name_at(base + u32_at(descriptor + 12)) == "user32.dll" {
            let lookup = match u32_at(descriptor) {
                0 => u32_at(descriptor + 16),
                rva => rva,
            };
            let thunks = base + u32_at(descriptor + 16);
            let mut i = 0;
            loop {
                let entry = u64_at(base + lookup + i * 8);
                if entry == 0 {
                    break;
                }
                if entry >> 63 == 0 {
                    let (slot, target) = match name_at(base + entry as usize + 2).as_str() {
                        "getasynckeystate" => (&ASYNC_STATE, async_state as usize),
                        "getkeystate" => (&KEY_STATE, key_state as usize),
                        _ => {
                            i += 1;
                            continue;
                        }
                    };
                    let current = u64_at(thunks + i * 8) as usize;
                    if current != 0 && current != target {
                        slot.store(current, Ordering::Relaxed);
                        if mem::write(thunks + i * 8, &(target as u64).to_le_bytes()) {
                            patched += 1;
                        }
                    }
                }
                i += 1;
            }
        }
        descriptor += 20;
    }
    patched
}
