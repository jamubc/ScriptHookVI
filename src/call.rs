use std::arch::global_asm;
use std::cell::Cell;
use std::sync::Once;

use mlua::{IntoLuaMulti, Lua, MultiValue, Result, Value};
use windows_sys::Win32::Foundation::EXCEPTION_STACK_OVERFLOW;
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    EXCEPTION_RECORD, RtlLookupFunctionEntry, RtlUnwind, RtlVirtualUnwind, UNW_FLAG_NHANDLER,
};

use crate::mem;

pub const MAX_ARGS: usize = 32;

#[repr(C)]
struct Frame {
    target: usize,
    ints: [u64; 4],
    floats: [u64; 4],
    stack: *const u64,
    count: usize,
    rax: u64,
    xmm0: u64,
    code: u32,
    at: usize,
    rbp: u64,
    sp: u64,
}

thread_local! {
    static ACTIVE: Cell<usize> = const { Cell::new(0) };
    static WALKING: Cell<bool> = const { Cell::new(false) };
}

static VECTORED: Once = Once::new();

global_asm!(
    ".globl scripthookvi_call",
    ".globl scripthookvi_call_return",
    ".globl scripthookvi_call_resume",
    "scripthookvi_call:",
    ".seh_proc scripthookvi_call",
    "push rbp",
    ".seh_pushreg rbp",
    "push rbx",
    ".seh_pushreg rbx",
    "push rsi",
    ".seh_pushreg rsi",
    "push rdi",
    ".seh_pushreg rdi",
    "push r12",
    ".seh_pushreg r12",
    "push r13",
    ".seh_pushreg r13",
    "push r14",
    ".seh_pushreg r14",
    "push r15",
    ".seh_pushreg r15",
    "sub rsp, 184",
    ".seh_stackalloc 184",
    "movaps [rsp + 16], xmm6",
    ".seh_savexmm xmm6, 16",
    "movaps [rsp + 32], xmm7",
    ".seh_savexmm xmm7, 32",
    "movaps [rsp + 48], xmm8",
    ".seh_savexmm xmm8, 48",
    "movaps [rsp + 64], xmm9",
    ".seh_savexmm xmm9, 64",
    "movaps [rsp + 80], xmm10",
    ".seh_savexmm xmm10, 80",
    "movaps [rsp + 96], xmm11",
    ".seh_savexmm xmm11, 96",
    "movaps [rsp + 112], xmm12",
    ".seh_savexmm xmm12, 112",
    "movaps [rsp + 128], xmm13",
    ".seh_savexmm xmm13, 128",
    "movaps [rsp + 144], xmm14",
    ".seh_savexmm xmm14, 144",
    "movaps [rsp + 160], xmm15",
    ".seh_savexmm xmm15, 160",
    "mov rbp, rsp",
    ".seh_setframe rbp, 0",
    ".seh_handler scripthookvi_fault, @except",
    ".seh_endprologue",
    "mov [rbp + 8], rcx",
    "mov rbx, rcx",
    "mov [rbx + 120], rbp",
    "stmxcsr [rbp]",
    "fnstcw [rbp + 4]",
    "mov rcx, [rbx + 80]",
    "sub rsp, {room}",
    "mov r10, [rbx + 72]",
    "xor eax, eax",
    "2:",
    "cmp rax, rcx",
    "jae 3f",
    "mov r11, [r10 + rax * 8]",
    "mov [rsp + rax * 8], r11",
    "inc rax",
    "jmp 2b",
    "3:",
    "sub rsp, 32",
    "mov [rbx + 128], rsp",
    "mov rcx, [rbx + 8]",
    "mov rdx, [rbx + 16]",
    "mov r8, [rbx + 24]",
    "mov r9, [rbx + 32]",
    "movq xmm0, [rbx + 40]",
    "movq xmm1, [rbx + 48]",
    "movq xmm2, [rbx + 56]",
    "movq xmm3, [rbx + 64]",
    "call [rbx]",
    "scripthookvi_call_return:",
    "mov [rbx + 88], rax",
    "movq [rbx + 96], xmm0",
    "xor eax, eax",
    "4:",
    "movaps xmm6, [rbp + 16]",
    "movaps xmm7, [rbp + 32]",
    "movaps xmm8, [rbp + 48]",
    "movaps xmm9, [rbp + 64]",
    "movaps xmm10, [rbp + 80]",
    "movaps xmm11, [rbp + 96]",
    "movaps xmm12, [rbp + 112]",
    "movaps xmm13, [rbp + 128]",
    "movaps xmm14, [rbp + 144]",
    "movaps xmm15, [rbp + 160]",
    "lea rsp, [rbp + 184]",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rdi",
    "pop rsi",
    "pop rbx",
    "pop rbp",
    "ret",
    "scripthookvi_call_resume:",
    "ldmxcsr [rbp]",
    "fldcw [rbp + 4]",
    "cld",
    "mov eax, 1",
    "jmp 4b",
    ".seh_endproc",
    "scripthookvi_fault:",
    ".seh_proc scripthookvi_fault",
    "push rbx",
    ".seh_pushreg rbx",
    "push rsi",
    ".seh_pushreg rsi",
    "sub rsp, 40",
    ".seh_stackalloc 40",
    ".seh_endprologue",
    "mov rbx, rdx",
    "mov rsi, rcx",
    "call {caught}",
    "test al, al",
    "jz 2f",
    "mov rcx, rbx",
    "lea rdx, [rip + scripthookvi_call_resume]",
    "mov r8, rsi",
    "mov r9d, 1",
    "call {unwind}",
    "2:",
    "mov eax, 1",
    "add rsp, 40",
    "pop rsi",
    "pop rbx",
    "ret",
    ".seh_endproc",
    caught = sym caught,
    unwind = sym RtlUnwind,
    room = const (MAX_ARGS - 4) * 8,
);

unsafe extern "system" {
    fn scripthookvi_call(frame: *mut Frame) -> u32;
    static scripthookvi_call_return: u8;
    static scripthookvi_call_resume: u8;
}

fn hardware(record: &EXCEPTION_RECORD) -> bool {
    let code = record.ExceptionCode as u32;
    record.ExceptionFlags & 6 == 0 && (code >> 28 == 0xC || (0x8000_0001..=0x8000_0003).contains(&code))
}

unsafe extern "system" fn caught(record: *const EXCEPTION_RECORD, establisher: *const *mut Frame) -> bool {
    // SAFETY: the dispatcher passes a live exception record, and the establisher frame of scripthookvi_call is its rbp, whose second slot holds the Frame it was given.
    let (record, frame) = unsafe { (&*record, &mut **establisher.add(1)) };
    if record.ExceptionFlags & 6 != 0 {
        return false;
    }
    frame.code = record.ExceptionCode as u32;
    frame.at = record.ExceptionAddress as usize;
    true
}

fn reaches(mut context: CONTEXT, sp: u64, rbp: u64) -> bool {
    while context.Rsp < sp {
        let before = context.Rsp;
        let mut base = 0;
        // SAFETY: the walk reads only the stack below the bridge and the unwind data of loaded images; a fault while it reads brings the vectored handler back with WALKING set, and it resumes at the bridge.
        unsafe {
            let entry = RtlLookupFunctionEntry(context.Rip, &mut base, std::ptr::null_mut());
            if entry.is_null() {
                context.Rip = *(context.Rsp as *const u64);
                context.Rsp += 8;
            } else {
                let (mut data, mut establisher) = (std::ptr::null_mut(), 0);
                RtlVirtualUnwind(
                    UNW_FLAG_NHANDLER,
                    base,
                    context.Rip,
                    entry,
                    &mut context,
                    &mut data,
                    &mut establisher,
                    std::ptr::null_mut(),
                );
            }
        }
        if context.Rsp <= before {
            return false;
        }
    }
    context.Rsp == sp && context.Rbp == rbp && context.Rip == &raw const scripthookvi_call_return as u64
}

unsafe extern "system" fn vectored(info: *mut EXCEPTION_POINTERS) -> i32 {
    let frame = ACTIVE.get() as *mut Frame;
    // SAFETY: the system passes live exception pointers, and ACTIVE holds the Frame of the innermost bridge running on this thread, or null.
    let (record, context, frame) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord, frame.as_mut()) };
    let Some(frame) = frame.filter(|_| hardware(record)) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    if !WALKING.replace(true) {
        frame.code = record.ExceptionCode as u32;
        frame.at = record.ExceptionAddress as usize;
        let walks = record.ExceptionCode != EXCEPTION_STACK_OVERFLOW && reaches(*context, frame.sp, frame.rbp);
        WALKING.set(false);
        if walks {
            return EXCEPTION_CONTINUE_SEARCH;
        }
    }
    WALKING.set(false);
    context.Rip = &raw const scripthookvi_call_resume as u64;
    (context.Rbp, context.Rsp) = (frame.rbp, frame.rbp);
    EXCEPTION_CONTINUE_EXECUTION
}

#[cfg(plugins)]
pub(crate) fn active() -> usize {
    ACTIVE.get()
}

#[cfg(plugins)]
pub(crate) fn set_active(frame: usize) {
    ACTIVE.set(frame);
}

pub(crate) fn invoke(target: usize, args: &[u64]) -> std::result::Result<(u64, u64), String> {
    let mut frame = Frame {
        target,
        ints: [0; 4],
        floats: [0; 4],
        stack: std::ptr::null(),
        count: 0,
        rax: 0,
        xmm0: 0,
        code: 0,
        at: 0,
        rbp: 0,
        sp: 0,
    };
    for (i, &bits) in args.iter().take(4).enumerate() {
        frame.ints[i] = bits;
        frame.floats[i] = bits;
    }
    let stack = args.get(4..).unwrap_or_default();
    (frame.stack, frame.count) = (stack.as_ptr(), stack.len());
    // SAFETY: registering has no preconditions, and vectored only acts on exceptions raised while ACTIVE is set on its own thread.
    VECTORED.call_once(|| unsafe {
        AddVectoredExceptionHandler(0, Some(vectored));
    });
    let outer = ACTIVE.replace(&raw mut frame as usize);
    // SAFETY: scripthookvi_call reads the frame and calls its target; a fault inside comes back as a nonzero status instead of unwinding into Rust.
    let status = unsafe { scripthookvi_call(&raw mut frame) };
    ACTIVE.set(outer);
    match status {
        0 => Ok((frame.rax, frame.xmm0)),
        _ => Err(format!("fault {:#010X} at {:#x}", frame.code, frame.at)),
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    Ptr,
    Bool,
    F32,
    F64,
    Void,
}

fn kind(name: &str) -> Option<Kind> {
    Some(match name.trim() {
        "i8" => Kind::I8,
        "u8" => Kind::U8,
        "i16" => Kind::I16,
        "u16" => Kind::U16,
        "i32" => Kind::I32,
        "u32" => Kind::U32,
        "i64" => Kind::I64,
        "u64" => Kind::U64,
        "ptr" => Kind::Ptr,
        "bool" => Kind::Bool,
        "f32" => Kind::F32,
        "f64" => Kind::F64,
        "void" => Kind::Void,
        _ => return None,
    })
}

pub fn signature(text: &str) -> Option<(Kind, Vec<Kind>)> {
    let (ret, rest) = text.split_once('(')?;
    let list = rest.trim_end().strip_suffix(')')?;
    let args = match list.trim() {
        "" => Vec::new(),
        list => list.split(',').map(kind).collect::<Option<Vec<_>>>()?,
    };
    (args.len() <= MAX_ARGS && !args.contains(&Kind::Void)).then_some((kind(ret)?, args))
}

pub fn arg(kind: Kind, value: &Value) -> Result<u64> {
    Ok(match (kind, value) {
        (Kind::F32, Value::Integer(i)) => u64::from((*i as f32).to_bits()),
        (Kind::F32, Value::Number(n)) => u64::from((*n as f32).to_bits()),
        (Kind::F64, Value::Integer(i)) => (*i as f64).to_bits(),
        (Kind::F64, Value::Number(n)) => n.to_bits(),
        (Kind::F32 | Kind::F64, _) => return Err(mlua::Error::runtime("float arguments must be numbers")),
        (_, Value::Integer(i)) => *i as u64,
        (_, Value::Boolean(b)) => u64::from(*b),
        (_, Value::Nil) => 0,
        _ => return Err(mlua::Error::runtime("arguments must be integers, booleans or nil")),
    })
}

pub fn run(lua: &Lua, addr: i64, ret: Kind, args: &[u64]) -> Result<MultiValue> {
    if args.len() > MAX_ARGS {
        return Err(mlua::Error::runtime(format!("at most {MAX_ARGS} arguments")));
    }
    if mem::remote() {
        return Err(mlua::Error::runtime("calls need the injected engine; this one is external"));
    }
    let (rax, xmm0) = match invoke(addr as usize, args) {
        Ok(out) => out,
        Err(fault) => return (Value::Nil, fault).into_lua_multi(lua),
    };
    let value = match ret {
        Kind::I8 => Value::Integer(i64::from(rax as i8)),
        Kind::U8 => Value::Integer(i64::from(rax as u8)),
        Kind::I16 => Value::Integer(i64::from(rax as i16)),
        Kind::U16 => Value::Integer(i64::from(rax as u16)),
        Kind::I32 => Value::Integer(i64::from(rax as i32)),
        Kind::U32 => Value::Integer(i64::from(rax as u32)),
        Kind::I64 | Kind::U64 | Kind::Ptr => Value::Integer(rax as i64),
        Kind::Bool => Value::Boolean(rax as u8 != 0),
        Kind::F32 => Value::Number(f64::from(f32::from_bits(xmm0 as u32))),
        Kind::F64 => Value::Number(f64::from_bits(xmm0)),
        Kind::Void => Value::Boolean(true),
    };
    value.into_lua_multi(lua)
}
