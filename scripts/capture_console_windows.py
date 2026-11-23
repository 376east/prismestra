"""Observe visible console windows or one identified process exit in a disposable guest."""

import argparse
import ctypes
import json
import os
import time
from ctypes import wintypes as wt
from pathlib import Path

from normalize_process_events import filetime, image_path, process_id, verify_process_exit
from shell_acceptance import CONSOLE_CLASSES


EVENT_OBJECT_SHOW = 0x8002
WINEVENT_OUTOFCONTEXT = 0
OBJID_WINDOW = 0
CHILDID_SELF = 0
GA_ROOT = 2
PM_REMOVE = 1
WM_QUIT = 0x0012
QS_ALLINPUT = 0x04FF
MWMO_INPUTAVAILABLE = 0x0004
WAIT_FAILED = 0xFFFFFFFF
WAIT_OBJECT_0 = 0
WAIT_TIMEOUT = 258
SYNCHRONIZE = 0x00100000
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
FILETIME_EPOCH = 116444736000000000
MAX_OBSERVATIONS = 50000


def utc_filetime():
    return str(FILETIME_EPOCH + time.time_ns() // 100)


def filetime_text(value):
    return str((value.dwHighDateTime << 32) | value.dwLowDateTime)


def window_observation(window, window_class, owner_pid, reason, event_tick=None, owner_start=None):
    if window_class.casefold() not in CONSOLE_CLASSES:
        return None
    return {
        'window': str(window), 'class': window_class, 'window_owner_pid': owner_pid or None,
        'window_owner_created_filetime': owner_start, 'reason': reason,
        'observed_filetime': utc_filetime(), 'event_tick': event_tick,
    }


class WindowCapture:
    def __init__(self):
        self.user = ctypes.WinDLL('user32', use_last_error=True)
        self.kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        event_proc = ctypes.WINFUNCTYPE(None, wt.HANDLE, wt.DWORD, wt.HWND, wt.LONG,
                                       wt.LONG, wt.DWORD, wt.DWORD)
        enum_proc = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
        self.event_callback = event_proc(self.on_show)
        self.enum_callback = enum_proc(self.on_enumerated)
        self.hook = self.bind(self.user.SetWinEventHook, wt.HANDLE, wt.DWORD, wt.DWORD, wt.HMODULE,
                              event_proc, wt.DWORD, wt.DWORD, wt.DWORD)
        self.unhook = self.bind(self.user.UnhookWinEvent, wt.BOOL, wt.HANDLE)
        self.enum_windows = self.bind(self.user.EnumWindows, wt.BOOL, enum_proc, wt.LPARAM)
        self.is_visible = self.bind(self.user.IsWindowVisible, wt.BOOL, wt.HWND)
        self.ancestor = self.bind(self.user.GetAncestor, wt.HWND, wt.HWND, wt.UINT)
        self.class_name = self.bind(self.user.GetClassNameW, ctypes.c_int, wt.HWND, wt.LPWSTR, ctypes.c_int)
        self.window_thread = self.bind(self.user.GetWindowThreadProcessId, wt.DWORD, wt.HWND,
                                       ctypes.POINTER(wt.DWORD))
        self.peek = self.bind(self.user.PeekMessageW, wt.BOOL, ctypes.POINTER(wt.MSG), wt.HWND,
                              wt.UINT, wt.UINT, wt.UINT)
        self.translate = self.bind(self.user.TranslateMessage, wt.BOOL, ctypes.POINTER(wt.MSG))
        self.dispatch = self.bind(self.user.DispatchMessageW, ctypes.c_ssize_t, ctypes.POINTER(wt.MSG))
        self.wait = self.bind(self.user.MsgWaitForMultipleObjectsEx, wt.DWORD, wt.DWORD, ctypes.c_void_p,
                              wt.DWORD, wt.DWORD, wt.DWORD)
        self.session = self.bind(self.kernel.ProcessIdToSessionId, wt.BOOL, wt.DWORD,
                                 ctypes.POINTER(wt.DWORD))
        self.current_thread = self.bind(self.kernel.GetCurrentThreadId, wt.DWORD)
        self.open_process = self.bind(self.kernel.OpenProcess, wt.HANDLE, wt.DWORD, wt.BOOL, wt.DWORD)
        self.close_handle = self.bind(self.kernel.CloseHandle, wt.BOOL, wt.HANDLE)
        times = ctypes.POINTER(wt.FILETIME)
        self.process_times = self.bind(self.kernel.GetProcessTimes, wt.BOOL, wt.HANDLE,
                                      times, times, times, times)
        self.handle_pid = self.bind(self.kernel.GetProcessId, wt.DWORD, wt.HANDLE)
        self.handle_image = self.bind(self.kernel.QueryFullProcessImageNameW, wt.BOOL,
                                      wt.HANDLE, wt.DWORD, wt.LPWSTR, ctypes.POINTER(wt.DWORD))
        self.wait_process = self.bind(self.kernel.WaitForSingleObject, wt.DWORD, wt.HANDLE, wt.DWORD)
        self.thread_desktop = self.bind(self.user.GetThreadDesktop, wt.HANDLE, wt.DWORD)
        self.object_info = self.bind(self.user.GetUserObjectInformationW, wt.BOOL, wt.HANDLE, ctypes.c_int,
                                     ctypes.c_void_p, wt.DWORD, ctypes.POINTER(wt.DWORD))
        self.observations = []
        self.gaps = []
        self.snapshot = ''
        self.overflow = False

    @staticmethod
    def bind(function, result, *args):
        function.restype = result
        function.argtypes = args
        return function

    def describe_desktop(self):
        session = wt.DWORD()
        if not self.session(os.getpid(), ctypes.byref(session)) or not session.value:
            raise RuntimeError('No interactive Windows session is attached to the collector')
        desktop = self.thread_desktop(self.current_thread())
        name = ctypes.create_unicode_buffer(256)
        needed = wt.DWORD()
        if not desktop or not self.object_info(desktop, 2, name, ctypes.sizeof(name), ctypes.byref(needed)):
            raise ctypes.WinError(ctypes.get_last_error())
        return {'session_id': session.value, 'name': name.value}

    def observe(self, hwnd, reason, event_tick=None):
        if self.overflow:
            return
        if len(self.observations) + len(self.gaps) >= MAX_OBSERVATIONS:
            self.overflow = True
            self.gaps.append('Window capture exceeded the observation limit')
            return
        root = self.ancestor(hwnd, GA_ROOT)
        if not root:
            self.gaps.append(f'A window disappeared before its top-level owner could be queried: {hwnd}')
            return
        if root != hwnd:
            return
        name = ctypes.create_unicode_buffer(256)
        if not self.class_name(hwnd, name, len(name)):
            self.gaps.append(f'Could not query the class of a top-level window: {hwnd}')
            return
        if name.value.casefold() not in CONSOLE_CLASSES:
            return
        owner = wt.DWORD()
        thread = self.window_thread(hwnd, ctypes.byref(owner))
        created = self.owner_created_filetime(owner.value) if owner.value else None
        current_owner = wt.DWORD()
        current_thread = self.window_thread(hwnd, ctypes.byref(current_owner))
        if not thread or current_thread != thread or current_owner.value != owner.value:
            owner.value = 0
            created = None
        self.observations.append(window_observation(hwnd, name.value, owner.value, reason, event_tick, created))

    def owner_created_filetime(self, pid):
        process = self.open_process(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not process:
            return None
        try:
            created, exited, kernel, user = (wt.FILETIME() for _ in range(4))
            if not self.process_times(process, ctypes.byref(created), ctypes.byref(exited),
                                      ctypes.byref(kernel), ctypes.byref(user)):
                return None
            return filetime_text(created)
        finally:
            self.close_handle(process)

    def watch_process_exit(self, pid, created, image, duration, armed_notice):
        process_id(pid)
        filetime(created)
        expected_image = image_path(image)
        process = self.open_process(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, False, pid)
        if not process:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            name = ctypes.create_unicode_buffer(32768)
            size = wt.DWORD(len(name))
            if not self.handle_image(process, 0, name, ctypes.byref(size)):
                raise ctypes.WinError(ctypes.get_last_error())
            times = [wt.FILETIME() for _ in range(4)]
            pointers = [ctypes.byref(value) for value in times]
            if not self.process_times(process, *pointers):
                raise ctypes.WinError(ctypes.get_last_error())
            observed = {
                'pid': self.handle_pid(process), 'created_filetime': filetime_text(times[0]),
                'image': name.value,
            }
            if (observed['pid'] != pid or observed['created_filetime'] != created
                    or image_path(observed['image']) != expected_image):
                raise RuntimeError('Process handle does not match the expected identity')
            armed = utc_filetime()
            initial = self.wait_process(process, 0)
            if initial == WAIT_FAILED:
                raise ctypes.WinError(ctypes.get_last_error())
            if initial != WAIT_TIMEOUT:
                raise RuntimeError('Expected process was not live at arming')
            armed_notice()
            result = self.wait_process(process, duration * 1000)
            if result == WAIT_FAILED:
                raise ctypes.WinError(ctypes.get_last_error())
            if result != WAIT_OBJECT_0:
                raise RuntimeError('Process handle did not signal within the observation interval')
            if not self.process_times(process, *pointers):
                raise ctypes.WinError(ctypes.get_last_error())
            if filetime_text(times[0]) != observed['created_filetime']:
                raise RuntimeError('Process handle creation time changed')
            return verify_process_exit({
                'format': 'prismestra-windows-process-exit-v1', 'status': 'confirmed',
                'expected': {'pid': pid, 'created_filetime': created, 'image': image},
                'observed': observed, 'armed_filetime': armed,
                'exit_filetime': filetime_text(times[1]), 'confirmed_filetime': utc_filetime(),
            })
        finally:
            self.close_handle(process)

    def on_show(self, _hook, _event, hwnd, object_id, child_id, _thread_id, event_tick):
        try:
            if hwnd and object_id == OBJID_WINDOW and child_id == CHILDID_SELF:
                self.observe(hwnd, 'show', event_tick)
        except (OSError, TypeError, ValueError) as error:
            self.gaps.append(f'Could not inspect a show notification: {error}')

    def on_enumerated(self, hwnd, _param):
        if self.is_visible(hwnd):
            self.observe(hwnd, self.snapshot)
        return True

    def enumerate_boundary(self, reason):
        self.snapshot = reason
        if not self.enum_windows(self.enum_callback, 0):
            raise ctypes.WinError(ctypes.get_last_error())

    def drain_messages(self, message):
        # Bound each drain so continuous guest input cannot extend the window indefinitely.
        for _ in range(256):
            if not self.peek(ctypes.byref(message), None, 0, 0, PM_REMOVE):
                return True
            if message.message == WM_QUIT:
                raise RuntimeError('The desktop message loop quit during capture')
            self.translate(ctypes.byref(message))
            self.dispatch(ctypes.byref(message))
        return False

    def capture(self, duration):
        desktop = self.describe_desktop()
        handle = self.hook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, self.event_callback, 0, 0,
                           WINEVENT_OUTOFCONTEXT)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            started = utc_filetime()
            self.enumerate_boundary('initial')
            print(f'Window capture ready on desktop {desktop["name"]} for {duration} seconds.', flush=True)
            deadline = time.monotonic() + duration
            message = wt.MSG()
            while time.monotonic() < deadline:
                self.drain_messages(message)
                remaining = max(0, min(200, int((deadline - time.monotonic()) * 1000)))
                if self.wait(0, None, remaining, QS_ALLINPUT, MWMO_INPUTAVAILABLE) == WAIT_FAILED:
                    raise ctypes.WinError(ctypes.get_last_error())
            if not self.drain_messages(message):
                self.gaps.append('The desktop was still delivering notifications at the capture boundary')
            self.enumerate_boundary('final')
            ended = utc_filetime()
        finally:
            if not self.unhook(handle):
                raise ctypes.WinError(ctypes.get_last_error())
        return {
            'format': 'prismestra-windows-console-windows-v1', 'desktop': desktop,
            'started_filetime': started, 'ended_filetime': ended,
            'observations': self.observations, 'window_capture_gaps': self.gaps,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--disposable-guest', action='store_true', required=True)
    parser.add_argument('--duration-seconds', default=1000, type=int)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--expect-pid', type=int)
    parser.add_argument('--expect-created-filetime')
    parser.add_argument('--expect-image')
    args = parser.parse_args()
    if os.name != 'nt' or not 1 <= args.duration_seconds <= 7200:
        parser.error('Use Windows and a capture duration between 1 and 7200 seconds')
    if args.output.exists() or not args.output.parent.is_dir():
        parser.error('Use a new output file in an existing directory')
    expected = (args.expect_pid, args.expect_created_filetime, args.expect_image)
    if any(value is not None for value in expected) and not all(value is not None for value in expected):
        parser.error('Supply all three expected process identity fields')
    try:
        observer = WindowCapture()
        if args.expect_pid is None:
            result = observer.capture(args.duration_seconds)
        else:
            result = observer.watch_process_exit(
                *expected, args.duration_seconds,
                lambda: print(f'Process exit observer armed for PID {args.expect_pid}.', flush=True),
            )
        with args.output.open('x', encoding='utf-8') as stream:
            json.dump(result, stream, indent=2)
            stream.write('\n')
    except (OSError, RuntimeError, ValueError) as error:
        parser.exit(2, f'Observation failed: {error}\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
