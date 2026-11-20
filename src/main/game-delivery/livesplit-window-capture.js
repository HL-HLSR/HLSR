"use strict";

// Windows: снимок окна LiveSplit для предпросмотра оверлея, без своего exe.
//
// Раньше окно снимал hlsr-capture.exe: маленький неподписанный файл, который
// перебирает процессы и снимает чужое окно. Антивирусы блокировали его уже в
// папке распаковки, и установка останавливалась (UNKNOWN при открытии файла).
// Теперь в сборке такого файла нет: тот же снимок делает обёртка над WinAPI в
// PowerShell, как у патча RInput. Если её заблокируют, пропадёт только живой
// предпросмотр, а не установка.
//
// Редактор просит снимок раз в две секунды, а компиляция обёртки занимает
// заметное время, поэтому процесс один и живёт, пока снимки нужны. Снимок и
// его коды те же, что были у hlsr-capture.exe: "HLSRLS1\0", ширина, высота
// (uint32 LE), затем строки BGRX сверху вниз; 0 — готово, 2 — окна нет
// (LiveSplit закрыт или свёрнут), 3 — снимок не получился.
const { MemoryHelper } = require("./rinput-menu-fix-win");

const HELPER_SOURCE = String.raw`
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace HlsrLiveSplit {
  public class WindowCapture {
    [StructLayout(LayoutKind.Sequential)]
    struct Rect { public int Left; public int Top; public int Right; public int Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    struct BitmapHeader {
      public uint Size; public int Width; public int Height; public ushort Planes; public ushort BitCount;
      public uint Compression; public uint SizeImage; public int XPelsPerMeter; public int YPelsPerMeter;
      public uint ClrUsed; public uint ClrImportant;
    }

    delegate bool WindowVisitor(IntPtr window, IntPtr param);

    [DllImport("user32.dll")] static extern bool EnumWindows(WindowVisitor visitor, IntPtr param);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr window, uint command);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out int pid);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr window);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr window, IntPtr dc);
    [DllImport("user32.dll")] static extern IntPtr GetWindowDpiAwarenessContext(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr item);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr item);
    [DllImport("gdi32.dll")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BitmapHeader header, uint usage, out IntPtr bits, IntPtr section, uint offset);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr target, int x, int y, int width, int height, IntPtr source, int sx, int sy, uint operation);

    const uint GW_OWNER = 4;
    const uint PW_CLIENTONLY_RENDERFULLCONTENT = 3;
    const uint SRCCOPY = 0x00CC0020;
    // SENTINEL и MAX_SIDE в scripts/livesplit-overlay/common.h: что осталось этим цветом, окно не нарисовало.
    const int SENTINEL = 0x5A00FF5A;
    const int MAX_SIDE = 2048;

    public string Handle(string line) {
      try {
        string[] parts = line.Trim().Split(' ');
        if (parts[0] == "capture") return Capture(Encoding.UTF8.GetString(Convert.FromBase64String(parts[1])));
        return Fail("unknown command");
      } catch (Exception e) {
        return Fail(e.Message);
      }
    }

    static string Clean(string message) {
      return (message ?? "").Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\r", " ").Replace("\n", " ").Replace("\t", " ");
    }

    static string Fail(string message) { return "{\"ok\":false,\"error\":\"" + Clean(message) + "\"}"; }

    static string Done(int code, string error) {
      return "{\"ok\":true,\"code\":" + code + (error == null ? "" : ",\"error\":\"" + Clean(error) + "\"") + "}";
    }

    // Главное окно LiveSplit: самое большое видимое окно верхнего уровня без
    // владельца у процесса LiveSplit.exe (окна настроек принадлежат главному).
    static IntPtr FindLiveSplit() {
      Process[] processes = Process.GetProcessesByName("LiveSplit");
      if (processes.Length == 0) return IntPtr.Zero;
      int[] pids = new int[processes.Length];
      for (int i = 0; i < processes.Length; i++) pids[i] = processes[i].Id;
      IntPtr best = IntPtr.Zero;
      long bestArea = 0;
      WindowVisitor visitor = delegate(IntPtr window, IntPtr param) {
        if (!IsWindowVisible(window) || GetWindow(window, GW_OWNER) != IntPtr.Zero || IsIconic(window)) return true;
        int pid;
        GetWindowThreadProcessId(window, out pid);
        if (Array.IndexOf(pids, pid) < 0) return true;
        Rect rect;
        if (!GetClientRect(window, out rect)) return true;
        long area = (long)rect.Right * rect.Bottom;
        if (area > bestArea) { best = window; bestArea = area; }
        return true;
      };
      EnumWindows(visitor, IntPtr.Zero);
      GC.KeepAlive(visitor);
      return best;
    }

    // Размер окна снимается в DPI-контексте самого LiveSplit, иначе при масштабе
    // Windows больше 100% картинка занимает только часть холста. Функции есть с
    // Windows 10 1607.
    static IntPtr EnterDpi(IntPtr window) {
      try {
        IntPtr context = GetWindowDpiAwarenessContext(window);
        return context == IntPtr.Zero ? IntPtr.Zero : SetThreadDpiAwarenessContext(context);
      } catch (EntryPointNotFoundException) {
        return IntPtr.Zero;
      }
    }

    static void LeaveDpi(IntPtr previous) {
      if (previous == IntPtr.Zero) return;
      try { SetThreadDpiAwarenessContext(previous); } catch (EntryPointNotFoundException) { }
    }

    // PrintWindow не тронул картинку.
    static bool Untouched(IntPtr bits, int total) {
      for (int i = 0; i < 64; i++)
        if (Marshal.ReadInt32(bits, (total / 64 * i) * 4) != SENTINEL) return false;
      return true;
    }

    static string Capture(string file) {
      IntPtr window = FindLiveSplit();
      if (window == IntPtr.Zero) return Done(2, null);
      IntPtr previous = EnterDpi(window);
      try {
        return CaptureClient(window, file);
      } finally {
        LeaveDpi(previous);
      }
    }

    // PrintWindow видит окно даже под полноэкранной игрой; если он ничего не
    // нарисовал, берётся BitBlt из DC окна.
    static string CaptureClient(IntPtr window, string file) {
      Rect rect;
      if (!GetClientRect(window, out rect) || rect.Right < 16 || rect.Bottom < 16) return Done(3, "окно LiveSplit слишком маленькое");
      int width = Math.Min(rect.Right, MAX_SIDE), height = Math.Min(rect.Bottom, MAX_SIDE);
      BitmapHeader header = new BitmapHeader();
      header.Size = (uint)Marshal.SizeOf(typeof(BitmapHeader));
      header.Width = width;
      header.Height = -height;
      header.Planes = 1;
      header.BitCount = 32;
      IntPtr bits;
      IntPtr dc = CreateCompatibleDC(IntPtr.Zero);
      IntPtr bitmap = CreateDIBSection(dc, ref header, 0, out bits, IntPtr.Zero, 0);
      try {
        if (dc == IntPtr.Zero || bitmap == IntPtr.Zero || bits == IntPtr.Zero) return Done(3, "холст не создан");
        SelectObject(dc, bitmap);
        int total = width * height;
        int[] fill = new int[total];
        for (int i = 0; i < total; i++) fill[i] = SENTINEL;
        Marshal.Copy(fill, 0, bits, total);
        if (!PrintWindow(window, dc, PW_CLIENTONLY_RENDERFULLCONTENT) || Untouched(bits, total)) {
          IntPtr source = GetDC(window);
          if (source == IntPtr.Zero) return Done(3, "нет DC окна");
          bool copied = BitBlt(dc, 0, 0, width, height, source, 0, 0, SRCCOPY);
          ReleaseDC(window, source);
          if (!copied) return Done(3, "BitBlt не удался");
        }
        byte[] pixels = new byte[total * 4];
        Marshal.Copy(bits, pixels, 0, pixels.Length);
        using (FileStream stream = new FileStream(file, FileMode.Create, FileAccess.Write)) {
          byte[] magic = Encoding.ASCII.GetBytes("HLSRLS1\0");
          stream.Write(magic, 0, magic.Length);
          stream.Write(BitConverter.GetBytes(width), 0, 4);
          stream.Write(BitConverter.GetBytes(height), 0, 4);
          stream.Write(pixels, 0, pixels.Length);
        }
        return Done(0, null);
      } finally {
        if (dc != IntPtr.Zero) DeleteDC(dc);
        if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
      }
    }
  }
}
`;

const CAPTURE_SCRIPT = [
  "$ErrorActionPreference = 'Stop'",
  `Add-Type -TypeDefinition @'\n${HELPER_SOURCE}\n'@`,
  "$capture = New-Object HlsrLiveSplit.WindowCapture",
  "[Console]::Out.WriteLine('{\"ok\":true,\"ready\":true}')",
  "while ($null -ne ($line = [Console]::In.ReadLine())) {",
  "  [Console]::Out.WriteLine($capture.Handle($line))",
  "}",
].join("\n");

// Снимки идут каждые две секунды, пока открыт редактор: дольше этого простоя
// процесс не нужен.
const IDLE_MS = 20000;

class LiveSplitWindowCapture {
  constructor(options = {}) {
    this.options = options;
    this.idleMs = options.idleMs || IDLE_MS;
    this.logger = options.logger || { warn() {} };
    this.helper = null;
    this.idleTimer = null;
  }

  // Снимок окна в file; возвращает код. Сбой или молчание PowerShell — ошибка,
  // а следующий снимок запустит новый процесс.
  async capture(file) {
    clearTimeout(this.idleTimer);
    if (!this.helper) this.helper = new MemoryHelper({ ...this.options, script: CAPTURE_SCRIPT });
    const path = Buffer.from(String(file), "utf8").toString("base64");
    let reply;
    try {
      reply = await this.helper.request(`capture ${path}`);
    } catch (error) {
      this.close();
      throw error;
    }
    if (reply.error) this.logger.warn(`[livesplit-overlay] снимок LiveSplit: ${reply.error}`);
    this.idleTimer = setTimeout(() => this.close(), this.idleMs);
    if (this.idleTimer.unref) this.idleTimer.unref();
    return reply.code;
  }

  close() {
    clearTimeout(this.idleTimer);
    this.idleTimer = null;
    if (this.helper) this.helper.close();
    this.helper = null;
  }
}

module.exports = LiveSplitWindowCapture;
module.exports.CAPTURE_SCRIPT = CAPTURE_SCRIPT;
