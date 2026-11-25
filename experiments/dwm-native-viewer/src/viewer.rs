use std::ffi::OsString;
use std::io::{self, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, Threading::*},
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

type Result<T> = std::result::Result<T, String>;
type Box2 = [i32; 4]; // left, top, width, height in physical pixels

const USAGE: &str = "dwm-native-viewer --pid PID --exe PATH --hwnd 0xHWND --class CLASS \
                    --crop LEFT,TOP,WIDTH,HEIGHT [--child 0xHWND] [--origin X,Y] \
                    [--width 1040 --height 585 --warmup 2 --seconds 16]\n\
                    dwm-native-viewer --inspect --pid PID --exe PATH\n\
                    dwm-native-viewer --smoke [--origin X,Y] [--width 1040 --height 585 \
                    --warmup 2 --seconds 16]";

#[derive(Default)]
struct Args {
    pid: Option<u32>,
    exe: Option<PathBuf>,
    hwnd: Option<usize>,
    class: Option<String>,
    child: Option<usize>,
    crop: Option<Box2>,
    origin: Option<[i32; 2]>,
    width: Option<i32>,
    height: Option<i32>,
    warmup: Option<u64>,
    seconds: Option<u64>,
    smoke: bool,
    inspect: bool,
}

fn numbers<const N: usize>(value: &str) -> Result<[i32; N]> {
    let items = value
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<_>, _>>();
    items
        .ok()
        .and_then(|items| items.try_into().ok())
        .ok_or_else(|| format!("expected {N} comma-separated integers"))
}

fn handle(value: &str) -> Result<usize> {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .and_then(|hex| usize::from_str_radix(hex, 16).ok())
        .filter(|&value| value != 0)
        .ok_or_else(|| "expected a nonzero HWND in 0x hexadecimal format".into())
}

fn parse(items: impl IntoIterator<Item = OsString>) -> Result<Args> {
    let mut items = items.into_iter();
    let mut args = Args::default();
    let mut seen = std::collections::HashSet::new();
    while let Some(key) = items.next() {
        let key = key.to_str().ok_or("flag is not UTF-8")?;
        if !seen.insert(key.to_owned()) {
            return Err(format!("duplicate flag {key}"));
        }
        match key {
            "--smoke" => args.smoke = true,
            "--inspect" => args.inspect = true,
            "--pid" | "--exe" | "--hwnd" | "--class" | "--child" | "--crop" | "--origin"
            | "--width" | "--height" | "--warmup" | "--seconds" => {
                let value = items
                    .next()
                    .ok_or_else(|| format!("{key} requires a value"))?;
                if key == "--exe" {
                    args.exe = Some(value.into());
                    continue;
                }
                let value = value
                    .to_str()
                    .ok_or_else(|| format!("{key} requires UTF-8"))?;
                match key {
                    "--pid" => args.pid = Some(value.parse().map_err(|_| "invalid PID")?),
                    "--hwnd" => args.hwnd = Some(handle(value)?),
                    "--child" => args.child = Some(handle(value)?),
                    "--class" => args.class = Some(value.into()),
                    "--crop" => args.crop = Some(numbers(value)?),
                    "--origin" => args.origin = Some(numbers(value)?),
                    "--width" => args.width = Some(value.parse().map_err(|_| "invalid width")?),
                    "--height" => args.height = Some(value.parse().map_err(|_| "invalid height")?),
                    "--warmup" => args.warmup = Some(value.parse().map_err(|_| "invalid warmup")?),
                    "--seconds" => {
                        args.seconds = Some(value.parse().map_err(|_| "invalid seconds")?)
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("unknown flag {key}\n{USAGE}")),
        }
    }
    let source_options =
        args.hwnd.is_some() || args.class.is_some() || args.child.is_some() || args.crop.is_some();
    if args.smoke && (args.inspect || args.pid.is_some() || args.exe.is_some() || source_options) {
        return Err("--smoke only accepts destination and timing flags".into());
    }
    if !args.smoke
        && (args.pid.unwrap_or(0) == 0 || args.exe.as_ref().is_none_or(|path| !path.is_absolute()))
    {
        return Err("provide a nonzero --pid and an absolute --exe path".into());
    }
    if args.inspect
        && (source_options
            || args.origin.is_some()
            || args.width.is_some()
            || args.height.is_some()
            || args.warmup.is_some()
            || args.seconds.is_some())
    {
        return Err("--inspect only accepts --pid and --exe".into());
    }
    if !args.smoke
        && !args.inspect
        && (args.hwnd.is_none()
            || args.class.as_ref().is_none_or(String::is_empty)
            || args.crop.is_none())
    {
        return Err("provide --hwnd, --class and --crop from the selected application's --inspect inventory".into());
    }
    if !(128..=8192).contains(&args.width.unwrap_or(1040))
        || !(128..=8192).contains(&args.height.unwrap_or(585))
        || !(1..=600).contains(&args.seconds.unwrap_or(16))
        || args.warmup.unwrap_or(2) > 30
    {
        return Err("client dimensions must be 128..8192, seconds 1..600 and warmup 0..30".into());
    }
    if args
        .crop
        .is_some_and(|crop| !contains([0, 0, i32::MAX, i32::MAX], crop))
    {
        return Err(
            "--crop must be a nonnegative in-bounds pixel origin and a positive size".into(),
        );
    }
    Ok(args)
}

fn win_error(operation: &str) -> String {
    format!("{operation}: {}", io::Error::last_os_error())
}

fn hresult(operation: &str, code: i32) -> Result<()> {
    if code < 0 {
        return Err(format!("{operation}: 0x{:08X}", code as u32));
    }
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

fn output(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}")
        .and_then(|_| stdout.flush())
        .map_err(|error| error.to_string())
}

fn handle_name(value: HWND) -> String {
    format!("0x{:X}", value as usize)
}

fn timestamp(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

struct Process(HANDLE);

impl Process {
    fn open(pid: u32, expected: &Path) -> Result<Self> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(win_error("OpenProcess"));
        }
        let process = Self(handle);
        process.check()?;
        let mut path = vec![0u16; 32_768];
        let mut size = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut size) } == 0 {
            return Err(win_error("QueryFullProcessImageNameW"));
        }
        let actual = PathBuf::from(OsString::from_wide(&path[..size as usize]));
        let actual = actual
            .canonicalize()
            .map_err(|error| format!("process executable: {error}"))?;
        let expected = expected
            .canonicalize()
            .map_err(|error| format!("expected executable: {error}"))?;
        if actual != expected {
            return Err("selected PID's executable differs from --exe".into());
        }
        Ok(process)
    }

    fn check(&self) -> Result<()> {
        let mut exit = 0;
        if unsafe { GetExitCodeProcess(self.0, &mut exit) } == 0 {
            return Err(win_error("GetExitCodeProcess"));
        }
        if exit != STILL_ACTIVE as u32 {
            return Err("selected source process exited".into());
        }
        Ok(())
    }

    fn times(&self) -> Result<(u64, u64)> {
        let (mut creation, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if unsafe { GetProcessTimes(self.0, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
        {
            return Err(win_error("GetProcessTimes"));
        }
        Ok((timestamp(creation), timestamp(kernel) + timestamp(user)))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn ownership(hwnd: HWND) -> (u32, u32) {
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    (pid, thread)
}

fn class(hwnd: HWND) -> Result<String> {
    let mut name = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    if length == 0 {
        return Err(win_error("GetClassNameW"));
    }
    String::from_utf16(&name[..length as usize]).map_err(|error| error.to_string())
}

fn screen_client(hwnd: HWND) -> Result<Box2> {
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) == 0 || ClientToScreen(hwnd, &mut origin) == 0 } {
        return Err(win_error("GetClientRect / ClientToScreen"));
    }
    Ok([
        origin.x,
        origin.y,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ])
}

fn contains(rect: Box2, inner: Box2) -> bool {
    rect[2] > 0
        && rect[3] > 0
        && inner[2] > 0
        && inner[3] > 0
        && (0..2).all(|i| {
            inner[i] >= rect[i]
                && i64::from(inner[i]) + i64::from(inner[i + 2])
                    <= i64::from(rect[i]) + i64::from(rect[i + 2])
        })
}

fn relative(parent: Box2, child: Box2) -> Result<Box2> {
    if !contains(parent, child) {
        return Err("renderer client lies outside Qt client".into());
    }
    Ok([
        child[0] - parent[0],
        child[1] - parent[1],
        child[2],
        child[3],
    ])
}

fn fit([width, height]: [i32; 2], [_, _, sw, sh]: Box2) -> Box2 {
    let ratio = |size, numerator, denominator| {
        (i64::from(size) * i64::from(numerator) / i64::from(denominator)).max(1) as i32
    };
    let (w, h) = if i64::from(width) * i64::from(sh) <= i64::from(height) * i64::from(sw) {
        (width, ratio(width, sh, sw))
    } else {
        (ratio(height, sw, sh), height)
    };
    [(width - w) / 2, (height - h) / 2, w, h]
}

fn rect([x, y, width, height]: Box2) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

unsafe extern "system" fn collect(hwnd: HWND, userdata: LPARAM) -> i32 {
    unsafe { (&mut *(userdata as *mut Vec<HWND>)).push(hwnd) };
    1
}

fn inspect(pid: u32, process: &Process) -> Result<()> {
    let mut handles: Vec<HWND> = Vec::new();
    if unsafe { EnumWindows(Some(collect), (&mut handles as *mut Vec<HWND>) as LPARAM) } == 0 {
        return Err(win_error("EnumWindows"));
    }
    let window_record = |hwnd| -> Option<Value> {
        if ownership(hwnd).0 != pid {
            return None;
        }
        Some(
            json!({"hwnd": handle_name(hwnd), "class": class(hwnd).ok()?, "client_physical_px": screen_client(hwnd).ok()?,
                    "visible": unsafe { IsWindowVisible(hwnd) != 0 }, "minimized": unsafe { IsIconic(hwnd) != 0 }}),
        )
    };
    let mut windows = Vec::new();
    for hwnd in handles {
        let Some(mut top) = window_record(hwnd) else {
            continue;
        };
        let mut children: Vec<HWND> = Vec::new();
        unsafe {
            EnumChildWindows(
                hwnd,
                Some(collect),
                (&mut children as *mut Vec<HWND>) as LPARAM,
            )
        };
        top["children"] = json!(
            children
                .into_iter()
                .filter_map(window_record)
                .collect::<Vec<_>>()
        );
        windows.push(top);
    }
    process.check()?;
    output(
        &json!({"event":"source_inventory","process_id":pid,"creation_filetime":process.times()?.0,"windows":windows}),
    )
}

struct Source<'a> {
    process: &'a Process,
    pid: u32,
    thread: u32,
    hwnd: HWND,
    class_name: String,
    child: Option<(HWND, u32)>,
    crop: Box2,
}

struct Geometry {
    root: Box2,
    child: Option<Box2>,
}

impl Source<'_> {
    fn verify(&self) -> Result<Geometry> {
        self.process.check()?;
        if self.thread == 0
            || ownership(self.hwnd) != (self.pid, self.thread)
            || unsafe { GetAncestor(self.hwnd, GA_ROOT) } != self.hwnd
            || class(self.hwnd)? != self.class_name
        {
            return Err("source HWND does not match the selected PID, root and class".into());
        }
        let mut cloaked = 0u32;
        hresult("DwmGetWindowAttribute(DWMWA_CLOAKED)", unsafe {
            DwmGetWindowAttribute(
                self.hwnd,
                DWMWA_CLOAKED as u32,
                (&mut cloaked as *mut u32).cast(),
                size_of::<u32>() as u32,
            )
        })?;
        if unsafe { IsWindowVisible(self.hwnd) == 0 || IsIconic(self.hwnd) != 0 } || cloaked != 0 {
            return Err("source window is hidden, minimized or cloaked".into());
        }
        let root = screen_client(self.hwnd)?;
        if !contains([0, 0, root[2], root[3]], self.crop) {
            return Err("--crop lies outside the source client rectangle".into());
        }
        let child = if let Some((child, thread)) = self.child {
            if thread == 0
                || ownership(child) != (self.pid, thread)
                || unsafe { IsChild(self.hwnd, child) == 0 || IsWindowVisible(child) == 0 }
            {
                return Err(
                    "--child must be a visible descendant owned by the verified process".into(),
                );
            }
            let child = relative(root, screen_client(child)?)?;
            if !contains(child, self.crop) {
                return Err("--crop lies outside --child's client rectangle".into());
            }
            Some(child)
        } else {
            None
        };
        Ok(Geometry { root, child })
    }
}

struct Thumbnail(isize);

impl Thumbnail {
    fn attach(
        source: HWND,
        destination: HWND,
        crop: Box2,
        fitted: Box2,
    ) -> Result<(Self, [i32; 2])> {
        let mut handle = 0;
        hresult("DwmRegisterThumbnail", unsafe {
            DwmRegisterThumbnail(destination, source, &mut handle)
        })?;
        if handle == 0 {
            return Err("DwmRegisterThumbnail returned an empty handle".into());
        }
        let thumbnail = Self(handle);
        let mut size = SIZE::default();
        hresult("DwmQueryThumbnailSourceSize", unsafe {
            DwmQueryThumbnailSourceSize(handle, &mut size)
        })?;
        if !contains([0, 0, size.cx, size.cy], crop) {
            return Err("DWM reported a capture surface smaller than the selected crop".into());
        }
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION
                | DWM_TNP_RECTSOURCE
                | DWM_TNP_OPACITY
                | DWM_TNP_VISIBLE
                | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: rect(fitted),
            rcSource: rect(crop),
            opacity: 255,
            fVisible: 1,
            fSourceClientAreaOnly: 1,
        };
        hresult("DwmUpdateThumbnailProperties", unsafe {
            DwmUpdateThumbnailProperties(handle, &properties)
        })?;
        Ok((thumbnail, [size.cx, size.cy]))
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        unsafe {
            DwmUnregisterThumbnail(self.0);
        }
    }
}

const SOURCE_CLASS: &str = "RustEmuDroidDwmSmokeSource";
const DEST_CLASS: &str = "RustEmuDroidDwmNativeViewer";

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_KEYDOWN if wparam == 0x1b => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &mut ps) };
            if !dc.is_null() && class(hwnd).is_ok_and(|name| name == SOURCE_CLASS) {
                // Grey margin, then a precisely located high-contrast client patch.
                for (area, color) in [
                    ([48, 72, 320, 180], 0x00D020E0),
                    ([48, 72, 160, 90], 0x0020E020),
                ] {
                    let brush = unsafe { CreateSolidBrush(color) };
                    if !brush.is_null() {
                        unsafe {
                            FillRect(dc, &rect(area), brush);
                            DeleteObject(brush);
                        }
                    }
                }
            }
            unsafe {
                EndPaint(hwnd, &ps);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

struct Window(HWND);

impl Window {
    fn create(class: &str, title: &str, geometry: Box2, background: i32) -> Result<Self> {
        let class = wide(class);
        let title = wide(title);
        let instance = unsafe { GetModuleHandleW(null()) };
        let registration = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hbrBackground: (background + 1) as HBRUSH,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&registration) } == 0 {
            return Err(win_error("RegisterClassW"));
        }
        let style = WS_OVERLAPPEDWINDOW & !(WS_MAXIMIZEBOX | WS_THICKFRAME);
        let [x, y, width, height] = geometry;
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style,
                x,
                y,
                width,
                height,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if hwnd.is_null() {
            return Err(win_error("CreateWindowExW"));
        }
        let window = Self(hwnd);
        // Derive frame offsets for the destination's actual DPI and window style.
        let mut outer = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut outer) } == 0 {
            return Err(win_error("GetWindowRect"));
        }
        let [cx, cy, cw, ch] = screen_client(hwnd)?;
        let (left, top) = (
            i64::from(x) + i64::from(outer.left) - i64::from(cx),
            i64::from(y) + i64::from(outer.top) - i64::from(cy),
        );
        let left = i32::try_from(left).map_err(|_| "destination origin overflow")?;
        let top = i32::try_from(top).map_err(|_| "destination origin overflow")?;
        if unsafe {
            SetWindowPos(
                hwnd,
                null_mut(),
                left,
                top,
                width + (outer.right - outer.left - cw),
                height + (outer.bottom - outer.top - ch),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        } == 0
        {
            return Err(win_error("SetWindowPos"));
        }
        unsafe {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            UpdateWindow(hwnd);
        }
        if screen_client(hwnd)? != geometry {
            return Err("Windows did not honor the exact client geometry".into());
        }
        Ok(window)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

fn work_area(hwnd: HWND) -> Result<Box2> {
    let hmonitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if hmonitor.is_null() || unsafe { GetMonitorInfoW(hmonitor, &mut info) } == 0 {
        return Err(win_error("GetMonitorInfoW"));
    }
    let area = info.rcWork;
    Ok([
        area.left,
        area.top,
        area.right - area.left,
        area.bottom - area.top,
    ])
}

pub fn run() -> Result<()> {
    let argv: Vec<_> = std::env::args_os().skip(1).collect();
    if argv == ["--help"] || argv == ["-h"] {
        println!("{USAGE}");
        return Ok(());
    }
    let args = parse(argv)?;
    if unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } == 0 {
        return Err(win_error("SetProcessDpiAwarenessContext"));
    }
    let smoke_source;
    let (pid, hwnd, class_name, crop, process) = if args.smoke {
        smoke_source = Window::create(
            SOURCE_CLASS,
            "DWM smoke source",
            [48, 72, 416, 288],
            COLOR_BTNFACE,
        )?;
        let pid = std::process::id();
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        (
            pid,
            smoke_source.0,
            SOURCE_CLASS.into(),
            [48, 72, 320, 180],
            Process::open(pid, &executable)?,
        )
    } else {
        let pid = args.pid.unwrap();
        let process = Process::open(pid, args.exe.as_deref().unwrap())?;
        if args.inspect {
            return inspect(pid, &process);
        }
        (
            pid,
            args.hwnd.unwrap() as HWND,
            args.class.unwrap(),
            args.crop.unwrap(),
            process,
        )
    };
    let source = Source {
        pid,
        process: &process,
        hwnd,
        class_name,
        thread: ownership(hwnd).1,
        child: args
            .child
            .map(|handle| (handle as HWND, ownership(handle as HWND).1)),
        crop,
    };
    let geometry = source.verify()?;
    let [width, height] = [args.width.unwrap_or(1040), args.height.unwrap_or(585)];
    let area = work_area(source.hwnd)?;
    let [x, y] = args.origin.unwrap_or([
        area[0] + (area[2] - width) / 2,
        area[1] + (area[3] - height) / 2,
    ]);
    let destination_rect = [x, y, width, height];
    if !contains(area, destination_rect) {
        return Err("destination client must fit wholly on the source monitor's work area".into());
    }
    let window = Window::create(
        DEST_CLASS,
        "RustEmuDroid - DWM native viewer",
        destination_rect,
        COLOR_WINDOWFRAME,
    )?;
    let fitted = fit([width, height], crop);
    let (_thumbnail, dwm_size) = Thumbnail::attach(source.hwnd, window.0, crop, fitted)?;
    source.verify()?;
    let (creation, _) = process.times()?;
    output(
        &json!({"event":"destination_ready","source_pid":pid,"source_creation_filetime":creation,
        "source_hwnd":handle_name(hwnd),"source_class":source.class_name,"source_client_physical_px":geometry.root,
        "source_render_child_hwnd":source.child.map(|(child,_)| handle_name(child)),
        "source_render_child_relative_px":geometry.child,"source_crop_relative_px":crop,"dwm_source_size_px":dwm_size,
        "destination_hwnd":handle_name(window.0),"destination_client_physical_px":destination_rect,
        "destination_fitted_relative_px":fitted,"destination_fitted_screen_px":[x+fitted[0],y+fitted[1],fitted[2],fitted[3]],
        "smoke":args.smoke,"viewer_pid":std::process::id()}),
    )?;
    // The capture operator can align an external CPU/ETW or desktop crop to this marker.
    let warmup = Duration::from_secs(args.warmup.unwrap_or(2));
    let measured = Duration::from_secs(args.seconds.unwrap_or(16));
    let started = Instant::now();
    println!("viewer_source_started");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let own = Process::open(
        std::process::id(),
        &std::env::current_exe().map_err(|error| error.to_string())?,
    )?;
    let mut sample = None;
    let mut checks = 0u32;
    let mut foreground = 0u32;
    loop {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if unsafe { IsWindow(window.0) == 0 } {
            return output(&json!({"event":"viewer_closed","completed":false}));
        }
        let current = source.verify()?;
        if current.root != geometry.root
            || current.child != geometry.child
            || screen_client(window.0)? != destination_rect
            || unsafe { IsIconic(window.0) != 0 || IsWindowVisible(window.0) == 0 }
        {
            return Err("source or destination geometry changed during observation".into());
        }
        let now = Instant::now();
        if now.duration_since(started) >= warmup {
            let cpu = (own.times()?.1, process.times()?.1);
            let (before, self_before, source_before) = *sample.get_or_insert((now, cpu.0, cpu.1));
            checks += 1;
            foreground += u32::from(unsafe { GetForegroundWindow() } == window.0);
            if now.duration_since(started) >= warmup + measured {
                return output(&json!({"event":"viewer_finished","completed":true,
                    "measured_wall_seconds":now.duration_since(before).as_secs_f64(),
                    "viewer_cpu_seconds":(cpu.0-self_before) as f64 / 10_000_000.0,
                    "source_cpu_seconds":(cpu.1-source_before) as f64 / 10_000_000.0,
                    "geometry_checks":checks,"destination_foreground_checks":foreground}));
            }
        }
        unsafe {
            MsgWaitForMultipleObjectsEx(0, null(), 100, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_keep_physical_offsets_and_aspect() {
        assert_eq!(
            relative([-1792, 60, 1600, 1000], [-1792, 100, 1600, 900]).unwrap(),
            [0, 40, 1600, 900]
        );
        assert!(relative([10, 10, 500, 500], [0, 100, 400, 400]).is_err());
        assert!(!contains([0, 0, i32::MAX, 100], [i32::MAX, 0, 1, 1]));
        assert_eq!(fit([1040, 585], [0, 20, 1600, 900]), [0, 0, 1040, 585]);
        assert_eq!(fit([800, 600], [0, 20, 1600, 900]), [0, 75, 800, 450]);
        assert_eq!(fit([600, 800], [0, 0, 900, 1600]), [75, 0, 450, 800]);
    }

    #[test]
    fn cli_refuses_implicit_source_and_invalid_crops() {
        let args = |tokens: &[&str]| parse(tokens.iter().map(OsString::from));
        assert!(
            args(&[
                "--pid",
                "42",
                "--exe",
                "C:/Kemu/player.exe",
                "--hwnd",
                "0xA1"
            ])
            .is_err()
        );
        assert!(args(&["--inspect", "--pid", "42", "--exe", "C:/Kemu/player.exe"]).is_ok());
        assert!(args(&["--smoke", "--pid", "42"]).is_err());
        assert!(args(&["--smoke", "--warmup", "0", "--seconds", "1"]).is_ok());
        assert!(
            args(&[
                "--pid",
                "42",
                "--exe",
                "C:/Kemu/player.exe",
                "--hwnd",
                "0xA1",
                "--class",
                "Qt",
                "--crop",
                "0,0,-1,300"
            ])
            .is_err()
        );
    }
}
