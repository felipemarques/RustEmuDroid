use std::{
    env,
    ffi::{OsStr, OsString, c_void},
    fmt, io,
    io::{Read, Seek, SeekFrom},
    net::{Ipv4Addr, SocketAddrV4},
    num::NonZeroUsize,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr, slice, thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{
        ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, FILETIME, HANDLE, HWND, LPARAM, LocalFree,
        NTSTATUS, RECT, STATUS_BUFFER_TOO_SMALL, STATUS_INFO_LENGTH_MISMATCH, STATUS_SUCCESS,
        WAIT_TIMEOUT,
    },
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP_STATE_LISTEN, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    },
    System::Threading::{
        CREATE_NO_WINDOW, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
    },
    UI::{
        Shell::CommandLineToArgvW,
        WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetClientRect, GetWindow, GetWindowThreadProcessId, IsIconic,
            IsWindowVisible,
        },
    },
};

use super::mirror_target::valid_instance;

const CIM_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
const AF_INET: u32 = 2;
const FILETIME_TICKS_PER_MS: u64 = 10_000;
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
const MAX_COMMAND_LINE_BYTES: usize = 128 * 1024;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        information_class: u32,
        information: *mut c_void,
        information_length: u32,
        return_length: *mut u32,
    ) -> NTSTATUS;
}

const QUERY_PLAYERS: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$players = @(Get-CimInstance -Query "SELECT ProcessId, CommandLine, CreationDate FROM Win32_Process WHERE Name = 'HD-Player.exe'")
foreach ($player in $players) {
    $creation = if ($null -eq $player.CreationDate) { 0 } else { $player.CreationDate.ToFileTimeUtc() }
    $cmd = if ($null -eq $player.CommandLine) { '' } else {
        [BitConverter]::ToString([Text.Encoding]::Unicode.GetBytes($player.CommandLine)).Replace('-', '')
    }
    "$($player.ProcessId)`t$creation`t$cmd"
}
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlayerSource {
    pub(super) pid: u32,
    pub(super) hwnd: NonZeroUsize,
    pub(super) creation_time: u64,
}

/// A retained handle to the process whose path, instance and creation time were verified.
#[derive(Debug)]
pub(crate) struct PlayerLifetime(OwnedHandle);

impl PlayerLifetime {
    pub(crate) fn is_live(&self) -> bool {
        process_is_live(&self.0)
    }

    #[cfg(test)]
    pub(crate) fn open_for_test(pid: u32) -> io::Result<Self> {
        open_process(pid).map(Self)
    }
}

#[derive(Debug)]
pub(super) enum PlayerError {
    InvalidInstance,
    InvalidInstallation,
    InvalidEndpoint,
    NoListener,
    AmbiguousListener,
    ListenerChanged,
    CimTimedOut,
    CimUnavailable,
    InvalidCimSnapshot,
    ProcessUnverified,
    NoPlayer,
    AmbiguousPlayer,
    NoWindow,
    AmbiguousWindow,
    Windows(io::Error),
}

impl fmt::Display for PlayerError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidInstance => "Invalid BlueStacks instance identifier.",
            Self::InvalidInstallation => "Could not verify the installed HD-Player.exe path.",
            Self::InvalidEndpoint => "Expected an IPv4 loopback ADB endpoint.",
            Self::NoListener => "No local TCP listener owns the selected BlueStacks ADB endpoint.",
            Self::AmbiguousListener => {
                "More than one process owns listeners for the local ADB endpoint."
            }
            Self::ListenerChanged => "The local ADB listener changed during verification.",
            Self::CimTimedOut => "Windows timed out reading BlueStacks process information.",
            Self::CimUnavailable => "Windows could not read BlueStacks process information.",
            Self::InvalidCimSnapshot => "Windows returned an invalid BlueStacks process snapshot.",
            Self::ProcessUnverified => {
                "The BlueStacks process identity or instance could not be verified."
            }
            Self::NoPlayer => {
                "The selected BlueStacks instance is not running in this installation."
            }
            Self::AmbiguousPlayer => {
                "More than one player matches the selected BlueStacks instance."
            }
            Self::NoWindow => "The selected BlueStacks player has no available top-level window.",
            Self::AmbiguousWindow => {
                "The selected BlueStacks player has multiple available top-level windows."
            }
            Self::Windows(error) => {
                return write!(
                    out,
                    "Windows could not verify the BlueStacks player ({error})."
                );
            }
        };
        out.write_str(message)
    }
}

impl std::error::Error for PlayerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Windows(error) => Some(error),
            _ => None,
        }
    }
}

/// Looks up one official player window without consulting or starting ADB. Call off the UI thread.
pub(super) fn find_preview_player(
    installed_player: &Path,
    instance: &str,
) -> Result<(PlayerSource, PlayerLifetime), PlayerError> {
    find_player(installed_player, instance, None)
}

/// Binds a loopback listener to one official player window. Call off the UI thread and verify the
/// config and ADB transport separately. Recheck after a disconnect or any change of target.
pub(super) fn verify_adb_player(
    installed_player: &Path,
    instance: &str,
    endpoint: SocketAddrV4,
) -> Result<PlayerSource, PlayerError> {
    find_player(installed_player, instance, Some(endpoint)).map(|(source, _)| source)
}

fn find_player(
    installed_player: &Path,
    instance: &str,
    endpoint: Option<SocketAddrV4>,
) -> Result<(PlayerSource, PlayerLifetime), PlayerError> {
    if !valid_instance(instance) {
        return Err(PlayerError::InvalidInstance);
    }
    if !installed_player
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("HD-Player.exe"))
    {
        return Err(PlayerError::InvalidInstallation);
    }
    let expected_path = installed_player
        .canonicalize()
        .map_err(|_| PlayerError::InvalidInstallation)?;
    let owner = endpoint.map(listener_pid).transpose()?;
    let rows = query_processes(QUERY_PLAYERS)?;
    let mut selected = None;
    for row in rows
        .into_iter()
        .filter(|row| owner.is_none_or(|pid| row.pid == pid))
    {
        let process = open_process(row.pid).map_err(|_| PlayerError::ProcessUnverified)?;
        if process_path(&process).map_err(|_| PlayerError::ProcessUnverified)? != expected_path {
            continue;
        }
        let created =
            process_creation_time(&process).map_err(|_| PlayerError::ProcessUnverified)?;
        // CIM's WMI datetime can have lower precision than the kernel FILETIME.
        if row.created == 0 || created.abs_diff(row.created) >= FILETIME_TICKS_PER_MS {
            return Err(PlayerError::ProcessUnverified);
        }
        let command = if row.command.is_empty() {
            process_command_line(&process)?
        } else {
            row.command
        };
        let id = instance_argument(command)?;
        if id != instance {
            continue;
        }
        if selected.replace((row.pid, created, process)).is_some() {
            return Err(PlayerError::AmbiguousPlayer);
        }
    }
    let (pid, creation_time, process) = selected.ok_or(if owner.is_some() {
        PlayerError::ProcessUnverified
    } else {
        PlayerError::NoPlayer
    })?;
    let hwnd = player_window(pid)?;
    if let Some(endpoint) = endpoint {
        if listener_pid(endpoint).ok() != Some(pid) {
            return Err(PlayerError::ListenerChanged);
        }
    }
    if !process_is_live(&process) || window_pid(hwnd.get() as HWND) != Some(pid) {
        return Err(PlayerError::ProcessUnverified);
    }
    Ok((
        PlayerSource {
            pid,
            hwnd,
            creation_time,
        },
        PlayerLifetime(process),
    ))
}

fn listener_pid(endpoint: SocketAddrV4) -> Result<u32, PlayerError> {
    if *endpoint.ip() != Ipv4Addr::LOCALHOST || endpoint.port() == 0 {
        return Err(PlayerError::InvalidEndpoint);
    }
    // This IPv4 DWORD table requires DWORD alignment. Size can change between kernel snapshots.
    let mut words = vec![0_u32; 256];
    for _ in 0..3 {
        let mut size = (words.len() * size_of::<u32>()) as u32;
        let result = unsafe {
            GetExtendedTcpTable(
                words.as_mut_ptr().cast(),
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match result {
            ERROR_SUCCESS => {
                let size = size as usize;
                if size > words.len() * size_of::<u32>() || size % size_of::<u32>() != 0 {
                    return Err(invalid_tcp_table());
                }
                return owner_from_table(&words[..size / size_of::<u32>()], endpoint);
            }
            ERROR_INSUFFICIENT_BUFFER if size as usize <= MAX_SNAPSHOT_BYTES => {
                words.resize((size as usize).div_ceil(size_of::<u32>()), 0);
            }
            code => {
                return Err(PlayerError::Windows(io::Error::from_raw_os_error(
                    code as i32,
                )));
            }
        }
    }
    Err(invalid_tcp_table())
}

fn owner_from_table(words: &[u32], endpoint: SocketAddrV4) -> Result<u32, PlayerError> {
    let (&count, rows) = words.split_first().ok_or_else(invalid_tcp_table)?;
    // MIB_TCPROW_OWNER_PID is six DWORDs and starts after a single DWORD table count.
    const WIDTH: usize = size_of::<MIB_TCPROW_OWNER_PID>() / size_of::<u32>();
    const {
        assert!(WIDTH == 6 && align_of::<MIB_TCPROW_OWNER_PID>() == align_of::<u32>());
        assert!(std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table) == size_of::<u32>());
    }
    let required = (count as usize)
        .checked_mul(WIDTH)
        .ok_or_else(invalid_tcp_table)?;
    let mut owner = None;
    for row in rows
        .get(..required)
        .ok_or_else(invalid_tcp_table)?
        .chunks_exact(WIDTH)
    {
        let address = Ipv4Addr::from(row[1].to_ne_bytes());
        let applicable = row[0] == MIB_TCP_STATE_LISTEN as u32
            && (address == *endpoint.ip() || address.is_unspecified())
            && u16::from_be(row[2] as u16) == endpoint.port();
        if applicable {
            let pid = row[5];
            if pid == 0 || owner.replace(pid).is_some_and(|old| old != pid) {
                return Err(PlayerError::AmbiguousListener);
            }
        }
    }
    owner.ok_or(PlayerError::NoListener)
}

fn invalid_tcp_table() -> PlayerError {
    PlayerError::Windows(io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid TCP listener snapshot",
    ))
}

struct CimPlayer {
    pid: u32,
    created: u64,
    command: Vec<u16>,
}

fn query_processes(script: &str) -> Result<Vec<CimPlayer>, PlayerError> {
    let executable = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or(PlayerError::CimUnavailable)?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    // Use a temporary file to avoid a full stdout pipe delaying child exit or cancellation.
    let mut output = tempfile::tempfile().map_err(PlayerError::Windows)?;
    let mut command = Command::new(executable);
    let mut process = command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(PlayerError::Windows)?)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| PlayerError::CimUnavailable)?;
    wait_for_query(&mut process)?;
    output
        .seek(SeekFrom::Start(0))
        .map_err(PlayerError::Windows)?;
    let mut text = String::new();
    output
        .take((MAX_SNAPSHOT_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|_| PlayerError::InvalidCimSnapshot)?;
    if text.len() > MAX_SNAPSHOT_BYTES {
        return Err(PlayerError::InvalidCimSnapshot);
    }
    parse_players(&text)
}

fn wait_for_query(process: &mut Child) -> Result<(), PlayerError> {
    let deadline = Instant::now() + CIM_TIMEOUT;
    loop {
        match process.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(PlayerError::CimUnavailable),
            Err(error) => {
                let _ = process.kill();
                return Err(PlayerError::Windows(error));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = process.kill();
                return Err(PlayerError::CimTimedOut);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn parse_players(text: &str) -> Result<Vec<CimPlayer>, PlayerError> {
    let mut rows = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let mut columns = line.split('\t');
        let pid: u32 = columns
            .next()
            .and_then(|value| value.parse().ok())
            .filter(|&pid| pid != 0)
            .ok_or(PlayerError::InvalidCimSnapshot)?;
        let created = columns
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or(PlayerError::InvalidCimSnapshot)?;
        let command = columns
            .next()
            .and_then(decode_wide_hex)
            .ok_or(PlayerError::InvalidCimSnapshot)?;
        if columns.next().is_some() || rows.iter().any(|row: &CimPlayer| row.pid == pid) {
            return Err(PlayerError::InvalidCimSnapshot);
        }
        rows.push(CimPlayer {
            pid,
            created,
            command,
        });
    }
    Ok(rows)
}

fn decode_wide_hex(value: &str) -> Option<Vec<u16>> {
    if value.len() % 4 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..value.len())
        .step_by(4)
        .map(|index| {
            let little_endian = u16::from_str_radix(&value[index..index + 4], 16).ok()?;
            Some(little_endian.swap_bytes())
        })
        .collect()
}

struct LocalArgv(*mut *mut u16);

impl Drop for LocalArgv {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast::<c_void>()) };
    }
}

fn instance_argument(mut command: Vec<u16>) -> Result<String, PlayerError> {
    if command.is_empty() || command.contains(&0) {
        return Err(PlayerError::ProcessUnverified);
    }
    command.push(0);
    let mut count = 0;
    let argv = LocalArgv(unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) });
    if argv.0.is_null() || count < 1 {
        return Err(PlayerError::ProcessUnverified);
    }
    let arguments = unsafe { slice::from_raw_parts(argv.0, count as usize) };
    if unsafe { *arguments[0] } == 0 {
        return Err(PlayerError::ProcessUnverified);
    }
    // CommandLineToArgvW guarantees a null-terminated UTF-16 string for each argument.
    let mut arguments = arguments[1..].iter().map(|&wide| {
        let length = unsafe { (0..).take_while(|&index| *wide.add(index) != 0).count() };
        OsString::from_wide(unsafe { slice::from_raw_parts(wide, length) })
    });
    let mut instance = None;
    while let Some(argument) = arguments.next() {
        if argument == OsStr::new("--") {
            break;
        }
        if argument == OsStr::new("--instance") {
            let value = arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .filter(|id| valid_instance(id))
                .ok_or(PlayerError::ProcessUnverified)?;
            if instance.replace(value).is_some() {
                return Err(PlayerError::ProcessUnverified);
            }
        } else if argument
            .to_string_lossy()
            .split('=')
            .next()
            .is_some_and(|flag| flag.eq_ignore_ascii_case("--instance"))
        {
            return Err(PlayerError::ProcessUnverified);
        }
    }
    instance.ok_or(PlayerError::ProcessUnverified)
}

fn open_process(pid: u32) -> io::Result<OwnedHandle> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn process_path(process: &OwnedHandle) -> io::Result<PathBuf> {
    let mut path = vec![0_u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    PathBuf::from(OsString::from_wide(&path[..length as usize])).canonicalize()
}

#[repr(C)]
struct ProcessCommandLine {
    length: u16,
    maximum_length: u16,
    buffer: usize,
}

fn process_command_line(process: &OwnedHandle) -> Result<Vec<u16>, PlayerError> {
    // Windows may omit the command line from CIM for an elevated player. Ask the kernel for
    // its command-line snapshot using the same limited handle we used for path and creation time.
    let mut required = 0;
    let status = unsafe {
        NtQueryInformationProcess(
            process.as_raw_handle(),
            PROCESS_COMMAND_LINE_INFORMATION,
            ptr::null_mut(),
            0,
            &mut required,
        )
    };
    if !matches!(
        status,
        STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL
    ) {
        return Err(PlayerError::ProcessUnverified);
    }

    for _ in 0..2 {
        let capacity = required as usize;
        if !(size_of::<ProcessCommandLine>()..=MAX_COMMAND_LINE_BYTES).contains(&capacity) {
            return Err(PlayerError::ProcessUnverified);
        }
        let mut storage = vec![0_usize; capacity.div_ceil(size_of::<usize>())];
        let mut returned = 0;
        let status = unsafe {
            NtQueryInformationProcess(
                process.as_raw_handle(),
                PROCESS_COMMAND_LINE_INFORMATION,
                storage.as_mut_ptr().cast(),
                required,
                &mut returned,
            )
        };
        if status == STATUS_SUCCESS {
            return command_line_from_buffer(&storage, capacity, returned as usize)
                .ok_or(PlayerError::ProcessUnverified);
        }
        if !matches!(
            status,
            STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL
        ) || returned <= required
        {
            break;
        }
        required = returned;
    }
    Err(PlayerError::ProcessUnverified)
}

fn command_line_from_buffer(
    storage: &[usize],
    capacity: usize,
    returned: usize,
) -> Option<Vec<u16>> {
    let header_bytes = size_of::<ProcessCommandLine>();
    if !(header_bytes..=capacity).contains(&returned) || capacity > size_of_val(storage) {
        return None;
    }
    let start = storage.as_ptr();
    // The buffer is aligned for the returned UNICODE_STRING. Validate its pointer against the
    // local response before following it, then read through the owned buffer's provenance.
    let header = unsafe { &*start.cast::<ProcessCommandLine>() };
    let length = usize::from(header.length);
    let maximum = usize::from(header.maximum_length);
    let offset = header.buffer.checked_sub(start as usize)?;
    if length == 0
        || length % size_of::<u16>() != 0
        || maximum < length
        || maximum % size_of::<u16>() != 0
        || offset < header_bytes
        || offset % align_of::<u16>() != 0
        || offset.checked_add(length)? > returned
        || offset.checked_add(maximum)? > capacity
    {
        return None;
    }
    let text = unsafe { start.cast::<u8>().add(offset).cast::<u16>() };
    Some(unsafe { slice::from_raw_parts(text, length / size_of::<u16>()) }.to_vec())
}

pub(super) fn process_creation_time(process: &OwnedHandle) -> io::Result<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn process_is_live(process: &OwnedHandle) -> bool {
    (unsafe { WaitForSingleObject(process.as_raw_handle(), 0) }) == WAIT_TIMEOUT
}

fn window_pid(hwnd: HWND) -> Option<u32> {
    let mut owner = 0;
    (unsafe { GetWindowThreadProcessId(hwnd, &mut owner) } != 0).then_some(owner)
}

fn player_window(pid: u32) -> Result<NonZeroUsize, PlayerError> {
    #[derive(Default)]
    struct Selection {
        pid: u32,
        source: Option<NonZeroUsize>,
        ambiguous: bool,
    }

    unsafe extern "system" fn visit(hwnd: HWND, context: LPARAM) -> i32 {
        // EnumWindows visits synchronously. `context` points to our stack-local Selection.
        let selection = unsafe { &mut *(context as *mut Selection) };
        if window_pid(hwnd) != Some(selection.pid) {
            return 1;
        }
        let mut client = RECT::default();
        let available = unsafe {
            IsWindowVisible(hwnd) != 0
                && IsIconic(hwnd) == 0
                && GetWindow(hwnd, GW_OWNER).is_null()
                && GetClientRect(hwnd, &mut client) != 0
        } && client.right > client.left
            && client.bottom > client.top;
        if let Some(window) = NonZeroUsize::new(hwnd as usize).filter(|_| available) {
            selection.ambiguous |= selection.source.replace(window).is_some();
        }
        1
    }

    let mut selection = Selection {
        pid,
        ..Selection::default()
    };
    if unsafe { EnumWindows(Some(visit), ptr::from_mut(&mut selection) as LPARAM) } == 0 {
        return Err(PlayerError::Windows(io::Error::last_os_error()));
    }
    if selection.ambiguous {
        return Err(PlayerError::AmbiguousWindow);
    }
    selection.source.ok_or(PlayerError::NoWindow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const PLAYER: &str = r#""C:\Program Files\BlueStacks_nxt\HD-Player.exe""#;

    fn instance(args: &str) -> Result<String, PlayerError> {
        instance_argument(format!("{PLAYER} {args}").encode_utf16().collect())
    }

    #[test]
    fn tokenizes_the_real_shortcut_form_and_rejects_incidental_text() {
        assert_eq!(
            instance(r#"--instance "Pie64_1" --cmd launchApp --package org.supertuxkart.stk"#)
                .unwrap(),
            "Pie64_1"
        );
        for args in [
            r#"--package "label --instance Pie64""#,
            "--instance",
            "--instance --cmd launchApp",
            "--instance=Pie64",
            "-- --instance Pie64",
            "--instance Pie64 --instance Pie64_1",
            "--instance Pie64 --instance=Pie64_1",
            "--instance Pie64 --INSTANCE Pie64_1",
        ] {
            assert!(
                matches!(instance(args), Err(PlayerError::ProcessUnverified)),
                "{args}"
            );
        }
    }

    #[test]
    fn parses_delimited_cim_without_treating_user_text_as_a_second_record() {
        let raw = format!("{PLAYER} --instance Pie64 --package \"tabs\tand\nnewlines\"");
        let encoded: String = raw
            .encode_utf16()
            .map(|unit| format!("{:04X}", unit.swap_bytes()))
            .collect();
        let text = format!("12428\t134400245009653721\t{encoded}\r\n22000\t0\t\r\n");
        let mut rows = parse_players(&text).unwrap().into_iter();
        let first = rows.next().unwrap();
        assert_eq!((first.pid, first.created), (12428, 134400245009653721));
        assert_eq!(instance_argument(first.command).unwrap(), "Pie64");
        assert!(matches!(
            instance_argument(rows.next().unwrap().command),
            Err(PlayerError::ProcessUnverified)
        ));
        assert!(matches!(
            parse_players("1\t2\t00\n"),
            Err(PlayerError::InvalidCimSnapshot)
        ));
        assert!(matches!(
            parse_players("1\t2\t\n1\t3\t\n"),
            Err(PlayerError::InvalidCimSnapshot)
        ));
    }

    #[test]
    fn rejects_conflicting_local_and_wildcard_pids_but_ignores_external_binds() {
        let endpoint = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5585);
        let row = |ip: Ipv4Addr, pid| {
            [
                2,
                u32::from_ne_bytes(ip.octets()),
                u32::from(endpoint.port().to_be()),
                0,
                0,
                pid,
            ]
        };
        let local = row(Ipv4Addr::LOCALHOST, 120);
        let external = row(Ipv4Addr::new(192, 168, 1, 2), 600);
        let wildcard = row(Ipv4Addr::UNSPECIFIED, 220);
        let mut table = vec![2];
        table.extend(external);
        table.extend(local);
        assert_eq!(owner_from_table(&table, endpoint).unwrap(), 120);
        table[0] = 3;
        assert!(matches!(
            owner_from_table(&table, endpoint),
            Err(PlayerError::Windows(_))
        ));
        table.extend(wildcard);
        assert!(matches!(
            owner_from_table(&table, endpoint),
            Err(PlayerError::AmbiguousListener)
        ));
    }

    #[test]
    fn resolves_the_actual_listener_and_executable_of_our_test_process() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let std::net::SocketAddr::V4(address) = listener.local_addr().unwrap() else {
            unreachable!()
        };
        let pid = std::process::id();
        assert_eq!(listener_pid(address).unwrap(), pid);
        let process = open_process(pid).unwrap();
        assert_eq!(
            process_path(&process).unwrap(),
            env::current_exe().unwrap().canonicalize().unwrap()
        );
        assert!(process_creation_time(&process).unwrap() != 0);
        assert!(process_is_live(&process));
        let command = process_command_line(&process).unwrap();
        assert!(String::from_utf16(&command).unwrap().contains(".exe"));
        assert!(!command.contains(&0));
    }

    #[test]
    fn rejects_a_command_line_descriptor_pointing_outside_its_snapshot() {
        let mut words = [0_usize; 8];
        let capacity = size_of_val(&words);
        let base = words.as_mut_ptr();
        let header = base.cast::<ProcessCommandLine>();
        let end = base as usize + capacity;
        unsafe {
            *header = ProcessCommandLine {
                length: 4,
                maximum_length: 6,
                buffer: end - 2,
            };
        }
        assert!(command_line_from_buffer(&words, capacity, capacity).is_none());
        unsafe {
            (*header).buffer = base as usize - 2;
        }
        assert!(command_line_from_buffer(&words, capacity, capacity).is_none());
    }
}
