//! The control-only protocol of the bundled scrcpy v5.0.1 server.
//! Connection, display queries and shutdown are blocking. Own this on an input worker.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    hash::{BuildHasher, RandomState},
    io::{self, Read, Seek, SeekFrom, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const SERVER: &[u8] = include_bytes!("../../android/scrcpy/scrcpy-server-v5.0.1");
const VERSION: &str = "5.0.1";
const CLASS: &str = "com.genymobile.scrcpy.Server";
const ADB_PORT: u16 = 5037;
const SHORT_IO: Duration = Duration::from_secs(1);
const SHORT_COMMAND: Duration = Duration::from_secs(3);
const STARTUP: Duration = Duration::from_secs(12);
const ROTATE_DEVICE: [u8; 1] = [11];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchAction {
    Down = 0,
    Up = 1,
    Move = 2,
    Cancel = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    Down = 0,
    Up = 1,
}

#[derive(Clone, Copy)]
struct Position {
    point: [u32; 2],
    display: [u16; 2],
}

impl Position {
    fn new(point: [u32; 2], [width, height]: [u32; 2]) -> io::Result<Self> {
        let valid = |value: u32| (1..=u32::from(u16::MAX)).contains(&value);
        if !valid(width) || !valid(height) || point[0] >= width || point[1] >= height {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Touch is outside the guest display",
            ));
        }
        Ok(Self {
            point,
            display: [width as u16, height as u16],
        })
    }

    fn encode(self, bytes: &mut [u8]) {
        bytes[0..4].copy_from_slice(&self.point[0].to_be_bytes());
        bytes[4..8].copy_from_slice(&self.point[1].to_be_bytes());
        bytes[8..10].copy_from_slice(&self.display[0].to_be_bytes());
        bytes[10..12].copy_from_slice(&self.display[1].to_be_bytes());
    }
}

fn touch_packet(action: TouchAction, position: Position) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[0] = 2;
    bytes[1] = action as u8;
    // Finger ID 0, with no mouse button bits.
    position.encode(&mut bytes[10..22]);
    if matches!(action, TouchAction::Down | TouchAction::Move) {
        bytes[22..24].copy_from_slice(&u16::MAX.to_be_bytes());
    }
    bytes
}

fn key_packet(action: KeyAction, code: u32, repeat: u32, metastate: u32) -> [u8; 14] {
    let mut bytes = [0; 14];
    bytes[1] = action as u8;
    bytes[2..6].copy_from_slice(&code.to_be_bytes());
    bytes[6..10].copy_from_slice(&repeat.to_be_bytes());
    bytes[10..14].copy_from_slice(&metastate.to_be_bytes());
    bytes
}

fn scroll_packet(position: Position, horizontal: f32, vertical: f32) -> io::Result<[u8; 21]> {
    if !horizontal.is_finite() || !vertical.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid scroll delta",
        ));
    }
    let fixed = |delta: f32| (delta.clamp(-16.0, 16.0) * 2048.0) as i16;
    let mut bytes = [0; 21];
    bytes[0] = 3;
    position.encode(&mut bytes[1..13]);
    bytes[13..15].copy_from_slice(&fixed(horizontal).to_be_bytes());
    bytes[15..17].copy_from_slice(&fixed(vertical).to_be_bytes());
    Ok(bytes)
}

fn service(stream: &mut (impl Read + Write), request: &str) -> io::Result<()> {
    write!(stream, "{:04x}{request}", request.len())?;
    let mut status = [0; 4];
    stream.read_exact(&mut status)?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            stream.read_exact(&mut status)?;
            let length = std::str::from_utf8(&status)
                .ok()
                .and_then(|v| usize::from_str_radix(v, 16).ok())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "Malformed ADB failure length")
                })?;
            let mut message = vec![0; length];
            stream.read_exact(&mut message)?;
            Err(io::Error::other(format!(
                "ADB {request}: {}",
                String::from_utf8_lossy(&message)
            )))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed ADB service response",
        )),
    }
}

fn select_control(stream: &mut (impl Read + Write), scid: &str) -> io::Result<()> {
    service(stream, &format!("localabstract:scrcpy_{scid}"))?;
    let mut dummy = [1];
    stream.read_exact(&mut dummy)?;
    if dummy != [0] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid scrcpy dummy byte",
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct Adb {
    program: PathBuf,
    transport_id: u64,
}

impl Adb {
    fn new(program: &Path, transport_id: u64) -> io::Result<Self> {
        if !program.is_file() || transport_id == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "An installed ADB and explicit transport ID are required",
            ));
        }
        Ok(Self {
            program: std::path::absolute(program)?,
            transport_id,
        })
    }

    fn socket(&self) -> io::Result<TcpStream> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, ADB_PORT));
        let mut stream = TcpStream::connect_timeout(&address, SHORT_IO)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(SHORT_IO))?;
        stream.set_write_timeout(Some(SHORT_IO))?;
        service(
            &mut stream,
            &format!("host:transport-id:{}", self.transport_id),
        )?;
        Ok(stream)
    }

    fn command(&self) -> io::Result<Command> {
        // Require the local ADB server and this monitor-selected transport to exist.
        drop(self.socket()?);
        let mut command = Command::new(&self.program);
        command.env_remove("ADB_SERVER_SOCKET");
        command.args([
            "-H",
            "127.0.0.1",
            "-P",
            "5037",
            "-t",
            &self.transport_id.to_string(),
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        Ok(command)
    }

    fn verify_serial(&self, expected: &str) -> io::Result<()> {
        if expected
            .strip_prefix("127.0.0.1:")
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a local BlueStacks serial",
            ));
        }
        let serial = self.output(["get-serialno"], SHORT_COMMAND)?;
        if serial.trim() != expected {
            return Err(io::Error::other(
                "ADB transport serial does not match the selected BlueStacks instance",
            ));
        }
        Ok(())
    }

    fn output<I, S>(&self, args: I, timeout: Duration) -> io::Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut log = tempfile::tempfile()?;
        let mut child = spawn_logged(self.command()?.args(args), &log)?;
        let status = match wait(&mut child, timeout) {
            Ok(Some(status)) => status,
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result.err().unwrap_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "ADB command timed out")
                }));
            }
        };
        let output = log_text(&mut log, false)?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "ADB command exited with {status}: {}",
                output.trim()
            )));
        }
        Ok(output)
    }
}

fn wait(child: &mut Child, duration: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + duration;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

fn spawn_logged(command: &mut Command, log: &File) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?)
        .spawn()
}

fn log_text(log: &mut File, tail: bool) -> io::Result<String> {
    let limit = if tail { 4096 } else { 2 * 1024 * 1024 };
    let start = if tail {
        log.metadata()?.len().saturating_sub(limit)
    } else {
        0
    };
    log.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    log.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

struct Server {
    adb: Adb,
    serial: String,
    remote: String,
    scid: String,
    child: Option<Child>,
    log: File,
    cleanup: bool,
}

fn start_command(remote: &str, scid: &str) -> String {
    format!(
        concat!(
            "echo $$ > {remote}.pid || exit 1; export CLASSPATH={remote}; ",
            "exec app_process / {class} {version} scid={scid} log_level=warn ",
            "video=false audio=false control=true tunnel_forward=true ",
            "clipboard_autosync=false power_on=false cleanup=false ",
            "send_device_meta=false send_dummy_byte=true",
        ),
        remote = remote,
        class = CLASS,
        version = VERSION,
        scid = scid,
    )
}

impl Server {
    fn launch(&mut self, local: &Path) -> io::Result<TcpStream> {
        self.adb.verify_serial(&self.serial)?;
        self.cleanup = true;
        self.adb.output(
            [
                OsStr::new("push"),
                local.as_os_str(),
                OsStr::new(&self.remote),
            ],
            STARTUP,
        )?;
        let scid = &self.scid;
        let shell = start_command(&self.remote, scid);
        let child = spawn_logged(self.adb.command()?.args(["shell", "-T", &shell]), &self.log)?;
        self.child = Some(child);
        let deadline = Instant::now() + STARTUP;
        loop {
            match self.adb.socket().and_then(|mut stream| {
                select_control(&mut stream, scid)?;
                Ok(stream)
            }) {
                Ok(stream) => return Ok(stream),
                Err(error) if error.kind() == io::ErrorKind::InvalidData => return Err(error),
                Err(error) => {
                    let exited = self.child.as_mut().unwrap().try_wait()?.is_some();
                    if exited || Instant::now() >= deadline {
                        let log = log_text(&mut self.log, true).unwrap_or_default();
                        return Err(io::Error::other(format!(
                            "scrcpy control did not start: {error}\n{}",
                            log.trim()
                        )));
                    }
                }
            }
            thread::sleep(Duration::from_millis(80));
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        if let Some(mut child) = self.child.take() {
            if !matches!(wait(&mut child, Duration::from_millis(600)), Ok(Some(_))) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if !self.cleanup {
            return Ok(());
        }
        self.adb.verify_serial(&self.serial)?;
        let remote = &self.remote;
        let scid = &self.scid;
        // Only signal the server whose PID marker and exact argument tokens belong to this session.
        let shell = format!(
            concat!(
                "status=0; pid=$(cat {remote}.pid 2>/dev/null); case \"$pid\" in ''|*[!0-9]*) ;; *) ",
                "if [ \"$pid\" -gt 1 ] && [ -r \"/proc/$pid/cmdline\" ]; then ",
                "args=$(tr '\\000' '\\n' < \"/proc/$pid/cmdline\"); ",
                "if printf '%s\\n' \"$args\" | grep -Fxq 'scid={scid}' && ",
                "printf '%s\\n' \"$args\" | grep -Fxq '{class}'; then ",
                "kill -TERM \"$pid\" 2>/dev/null || {{ [ ! -d \"/proc/$pid\" ] || status=1; }}; ",
                "fi; fi;; esac; rm -f {remote}.pid {remote} || status=1; exit \"$status\"",
            ),
            remote = remote,
            class = CLASS,
            scid = scid,
        );
        self.adb.output(["shell", "-T", &shell], SHORT_COMMAND)?;
        self.cleanup = false;
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Own the selected guest's control session and release actively held input before closing.
pub(crate) struct MirrorControl {
    server: Server,
    stream: TcpStream,
    connected: bool,
    touch: Option<Position>,
    keys: BTreeMap<u32, u32>,
}

impl MirrorControl {
    pub(crate) fn connect(
        adb: &Path,
        transport_id: u64,
        expected_serial: &str,
    ) -> io::Result<Self> {
        let adb = Adb::new(adb, transport_id)?;
        let mut local = tempfile::Builder::new()
            .prefix("rustemu-mirror-")
            .rand_bytes(16)
            .tempfile()?;
        local.write_all(SERVER)?;
        local.flush()?;
        let name = local
            .path()
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| io::Error::other("Invalid temporary file name"))?;
        if !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(io::Error::other("Invalid temporary file name"));
        }
        let remote = format!("/data/local/tmp/{name}.jar");
        let scid = format!(
            "{:08x}",
            RandomState::new().hash_one(name) as u32 & 0x7fff_ffff
        );
        let mut server = Server {
            adb,
            serial: expected_serial.to_owned(),
            remote,
            scid,
            child: None,
            log: tempfile::tempfile()?,
            cleanup: false,
        };
        let stream = server.launch(local.path())?;
        Ok(Self {
            server,
            stream,
            connected: true,
            touch: None,
            keys: BTreeMap::new(),
        })
    }

    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !self.connected {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "The scrcpy control connection is closed",
            ));
        }
        if let Err(error) = self.stream.write_all(bytes) {
            // A short write may already have emitted part of a packet.
            self.connected = false;
            let _ = self.stream.shutdown(Shutdown::Both);
            self.touch = None;
            self.keys.clear();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn touch(
        &mut self,
        action: TouchAction,
        point: [u32; 2],
        display: [u32; 2],
    ) -> io::Result<()> {
        let position = Position::new(point, display)?;
        match (action, self.touch) {
            (TouchAction::Down, None) => {}
            (TouchAction::Move | TouchAction::Up | TouchAction::Cancel, Some(previous))
                if previous.display == position.display => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Touch action does not match the active contact",
                ));
            }
        }
        self.send(&touch_packet(action, position))?;
        self.touch = matches!(action, TouchAction::Down | TouchAction::Move).then_some(position);
        Ok(())
    }

    pub(crate) fn scroll(
        &mut self,
        point: [u32; 2],
        display: [u32; 2],
        horizontal: f32,
        vertical: f32,
    ) -> io::Result<()> {
        self.send(&scroll_packet(
            Position::new(point, display)?,
            horizontal,
            vertical,
        )?)
    }

    pub(crate) fn key(
        &mut self,
        action: KeyAction,
        code: u32,
        repeat: u32,
        metastate: u32,
    ) -> io::Result<()> {
        if code == 0 || code > i32::MAX as u32 || repeat > i32::MAX as u32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid Android key code or repeat count",
            ));
        }
        self.send(&key_packet(action, code, repeat, metastate))?;
        match action {
            KeyAction::Down => {
                self.keys.insert(code, metastate);
            }
            KeyAction::Up => {
                self.keys.remove(&code);
            }
        }
        Ok(())
    }

    pub(crate) fn back(&mut self) -> io::Result<()> {
        self.press(4)
    }
    pub(crate) fn home(&mut self) -> io::Result<()> {
        self.press(3)
    }
    pub(crate) fn recents(&mut self) -> io::Result<()> {
        self.press(187)
    }

    pub(crate) fn rotate(&mut self) -> io::Result<()> {
        self.release_all()?;
        self.send(&ROTATE_DEVICE)
    }

    fn press(&mut self, code: u32) -> io::Result<()> {
        self.key(KeyAction::Down, code, 0, 0)?;
        self.key(KeyAction::Up, code, 0, 0)
    }

    pub(crate) fn release_all(&mut self) -> io::Result<()> {
        if let Some(position) = self.touch.take() {
            // Losing host capture must not complete an Android click on the last point.
            self.send(&touch_packet(TouchAction::Cancel, position))?;
        }
        for (code, metastate) in std::mem::take(&mut self.keys) {
            self.send(&key_packet(KeyAction::Up, code, 0, metastate))?;
        }
        Ok(())
    }

    pub(crate) fn close(mut self) -> io::Result<()> {
        let released = self.release_all();
        let _ = self.stream.shutdown(Shutdown::Both);
        match (released, self.server.stop()) {
            (Err(input), Err(cleanup)) => Err(io::Error::other(format!(
                "{input}; scrcpy cleanup: {cleanup}"
            ))),
            (result, cleanup) => result.and(cleanup),
        }
    }
}

impl Drop for MirrorControl {
    fn drop(&mut self) {
        let _ = self.release_all();
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn dimensions(value: &str) -> Option<[u32; 2]> {
    let (width, height) = value.trim_start().split_once('x')?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    (width > 0 && height > 0 && width <= u32::from(u16::MAX) && height <= u32::from(u16::MAX))
        .then_some([width, height])
}

fn display_id(line: &str) -> Option<u32> {
    [
        "mDisplayId=",
        "displayId=",
        "displayId ",
        "Display #",
        "Display ",
    ]
    .into_iter()
    .find_map(|token| {
        line.split_once(token)?
            .1
            .trim_start()
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    })
}

fn window_display(output: &str) -> Option<[u32; 2]> {
    let mut current = None;
    for line in output.lines().map(str::trim) {
        if line.starts_with("Display:")
            || line.starts_with("Display #")
            || line.starts_with("DisplayContent{")
        {
            current = display_id(line);
        }
        if current == Some(0)
            && let Some(size) = line
                .split_once("cur=")
                .and_then(|(_, value)| dimensions(value))
        {
            return Some(size);
        }
    }
    None
}

fn logical_display(output: &str) -> Option<[u32; 2]> {
    let mut current = None;
    let mut base = None;
    for line in output.lines().map(str::trim) {
        if line.starts_with("Display ") || line.starts_with("Display #") {
            current = display_id(line);
        }
        if !line.contains("DisplayInfo{") || display_id(line).or(current) != Some(0) {
            continue;
        }
        let size = line
            .split_once("real ")
            .and_then(|(_, value)| dimensions(value));
        if line.starts_with("mOverrideDisplayInfo") && size.is_some() {
            return size;
        }
        if base.is_none() {
            base = size;
        }
    }
    base
}

fn fallback_display(output: &str, preview_size: Option<[u32; 2]>) -> Option<[u32; 2]> {
    let [width, height] = preview_size?;
    if width == 0 || height == 0 {
        return None;
    }
    let value = ["Override size:", "Physical size:"]
        .into_iter()
        .find_map(|label| {
            output
                .lines()
                .find_map(|line| line.trim().strip_prefix(label))
        })?;
    let size = dimensions(value)?;
    (width.cmp(&height) == size[0].cmp(&size[1])).then_some(size)
}

/// Query the currently oriented logical size of the default guest display.
/// A current preview size is required to use the orientation-ambiguous wm-size fallback.
pub(crate) fn query_display_size(
    adb: &Path,
    transport_id: u64,
    expected_serial: &str,
    preview_size: Option<[u32; 2]>,
) -> io::Result<[u32; 2]> {
    let adb = Adb::new(adb, transport_id)?;
    adb.verify_serial(expected_serial)?;
    let timeout = Duration::from_secs(4);
    let window = adb.output(["shell", "dumpsys", "window", "displays"], timeout);
    if let Some(size) = window.as_deref().ok().and_then(window_display) {
        return Ok(size);
    }
    let display = adb.output(["shell", "dumpsys", "display"], timeout);
    if let Some(size) = display.as_deref().ok().and_then(logical_display) {
        return Ok(size);
    }
    let wm = adb.output(["shell", "wm", "size"], timeout)?;
    fallback_display(&wm, preview_size).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Could not determine the oriented Android display size",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrcpy_v501_packets_are_big_endian_and_use_guest_coordinates() {
        let position = Position::new([0x0123, 0x0456], [0x0640, 0x0500]).unwrap();
        assert_eq!(
            touch_packet(TouchAction::Down, position),
            [
                2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0x23, 0, 0, 4, 0x56, 6, 0x40, 5, 0, 0xff,
                0xff, 0, 0, 0, 0, 0, 0, 0, 0,
            ]
        );
        assert_eq!(touch_packet(TouchAction::Move, position)[1], 2);
        assert_eq!(touch_packet(TouchAction::Up, position)[22..24], [0, 0]);
        let cancel = touch_packet(TouchAction::Cancel, position);
        assert_eq!(cancel[..2], [2, 3]);
        assert_eq!(cancel[22..24], [0, 0]);
        assert_eq!(
            key_packet(KeyAction::Up, 187, 2, 1),
            [0, 1, 0, 0, 0, 187, 0, 0, 0, 2, 0, 0, 0, 1]
        );
        let scroll = scroll_packet(position, -16.0, 16.0).unwrap();
        assert_eq!(scroll[13..17], [0x80, 0, 0x7f, 0xff]);
        assert_eq!(ROTATE_DEVICE, [11]);
        assert!(scroll_packet(position, f32::NAN, 0.0).is_err());
        assert!(Position::new([1600, 8], [1600, 900]).is_err());
    }

    #[test]
    fn landscape_display_uses_default_logical_size_and_safe_fallback() {
        let window = "Display: mDisplayId=2\n init=511x321 cur=511x321\nDisplay: mDisplayId=0\n init=900x1600 cur=1600x900 app=1600x900";
        assert_eq!(window_display(window), Some([1600, 900]));
        let display = "  Display 0:\n  mBaseDisplayInfo=DisplayInfo{displayId 0, real 900 x 1600, density 320}\n  mOverrideDisplayInfo=DisplayInfo{displayId 0, real 1600 x 900, density 320}";
        assert_eq!(logical_display(display), Some([1600, 900]));
        let wm = "Physical size: 900x1600\nOverride size: 800x1400";
        assert_eq!(fallback_display(wm, Some([900, 1600])), Some([800, 1400]));
        assert_eq!(fallback_display(wm, Some([1600, 900])), None);
        assert_eq!(fallback_display(wm, None), None);
    }

    #[test]
    fn adb_smart_socket_encodes_length_and_consumes_failure() {
        struct Fake<'a> {
            response: &'a [u8],
            writes: Vec<u8>,
        }
        impl Read for Fake<'_> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.response.read(output)
            }
        }
        impl Write for Fake<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.writes.write(bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut fake = Fake {
            response: b"OKAY",
            writes: Vec::new(),
        };
        service(&mut fake, "host:transport-id:7").unwrap();
        assert_eq!(fake.writes, b"0013host:transport-id:7");
        fake.response = b"FAIL0006closed";
        assert!(
            service(&mut fake, "localabstract:scrcpy_12345678")
                .unwrap_err()
                .to_string()
                .ends_with("closed")
        );
        fake.response = b"NOPE";
        assert_eq!(
            service(&mut fake, "localabstract:fixture")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        fake.response = b"OKAY\0";
        select_control(&mut fake, "01234567").unwrap();
        fake.response = b"OKAY\x01";
        assert_eq!(
            select_control(&mut fake, "01234567").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn server_starts_with_only_the_control_channel() {
        let command = start_command("/data/local/tmp/rustemu-mirror-fixture.jar", "01234567");
        let arguments: Vec<_> = command
            .split("exec app_process / ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .collect();
        assert_eq!(&arguments[..3], [CLASS, VERSION, "scid=01234567"]);
        assert_eq!(
            &arguments[4..],
            [
                "video=false",
                "audio=false",
                "control=true",
                "tunnel_forward=true",
                "clipboard_autosync=false",
                "power_on=false",
                "cleanup=false",
                "send_device_meta=false",
                "send_dummy_byte=true",
            ]
        );
    }
}
