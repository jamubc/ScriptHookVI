use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use mlua::{Either, Function, IntoLuaMulti, Lua, MultiValue, Result, Table, Value, Variadic};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK, XINPUT_GAMEPAD_DPAD_DOWN,
    XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER,
    XINPUT_GAMEPAD_LEFT_THUMB, XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_RIGHT_THUMB, XINPUT_GAMEPAD_START,
    XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE,
};

use crate::call::{self, Kind};
use crate::canvas::{self, Cmd, Image, Renderer};
use crate::config::{Config, Item, valid_key};
use crate::overlay::{Overlay, Panel};
#[cfg(plugins)]
use crate::plugin;
use crate::{detour, hook, mem, process};

const MIN_TITLE: usize = 64 << 20;

struct HookState {
    lua: usize,
    shared: usize,
    start: Instant,
    failed: String,
}

static HOOK: Mutex<Option<HookState>> = Mutex::new(None);
static PENDING: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
static HOOK_TICKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static IN_HOOK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const TICK_OWNER: &str = "SCRIPTHOOKVI_TICK";

fn tick_owner(generation: &str) -> String {
    format!("{:x}:{generation}", std::process::id())
}

fn tick_taken(generation: &str) -> bool {
    let prefix = format!("{:x}:", std::process::id());
    std::env::var(TICK_OWNER).is_ok_and(|owner| owner.starts_with(&prefix) && owner != tick_owner(generation))
}

fn tick_release(generation: &str) {
    detour::tick_remove();
    if std::env::var(TICK_OWNER).ok() == Some(tick_owner(generation)) {
        unsafe { std::env::remove_var(TICK_OWNER) };
    }
}

fn hook_worker() {
    let Ok(mut guard) = HOOK.try_lock() else {
        return;
    };
    let Some(state) = guard.as_mut() else {
        return;
    };
    // SAFETY: lua and shared point to live locals of run() on the engine thread; run() clears HOOK before they are dropped, and the HOOK mutex serialises this call with the engine thread so the Lua state and Shared are never touched concurrently.
    let lua = unsafe { &*(state.lua as *const Option<Lua>) };
    // SAFETY: see above; shared is the RefCell owned by run()'s Rc for the whole of run().
    let shared = unsafe { &*(state.shared as *const RefCell<Shared>) };
    let Some(lua) = lua.as_ref() else {
        return;
    };
    shared.borrow_mut().cmds.clear();
    HOOK_TICKED.store(state.start.elapsed().as_millis() as u64 + 1, std::sync::atomic::Ordering::Release);
    IN_HOOK.store(true, std::sync::atomic::Ordering::Release);
    if let Err(e) = tick(lua, state.start.elapsed().as_secs_f64() * 1000.0) {
        let text = e.to_string();
        if text != state.failed {
            crate::log(&format!("tick: {text}"));
            state.failed = text;
        }
    }
    #[cfg(plugins)]
    plugin::tick(lua);
    IN_HOOK.store(false, std::sync::atomic::Ordering::Release);
    let cmds = std::mem::take(&mut shared.borrow_mut().cmds);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = cmds;
}

struct Shared {
    cmds: Vec<Cmd>,
    size: (i32, i32),
    base: usize,
    image: usize,
    found: bool,
    mode: &'static str,
    pid: u32,
    looked: Option<Instant>,
    walked: bool,
    reload: bool,
    tick_request: Option<Option<usize>>,
    scripts: PathBuf,
    images: Vec<(PathBuf, Arc<Image>)>,
}

pub enum Mode {
    Internal,
    External { names: Vec<String>, overlay: bool },
}

fn shared(mode: &'static str, size: (i32, i32), pid: u32, scripts: &Path) -> Shared {
    Shared {
        cmds: Vec::new(),
        size,
        base: mem::main_module(),
        image: mem::image_size(mem::main_module()).unwrap_or(0),
        found: false,
        mode,
        pid,
        looked: None,
        walked: false,
        reload: false,
        tick_request: None,
        scripts: scripts.to_path_buf(),
        images: Vec::new(),
    }
}

fn detect(shared: &mut Shared) {
    if shared.found || shared.looked.is_some_and(|at| at.elapsed() < Duration::from_secs(2)) {
        return;
    }
    shared.looked = Some(Instant::now());
    let main = mem::main_module();
    let module = process::modules(shared.pid)
        .into_iter()
        .filter(|&(base, size)| base != main && size >= MIN_TITLE)
        .max_by_key(|&(_, size)| size);
    let walk = module.is_none() && !(mem::remote() && shared.walked);
    shared.walked |= walk;
    if let Some((base, size)) = module.or_else(|| walk.then(|| mem::find_title(MIN_TITLE)).flatten()) {
        (shared.base, shared.image, shared.found) = (base, size, true);
        crate::log(&format!("title image at {base:#x}, {size:#x} bytes"));
    }
}

fn read_value(addr: usize, kind: &str) -> Result<Value> {
    let size = match kind {
        "u8" | "i8" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" | "ptr" => 8,
        _ => return Err(mlua::Error::runtime(format!("unknown type {kind}"))),
    };
    let mut b = [0u8; 8];
    if !mem::read(addr, &mut b[..size]) {
        return Ok(Value::Nil);
    }
    let raw = u64::from_le_bytes(b);
    Ok(match kind {
        "i8" => Value::Integer(i64::from(raw as i8)),
        "i16" => Value::Integer(i64::from(raw as i16)),
        "i32" => Value::Integer(i64::from(raw as i32)),
        "f32" => Value::Number(f64::from(f32::from_bits(raw as u32))),
        "f64" => Value::Number(f64::from_bits(raw)),
        _ => Value::Integer(raw as i64),
    })
}

fn write_value(addr: usize, kind: &str, value: Value) -> Result<bool> {
    let number = match value {
        Value::Integer(i) => i as f64,
        Value::Number(n) => n,
        Value::Boolean(b) => f64::from(u8::from(b)),
        _ => return Err(mlua::Error::runtime("value must be a number or boolean")),
    };
    let int = match value {
        Value::Integer(i) => i as u64,
        Value::Number(n) => n as i64 as u64,
        _ => number as u64,
    };
    let bytes = match kind {
        "f32" => (number as f32).to_le_bytes().to_vec(),
        "f64" => number.to_le_bytes().to_vec(),
        "u8" | "i8" => int.to_le_bytes()[..1].to_vec(),
        "u16" | "i16" => int.to_le_bytes()[..2].to_vec(),
        "u32" | "i32" => int.to_le_bytes()[..4].to_vec(),
        "u64" | "i64" | "ptr" => int.to_le_bytes().to_vec(),
        _ => return Err(mlua::Error::runtime(format!("unknown type {kind}"))),
    };
    Ok(mem::write(addr, &bytes))
}

type GetState = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;

const BUTTONS: [(&str, u16); 14] = [
    ("a", XINPUT_GAMEPAD_A),
    ("b", XINPUT_GAMEPAD_B),
    ("x", XINPUT_GAMEPAD_X),
    ("y", XINPUT_GAMEPAD_Y),
    ("lb", XINPUT_GAMEPAD_LEFT_SHOULDER),
    ("rb", XINPUT_GAMEPAD_RIGHT_SHOULDER),
    ("back", XINPUT_GAMEPAD_BACK),
    ("start", XINPUT_GAMEPAD_START),
    ("ls", XINPUT_GAMEPAD_LEFT_THUMB),
    ("rs", XINPUT_GAMEPAD_RIGHT_THUMB),
    ("dpad_up", XINPUT_GAMEPAD_DPAD_UP),
    ("dpad_down", XINPUT_GAMEPAD_DPAD_DOWN),
    ("dpad_left", XINPUT_GAMEPAD_DPAD_LEFT),
    ("dpad_right", XINPUT_GAMEPAD_DPAD_RIGHT),
];

fn xinput() -> Option<GetState> {
    static GET_STATE: OnceLock<Option<GetState>> = OnceLock::new();
    *GET_STATE.get_or_init(|| {
        ["xinput1_4.dll", "xinput9_1_0.dll"].into_iter().find_map(|name| {
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                return None;
            }
            let get = unsafe { GetProcAddress(module, c"XInputGetState".as_ptr().cast()) }?;
            Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, GetState>(get) })
        })
    })
}

fn read_pad(slot: &Cell<(Option<u32>, Option<Instant>)>) -> Option<XINPUT_GAMEPAD> {
    let get = xinput()?;
    let (index, looked) = slot.get();
    let mut state = XINPUT_STATE::default();
    if let Some(index) = index {
        if unsafe { get(index, &mut state) } == 0 {
            return Some(state.Gamepad);
        }
    }
    if looked.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
        slot.set((None, looked));
        return None;
    }
    let found = (0..4).find(|&i| unsafe { get(i, &mut state) } == 0);
    slot.set((found, Some(Instant::now())));
    found.map(|_| state.Gamepad)
}

fn to_lua(lua: &Lua, item: &Item) -> Result<Value> {
    Ok(match item {
        Item::Bool(b) => Value::Boolean(*b),
        Item::Int(i) => Value::Integer(*i),
        Item::Num(n) => Value::Number(*n),
        Item::Text(s) => Value::String(lua.create_string(s)?),
    })
}

fn text_options(rest: &mut MultiValue) -> Result<(f32, bool, Option<String>, Option<f32>)> {
    let Some(Value::Table(options)) = rest.back().cloned() else {
        return Ok((0.0, false, None, None));
    };
    rest.pop_back();
    Ok((
        options.get::<Option<f32>>("spacing")?.unwrap_or(0.0),
        options.get::<Option<bool>>("tabular")?.unwrap_or(false),
        options.get("align")?,
        options.get("max_width")?,
    ))
}

fn joaat(text: &[u8]) -> u32 {
    let mut hash = 0u32;
    for &c in text {
        hash = hash.wrapping_add(u32::from(c.to_ascii_lowercase()));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash.wrapping_add(hash << 15)
}

type RectArgs = (f64, f64, f64, f64, i64, Option<f64>, Option<i64>, Option<String>);

fn register(lua: &Lua, shared: &Rc<RefCell<Shared>>, config: &Rc<RefCell<Config>>, start: Instant) -> Result<()> {
    let globals = lua.globals();

    globals.set(
        "joaat",
        lua.create_function(|_, text: mlua::String| Ok(i64::from(joaat(&text.as_bytes()))))?,
    )?;

    let c = lua.create_table()?;
    let cfg = config.clone();
    c.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Value)| match cfg.borrow().get(&key) {
            Some(item) => to_lua(lua, item),
            None => Ok(default),
        })?,
    )?;
    let cfg = config.clone();
    c.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            if !valid_key(&key) {
                return Err(mlua::Error::runtime(format!("bad config key {key:?}")));
            }
            let item = match value {
                Value::Nil => None,
                Value::Boolean(b) => Some(Item::Bool(b)),
                Value::Integer(i) => Some(Item::Int(i)),
                Value::Number(n) => Some(Item::Num(n)),
                Value::String(s) => Some(Item::Text(s.to_str()?.to_owned())),
                _ => return Err(mlua::Error::runtime("config values are booleans, numbers or strings")),
            };
            cfg.borrow_mut().set(&key, item);
            Ok(())
        })?,
    )?;
    let cfg = config.clone();
    c.set(
        "all",
        lua.create_function(move |lua, ()| {
            let out = lua.create_table()?;
            for (key, item) in cfg.borrow().all() {
                out.set(key.as_str(), to_lua(lua, item)?)?;
            }
            Ok(out)
        })?,
    )?;
    let cfg = config.clone();
    c.set(
        "path",
        lua.create_function(move |_, ()| Ok(cfg.borrow().path().map(|p| p.display().to_string())))?,
    )?;
    globals.set("config", c)?;

    let m = lua.create_table()?;
    let s = shared.clone();
    m.set(
        "base",
        lua.create_function(move |_, ()| {
            detect(&mut s.borrow_mut());
            Ok(s.borrow().base as i64)
        })?,
    )?;
    let s = shared.clone();
    m.set(
        "size",
        lua.create_function(move |_, ()| {
            detect(&mut s.borrow_mut());
            Ok(s.borrow().image as i64)
        })?,
    )?;
    let s = shared.clone();
    m.set("found", lua.create_function(move |_, ()| Ok(s.borrow().found))?)?;
    let s = shared.clone();
    m.set(
        "set_base",
        lua.create_function(move |_, base: i64| {
            let mut s = s.borrow_mut();
            s.base = base as usize;
            s.image = mem::image_size(s.base).unwrap_or(0);
            s.found = true;
            Ok(())
        })?,
    )?;
    m.set(
        "read",
        lua.create_function(|_, (addr, kind): (i64, String)| read_value(addr as usize, &kind))?,
    )?;
    m.set(
        "write",
        lua.create_function(|_, (addr, kind, value): (i64, String, Value)| {
            write_value(addr as usize, &kind, value)
        })?,
    )?;
    m.set(
        "bytes",
        lua.create_function(|lua, (addr, len): (i64, usize)| {
            let mut buf = vec![0u8; len.min(64 << 20)];
            if mem::read(addr as usize, &mut buf) {
                Ok(Value::String(lua.create_string(&buf)?))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;
    m.set(
        "poke",
        lua.create_function(|_, (addr, data): (i64, mlua::String)| Ok(mem::write(addr as usize, &data.as_bytes())))?,
    )?;
    m.set(
        "string",
        lua.create_function(|lua, (addr, max): (i64, Option<usize>)| {
            let max = max.unwrap_or(256).min(1 << 16);
            let mut out = Vec::new();
            let mut byte = [0u8; 1];
            while out.len() < max && mem::read(addr as usize + out.len(), &mut byte) && byte[0] != 0 {
                out.push(byte[0]);
            }
            lua.create_string(&out)
        })?,
    )?;
    m.set(
        "alloc",
        lua.create_function(|_, len: usize| Ok(mem::alloc(len) as i64))?,
    )?;
    let s = shared.clone();
    m.set(
        "scan",
        lua.create_function(move |_, (pattern, start, len, limit): (String, Option<i64>, Option<i64>, Option<usize>)| {
            detect(&mut s.borrow_mut());
            let (base, image) = (s.borrow().base, s.borrow().image);
            let pattern = mem::parse_pattern(&pattern).ok_or_else(|| mlua::Error::runtime("bad pattern"))?;
            let start = start.map_or(base, |v| v as usize);
            let len = len.map_or(image, |v| v as usize);
            Ok(mem::scan(start, len, &pattern, limit.unwrap_or(1))
                .into_iter()
                .map(|a| a as i64)
                .collect::<Vec<_>>())
        })?,
    )?;
    globals.set("mem", m)?;

    let g = lua.create_table()?;
    g.set(
        "call",
        lua.create_function(|lua, (addr, args): (i64, Variadic<Value>)| {
            let args = args.iter().map(|v| call::arg(Kind::I64, v)).collect::<Result<Vec<_>>>()?;
            call::run(lua, addr, Kind::I64, &args)
        })?,
    )?;
    g.set(
        "fn",
        lua.create_function(|lua, (addr, text): (i64, String)| {
            let (ret, kinds) =
                call::signature(&text).ok_or_else(|| mlua::Error::runtime(format!("bad signature {text:?}")))?;
            lua.create_function(move |lua, args: Variadic<Value>| {
                if args.len() != kinds.len() {
                    return Err(mlua::Error::runtime(format!("{text} takes {} arguments, got {}", kinds.len(), args.len())));
                }
                let args = kinds.iter().zip(args.iter()).map(|(&k, v)| call::arg(k, v)).collect::<Result<Vec<_>>>()?;
                call::run(lua, addr, ret, &args)
            })
        })?,
    )?;
    let s = shared.clone();
    g.set("mode", lua.create_function(move |_, ()| Ok(s.borrow().mode))?)?;
    let s = shared.clone();
    g.set("pid", lua.create_function(move |_, ()| Ok(s.borrow().pid))?)?;
    let s = shared.clone();
    g.set(
        "stamp",
        lua.create_function(move |_, ()| {
            detect(&mut s.borrow_mut());
            let s = s.borrow();
            Ok(s.found.then(|| mem::image_stamp(s.base)).flatten())
        })?,
    )?;
    globals.set("game", g)?;

    let i = lua.create_table()?;
    i.set(
        "down",
        lua.create_function(|_, vk: i32| Ok(unsafe { GetAsyncKeyState(vk) } < 0))?,
    )?;
    i.set(
        "block",
        lua.create_function(|_, keys: Option<Vec<u8>>| {
            hook::block(&keys.unwrap_or_default());
            Ok(())
        })?,
    )?;
    let slot = Cell::new((None, None));
    i.set(
        "pad",
        lua.create_function(move |lua, ()| {
            let pad = read_pad(&slot);
            let g = pad.unwrap_or_default();
            let t = lua.create_table()?;
            t.set("connected", pad.is_some())?;
            for (name, bit) in BUTTONS {
                t.set(name, g.wButtons & bit != 0)?;
            }
            let stick = |v: i16| (f64::from(v) / 32767.0).max(-1.0);
            t.set("lx", stick(g.sThumbLX))?;
            t.set("ly", stick(g.sThumbLY))?;
            t.set("rx", stick(g.sThumbRX))?;
            t.set("ry", stick(g.sThumbRY))?;
            t.set("lt", f64::from(g.bLeftTrigger) / 255.0)?;
            t.set("rt", f64::from(g.bRightTrigger) / 255.0)?;
            Ok(t)
        })?,
    )?;
    globals.set("input", i)?;

    let h = lua.create_table()?;
    let s = shared.clone();
    h.set(
        "tick",
        lua.create_function(move |_, addr: Option<i64>| {
            s.borrow_mut().tick_request = Some(addr.map(|a| a as usize));
            Ok(())
        })?,
    )?;
    let s = shared.clone();
    h.set(
        "untick",
        lua.create_function(move |_, ()| {
            s.borrow_mut().tick_request = Some(None);
            Ok(())
        })?,
    )?;
    h.set(
        "ticking",
        lua.create_function(|_, ()| Ok(IN_HOOK.load(std::sync::atomic::Ordering::Acquire)))?,
    )?;
    globals.set("hook", h)?;

    let u = lua.create_table()?;
    let s = shared.clone();
    u.set(
        "rect",
        lua.create_function(
            move |_,
                  (x, y, w, h, color, radius, end, direction): RectArgs| {
                s.borrow_mut().cmds.push(Cmd::Rect {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    w: w.round() as i32,
                    h: h.round() as i32,
                    from: color as u32,
                    to: end.unwrap_or(color) as u32,
                    horizontal: direction.as_deref() == Some("horizontal"),
                    radius: radius.unwrap_or(0.0).round() as i32,
                });
                Ok(())
            },
        )?,
    )?;
    let s = shared.clone();
    u.set(
        "frame",
        lua.create_function(
            move |_, (x, y, w, h, color, thickness, radius): (f64, f64, f64, f64, i64, Option<f32>, Option<f64>)| {
                s.borrow_mut().cmds.push(Cmd::Frame {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    w: w.round() as i32,
                    h: h.round() as i32,
                    color: color as u32,
                    thickness: thickness.unwrap_or(1.0),
                    radius: radius.unwrap_or(0.0).round() as i32,
                });
                Ok(())
            },
        )?,
    )?;
    let s = shared.clone();
    u.set(
        "line",
        lua.create_function(
            move |_, (x1, y1, x2, y2, color, thickness): (f32, f32, f32, f32, i64, Option<f32>)| {
                s.borrow_mut().cmds.push(Cmd::Line { x1, y1, x2, y2, color: color as u32, thickness: thickness.unwrap_or(1.0) });
                Ok(())
            },
        )?,
    )?;
    let s = shared.clone();
    u.set(
        "circle",
        lua.create_function(move |_, (x, y, radius, color): (f32, f32, f32, i64)| {
            s.borrow_mut().cmds.push(Cmd::Circle { x, y, radius, color: color as u32 });
            Ok(())
        })?,
    )?;
    let s = shared.clone();
    u.set(
        "triangle",
        lua.create_function(
            move |_, (x1, y1, x2, y2, x3, y3, color): (f32, f32, f32, f32, f32, f32, i64)| {
                s.borrow_mut().cmds.push(Cmd::Triangle { points: [(x1, y1), (x2, y2), (x3, y3)], color: color as u32 });
                Ok(())
            },
        )?,
    )?;
    let s = shared.clone();
    u.set(
        "clip",
        lua.create_function(move |_, rect: (Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
            let cmd = match rect {
                (Some(x), Some(y), Some(w), Some(h)) => Cmd::Clip {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    w: w.round() as i32,
                    h: h.round() as i32,
                },
                (None, None, None, None) => Cmd::Clip { x: 0, y: 0, w: i32::MAX, h: i32::MAX },
                _ => return Err(mlua::Error::runtime("ui.clip takes x, y, w, h, or nothing to reset")),
            };
            s.borrow_mut().cmds.push(cmd);
            Ok(())
        })?,
    )?;
    let s = shared.clone();
    u.set(
        "image_load",
        lua.create_function(move |lua, path: String| {
            let mut s = s.borrow_mut();
            let full = s.scripts.join(&path);
            let full = std::fs::canonicalize(&full).unwrap_or(full);
            let index = match s.images.iter().position(|(p, _)| *p == full) {
                Some(index) => index,
                None => match std::fs::read(&full).map_err(|e| e.to_string()).and_then(|b| canvas::decode_png(&b)) {
                    Ok(image) => {
                        s.images.push((full, Arc::new(image)));
                        s.images.len() - 1
                    }
                    Err(e) => return (Value::Nil, format!("{path}: {e}")).into_lua_multi(lua),
                },
            };
            let image = &s.images[index].1;
            (index + 1, image.width, image.height).into_lua_multi(lua)
        })?,
    )?;
    let s = shared.clone();
    u.set(
        "image",
        lua.create_function(
            move |_, (id, x, y, w, h, tint): (usize, f64, f64, Option<f64>, Option<f64>, Option<i64>)| {
                let mut s = s.borrow_mut();
                let image = id
                    .checked_sub(1)
                    .and_then(|i| s.images.get(i))
                    .map(|(_, image)| image.clone())
                    .ok_or_else(|| mlua::Error::runtime(format!("no image {id}")))?;
                let w = w.unwrap_or(f64::from(image.width));
                let h = h.unwrap_or(w * f64::from(image.height) / f64::from(image.width));
                s.cmds.push(Cmd::Image {
                    x: x.round() as i32,
                    y: y.round() as i32,
                    w: w.round() as i32,
                    h: h.round() as i32,
                    tint: tint.unwrap_or(0xffff_ffff) as u32,
                    image,
                });
                Ok(())
            },
        )?,
    )?;
    let s = shared.clone();
    u.set(
        "text",
        lua.create_function(move |lua, (x, y, text, mut rest): (f64, f64, String, MultiValue)| {
            let (spacing, tabular, align, max_width) = text_options(&mut rest)?;
            let (color, size, font) = lua.unpack_multi::<(Option<i64>, Option<f32>, Option<String>)>(rest)?;
            let size = size.unwrap_or(16.0).clamp(4.0, 200.0);
            let font = canvas::font_index(font.as_deref());
            let text = match max_width {
                Some(max) => canvas::fit(&text, size, font, spacing, tabular, max),
                None => text,
            };
            let shift = match align.as_deref() {
                None | Some("left") => 0.0,
                Some("center") => canvas::text_width(&text, size, font, spacing, tabular) / 2.0,
                Some("right") => canvas::text_width(&text, size, font, spacing, tabular),
                Some(other) => return Err(mlua::Error::runtime(format!("align is left, center or right, not {other}"))),
            };
            s.borrow_mut().cmds.push(Cmd::Text {
                x: (x - f64::from(shift)).round() as i32,
                y: y.round() as i32,
                size,
                color: color.unwrap_or(0xffff_ffff) as u32,
                font,
                spacing,
                tabular,
                text,
            });
            Ok(())
        })?,
    )?;
    u.set(
        "text_width",
        lua.create_function(|lua, (text, mut rest): (String, MultiValue)| {
            let (spacing, tabular, _, _) = text_options(&mut rest)?;
            let (size, font) = lua.unpack_multi::<(Option<f32>, Option<String>)>(rest)?;
            Ok(canvas::text_width(&text, size.unwrap_or(16.0), canvas::font_index(font.as_deref()), spacing, tabular))
        })?,
    )?;
    u.set(
        "line_height",
        lua.create_function(|_, (size, font): (Option<f32>, Option<String>)| {
            Ok(canvas::line_height(size.unwrap_or(16.0), canvas::font_index(font.as_deref())))
        })?,
    )?;
    let s = shared.clone();
    u.set("width", lua.create_function(move |_, ()| Ok(s.borrow().size.0))?)?;
    let s = shared.clone();
    u.set("height", lua.create_function(move |_, ()| Ok(s.borrow().size.1))?)?;
    globals.set("ui", u)?;

    globals.set(
        "log",
        lua.create_function(|_, args: Variadic<Value>| {
            let parts: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| "?".into()))
                .collect();
            crate::log(&parts.join(" "));
            Ok(())
        })?,
    )?;
    globals.set("print", globals.get::<Function>("log")?)?;
    globals.set(
        "now",
        lua.create_function(move |_, ()| Ok(start.elapsed().as_secs_f64() * 1000.0))?,
    )?;
    Ok(())
}

fn stamps(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .filter_map(|p| {
            let time = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, time))
        })
        .collect();
    files.sort();
    files
}

fn scripts(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut files = stamps(dir);
    let mut folders: Vec<PathBuf> =
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    folders.sort_by_key(|p| (p.file_name().is_some_and(|n| n == "user"), p.clone()));
    for folder in folders {
        files.extend(stamps(&folder));
    }
    files
}

fn setup(lua: &Lua, shared: &Rc<RefCell<Shared>>, config: &Rc<RefCell<Config>>, start: Instant) -> Result<Function> {
    register(lua, shared, config, start)?;
    let run: Function = lua.load(include_str!("prelude.lua")).set_name("prelude").eval()?;
    let v = lua.globals().get::<Table>("scripthookvi")?;
    v.set("version", env!("CARGO_PKG_VERSION"))?;
    let s = shared.clone();
    v.set(
        "reload",
        lua.create_function(move |_, ()| {
            s.borrow_mut().reload = true;
            Ok(())
        })?,
    )?;
    v.set("scripts_dir", shared.borrow().scripts.to_string_lossy().into_owned())?;
    if let Some(home) = crate::home() {
        v.set("log_path", home.join("ScriptHookVI.log").to_string_lossy().into_owned())?;
    }
    let s = shared.clone();
    v.set(
        "rescan",
        lua.create_function(move |_, ()| {
            let mut s = s.borrow_mut();
            (s.found, s.looked, s.walked) = (false, None, false);
            Ok(())
        })?,
    )?;
    v.set(
        "log_to_file",
        lua.create_function(|_, on: bool| {
            crate::log_to_file(on);
            Ok(())
        })?,
    )?;
    Ok(run)
}

fn load(
    files: &[(PathBuf, SystemTime)],
    shared: &Rc<RefCell<Shared>>,
    config: &Rc<RefCell<Config>>,
    start: Instant,
) -> Option<Lua> {
    let lua = Lua::new();
    let run = match setup(&lua, shared, config, start) {
        Ok(run) => run,
        Err(e) => {
            crate::log(&format!("engine setup failed: {e}"));
            return None;
        }
    };
    let dir = shared.borrow().scripts.clone();
    for (path, _) in files {
        let name = path.strip_prefix(&dir).unwrap_or(path).to_string_lossy().replace('\\', "/");
        let chunk = match std::fs::read(path) {
            Ok(code) => match lua.load(code).set_name(format!("@{name}")).into_function() {
                Ok(f) => Either::Left(f),
                Err(e) => Either::Right(e.to_string()),
            },
            Err(e) => Either::Right(e.to_string()),
        };
        if let Err(e) = run.call::<()>((name.as_str(), chunk)) {
            crate::log(&format!("{name}: {e}"));
        }
    }
    Some(lua)
}

fn tick(lua: &Lua, now_ms: f64) -> Result<()> {
    lua.globals()
        .get::<Table>("scripthookvi")
        .and_then(|t| t.get::<Function>("tick"))
        .and_then(|f| f.call(now_ms))
}

pub fn preview(dir: &Path, size: (i32, i32), code: &str, frames: u32) -> Vec<u32> {
    let start = Instant::now();
    let shared = Rc::new(RefCell::new(shared("preview", size, 0, dir)));
    let mut pixels = vec![0u32; (size.0 * size.1) as usize];
    let config = Rc::new(RefCell::new(Config::new(None)));
    let Some(lua) = load(&scripts(dir), &shared, &config, start) else {
        return pixels;
    };
    if let Err(e) = lua.load(code).set_name("preview").exec() {
        crate::log(&format!("preview: {e}"));
    }
    let feeding = || {
        lua.globals()
            .get::<Table>("input")
            .and_then(|t| t.get::<Function>("feeding"))
            .and_then(|f| f.call::<bool>(()))
            .unwrap_or(false)
    };
    let mut frame = 0;
    while frame < frames || feeding() {
        shared.borrow_mut().cmds.clear();
        if let Err(e) = tick(&lua, f64::from(frame) * 16.0) {
            crate::log(&format!("tick: {e}"));
        }
        frame += 1;
    }
    Renderer::default().render(&shared.borrow().cmds, size, &mut pixels, 0);
    pixels
}

fn connect(names: &[String]) -> Option<u32> {
    let pid = names.iter().find_map(|name| process::find(name))?;
    let handle = process::open(pid)?;
    mem::attach(handle, process::main_base(pid));
    Some(pid)
}

pub fn probe(names: &[String], file: &Path, wait: Duration) -> i32 {
    let start = Instant::now();
    let pid = loop {
        if let Some(pid) = connect(names) {
            break pid;
        }
        if start.elapsed() >= wait {
            crate::log("no game process to attach to");
            return 2;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let state = Rc::new(RefCell::new(shared("external", (1920, 1080), pid, file.parent().unwrap_or(Path::new(".")))));
    detect(&mut state.borrow_mut());
    let config = Rc::new(RefCell::new(Config::new(None)));
    let lua = Lua::new();
    let ran = setup(&lua, &state, &config, start)
        .and_then(|_| lua.load(std::fs::read(file)?).set_name(format!("@{}", file.display())).exec());
    match ran {
        Ok(()) => 0,
        Err(e) => {
            crate::log(&format!("{e}"));
            1
        }
    }
}

pub fn run(dir: PathBuf, mode: Mode) {
    let start = Instant::now();
    let config = Rc::new(RefCell::new(Config::new(crate::home().map(|h| h.join("ScriptHookVI.cfg")))));
    let generation = format!("{:?}", SystemTime::now());
    let internal = matches!(mode, Mode::Internal);
    let own = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    if internal {
        unsafe { std::env::set_var("SCRIPTHOOKVI_GENERATION", &generation) };
        crate::log(&format!("keyboard imports of the host redirected: {}", hook::install()));
    }
    let state = Rc::new(RefCell::new(shared(if internal { "internal" } else { "detached" }, (1280, 720), own, &dir)));
    crate::log(&format!("scripts from {}", dir.display()));
    if let Err(e) = std::fs::create_dir_all(dir.join("user")) {
        crate::log(&format!("user scripts folder: {e}"));
    }
    let mut overlay = Overlay::new();
    let mut panel = (!internal).then(Panel::new);
    let mut lua: Option<Lua> = None;
    let mut loaded: Option<Vec<(PathBuf, SystemTime)>> = None;
    let mut checked = start - Duration::from_secs(1);
    let mut failed = String::new();
    let mut tick_target = None;
    loop {
        let cmds;
        let over_game;
        let present_pid;
        let pause;
        {
            let mut hooked = HOOK.lock().unwrap_or_else(|e| e.into_inner());
            config.borrow_mut().poll();
            let reload = std::mem::take(&mut state.borrow_mut().reload);
            if reload {
                loaded = None;
            }
            if reload || checked.elapsed() >= Duration::from_millis(500) {
                checked = Instant::now();
                if internal && std::env::var("SCRIPTHOOKVI_GENERATION").ok().as_deref() != Some(&generation) {
                    tick_release(&generation);
                    *hooked = None;
                    #[cfg(plugins)]
                    plugin::unload();
                    overlay.present(&[], own);
                    hook::block(&[]);
                    config.borrow_mut().flush();
                    crate::log("a newer ScriptHookVI took over");
                    return;
                }
                #[cfg(plugins)]
                {
                    if let Some(home) = crate::home().filter(|_| internal) {
                        plugin::load(home);
                    }
                }
                if let Mode::External { names, .. } = &mode {
                    let attached = mem::remote();
                    if attached && !process::alive(mem::process()) {
                        mem::detach();
                        crate::log("the game went away");
                    }
                    let pid = if mem::remote() { None } else { connect(names) };
                    if let Some(pid) = pid {
                        crate::log(&format!("attached to process {pid}"));
                    }
                    if attached != mem::remote() {
                        let pid = pid.unwrap_or(own);
                        let size = state.borrow().size;
                        *state.borrow_mut() = shared(if mem::remote() { "external" } else { "detached" }, size, pid, &dir);
                        loaded = None;
                    }
                }
                detect(&mut state.borrow_mut());
                let files = scripts(&dir);
                if loaded.as_ref() != Some(&files) {
                    hook::block(&[]);
                    lua = load(&files, &state, &config, start);
                    loaded = Some(files);
                    if detour::tick_active() {
                        *hooked = Some(HookState {
                            lua: &lua as *const Option<Lua> as usize,
                            shared: Rc::as_ptr(&state) as usize,
                            start,
                            failed: String::new(),
                        });
                    }
                }
            }
            let request = state.borrow_mut().tick_request.take();
            match request {
                Some(Some(addr)) if internal && detour::tick_active() && tick_target == Some(addr) => {}
                Some(Some(addr)) if internal && tick_taken(&generation) => {
                    state.borrow_mut().tick_request = Some(Some(addr));
                }
                Some(Some(addr)) if internal => match detour::tick_install(addr, hook_worker) {
                    Ok(()) => {
                        unsafe { std::env::set_var(TICK_OWNER, tick_owner(&generation)) };
                        tick_target = Some(addr);
                        *hooked = Some(HookState {
                            lua: &lua as *const Option<Lua> as usize,
                            shared: Rc::as_ptr(&state) as usize,
                            start,
                            failed: String::new(),
                        });
                        crate::log(&format!("engine tick runs on the game thread via a detour at {addr:#x}"));
                    }
                    Err(e) => crate::log(&format!("hook.tick failed: {e}")),
                },
                Some(Some(_)) => crate::log("hook.tick needs the injected engine"),
                Some(None) => {
                    tick_release(&generation);
                    *hooked = None;
                    crate::log("engine tick returns to the timer");
                }
                None => {}
            }
            let ticked = HOOK_TICKED.load(std::sync::atomic::Ordering::Acquire);
            if detour::tick_active() && ticked + 250 > start.elapsed().as_millis() as u64 {
                cmds = PENDING.lock().unwrap_or_else(|e| e.into_inner()).clone();
            } else {
                state.borrow_mut().cmds.clear();
                if let Some(lua) = &lua {
                    let tick = tick(lua, start.elapsed().as_secs_f64() * 1000.0);
                    if let Err(e) = tick {
                        let text = e.to_string();
                        if text != failed {
                            crate::log(&format!("tick: {text}"));
                            failed = text;
                        }
                    }
                }
                cmds = std::mem::take(&mut state.borrow_mut().cmds);
            }
            over_game = match &mode {
                Mode::Internal => true,
                Mode::External { overlay, .. } => *overlay && mem::remote(),
            };
            present_pid = state.borrow().pid;
            pause = config.borrow().tick();
        }
        let size = if over_game {
            if let Some(panel) = &mut panel {
                panel.hide();
            }
            overlay.present(&cmds, present_pid)
        } else {
            overlay.present(&[], own);
            match panel.as_mut().map(|p| p.present(&cmds)) {
                Some(None) => {
                    tick_release(&generation);
                    *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    config.borrow_mut().flush();
                    crate::log("window closed");
                    return;
                }
                Some(size) => size,
                None => None,
            }
        };
        if let Some(size) = size {
            let _hooked = HOOK.lock().unwrap_or_else(|e| e.into_inner());
            state.borrow_mut().size = size;
        }
        std::thread::sleep(pause);
    }
}
