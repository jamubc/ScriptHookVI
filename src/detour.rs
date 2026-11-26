use std::ffi::c_void;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use iced_x86::{BlockEncoder, BlockEncoderOptions, Code, Decoder, DecoderOptions, FlowControl, Instruction, InstructionBlock};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualFree,
    VirtualProtect,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::mem;

const PATCH_REL: usize = 5;
const PATCH_ABS: usize = 14;
const BLOCK: usize = 0x1000;

#[derive(Debug, PartialEq, Eq)]
pub enum DetourError {
    Short,
    BadInstruction,
    Relocation,
    Alloc,
    Write,
}

impl fmt::Display for DetourError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            DetourError::Short => "the prologue is too short to redirect",
            DetourError::BadInstruction => "the prologue holds an instruction that cannot be decoded",
            DetourError::Relocation => "a prologue operand cannot be relocated into the trampoline",
            DetourError::Alloc => "could not allocate executable memory for the trampoline",
            DetourError::Write => "could not write the target or the trampoline",
        };
        f.write_str(text)
    }
}

fn needs_abs(target: usize, replacement: usize) -> bool {
    let rel = replacement as i64 - (target as i64 + PATCH_REL as i64);
    i32::try_from(rel).is_err()
}

fn patch_bytes(target: usize, replacement: usize) -> Vec<u8> {
    if needs_abs(target, replacement) {
        let mut out = vec![0xff, 0x25, 0x00, 0x00, 0x00, 0x00];
        out.extend_from_slice(&(replacement as u64).to_le_bytes());
        out
    } else {
        let mut out = vec![0xe9];
        out.extend_from_slice(&(((replacement as i64) - (target as i64 + PATCH_REL as i64)) as i32).to_le_bytes());
        out
    }
}

fn relocate(prologue: &[u8], target: u64, min: usize, tramp: u64) -> Result<(Vec<u8>, usize), DetourError> {
    let mut decoder = Decoder::with_ip(64, prologue, target, DecoderOptions::NONE);
    let mut instrs = Vec::new();
    let mut len = 0usize;
    let mut fallthrough = true;
    while len < min {
        if !decoder.can_decode() {
            return Err(DetourError::Short);
        }
        let insn = decoder.decode();
        if insn.is_invalid() {
            return Err(DetourError::BadInstruction);
        }
        len += insn.len();
        fallthrough = !matches!(
            insn.flow_control(),
            FlowControl::UnconditionalBranch | FlowControl::IndirectBranch | FlowControl::Return
        );
        instrs.push(insn);
        if !fallthrough && len < min {
            return Err(DetourError::Short);
        }
    }
    if fallthrough {
        instrs.push(Instruction::with_branch(Code::Jmp_rel32_64, target + len as u64).map_err(|_| DetourError::Relocation)?);
    }
    let block = InstructionBlock::new(&instrs, tramp);
    let encoded = BlockEncoder::encode(64, block, BlockEncoderOptions::NONE).map_err(|_| DetourError::Relocation)?;
    Ok((encoded.code_buffer, len))
}

fn alloc_near(target: usize, size: usize) -> Option<usize> {
    const GRAN: usize = 0x10000;
    const RANGE: usize = 0x7fff_0000;
    let base = target & !(GRAN - 1);
    let mut step = GRAN;
    while step < RANGE {
        for cand in [base.checked_sub(step), base.checked_add(step)].into_iter().flatten() {
            // SAFETY: VirtualAlloc with a hint address returns null when the region is unavailable; we never deref it.
            let p = unsafe { VirtualAlloc(cand as *const c_void, size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) };
            if !p.is_null() {
                return Some(p as usize);
            }
        }
        step += GRAN;
    }
    // SAFETY: a null hint lets the OS pick any address; we check for failure before use.
    let p = unsafe { VirtualAlloc(std::ptr::null(), size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) };
    (!p.is_null()).then_some(p as usize)
}

fn free(block: usize) {
    // SAFETY: block came from VirtualAlloc; MEM_RELEASE with size 0 releases the whole reservation.
    unsafe { VirtualFree(block as *mut c_void, 0, MEM_RELEASE) };
}

fn flush(addr: usize, len: usize) {
    // SAFETY: FlushInstructionCache only reads the range to synchronise the i-cache; a bad range is a no-op.
    unsafe { FlushInstructionCache(GetCurrentProcess(), addr as *const c_void, len) };
}

fn write_exec(block: usize, code: &[u8]) -> bool {
    // SAFETY: block is a fresh VirtualAlloc reservation of BLOCK bytes, larger than code, writable (PAGE_EXECUTE_READWRITE).
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len()) };
    let mut old = 0u32;
    // SAFETY: block/len lie inside the reservation; old receives the previous protection.
    let ok = unsafe { VirtualProtect(block as *const c_void, code.len(), PAGE_EXECUTE_READ, &mut old) } != 0;
    flush(block, code.len());
    ok
}

struct Prepared {
    target: usize,
    block: usize,
    trampoline: usize,
    original: Vec<u8>,
    patch: Vec<u8>,
}

impl Prepared {
    fn build(target: usize, replacement: usize) -> Result<Prepared, DetourError> {
        let block = alloc_near(target, BLOCK).ok_or(DetourError::Alloc)?;
        let relay = block + BLOCK - PATCH_ABS;
        let via = if needs_abs(target, replacement) && !needs_abs(target, relay) { relay } else { replacement };
        let patch = patch_bytes(target, via);
        let min = patch.len();
        let want = min + 15;
        let mut buf = vec![0u8; want];
        if !mem::read(target, &mut buf) {
            free(block);
            return Err(DetourError::Short);
        }
        let (code, stolen) = match relocate(&buf, target as u64, min, block as u64) {
            Ok(value) => value,
            Err(e) => {
                free(block);
                return Err(e);
            }
        };
        if code.len() > BLOCK - PATCH_ABS {
            free(block);
            return Err(DetourError::Alloc);
        }
        let mut body = vec![0xccu8; BLOCK];
        body[..code.len()].copy_from_slice(&code);
        body[BLOCK - PATCH_ABS..BLOCK - PATCH_ABS + 6].copy_from_slice(&[0xff, 0x25, 0x00, 0x00, 0x00, 0x00]);
        body[BLOCK - PATCH_ABS + 6..].copy_from_slice(&(replacement as u64).to_le_bytes());
        if !write_exec(block, &body) {
            free(block);
            return Err(DetourError::Write);
        }
        Ok(Prepared { target, block, trampoline: block, original: buf[..stolen].to_vec(), patch })
    }

    fn apply(self) -> Result<Hook, DetourError> {
        if !mem::write(self.target, &self.patch) {
            free(self.block);
            return Err(DetourError::Write);
        }
        flush(self.target, self.patch.len());
        Ok(Hook { target: self.target, block: self.block, trampoline: self.trampoline, original: self.original })
    }
}

pub struct Hook {
    target: usize,
    block: usize,
    trampoline: usize,
    original: Vec<u8>,
}

impl Hook {
    pub fn trampoline(&self) -> usize {
        self.trampoline
    }

    pub fn remove(mut self) -> bool {
        let ok = self.restore();
        free(self.block);
        self.block = 0;
        ok
    }

    fn detach(mut self) -> bool {
        let ok = self.restore();
        self.block = 0;
        ok
    }

    fn restore(&mut self) -> bool {
        let ok = mem::write(self.target, &self.original);
        flush(self.target, self.original.len());
        ok
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        if self.block != 0 {
            self.restore();
            free(self.block);
            self.block = 0;
        }
    }
}

pub fn install(target: usize, replacement: usize) -> Result<Hook, DetourError> {
    Prepared::build(target, replacement)?.apply()
}

static TICK_ON: AtomicBool = AtomicBool::new(false);
static TICK_BUSY: AtomicBool = AtomicBool::new(false);
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static WORKER: AtomicUsize = AtomicUsize::new(0);
static TICK_HOOK: Mutex<Option<Hook>> = Mutex::new(None);

#[unsafe(naked)]
unsafe extern "C" fn tick_thunk() {
    core::arch::naked_asm!(
        "sub rsp, 0x88",
        "mov [rsp+0x20], rcx",
        "mov [rsp+0x28], rdx",
        "mov [rsp+0x30], r8",
        "mov [rsp+0x38], r9",
        "movaps [rsp+0x40], xmm0",
        "movaps [rsp+0x50], xmm1",
        "movaps [rsp+0x60], xmm2",
        "movaps [rsp+0x70], xmm3",
        "call {worker}",
        "mov rcx, [rsp+0x20]",
        "mov rdx, [rsp+0x28]",
        "mov r8, [rsp+0x30]",
        "mov r9, [rsp+0x38]",
        "movaps xmm0, [rsp+0x40]",
        "movaps xmm1, [rsp+0x50]",
        "movaps xmm2, [rsp+0x60]",
        "movaps xmm3, [rsp+0x70]",
        "add rsp, 0x88",
        "jmp rax",
        worker = sym tick_worker,
    )
}

unsafe extern "system" fn tick_worker() -> usize {
    if TICK_ON.load(Ordering::Acquire) && !TICK_BUSY.swap(true, Ordering::Acquire) {
        let worker = WORKER.load(Ordering::Acquire);
        if worker != 0 {
            // SAFETY: WORKER holds a `fn()` pointer stored by tick_install before the target was patched; it stays valid until tick_remove clears it under the same ordering.
            let run: fn() = unsafe { std::mem::transmute(worker) };
            let _ = std::panic::catch_unwind(run);
        }
        TICK_BUSY.store(false, Ordering::Release);
    }
    TRAMPOLINE.load(Ordering::Acquire)
}

pub fn tick_active() -> bool {
    TICK_ON.load(Ordering::Acquire)
}

pub fn tick_install(target: usize, worker: fn()) -> Result<(), DetourError> {
    tick_remove();
    let prepared = Prepared::build(target, tick_thunk as usize)?;
    TRAMPOLINE.store(prepared.trampoline, Ordering::Release);
    WORKER.store(worker as usize, Ordering::Release);
    TICK_ON.store(true, Ordering::Release);
    match prepared.apply() {
        Ok(hook) => {
            *TICK_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
            Ok(())
        }
        Err(e) => {
            TICK_ON.store(false, Ordering::Release);
            WORKER.store(0, Ordering::Release);
            Err(e)
        }
    }
}

pub fn tick_remove() {
    TICK_ON.store(false, Ordering::Release);
    if let Some(hook) = TICK_HOOK.lock().unwrap_or_else(|e| e.into_inner()).take() {
        hook.detach();
    }
    WORKER.store(0, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::sync::atomic::AtomicI32;

    fn decode_all(bytes: &[u8], ip: u64) -> Vec<Instruction> {
        let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
        let mut out = Vec::new();
        while decoder.can_decode() {
            out.push(decoder.decode());
        }
        out
    }

    #[test]
    fn plain_prologue_relocates_byte_for_byte() {
        let target = 0x1_4000_1000u64;
        let tramp = 0x1_4000_2000u64;
        let prologue = [0x53, 0x48, 0x83, 0xec, 0x20];
        let (code, stolen) = relocate(&prologue, target, 5, tramp).unwrap();
        assert_eq!(stolen, 5);
        assert_eq!(&code[..5], &prologue);
        let back = &code[5..];
        assert_eq!(back[0], 0xe9);
        let rel = i32::from_le_bytes(back[1..5].try_into().unwrap());
        assert_eq!((tramp as i64 + 10 + rel as i64) as u64, target + 5);
    }

    #[test]
    fn rip_relative_lea_keeps_absolute_target() {
        let target = 0x1_4000_1000u64;
        let tramp = 0x1_4000_9000u64;
        let prologue = [0x48, 0x8d, 0x05, 0x00, 0x00, 0x00, 0x00];
        let orig_abs = target + prologue.len() as u64;
        let (code, stolen) = relocate(&prologue, target, 5, tramp).unwrap();
        assert_eq!(stolen, 7);
        let insn = decode_all(&code, tramp)[0];
        assert!(insn.is_ip_rel_memory_operand());
        assert_eq!(insn.ip_rel_memory_address(), orig_abs);
    }

    #[test]
    fn short_jcc_promotes_and_keeps_target() {
        let target = 0x1_4000_1000u64;
        let tramp = 0x1_4000_4000u64;
        let prologue = [0x74, 0x10, 0x48, 0x83, 0xec, 0x20];
        let orig_dest = target + 2 + 0x10;
        let (code, stolen) = relocate(&prologue, target, 5, tramp).unwrap();
        assert_eq!(stolen, 6);
        let jcc = decode_all(&code, tramp)[0];
        assert_eq!(jcc.flow_control(), FlowControl::ConditionalBranch);
        assert_eq!(jcc.near_branch_target(), orig_dest);
    }

    #[test]
    fn jmp_rel32_stub_steals_only_the_jump() {
        let target = 0x1_4000_1000u64;
        let tramp = 0x1_4000_3000u64;
        let real = target + 0x10;
        let mut prologue = vec![0xe9];
        prologue.extend_from_slice(&(((real as i64) - (target as i64 + 5)) as i32).to_le_bytes());
        prologue.extend_from_slice(&[0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc]);
        let (code, stolen) = relocate(&prologue, target, 5, tramp).unwrap();
        assert_eq!(stolen, 5);
        let insn = decode_all(&code, tramp);
        assert_eq!(insn.len(), 1);
        assert_eq!(insn[0].flow_control(), FlowControl::UnconditionalBranch);
        assert_eq!(insn[0].near_branch_target(), real);
    }

    #[test]
    fn prologue_shorter_than_patch_is_refused() {
        let target = 0x1_4000_1000u64;
        let prologue = [0xc3];
        assert_eq!(relocate(&prologue, target, 5, 0x1_4000_2000), Err(DetourError::Short));
    }

    #[test]
    fn rip_relative_out_of_range_is_refused() {
        let target = 0x5_0000_0000u64;
        let tramp = 0x9000_0000_0000u64;
        let prologue = [0x48, 0x8d, 0x05, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(relocate(&prologue, target, 5, tramp), Err(DetourError::Relocation));
    }

    #[test]
    fn patch_bytes_switches_to_absolute_when_far() {
        let near = patch_bytes(0x1_4000_0000, 0x1_4000_0100);
        assert_eq!(near.len(), 5);
        assert_eq!(near[0], 0xe9);
        let far = patch_bytes(0x1_4000_0000, 0x9_0000_0000);
        assert_eq!(far.len(), 14);
        assert_eq!(&far[..2], &[0xff, 0x25]);
        assert_eq!(u64::from_le_bytes(far[6..14].try_into().unwrap()), 0x9_0000_0000);
    }

    #[inline(never)]
    extern "C" fn demo_add(a: i32, b: i32) -> i32 {
        let mut acc = black_box(a);
        acc = black_box(acc).wrapping_add(b);
        acc = black_box(acc).wrapping_add(7);
        black_box(acc).wrapping_sub(7)
    }

    static TRAMP_ADD: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn hooked_add(a: i32, b: i32) -> i32 {
        // SAFETY: TRAMP_ADD holds the trampoline address of a live hook on demo_add with the same signature.
        let orig: extern "C" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(TRAMP_ADD.load(Ordering::Acquire)) };
        orig(a, b) + 100
    }

    #[test]
    fn install_and_remove_round_trips() {
        let target: extern "C" fn(i32, i32) -> i32 = demo_add;
        assert_eq!(black_box(target)(2, 3), 5);
        let hook = install(demo_add as usize, hooked_add as usize).unwrap();
        TRAMP_ADD.store(hook.trampoline(), Ordering::Release);
        assert_eq!(black_box(target)(2, 3), 105);
        assert!(hook.remove());
        assert_eq!(black_box(target)(2, 3), 5);
    }

    static REAL_HITS: AtomicI32 = AtomicI32::new(0);

    extern "C" fn stub_real() -> i32 {
        REAL_HITS.fetch_add(1, Ordering::Release);
        7
    }

    static TRAMP_STUB: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn hooked_stub() -> i32 {
        // SAFETY: TRAMP_STUB holds the trampoline address of a live hook on a jmp-rel32 stub returning i32.
        let orig: extern "C" fn() -> i32 = unsafe { std::mem::transmute(TRAMP_STUB.load(Ordering::Acquire)) };
        orig() + 100
    }

    #[test]
    fn jmp_rel32_stub_installs_end_to_end() {
        let stub = alloc_near(stub_real as usize, BLOCK).unwrap();
        let mut body = vec![0xe9];
        body.extend_from_slice(&(((stub_real as usize as i64) - (stub as i64 + 5)) as i32).to_le_bytes());
        assert!(write_exec(stub, &body));
        // SAFETY: stub points at a 5-byte jmp rel32 to stub_real; calling it enters stub_real with no arguments.
        let call_stub: extern "C" fn() -> i32 = unsafe { std::mem::transmute(stub) };
        assert_eq!(black_box(call_stub)(), 7);
        let hook = install(stub, hooked_stub as usize).unwrap();
        TRAMP_STUB.store(hook.trampoline(), Ordering::Release);
        REAL_HITS.store(0, Ordering::Release);
        assert_eq!(black_box(call_stub)(), 107);
        assert_eq!(REAL_HITS.load(Ordering::Acquire), 1);
        assert!(hook.remove());
        assert_eq!(black_box(call_stub)(), 7);
        free(stub);
    }
}
