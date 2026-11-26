use crate::canvas::{Cmd, Renderer, digest};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BitBlt, ClientToScreen,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, GetDC, HBITMAP, HDC, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClientRect,
    GetForegroundWindow, GetWindowThreadProcessId, IsWindow, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, ULW_ALPHA,
    UpdateLayeredWindow, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_OVERLAPPEDWINDOW, WS_POPUP,
};

pub struct Overlay {
    class: Vec<u16>,
    hwnd: HWND,
    owner: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    size: (i32, i32),
    last: u64,
    shown: bool,
    renderer: Renderer,
}

struct Search {
    pid: u32,
    skip: HWND,
    best: HWND,
    area: i64,
}

unsafe extern "system" fn each_window(hwnd: HWND, param: LPARAM) -> i32 {
    let search = unsafe { &mut *(param as *mut Search) };
    let mut pid = 0;
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != search.pid || hwnd == search.skip || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        GetClientRect(hwnd, &mut rect);
    }
    let area = i64::from(rect.right) * i64::from(rect.bottom);
    if area > search.area {
        search.area = area;
        search.best = hwnd;
    }
    1
}

impl Overlay {
    pub fn new() -> Overlay {
        let class: Vec<u16> = "ScriptHookVI-overlay\0".encode_utf16().collect();
        unsafe {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = GetModuleHandleW(std::ptr::null());
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
        }
        Overlay {
            class,
            hwnd: std::ptr::null_mut(),
            owner: std::ptr::null_mut(),
            dc: unsafe { CreateCompatibleDC(std::ptr::null_mut()) },
            bitmap: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
            size: (0, 0),
            last: 0,
            shown: false,
            renderer: Renderer::default(),
        }
    }

    fn hide(&mut self) {
        if self.shown {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.shown = false;
            self.last = 0;
        }
    }

    pub fn present(&mut self, cmds: &[Cmd], pid: u32) -> Option<(i32, i32)> {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        unsafe {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let mut search = Search { pid, skip: self.hwnd, best: std::ptr::null_mut(), area: 0 };
        unsafe { EnumWindows(Some(each_window), &mut search as *mut Search as LPARAM) };
        let target = search.best;
        if target.is_null() {
            self.hide();
            return None;
        }
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let mut origin = POINT { x: 0, y: 0 };
        unsafe {
            GetClientRect(target, &mut rect);
            ClientToScreen(target, &mut origin);
        }
        let size = (rect.right, rect.bottom);
        if size.0 <= 0 || size.1 <= 0 {
            self.hide();
            return None;
        }
        let front = unsafe { GetForegroundWindow() };
        if cmds.is_empty() || (front != target && front != self.hwnd) {
            self.hide();
            return Some(size);
        }
        if self.hwnd.is_null() || self.owner != target || unsafe { IsWindow(self.hwnd) } == 0 {
            unsafe {
                if !self.hwnd.is_null() {
                    DestroyWindow(self.hwnd);
                }
                self.hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    self.class.as_ptr(),
                    self.class.as_ptr(),
                    WS_POPUP,
                    origin.x,
                    origin.y,
                    size.0,
                    size.1,
                    target,
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                );
            }
            crate::log(&format!(
                "overlay over window {target:?} at {},{} size {}x{}",
                origin.x, origin.y, size.0, size.1
            ));
            self.owner = target;
            self.shown = false;
            self.last = 0;
            if self.hwnd.is_null() {
                return Some(size);
            }
        }
        if size != self.size || self.bits.is_null() {
            let Some((bitmap, bits)) = dib(self.dc, size) else {
                return Some(size);
            };
            unsafe {
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
            }
            self.bitmap = bitmap;
            self.bits = bits;
            self.size = size;
            self.last = 0;
        }
        let hash = digest(cmds, (size, origin.x, origin.y));
        if hash != self.last {
            let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, (size.0 * size.1) as usize) };
            self.renderer.render(cmds, size, pixels, 0);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let extent = SIZE { cx: size.0, cy: size.1 };
            let zero = POINT { x: 0, y: 0 };
            unsafe {
                UpdateLayeredWindow(
                    self.hwnd,
                    std::ptr::null_mut(),
                    &origin,
                    &extent,
                    self.dc,
                    &zero,
                    0,
                    &blend,
                    ULW_ALPHA,
                );
            }
            self.last = hash;
        }
        if !self.shown {
            unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
            self.shown = true;
        }
        Some(size)
    }
}

fn dib(dc: HDC, size: (i32, i32)) -> Option<(HBITMAP, *mut u32)> {
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size.0;
    info.bmiHeader.biHeight = -size.1;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let mut bits = std::ptr::null_mut();
    unsafe {
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bitmap.is_null() {
            return None;
        }
        SelectObject(dc, bitmap);
        Some((bitmap, bits.cast()))
    }
}

pub struct Panel {
    hwnd: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    size: (i32, i32),
    hidden: bool,
    renderer: Renderer,
}

impl Panel {
    pub fn new() -> Panel {
        let class: Vec<u16> = "ScriptHookVI-panel\0".encode_utf16().collect();
        let title: Vec<u16> = "ScriptHookVI\0".encode_utf16().collect();
        let mut rect = RECT { left: 0, top: 0, right: 1280, bottom: 720 };
        let hwnd = unsafe {
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = GetModuleHandleW(std::ptr::null());
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0);
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        Panel {
            hwnd,
            dc: unsafe { CreateCompatibleDC(std::ptr::null_mut()) },
            bitmap: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
            size: (1280, 720),
            hidden: true,
            renderer: Renderer::default(),
        }
    }

    pub fn hide(&mut self) {
        if !self.hidden {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.hidden = true;
        }
    }

    pub fn present(&mut self, cmds: &[Cmd]) -> Option<(i32, i32)> {
        if unsafe { IsWindow(self.hwnd) } == 0 {
            return None;
        }
        if self.hidden {
            unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
            self.hidden = false;
        }
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetClientRect(self.hwnd, &mut rect) };
        let size = (rect.right, rect.bottom);
        if size.0 <= 0 || size.1 <= 0 {
            return Some(self.size);
        }
        if size != self.size || self.bits.is_null() {
            let (bitmap, bits) = dib(self.dc, size)?;
            unsafe {
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap);
                }
            }
            (self.bitmap, self.bits, self.size) = (bitmap, bits, size);
        }
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, (size.0 * size.1) as usize) };
        self.renderer.render(cmds, size, pixels, 0xFF1A1622);
        unsafe {
            let window = GetDC(self.hwnd);
            BitBlt(window, 0, 0, size.0, size.1, self.dc, 0, 0, SRCCOPY);
            ReleaseDC(self.hwnd, window);
        }
        Some(size)
    }
}
