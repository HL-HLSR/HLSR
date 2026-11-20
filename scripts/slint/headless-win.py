#!/usr/bin/env python3
"""Запустить команду на невидимом рабочем столе Win32 (аналог headless.sh для Windows).

    python scripts/slint/headless-win.py [--timeout СЕК] [--] команда [аргументы…]

Стол `hlsr-headless-<pid>` создаётся в оконной станции пользователя (CreateDesktopW), команда
запускается на нём (STARTUPINFOW.lpDesktop) в объекте задания: её дочерние процессы (GPU и
рендереры Electron, процессы `hlsr` из hlsr-snap) наследуют и стол, и задание.
На столе нет explorer: ни окна, ни панели задач, ни Alt+Tab у пользователя, фокус не
перехватывается; значок в трее не появляется (Shell_NotifyIcon не находит трей с чужого стола).
Ввод ОС (SendInput) туда не попадает — только dispatch_event/sendInputEvent самого процесса.

Потоки ввода-вывода наследуются. Код выхода — код команды; 124 — таймаут, 125 — команда не
запустилась. После выхода команды оставшиеся процессы задания ждутся до HLSR_HEADLESS_GRACE
секунд (по умолчанию 90), затем задание завершается целиком.

Переменные:
  HLSR_HEADLESS_TIMEOUT=1800 — общий таймаут, секунды (или --timeout);
  HLSR_HEADLESS_GRACE=90     — сколько ждать дочерние процессы после выхода команды.
Команде передаются HLSR_HEADLESS_ROOT (временный каталог, удаляется после) и
HLSR_HEADLESS_DESKTOP (имя стола) — по ним скрипты проверяют, что запущены без экрана.
"""

import ctypes
import msvcrt
import os
import shutil
import subprocess
import sys
import tempfile
import time
from ctypes import wintypes

if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")
if sys.platform != "win32":
    sys.exit("headless-win.py: только Windows (на Linux — scripts/slint/headless.sh)")

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32 = ctypes.WinDLL("user32", use_last_error=True)

GENERIC_ALL = 0x10000000
CREATE_SUSPENDED = 0x00000004
CREATE_UNICODE_ENVIRONMENT = 0x00000400
STARTF_USESTDHANDLES = 0x00000100
STARTF_USESHOWWINDOW = 0x00000001
SW_SHOWNORMAL = 1
DUPLICATE_SAME_ACCESS = 0x00000002
INFINITE = 0xFFFFFFFF
WAIT_OBJECT_0 = 0
WAIT_TIMEOUT = 0x102
STD_HANDLES = (-10, -11, -12)
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9
JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION_CLASS = 1


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("lpReserved", wintypes.LPWSTR),
        ("lpDesktop", wintypes.LPWSTR),
        ("lpTitle", wintypes.LPWSTR),
        ("dwX", wintypes.DWORD),
        ("dwY", wintypes.DWORD),
        ("dwXSize", wintypes.DWORD),
        ("dwYSize", wintypes.DWORD),
        ("dwXCountChars", wintypes.DWORD),
        ("dwYCountChars", wintypes.DWORD),
        ("dwFillAttribute", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("wShowWindow", wintypes.WORD),
        ("cbReserved2", wintypes.WORD),
        ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wintypes.HANDLE),
        ("hStdOutput", wintypes.HANDLE),
        ("hStdError", wintypes.HANDLE),
    ]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("hProcess", wintypes.HANDLE),
        ("hThread", wintypes.HANDLE),
        ("dwProcessId", wintypes.DWORD),
        ("dwThreadId", wintypes.DWORD),
    ]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(name, ctypes.c_ulonglong) for name in (
        "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
        "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]


class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", ctypes.c_longlong),
        ("PerJobUserTimeLimit", ctypes.c_longlong),
        ("LimitFlags", wintypes.DWORD),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", wintypes.DWORD),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", wintypes.DWORD),
        ("SchedulingClass", wintypes.DWORD),
    ]


class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
        ("IoInfo", IO_COUNTERS),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


class JOBOBJECT_BASIC_ACCOUNTING_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("TotalUserTime", ctypes.c_longlong),
        ("TotalKernelTime", ctypes.c_longlong),
        ("ThisPeriodTotalUserTime", ctypes.c_longlong),
        ("ThisPeriodTotalKernelTime", ctypes.c_longlong),
        ("TotalPageFaultCount", wintypes.DWORD),
        ("TotalProcesses", wintypes.DWORD),
        ("ActiveProcesses", wintypes.DWORD),
        ("TotalTerminatedProcesses", wintypes.DWORD),
    ]


user32.CreateDesktopW.restype = wintypes.HANDLE
user32.CreateDesktopW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p,
                                  wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
user32.CloseDesktop.argtypes = [wintypes.HANDLE]
kernel32.CreateJobObjectW.restype = wintypes.HANDLE
kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
kernel32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
kernel32.QueryInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                               wintypes.DWORD, ctypes.c_void_p]
kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
kernel32.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
kernel32.CreateProcessW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p,
                                    wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR,
                                    ctypes.POINTER(STARTUPINFOW), ctypes.POINTER(PROCESS_INFORMATION)]
kernel32.ResumeThread.argtypes = [wintypes.HANDLE]
kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel32.WaitForSingleObject.restype = wintypes.DWORD
kernel32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
kernel32.GetStdHandle.restype = wintypes.HANDLE
kernel32.GetStdHandle.argtypes = [wintypes.DWORD]
kernel32.GetCurrentProcess.restype = wintypes.HANDLE
kernel32.DuplicateHandle.argtypes = [wintypes.HANDLE, wintypes.HANDLE, wintypes.HANDLE,
                                     ctypes.POINTER(wintypes.HANDLE), wintypes.DWORD, wintypes.BOOL,
                                     wintypes.DWORD]
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]


def fail(message, code=125):
    print(f"headless-win.py: {message}", file=sys.stderr)
    sys.exit(code)


def check(ok, what):
    if not ok:
        fail(f"{what}: {ctypes.WinError(ctypes.get_last_error())}")


def inheritable_std_handles():
    """stdin — NUL (на невидимом столе отвечать некому), stdout/stderr — копии своих."""
    me = kernel32.GetCurrentProcess()
    null = os.open("NUL", os.O_RDONLY)
    copies = []
    for std in STD_HANDLES:
        source = msvcrt.get_osfhandle(null) if std == -10 else kernel32.GetStdHandle(wintypes.DWORD(std & 0xFFFFFFFF))
        copy = wintypes.HANDLE()
        if source and source != wintypes.HANDLE(-1).value and kernel32.DuplicateHandle(
                me, source, me, ctypes.byref(copy), 0, True, DUPLICATE_SAME_ACCESS):
            copies.append(copy.value)
        else:
            copies.append(None)
    os.close(null)
    return copies


def resolve(command):
    """Исполняемый файл и командная строка; .cmd/.bat — через cmd.exe /c."""
    exe = command[0]
    found = shutil.which(exe)
    if found is None:
        fail(f"не найдено: {exe}", 127)
    if found.lower().endswith((".cmd", ".bat")):
        comspec = os.environ.get("COMSPEC", r"C:\Windows\System32\cmd.exe")
        return comspec, subprocess.list2cmdline([comspec, "/d", "/c", found, *command[1:]])
    return found, subprocess.list2cmdline([found, *command[1:]])


def environment_block(env):
    text = "".join(f"{k}={v}\0" for k, v in sorted(env.items(), key=lambda kv: kv[0].upper())) + "\0"
    return ctypes.create_unicode_buffer(text, len(text))


def active_processes(job):
    info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION()
    if not kernel32.QueryInformationJobObject(job, JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION_CLASS,
                                              ctypes.byref(info), ctypes.sizeof(info), None):
        return 0
    return info.ActiveProcesses


def main(argv):
    timeout = float(os.environ.get("HLSR_HEADLESS_TIMEOUT", "1800"))
    grace = float(os.environ.get("HLSR_HEADLESS_GRACE", "90"))
    args = list(argv)
    while args and args[0].startswith("--"):
        flag = args.pop(0)
        if flag == "--":
            break
        if flag == "--timeout" and args:
            timeout = float(args.pop(0))
        else:
            fail(f"неизвестный ключ {flag}", 2)
    if not args:
        fail("нужна команда", 2)

    exe, command_line = resolve(args)
    name = f"hlsr-headless-{os.getpid()}"
    root = tempfile.mkdtemp(prefix="hlsr-headless-")
    env = dict(os.environ)
    env["HLSR_HEADLESS_ROOT"] = root
    env["HLSR_HEADLESS_DESKTOP"] = name

    desktop = user32.CreateDesktopW(name, None, None, 0, GENERIC_ALL, None)
    check(desktop, "CreateDesktopW")
    job = kernel32.CreateJobObjectW(None, None)
    check(job, "CreateJobObjectW")
    limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    check(kernel32.SetInformationJobObject(job, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                                           ctypes.byref(limits), ctypes.sizeof(limits)),
          "SetInformationJobObject")

    handles = inheritable_std_handles()
    startup = STARTUPINFOW()
    startup.cb = ctypes.sizeof(startup)
    startup.lpDesktop = name
    startup.dwFlags = STARTF_USESHOWWINDOW
    startup.wShowWindow = SW_SHOWNORMAL
    if all(handles):
        startup.dwFlags |= STARTF_USESTDHANDLES
        startup.hStdInput, startup.hStdOutput, startup.hStdError = handles
    process = PROCESS_INFORMATION()
    block = environment_block(env)
    buffer = ctypes.create_unicode_buffer(command_line)
    check(kernel32.CreateProcessW(exe, buffer, None, None, True,
                                  CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT, block, os.getcwd(),
                                  ctypes.byref(startup), ctypes.byref(process)),
          f"CreateProcessW {exe}")
    for handle in handles:
        if handle:
            kernel32.CloseHandle(handle)
    if not kernel32.AssignProcessToJobObject(job, process.hProcess):
        error = ctypes.WinError(ctypes.get_last_error())
        kernel32.TerminateProcess(process.hProcess, 125)
        fail(f"AssignProcessToJobObject: {error}")
    kernel32.ResumeThread(process.hThread)
    kernel32.CloseHandle(process.hThread)

    deadline = time.monotonic() + timeout
    status = 124
    try:
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                print(f"headless-win.py: таймаут {timeout:g} с, процессы завершены", file=sys.stderr)
                break
            # Короткими шагами: Ctrl+C в Python приходит между вызовами.
            if kernel32.WaitForSingleObject(process.hProcess, int(min(left, 0.5) * 1000)) == WAIT_OBJECT_0:
                code = wintypes.DWORD()
                kernel32.GetExitCodeProcess(process.hProcess, ctypes.byref(code))
                status = code.value
                break
        # Дочерние процессы доживают своё.
        until = min(deadline, time.monotonic() + grace)
        while status != 124 and active_processes(job) and time.monotonic() < until:
            time.sleep(0.25)
        if active_processes(job):
            print(f"headless-win.py: завершено оставшихся процессов: {active_processes(job)}", file=sys.stderr)
    except KeyboardInterrupt:
        status = 130
    finally:
        kernel32.TerminateJobObject(job, 1)
        kernel32.CloseHandle(process.hProcess)
        kernel32.CloseHandle(job)
        # Стол закрывается, когда на нём не осталось окон и потоков.
        for _ in range(40):
            if user32.CloseDesktop(desktop):
                break
            time.sleep(0.25)
        shutil.rmtree(root, ignore_errors=True)
    if status > 255:
        # NTSTATUS (0xC0000005 и т. п.): в оболочку уходит 1, сам код — в журнал.
        print(f"headless-win.py: код выхода 0x{status:08X}", file=sys.stderr)
        return 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
