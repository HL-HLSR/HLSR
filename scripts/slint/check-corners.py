#!/usr/bin/env python3
"""Углы окна лаунчера после DWM: прозрачные или чёрные (Windows, только под headless-win.py).

    python scripts/slint/headless-win.py python scripts/slint/check-corners.py --exe target/debug/hlsr.exe
        [--out target/snap/corners] [--wait 5]

Сборка с dev-hooks (HLSR_DATA_DIR, HLSR_LOGS_DIR): временные данные (библиотека с «установленной»
игрой, Discord выключен), своё окно во весь стол цвета маджента (фон стола) и в нём — эскиз DWM окна
лаунчера (DwmRegisterThumbnail, 1:1). Снимок окна-фона — PrintWindow с PW_RENDERFULLCONTENT: DWM
составляет эскиз с альфой окна лаунчера поверх мадженты (`corners.png`). Сам PrintWindow окна
лаунчера альфу не передаёт: прозрачное выходит непрозрачным чёрным (проверено на слоистом окне),
поэтому — через эскиз. Все четыре угла на отступе 1 px должны быть прозрачными.
"""

import argparse
import ctypes
import json
import os
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from ctypes import wintypes

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if sys.platform != "win32":
    sys.exit("check-corners.py: только Windows")

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

# Физические пиксели окон.
user32.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))

WNDPROC = ctypes.WINFUNCTYPE(ctypes.c_ssize_t, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)
user32.DefWindowProcW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.DefWindowProcW.restype = ctypes.c_ssize_t
user32.CreateWindowExW.restype = wintypes.HWND
user32.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_int,
                                   ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.HWND, wintypes.HMENU,
                                   wintypes.HINSTANCE, wintypes.LPVOID]
user32.PrintWindow.argtypes = [wintypes.HWND, wintypes.HDC, wintypes.UINT]
user32.GetDC.restype = wintypes.HDC
user32.GetDC.argtypes = [wintypes.HWND]
user32.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateSolidBrush.restype = wintypes.HBRUSH
gdi32.CreateDIBSection.restype = wintypes.HBITMAP
gdi32.CreateDIBSection.argtypes = [wintypes.HDC, ctypes.c_void_p, wintypes.UINT, ctypes.POINTER(ctypes.c_void_p),
                                   wintypes.HANDLE, wintypes.DWORD]
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
gdi32.DeleteDC.argtypes = [wintypes.HDC]

dwmapi = ctypes.WinDLL("dwmapi")

PW_RENDERFULLCONTENT = 2
# DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY.
THUMBNAIL_FLAGS = 0x1 | 0x4 | 0x8 | 0x10
MAGENTA_COLORREF = 0x00FF00FF
MAGENTA_BGRA = 0xFFFF00FF


class WNDCLASSW(ctypes.Structure):
    _fields_ = [
        ("style", wintypes.UINT),
        ("lpfnWndProc", WNDPROC),
        ("cbClsExtra", ctypes.c_int),
        ("cbWndExtra", ctypes.c_int),
        ("hInstance", wintypes.HINSTANCE),
        ("hIcon", wintypes.HICON),
        ("hCursor", wintypes.HANDLE),
        ("hbrBackground", wintypes.HBRUSH),
        ("lpszMenuName", wintypes.LPCWSTR),
        ("lpszClassName", wintypes.LPCWSTR),
    ]


class DWM_THUMBNAIL_PROPERTIES(ctypes.Structure):
    _fields_ = [
        ("dwFlags", wintypes.DWORD),
        ("rcDestination", wintypes.RECT),
        ("rcSource", wintypes.RECT),
        ("opacity", ctypes.c_ubyte),
        ("fVisible", wintypes.BOOL),
        ("fSourceClientAreaOnly", wintypes.BOOL),
    ]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wintypes.DWORD),
        ("biWidth", wintypes.LONG),
        ("biHeight", wintypes.LONG),
        ("biPlanes", wintypes.WORD),
        ("biBitCount", wintypes.WORD),
        ("biCompression", wintypes.DWORD),
        ("biSizeImage", wintypes.DWORD),
        ("biXPelsPerMeter", wintypes.LONG),
        ("biYPelsPerMeter", wintypes.LONG),
        ("biClrUsed", wintypes.DWORD),
        ("biClrImportant", wintypes.DWORD),
    ]


def backdrop(ready):
    """Окно во весь стол цвета маджента (фон стола позади лаунчера); цикл сообщений — в этом потоке."""
    proc = WNDPROC(lambda h, m, w, l: user32.DefWindowProcW(h, m, w, l))
    hinst = kernel32.GetModuleHandleW(None)
    wc = WNDCLASSW()
    wc.lpfnWndProc = proc
    wc.hInstance = hinst
    wc.hbrBackground = gdi32.CreateSolidBrush(MAGENTA_COLORREF)
    wc.lpszClassName = "hlsr-check-corners"
    user32.RegisterClassW(ctypes.byref(wc))
    # WS_EX_TOOLWINDOW, WS_POPUP | WS_VISIBLE.
    hwnd = user32.CreateWindowExW(0x80, wc.lpszClassName, "backdrop", 0x80000000 | 0x10000000, 0, 0,
                                  user32.GetSystemMetrics(0), user32.GetSystemMetrics(1), None, None, hinst, None)
    ready.append((hwnd, proc))
    msg = wintypes.MSG()
    while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
        user32.TranslateMessage(ctypes.byref(msg))
        user32.DispatchMessageW(ctypes.byref(msg))


def windows_of(pid):
    found = []
    enum_proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def collect(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        rect = wintypes.RECT()
        if owner.value == pid and user32.IsWindowVisible(hwnd) and user32.GetWindowRect(hwnd, ctypes.byref(rect)):
            if rect.right - rect.left > 300:
                found.append(hwnd)
        return True

    user32.EnumWindows(enum_proc(collect), 0)
    return found


def print_window(hwnd, width, height):
    """PrintWindow(PW_RENDERFULLCONTENT) в 32-битный буфер, залитый маджентой: пиксели BGRA (int)."""
    screen = user32.GetDC(None)
    memdc = gdi32.CreateCompatibleDC(screen)
    user32.ReleaseDC(None, screen)
    info = BITMAPINFOHEADER(biSize=ctypes.sizeof(BITMAPINFOHEADER), biWidth=width, biHeight=-height, biPlanes=1,
                            biBitCount=32)
    bits = ctypes.c_void_p()
    bitmap = gdi32.CreateDIBSection(memdc, ctypes.byref(info), 0, ctypes.byref(bits), None, 0)
    old = gdi32.SelectObject(memdc, bitmap)
    buffer = (ctypes.c_uint32 * (width * height)).from_address(bits.value)
    for i in range(width * height):
        buffer[i] = MAGENTA_BGRA
    ok = user32.PrintWindow(hwnd, memdc, PW_RENDERFULLCONTENT)
    pixels = list(buffer)
    gdi32.SelectObject(memdc, old)
    gdi32.DeleteObject(bitmap)
    gdi32.DeleteDC(memdc)
    return ok, pixels


def write_png(path, width, height, pixels):
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for v in pixels[y * width:(y + 1) * width]:
            raw += bytes(((v >> 16) & 255, (v >> 8) & 255, v & 255, (v >> 24) & 255))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b""))


def classify(v):
    a, r, g, b = (v >> 24) & 255, (v >> 16) & 255, (v >> 8) & 255, v & 255
    if a == 0 or (r, g, b) == (255, 0, 255):
        return "прозрачный"
    if max(r, g, b) <= 16:
        return "чёрный"
    return f"#{r:02x}{g:02x}{b:02x}"


def prepare(root):
    data = os.path.join(root, "data")
    library = os.path.join(root, "library")
    os.makedirs(os.path.join(library, "Games", "hl1-won"))
    os.makedirs(data)
    with open(os.path.join(library, "Games", "hl1-won", "installed.json"), "w") as f:
        f.write('{"state":"ready"}')
    with open(os.path.join(data, "library.json"), "w") as f:
        json.dump({"libraryPath": library}, f)
    # Discord: перенос config.rpc уже сделан, клиент пользователя не трогать.
    with open(os.path.join(data, "discord-rpc.json"), "w") as f:
        f.write('{"schemaVersion":2,"enabled":false,"source":"user","migrated":true,"legacyValue":null}')
    return data, os.path.join(root, "logs")


def composed(host, hwnd, width, height):
    """Окно `hwnd`, составленное DWM поверх окна-фона `host` (эскиз 1:1 в его левом верхнем углу)."""
    thumb = ctypes.c_void_p()
    hr = dwmapi.DwmRegisterThumbnail(wintypes.HWND(host), wintypes.HWND(hwnd), ctypes.byref(thumb))
    if hr != 0:
        return False, []
    try:
        props = DWM_THUMBNAIL_PROPERTIES(dwFlags=THUMBNAIL_FLAGS, rcDestination=wintypes.RECT(0, 0, width, height),
                                         opacity=255, fVisible=True, fSourceClientAreaOnly=False)
        if dwmapi.DwmUpdateThumbnailProperties(thumb, ctypes.byref(props)) != 0:
            return False, []
        time.sleep(0.5)
        return print_window(host, width, height)
    finally:
        dwmapi.DwmUnregisterThumbnail(thumb)


def check(exe, out, wait, host):
    root = tempfile.mkdtemp(prefix="corners-", dir=os.environ.get("HLSR_HEADLESS_ROOT"))
    data, logs = prepare(root)
    env = dict(os.environ, HLSR_DATA_DIR=data, HLSR_LOGS_DIR=logs)
    proc = subprocess.Popen([exe], env=env)
    try:
        hwnd = None
        deadline = time.time() + 40
        while hwnd is None and time.time() < deadline:
            found = windows_of(proc.pid)
            hwnd = found[0] if found else None
            if hwnd is None:
                time.sleep(0.2)
        if hwnd is None:
            raise RuntimeError("нет окна")
        time.sleep(wait)
        rect = wintypes.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        width, height = rect.right - rect.left, rect.bottom - rect.top
        ok, pixels = composed(host, hwnd, width, height)
        if not ok:
            raise RuntimeError("эскиз DWM или PrintWindow не удался")
        write_png(os.path.join(out, "corners.png"), width, height, pixels)
        center = classify(pixels[(height // 2) * width + width // 2])
        corners = [classify(pixels[y * width + x]) for x, y in
                   ((1, 1), (width - 2, 1), (1, height - 2), (width - 2, height - 2))]
        if center in ("чёрный", "прозрачный"):
            raise RuntimeError(f"не снимается (середина окна {width}x{height} — {center})")
        return corners
    finally:
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--out", default=os.path.join("target", "snap", "corners"))
    ap.add_argument("--wait", type=float, default=5.0)
    a = ap.parse_args()
    if not os.environ.get("HLSR_HEADLESS_DESKTOP"):
        sys.exit("check-corners.py: запускать через scripts/slint/headless-win.py — окна не на экран")
    os.makedirs(a.out, exist_ok=True)
    ready = []
    threading.Thread(target=backdrop, args=(ready,), daemon=True).start()
    while not ready:
        time.sleep(0.05)
    host = ready[0][0]
    try:
        corners = check(os.path.abspath(a.exe), a.out, a.wait, host)
    except RuntimeError as error:
        print(f"углы: {error}", flush=True)
        return 1
    print(f"углы: {', '.join(corners)}", flush=True)
    return 0 if all(corner == "прозрачный" for corner in corners) else 1


if __name__ == "__main__":
    sys.exit(main())
