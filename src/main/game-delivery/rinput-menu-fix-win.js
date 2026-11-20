"use strict";

// Windows: патч курсора RInput в меню (см. rinput-menu-fix.js).
//
// Node не умеет читать память чужого процесса, а hlsr-native такой функции
// не даёт. Поэтому на время патча поднимается один процесс PowerShell с
// небольшой обёрткой над ReadProcessMemory/WriteProcessMemory и отвечает на
// команды построчно: так компиляция обёртки идёт один раз, а не на каждый
// опрос. hl.exe 32-битный, поэтому модули ищем снимком TH32CS_SNAPMODULE32.

const { spawn } = require("child_process");
const core = require("./rinput-menu-fix");
const { powershellPath } = require("../windows-tools");

const HELPER_SOURCE = String.raw`
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace HlsrRInput {
  public class Session {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct ModuleEntry {
      public uint dwSize; public uint th32ModuleID; public uint th32ProcessID;
      public uint GlblcntUsage; public uint ProccntUsage;
      public IntPtr modBaseAddr; public uint modBaseSize; public IntPtr hModule;
      [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szModule;
      [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szExePath;
    }

    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool ReadProcessMemory(IntPtr process, IntPtr address, byte[] buffer, IntPtr size, out IntPtr done);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool WriteProcessMemory(IntPtr process, IntPtr address, byte[] buffer, IntPtr size, out IntPtr done);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool VirtualProtectEx(IntPtr process, IntPtr address, IntPtr size, uint protect, out uint old);
    [DllImport("kernel32.dll")] static extern bool FlushInstructionCache(IntPtr process, IntPtr address, IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags, int pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool Module32FirstW(IntPtr snapshot, ref ModuleEntry entry);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool Module32NextW(IntPtr snapshot, ref ModuleEntry entry);

    // PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_QUERY_INFORMATION
    const uint ACCESS = 0x0438;
    const uint SNAP_MODULES = 0x18;
    const uint PAGE_EXECUTE_READWRITE = 0x40;

    IntPtr handle = IntPtr.Zero;
    int pid;

    public string Handle(string line) {
      try {
        string[] p = line.Trim().Split(' ');
        switch (p[0]) {
          case "pids": return Pids(p[1]);
          case "open": return Open(int.Parse(p[1]));
          case "module": return Module(p[1]);
          case "read": return Read(long.Parse(p[1]), int.Parse(p[2]));
          case "write": return Write(long.Parse(p[1]), p[2]);
          case "close": Close(); return "{\"ok\":true}";
        }
        return Fail("unknown command");
      } catch (Exception e) {
        return Fail(e.Message);
      }
    }

    static string Fail(string message) {
      string text = (message ?? "").Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\r", " ").Replace("\n", " ");
      return "{\"ok\":false,\"error\":\"" + text + "\"}";
    }

    string Pids(string name) {
      StringBuilder list = new StringBuilder();
      foreach (Process process in Process.GetProcessesByName(name)) {
        if (list.Length > 0) list.Append(',');
        list.Append(process.Id);
      }
      return "{\"ok\":true,\"pids\":[" + list + "]}";
    }

    string Open(int id) {
      Close();
      handle = OpenProcess(ACCESS, false, id);
      if (handle == IntPtr.Zero) return Fail("OpenProcess " + Marshal.GetLastWin32Error());
      pid = id;
      return "{\"ok\":true}";
    }

    void Close() {
      if (handle != IntPtr.Zero) CloseHandle(handle);
      handle = IntPtr.Zero;
    }

    // ERROR_BAD_LENGTH: модули процесса как раз меняются, снимок повторяем.
    string Module(string name) {
      for (int attempt = 0; attempt < 5; attempt++) {
        IntPtr snapshot = CreateToolhelp32Snapshot(SNAP_MODULES, pid);
        if (snapshot == new IntPtr(-1)) {
          int error = Marshal.GetLastWin32Error();
          if (error == 24) continue;
          return Fail("snapshot " + error);
        }
        try {
          ModuleEntry entry = new ModuleEntry();
          entry.dwSize = (uint)Marshal.SizeOf(typeof(ModuleEntry));
          for (bool ok = Module32FirstW(snapshot, ref entry); ok; ok = Module32NextW(snapshot, ref entry)) {
            long address = entry.modBaseAddr.ToInt64();
            if (address > 0xffffffffL) continue;
            if (string.Equals(entry.szModule, name, StringComparison.OrdinalIgnoreCase))
              return "{\"ok\":true,\"base\":" + address + "}";
          }
        } finally {
          CloseHandle(snapshot);
        }
        return "{\"ok\":true,\"base\":null}";
      }
      return "{\"ok\":true,\"base\":null}";
    }

    string Read(long address, int length) {
      byte[] buffer = new byte[length];
      IntPtr done;
      if (!ReadProcessMemory(handle, new IntPtr(address), buffer, new IntPtr(length), out done))
        return Fail("read " + Marshal.GetLastWin32Error());
      return "{\"ok\":true,\"hex\":\"" + BitConverter.ToString(buffer).Replace("-", "") + "\"}";
    }

    string Write(long address, string hex) {
      byte[] buffer = new byte[hex.Length / 2];
      for (int i = 0; i < buffer.Length; i++) buffer[i] = Convert.ToByte(hex.Substring(i * 2, 2), 16);
      IntPtr at = new IntPtr(address);
      IntPtr size = new IntPtr(buffer.Length);
      uint old;
      bool unprotected = VirtualProtectEx(handle, at, size, PAGE_EXECUTE_READWRITE, out old);
      IntPtr done;
      bool ok = WriteProcessMemory(handle, at, buffer, size, out done);
      int error = Marshal.GetLastWin32Error();
      if (unprotected) {
        uint ignored;
        VirtualProtectEx(handle, at, size, old, out ignored);
      }
      FlushInstructionCache(handle, at, size);
      return ok ? "{\"ok\":true}" : Fail("write " + error);
    }
  }
}
`;

const HELPER_SCRIPT = [
  "$ErrorActionPreference = 'Stop'",
  `Add-Type -TypeDefinition @'\n${HELPER_SOURCE}\n'@`,
  "$session = New-Object HlsrRInput.Session",
  "[Console]::Out.WriteLine('{\"ok\":true,\"ready\":true}')",
  "while ($null -ne ($line = [Console]::In.ReadLine())) {",
  "  [Console]::Out.WriteLine($session.Handle($line))",
  "}",
].join("\n");

const REQUEST_TIMEOUT_MS = 30000;

// Один помощник PowerShell; запросы идут строго по очереди.
class MemoryHelper {
  constructor(options = {}) {
    this.spawnFn = options.spawnFn || spawn;
    this.timeoutMs = options.timeoutMs || REQUEST_TIMEOUT_MS;
    // Обёртку с тем же порядком ответов ставит и снимок окна LiveSplit.
    this.script = options.script || HELPER_SCRIPT;
    this.child = null;
    this.buffer = "";
    this.pending = [];
    this.failure = null;
  }

  start() {
    if (this.child) return this.ready;
    const encoded = Buffer.from(this.script, "utf16le").toString("base64");
    this.child = this.spawnFn(
      powershellPath(),
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", encoded],
      { windowsHide: true, shell: false, stdio: ["pipe", "pipe", "pipe"] }
    );
    // Запись в упавший процесс (EPIPE) — это его выход, он обрабатывается ниже;
    // без обработчика ошибка потока уронила бы весь лаунчер.
    this.child.stdin.on("error", () => {});
    this.child.stdout.setEncoding("utf8");
    this.child.stdout.on("data", (chunk) => this.onData(chunk));
    let stderr = "";
    if (this.child.stderr) this.child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-500); });
    const fail = (message) => {
      this.failure = new Error(message);
      for (const waiter of this.pending.splice(0)) waiter.reject(this.failure);
    };
    this.child.once("error", (error) => fail(`PowerShell не запущен: ${error.message}`));
    this.child.once("exit", (code) => fail(`PowerShell завершился (код ${code})${stderr ? `: ${stderr.trim()}` : ""}`));
    this.ready = this.wait();
    return this.ready;
  }

  onData(chunk) {
    this.buffer += chunk;
    let index;
    while ((index = this.buffer.indexOf("\n")) !== -1) {
      const line = this.buffer.slice(0, index).trim();
      this.buffer = this.buffer.slice(index + 1);
      if (!line) continue;
      const waiter = this.pending.shift();
      if (!waiter) continue;
      let reply;
      try { reply = JSON.parse(line); } catch (error) { waiter.reject(new Error(`ответ помощника не разобран: ${line.slice(0, 200)}`)); continue; }
      if (reply.ok) waiter.resolve(reply);
      else waiter.reject(new Error(reply.error || "ошибка помощника"));
    }
  }

  wait() {
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("помощник PowerShell не ответил")), this.timeoutMs);
      this.pending.push({
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
    });
  }

  async request(line) {
    await this.start();
    const reply = this.wait();
    this.child.stdin.write(`${line}\n`);
    return reply;
  }

  close() {
    if (!this.child) return;
    try { this.child.stdin.end(); } catch (error) { /* уже закрыт */ }
    try { this.child.kill(); } catch (error) { /* уже завершился */ }
    this.child = null;
  }
}

// Память одного процесса поверх помощника.
class HelperMemory {
  constructor(helper) { this.helper = helper; }
  async moduleBase(name) { return (await this.helper.request(`module ${name}`)).base; }
  async read(address, length) {
    return Buffer.from((await this.helper.request(`read ${address} ${length}`)).hex, "hex");
  }
  async write(address, buf) { await this.helper.request(`write ${address} ${buf.toString("hex")}`); }
}

// Сессия держит помощник между опросами, пока RInput ставит перехваты.
function openSession(options = {}) {
  const helper = options.helper || new MemoryHelper(options);
  const memory = new HelperMemory(helper);
  return {
    async patch(processName) {
      const name = String(processName).replace(/\.exe$/i, "");
      const { pids } = await helper.request(`pids ${name}`);
      let result = "waiting";
      for (const pid of pids || []) {
        await helper.request(`open ${pid}`);
        try {
          result = core.mergeStatus(result, await core.patchMemory(memory));
        } finally {
          await helper.request("close");
        }
      }
      return result;
    },
    close() { helper.close(); },
  };
}

module.exports = { openSession, MemoryHelper, HelperMemory, HELPER_SCRIPT };
