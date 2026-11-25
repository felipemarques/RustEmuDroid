"""Sample CPU time on retained Windows process handles around Android polls."""

from contextlib import ExitStack, contextmanager
import ctypes as ct
from ctypes import wintypes
from dataclasses import dataclass
from time import monotonic_ns


@dataclass(frozen=True)
class Counters:
    observed_ns: int
    kernel_ticks: int
    user_ticks: int


def filetime(value):
    return (value.dwHighDateTime << 32) | value.dwLowDateTime


def align(before, after, midpoint):
    span = after.observed_ns - before.observed_ns
    offset = midpoint - before.observed_ns
    if span <= 0 or not 0 <= offset <= span:
        raise ValueError("Could not bracket a SurfaceFlinger poll with process CPU samples.")
    if after.kernel_ticks < before.kernel_ticks or after.user_ticks < before.user_ticks:
        raise ValueError("Windows process CPU counters moved backwards.")
    return (
        before.kernel_ticks + (after.kernel_ticks - before.kernel_ticks) * offset // span,
        before.user_ticks + (after.user_ticks - before.user_ticks) * offset // span,
        span,
    )


class ProcessCpu:
    def __init__(self, api, processes):
        self.api = api
        self.processes = processes
        self.created = {}
        self.first = None
        self.last = None

    def require_alive(self):
        for role, (pid, handle) in self.processes.items():
            status = self.api.WaitForSingleObject(handle, 0)
            if status == 0xffffffff:
                raise ct.WinError(ct.get_last_error(), f"Could not check Windows process {role} ({pid})")
            if status != 258:  # WAIT_TIMEOUT means the process has not exited.
                raise ValueError(f"Windows process {role} ({pid}) did not stay alive.")

    def snapshot(self):
        samples = {}
        for role, (pid, handle) in self.processes.items():
            created, exited, kernel, user = (wintypes.FILETIME() for _ in range(4))
            started = monotonic_ns()
            ok = self.api.GetProcessTimes(handle, ct.byref(created), ct.byref(exited),
                                          ct.byref(kernel), ct.byref(user))
            finished = monotonic_ns()
            if not ok:
                raise ct.WinError(ct.get_last_error(), f"Could not read CPU time for {role} ({pid})")
            identity = filetime(created)
            if self.created.setdefault(role, identity) != identity:
                raise ValueError(f"Windows process identity changed for {role} ({pid}).")
            samples[role] = Counters((started + finished) // 2, filetime(kernel), filetime(user))
        return samples

    def poll(self, probe, device, name):
        before = self.snapshot()
        result = probe(device, name)
        after = self.snapshot()
        boundary = {role: align(before[role], after[role], result[0]) for role in self.processes}
        if self.first is None:
            self.first = boundary
        self.last = boundary
        return result

    def summarize(self, elapsed_ns, presentations):
        self.require_alive()
        rows = []
        for role, (pid, _) in self.processes.items():
            start_kernel, start_user, start_span = self.first[role]
            end_kernel, end_user, end_span = self.last[role]
            if end_kernel < start_kernel or end_user < start_user:
                raise ValueError(f"Windows CPU counters moved backwards for {role} ({pid}).")
            kernel_ms = (end_kernel - start_kernel) / 10_000
            user_ms = (end_user - start_user) / 10_000
            cpu_ms = kernel_ms + user_ms
            rows.append({
                "role": role, "pid": pid, "creation_filetime_100ns": self.created[role],
                "kernel_cpu_ms": kernel_ms, "user_cpu_ms": user_ms, "cpu_ms": cpu_ms,
                "cpu_ms_per_second": cpu_ms * 1e9 / elapsed_ns,
                "cpu_ms_per_android_presentation": cpu_ms / presentations if presentations else None,
                "boundary_bracket_ms": [start_span / 1e6, end_span / 1e6],
            })
        return {
            "scope": "Windows CPU time across all threads of each selected process",
            "window": "Linearly interpolated to the first and last SurfaceFlinger poll midpoints",
            "processes": rows,
        }


@contextmanager
def monitor_processes(targets):
    api = ct.WinDLL("kernel32", use_last_error=True)
    api.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    api.OpenProcess.restype = wintypes.HANDLE
    api.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    api.WaitForSingleObject.restype = wintypes.DWORD
    api.GetProcessTimes.argtypes = (wintypes.HANDLE,) + (ct.POINTER(wintypes.FILETIME),) * 4
    api.GetProcessTimes.restype = wintypes.BOOL
    api.CloseHandle.argtypes = (wintypes.HANDLE,)
    api.CloseHandle.restype = wintypes.BOOL

    # PROCESS_QUERY_LIMITED_INFORMATION and SYNCHRONIZE grant no process mutation rights.
    read_and_wait = 0x1000 | 0x100000
    with ExitStack() as handles:
        processes = {}
        for role, pid in targets:
            handle = api.OpenProcess(read_and_wait, False, pid)
            if not handle:
                raise ct.WinError(ct.get_last_error(), f"Could not open Windows process {role} ({pid})")
            handles.callback(api.CloseHandle, handle)
            processes[role] = (pid, handle)
        monitor = ProcessCpu(api, processes)
        monitor.require_alive()
        yield monitor
