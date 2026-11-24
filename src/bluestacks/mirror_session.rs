//! Coordinates an official player window, its local ADB transport, and the input worker.

use std::{
    env, fs,
    io::{self, Read, Seek, SeekFrom, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use super::{
    Installation, configured_instances, launch,
    mirror_control::{MirrorControl, query_display_size},
    mirror_target::{ConfiguredTarget, MirrorTarget, TargetError, configured_target},
    player_process::{PlayerError, PlayerSource, find_preview_player, verify_adb_player},
    valid_package,
};

pub(crate) use super::mirror_control::{KeyAction, TouchAction};

const ADB_SERVER: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5037));
const MIN_ADB_PROTOCOL: u32 = 41;
const HOST_IO_TIMEOUT: Duration = Duration::from_secs(2);
const WAIT_PLAYER: Duration = Duration::from_secs(48);
const WAIT_DEVICE: Duration = Duration::from_secs(10);
const ADB_VERSION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub(crate) enum MirrorEvent {
    Player(u32, u64),
    Ready([u32; 2]),
    ViewOnly(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum MirrorInput {
    Touch(TouchAction, [u32; 2], [u32; 2]),
    Scroll([u32; 2], [u32; 2], f32, f32),
    Key(KeyAction, u32, u32, u32),
    Back,
    Home,
    Recents,
    Release,
    Refresh([u32; 2]),
}

pub(crate) struct MirrorSession {
    pub(crate) events: Receiver<MirrorEvent>,
    pub(crate) input: Sender<MirrorInput>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl MirrorSession {
    pub(crate) fn start(
        instance: String,
        package: Option<String>,
        adb_hint: Option<PathBuf>,
    ) -> Self {
        Self::spawn(instance, package, adb_hint, None)
    }

    #[cfg(test)]
    pub(crate) fn start_existing_for_test(instance: String, pid: u32, adb: PathBuf) -> Self {
        Self::spawn(instance, None, Some(adb), Some(pid))
    }

    fn spawn(
        instance: String,
        package: Option<String>,
        adb_hint: Option<PathBuf>,
        existing_pid: Option<u32>,
    ) -> Self {
        let (event_sender, events) = mpsc::channel();
        let (input, input_receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            cancelled: cancelled.clone(),
            events: event_sender.clone(),
            input: input_receiver,
        };
        let worker = match thread::Builder::new()
            .name("bluestacks-mirror".into())
            .spawn(move || worker.run(instance, package, adb_hint, existing_pid))
        {
            Ok(worker) => Some(worker),
            Err(error) => {
                let _ = event_sender.send(MirrorEvent::Failed(error.to_string()));
                None
            }
        };
        Self {
            events,
            input,
            cancelled,
            worker,
        }
    }

    /// Signal the worker and keep pumping the UI until `is_finished` becomes true.
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for MirrorSession {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    cancelled: Arc<AtomicBool>,
    events: Sender<MirrorEvent>,
    input: Receiver<MirrorInput>,
}

impl Worker {
    fn run(
        self,
        instance: String,
        package: Option<String>,
        adb_hint: Option<PathBuf>,
        existing_pid: Option<u32>,
    ) {
        let (installation, player) = match self.player(&instance, package.as_deref(), existing_pid)
        {
            Ok(value) => value,
            Err(error) => {
                self.emit(MirrorEvent::Failed(error.to_string()));
                return;
            }
        };
        if !self.emit(MirrorEvent::Player(player.pid, player.creation_time)) {
            return;
        }
        if let Err(error) = self.control(&installation, &instance, player, adb_hint.as_deref()) {
            self.emit(MirrorEvent::ViewOnly(error.to_string()));
        }
    }

    fn player(
        &self,
        instance: &str,
        package: Option<&str>,
        existing_pid: Option<u32>,
    ) -> io::Result<(Installation, PlayerSource)> {
        self.check_cancelled()?;
        if package.is_some_and(|id| !valid_package(id)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid Android package identifier.",
            ));
        }
        let installation = Installation::discover().map_err(io::Error::other)?;
        let config = fs::read_to_string(&installation.config)?;
        if !configured_instances(&config)
            .iter()
            .any(|entry| entry.id == instance)
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The selected BlueStacks instance is not installed.",
            ));
        }
        if let Some(pid) = existing_pid {
            let player =
                find_preview_player(&installation.player, instance).map_err(io::Error::other)?;
            if player.pid != pid {
                return Err(io::Error::other(
                    "The already-running player does not match the operator's test PID.",
                ));
            }
            return Ok((installation, player));
        }
        if let Some(package) = package {
            self.check_cancelled()?;
            launch(Some(instance), Some(package)).map_err(io::Error::other)?;
        } else {
            match find_preview_player(&installation.player, instance) {
                Ok(player) => return Ok((installation, player)),
                Err(PlayerError::NoPlayer) => {
                    self.check_cancelled()?;
                    launch(Some(instance), None).map_err(io::Error::other)?;
                }
                Err(PlayerError::NoWindow) => {}
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        let deadline = Instant::now() + WAIT_PLAYER;
        loop {
            self.check_cancelled()?;
            match find_preview_player(&installation.player, instance) {
                Ok(player) => return Ok((installation, player)),
                Err(PlayerError::NoPlayer | PlayerError::NoWindow) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(500));
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }

    fn control(
        &self,
        installation: &Installation,
        instance: &str,
        source: PlayerSource,
        adb_hint: Option<&Path>,
    ) -> io::Result<()> {
        self.check_cancelled()?;
        let target = self.wait_configured_target(installation, instance)?;
        self.check_player(installation, instance, target, source, true)?;
        let adb = self.adb(adb_hint)?;
        let mirrored = self.wait_transport(target, &adb, || {
            self.check_player(installation, instance, target, source, false)
        })?;
        self.check_player(installation, instance, target, source, false)?;
        self.confirm_transport(target, mirrored, &adb)?;
        self.check_cancelled()?;

        let mut latest_preview = None;
        let initial = self.display(&adb, mirrored, target, latest_preview);
        let display = match initial {
            Ok(size) => size,
            Err(_) => {
                let preview = self.wait_for_preview_size()?;
                latest_preview = Some(preview);
                self.display(&adb, mirrored, target, latest_preview)?
            }
        };
        while let Ok(event) = self.input.try_recv() {
            if let MirrorInput::Refresh(size) = event {
                latest_preview = Some(size);
            }
        }
        self.check_cancelled()?;
        self.check_player(installation, instance, target, source, false)?;
        self.confirm_transport(target, mirrored, &adb)?;
        let mut control = MirrorControl::connect(
            &adb.path,
            mirrored.transport_id.get(),
            &mirrored.endpoint.to_string(),
        )?;
        self.check_cancelled()?;
        let result = self.forward(
            &mut control,
            &adb,
            target,
            mirrored,
            display,
            latest_preview,
        );
        // close() releases held controls and removes only the resources created by this session.
        result.and(control.close())
    }

    fn wait_configured_target(
        &self,
        installation: &Installation,
        instance: &str,
    ) -> io::Result<ConfiguredTarget> {
        let deadline = Instant::now() + WAIT_DEVICE;
        loop {
            self.check_cancelled()?;
            match read_target(installation, instance) {
                Err(error)
                    if Instant::now() < deadline
                        && matches!(
                            error.get_ref().and_then(|error| error.downcast_ref()),
                            Some(TargetError::NoActivePort | TargetError::PortMismatch)
                        ) =>
                {
                    thread::sleep(Duration::from_millis(350));
                }
                ready => return ready,
            }
        }
    }

    fn check_player(
        &self,
        installation: &Installation,
        instance: &str,
        target: ConfiguredTarget,
        source: PlayerSource,
        allow_boot_wait: bool,
    ) -> io::Result<()> {
        let deadline = Instant::now() + WAIT_DEVICE;
        loop {
            self.check_cancelled()?;
            let current = read_target(installation, instance)?;
            if current != target {
                return Err(io::Error::other(
                    "The selected BlueStacks ADB configuration changed.",
                ));
            }
            match verify_adb_player(&installation.player, instance, target.endpoint()) {
                Ok(verified) if verified == source => return Ok(()),
                Ok(_) => {
                    return Err(io::Error::other(
                        "The selected BlueStacks window or player changed.",
                    ));
                }
                Err(PlayerError::NoListener) if allow_boot_wait && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(350));
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }

    fn adb(&self, hint: Option<&Path>) -> io::Result<AdbTool> {
        self.check_cancelled()?;
        let server_protocol = match server_protocol() {
            Ok(version) if version >= MIN_ADB_PROTOCOL => Some(version),
            Ok(_) => {
                return Err(io::Error::other(
                    "ADB server on local port 5037 is too old for mirror input.",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => None,
            Err(error) => return Err(error),
        };
        let tool = select_adb(hint, server_protocol, || self.check_cancelled())?;
        if server_protocol.is_none() {
            self.start_server(&tool)?;
        }
        Ok(tool)
    }

    fn start_server(&self, tool: &AdbTool) -> io::Result<()> {
        self.check_cancelled()?;
        match server_protocol() {
            Ok(version) if version == tool.protocol => return Ok(()),
            Ok(_) => {
                return Err(io::Error::other(
                    "The local ADB server changed before startup.",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
            Err(error) => return Err(error),
        }
        // Run server mode directly. Unlike the client command `start-server`, it cannot replace
        // a different server if another application claims the port between our check and bind.
        let mut child = adb_command(&tool.path)
            .args(["-L", "tcp:127.0.0.1:5037", "server", "nodaemon"])
            .stdout(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + ADB_VERSION_TIMEOUT;
        let result = loop {
            if let Err(error) = self.check_cancelled() {
                break Err(error);
            }
            match server_protocol() {
                Ok(version) if version == tool.protocol => break Ok(()),
                Ok(_) => {
                    break Err(io::Error::other(
                        "A different local ADB server claimed port 5037.",
                    ));
                }
                Err(error)
                    if error.kind() == io::ErrorKind::ConnectionRefused
                        && Instant::now() < deadline =>
                {
                    match child.try_wait() {
                        Ok(None) => thread::sleep(Duration::from_millis(80)),
                        Ok(Some(_)) => {
                            break Err(io::Error::other(
                                "The selected ADB could not start its local server.",
                            ));
                        }
                        Err(error) => break Err(error),
                    }
                }
                Err(error) => break Err(error),
            }
        };
        if result.is_err() {
            let _ = child.kill();
            let _ = child.try_wait();
        }
        // A ready server is shared host infrastructure and outlives the mirror session.
        result
    }

    fn wait_transport(
        &self,
        configured: ConfiguredTarget,
        adb: &AdbTool,
        before_connect: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<MirrorTarget> {
        let deadline = Instant::now() + WAIT_DEVICE;
        let mut before_connect = Some(before_connect);
        loop {
            self.check_cancelled()?;
            match adb_target(configured)? {
                Ok(target) => return Ok(target),
                Err(TargetError::NotConnected) if before_connect.is_some() => {
                    before_connect.take().unwrap()()?;
                    self.check_cancelled()?;
                    if server_protocol()? != adb.protocol {
                        return Err(io::Error::other(
                            "The local ADB server changed before connecting.",
                        ));
                    }
                    host_request(&format!("host:connect:{}", configured.endpoint()))?;
                }
                Err(TargetError::NotConnected | TargetError::NotReady)
                    if Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(200));
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }

    fn confirm_transport(
        &self,
        configured: ConfiguredTarget,
        expected: MirrorTarget,
        adb: &AdbTool,
    ) -> io::Result<()> {
        self.check_cancelled()?;
        if server_protocol()? != adb.protocol
            || adb_target(configured)?.map_err(io::Error::other)? != expected
        {
            return Err(io::Error::other(
                "The selected local ADB transport changed.",
            ));
        }
        Ok(())
    }

    fn display(
        &self,
        adb: &AdbTool,
        mirrored: MirrorTarget,
        target: ConfiguredTarget,
        preview: Option<[u32; 2]>,
    ) -> io::Result<[u32; 2]> {
        self.confirm_transport(target, mirrored, adb)?;
        query_display_size(
            &adb.path,
            mirrored.transport_id.get(),
            &mirrored.endpoint.to_string(),
            preview,
        )
    }

    fn wait_for_preview_size(&self) -> io::Result<[u32; 2]> {
        let deadline = Instant::now() + WAIT_DEVICE;
        while Instant::now() < deadline {
            self.check_cancelled()?;
            match self.input.recv_timeout(Duration::from_millis(100)) {
                Ok(MirrorInput::Refresh(size)) if valid_dimensions(size) => return Ok(size),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(cancelled()),
            }
        }
        Err(io::Error::other(
            "The selected Android display dimensions could not be verified.",
        ))
    }

    fn forward(
        &self,
        control: &mut MirrorControl,
        adb: &AdbTool,
        target: ConfiguredTarget,
        mirrored: MirrorTarget,
        mut display: [u32; 2],
        mut preview: Option<[u32; 2]>,
    ) -> io::Result<()> {
        if !self.emit(MirrorEvent::Ready(display)) {
            return Ok(());
        }
        while !self.cancelled.load(Ordering::Acquire) {
            let command = match self.input.recv_timeout(Duration::from_millis(100)) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            self.check_cancelled()?;
            match command {
                MirrorInput::Touch(action, point, size) if size == display => {
                    control.touch(action, point, size)?
                }
                MirrorInput::Scroll(point, size, horizontal, vertical) if size == display => {
                    control.scroll(point, size, horizontal, vertical)?
                }
                MirrorInput::Key(action, code, repeat, metastate) => {
                    control.key(action, code, repeat, metastate)?
                }
                MirrorInput::Back => control.back()?,
                MirrorInput::Home => control.home()?,
                MirrorInput::Recents => control.recents()?,
                MirrorInput::Release => control.release_all()?,
                MirrorInput::Refresh(size) if valid_dimensions(size) && Some(size) != preview => {
                    control.release_all()?;
                    preview = Some(size);
                    let current = self.display(adb, mirrored, target, preview)?;
                    if current != display && !self.emit(MirrorEvent::Ready(current)) {
                        break;
                    }
                    display = current;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn check_cancelled(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(cancelled())
        } else {
            Ok(())
        }
    }

    fn emit(&self, event: MirrorEvent) -> bool {
        self.check_cancelled().is_ok() && self.events.send(event).is_ok()
    }
}

fn cancelled() -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        "BlueStacks mirror session was closed.",
    )
}

fn valid_dimensions(size: [u32; 2]) -> bool {
    size.into_iter()
        .all(|component| (1..=u32::from(u16::MAX)).contains(&component))
}

fn read_target(installation: &Installation, instance: &str) -> io::Result<ConfiguredTarget> {
    let config = fs::read_to_string(&installation.config)?;
    configured_target(&config, instance).map_err(io::Error::other)
}

fn adb_target(configured: ConfiguredTarget) -> io::Result<Result<MirrorTarget, TargetError>> {
    let raw = host_request("host:devices-l")?;
    Ok(configured.validate_adb_devices(&format!("List of devices attached\n{raw}")))
}

fn server_protocol() -> io::Result<u32> {
    u32::from_str_radix(host_request("host:version")?.trim(), 16).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid response from ADB on local port 5037.",
        )
    })
}

fn host_request(request: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect_timeout(&ADB_SERVER, HOST_IO_TIMEOUT)?;
    stream.set_read_timeout(Some(HOST_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(HOST_IO_TIMEOUT))?;
    exchange(&mut stream, request)
}

fn exchange(stream: &mut (impl Read + Write), request: &str) -> io::Result<String> {
    if request.len() > usize::from(u16::MAX) || !request.is_ascii() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid local ADB request.",
        ));
    }
    stream.write_all(format!("{:04x}{request}", request.len()).as_bytes())?;
    let mut status = [0; 4];
    stream.read_exact(&mut status)?;
    if !matches!(&status, b"OKAY" | b"FAIL") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid local ADB response status.",
        ));
    }
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let size = std::str::from_utf8(&size)
        .ok()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .and_then(|value| u16::from_str_radix(value, 16).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid local ADB response length.",
            )
        })?;
    let mut payload = vec![0; usize::from(size)];
    stream.read_exact(&mut payload)?;
    if &status == b"FAIL" {
        return Err(io::Error::other(
            "The local ADB server refused the mirror request.",
        ));
    }
    String::from_utf8(payload).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Local ADB returned invalid text.",
        )
    })
}

struct AdbTool {
    path: PathBuf,
    protocol: u32,
}

fn select_adb(
    hint: Option<&Path>,
    protocol: Option<u32>,
    check_cancelled: impl Fn() -> io::Result<()>,
) -> io::Result<AdbTool> {
    let filename = "platform-tools/adb.exe";
    let managed = crate::runtime_install::default_destination().ok();
    let default = env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("Android/Sdk"));
    let portable = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("runtime")));
    let mut candidates = Vec::new();
    if let Some(hint) = hint {
        candidates.push(if hint.is_dir() {
            hint.join(filename)
        } else {
            hint.to_owned()
        });
    }
    candidates.extend(
        ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
            .into_iter()
            .filter_map(env::var_os)
            .map(PathBuf::from)
            .chain(portable)
            .chain(default)
            .chain(managed)
            .filter(|root| root.is_absolute())
            .map(|root| root.join(filename)),
    );
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(
            env::split_paths(&path)
                .filter(|root| root.is_absolute())
                .map(|root| root.join("adb.exe")),
        );
    }
    let mut tested = Vec::new();
    for candidate in candidates {
        check_cancelled()?;
        let Ok(path) = candidate.canonicalize() else {
            continue;
        };
        if !path.is_file()
            || !path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("adb.exe"))
            || tested.contains(&path)
        {
            continue;
        }
        tested.push(path.clone());
        if let Ok(version) = adb_protocol(&path) {
            if version >= MIN_ADB_PROTOCOL && protocol.is_none_or(|server| server == version) {
                return Ok(AdbTool {
                    path,
                    protocol: version,
                });
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "No installed modern Android SDK ADB matches the local server on port 5037.",
    ))
}

fn adb_command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command
        .creation_flags(CREATE_NO_WINDOW)
        .env_remove("ADB_SERVER_SOCKET")
        .env_remove("ANDROID_ADB_SERVER_ADDRESS")
        .env_remove("ANDROID_ADB_SERVER_PORT")
        .env_remove("ADB_TRACE")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn adb_protocol(path: &Path) -> io::Result<u32> {
    let mut output = tempfile::tempfile()?;
    let mut child = adb_command(path)
        .arg("version")
        .stdout(output.try_clone()?)
        .spawn()?;
    let deadline = Instant::now() + ADB_VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err(io::Error::other("ADB version could not be read.")),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            pending => {
                let _ = child.kill();
                let _ = child.try_wait();
                return Err(pending.err().unwrap_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "ADB version timed out.")
                }));
            }
        }
    }
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.take(4096).read_to_string(&mut text)?;
    parse_version(&text).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Unknown ADB client protocol version.",
        )
    })
}

fn parse_version(output: &str) -> Option<u32> {
    let mut version = output.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("Android Debug Bridge version 1.0.")
    });
    let protocol = version.next()?.parse().ok()?;
    version.next().is_none().then_some(protocol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an operator-owned BlueStacks session; reads Windows, configuration and ADB host state only"]
    fn inspect_owned_mirror_startup() {
        let instance = env::var("RUSTEMUDROID_MIRROR_DIAG_INSTANCE")
            .expect("set RUSTEMUDROID_MIRROR_DIAG_INSTANCE to the instance owned by the operator");
        let hint = env::var_os("RUSTEMUDROID_MIRROR_DIAG_ADB").map(PathBuf::from);
        let started = Instant::now();
        let installation = Installation::discover().expect("discover installed player");
        eprintln!(
            "+{:?} installation {:?}",
            started.elapsed(),
            installation.player
        );
        eprintln!("+{:?} begin find_preview_player", started.elapsed());
        let preview = find_preview_player(&installation.player, &instance);
        eprintln!("+{:?} find_preview_player {preview:?}", started.elapsed());
        let configured = read_target(&installation, &instance);
        eprintln!("+{:?} configured_target {configured:?}", started.elapsed());
        if let Ok(target) = configured {
            let verified = verify_adb_player(&installation.player, &instance, target.endpoint());
            eprintln!("+{:?} verify_adb_player {verified:?}", started.elapsed());
            if verified.is_ok() {
                let listed = adb_target(target);
                eprintln!("+{:?} host:devices-l {listed:?}", started.elapsed());
            }
        }
        let protocol = server_protocol();
        eprintln!("+{:?} host:version {protocol:?}", started.elapsed());
        if let Some(protocol) = match &protocol {
            Ok(protocol) => Some(Some(*protocol)),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Some(None),
            Err(_) => None,
        } {
            let selected = select_adb(hint.as_deref(), protocol, || Ok(()))
                .map(|tool| (tool.path, tool.protocol));
            eprintln!("+{:?} ADB selection {selected:?}", started.elapsed());
        }
    }

    struct ServerReply {
        read: io::Cursor<&'static [u8]>,
        written: Vec<u8>,
    }

    impl Read for ServerReply {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.read.read(buffer)
        }
    }

    impl Write for ServerReply {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.written.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn formats_smart_socket_requests_and_reads_only_the_framed_payload() {
        let mut server = ServerReply {
            read: io::Cursor::new(b"OKAY00040029"),
            written: Vec::new(),
        };
        assert_eq!(exchange(&mut server, "host:version").unwrap(), "0029");
        assert_eq!(server.written, b"000chost:version");

        let mut server = ServerReply {
            read: io::Cursor::new(b"OKAY0000"),
            written: Vec::new(),
        };
        assert!(exchange(&mut server, "host:devices-l").unwrap().is_empty());
        assert_eq!(server.written, b"000ehost:devices-l");
    }

    #[test]
    fn rejects_refused_and_malformed_host_replies() {
        for (reply, kind) in [
            (&b"FAIL0004nope"[..], io::ErrorKind::Other),
            (&b"OKAYzzzz"[..], io::ErrorKind::InvalidData),
            (&b"WAIT0000"[..], io::ErrorKind::InvalidData),
            (&b"OKAY0004x"[..], io::ErrorKind::UnexpectedEof),
        ] {
            let mut server = ServerReply {
                read: io::Cursor::new(reply),
                written: Vec::new(),
            };
            assert_eq!(
                exchange(&mut server, "host:devices-l").unwrap_err().kind(),
                kind
            );
        }
    }

    #[test]
    fn modern_client_protocol_comes_from_adb_version_not_package_revision() {
        let windows = "Android Debug Bridge version 1.0.41\r\nVersion 37.0.0\r\nInstalled as C:\\Android\\adb.exe\r\n";
        assert_eq!(parse_version(windows), Some(41));
        assert_eq!(parse_version("Version 37.0.0"), None);
        assert_eq!(parse_version(&format!("{windows}{windows}")), None);
    }
}
