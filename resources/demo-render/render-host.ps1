# Помощник рендера демо (инструмент «Демо» лаунчера HLSR, src/main/demo-tool/render-host.js).
#
# Node (child_process) не умеет запускать процесс на другом рабочем столе и
# класть его в объект задания, поэтому это делает помощник:
#   - создаёт свой скрытый рабочий стол (CreateDesktopW) — окно игры не
#     появляется на экране игрока;
#   - запускает игру на нём (CreateProcessW, STARTUPINFO.lpDesktop), сразу
#     кладёт её в объект задания с KILL_ON_JOB_CLOSE: если помощник умрёт,
#     Windows закроет игру сама;
#   - глушит звуковые сессии только процессов задания (ISimpleAudioVolume).
#     Живую игру не разглушаем никогда: она закрывается заглушённой, а mute,
#     который Windows запоминает по пути exe (иначе игра игрока потом была бы
#     без звука), снимает короткий беззвучный (+volume 0) перезапуск в конце
#     рендера. Событие stopped сообщает, остался ли mute (pending).
#
# Протокол: строки JSON. stdin — команды {"cmd":"start"|"stop", "reset":true|false};
# stdout — события {"event":"ready"|"started"|"audio"|"exited"|
# "audio-reset"|"stopped"|"error"}. Закрытие stdin (лаунчер вышел или упал) —
# то же, что "stop": закрыть задание, снять запомненный mute, закрыть стол.
# Игра закрывается только TerminateJobObject своего задания.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace HlsrRender {
  [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class MMDeviceEnumeratorCo {}

  [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDeviceEnumerator { int EnumAudioEndpoints(int dataFlow, int stateMask, out IMMDeviceCollection devices); }
  [Guid("0BD7A1BE-7A1A-44DB-8397-CC5392387B5E"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDeviceCollection { int GetCount(out uint count); int Item(uint index, out IMMDevice device); }
  [Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDevice { int Activate(ref Guid iid, int clsCtx, IntPtr activationParams, [MarshalAs(UnmanagedType.IUnknown)] out object iface); }
  [Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IAudioSessionManager2 {
    int GetAudioSessionControl(IntPtr a, int b, out IntPtr c);
    int GetSimpleAudioVolume(IntPtr a, int b, out IntPtr c);
    int GetSessionEnumerator(out IAudioSessionEnumerator e);
  }
  [Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IAudioSessionEnumerator { int GetCount(out int n); int GetSession(int i, out IAudioSessionControl2 s); }
  [Guid("bfb7ff88-7239-4fc9-8fa2-07c950be9c6d"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IAudioSessionControl2 {
    int GetState(out int s); int GetDisplayName(out IntPtr n); int SetDisplayName(string n, ref Guid g);
    int GetIconPath(out IntPtr p); int SetIconPath(string p, ref Guid g); int GetGroupingParam(out Guid g);
    int SetGroupingParam(ref Guid g, ref Guid c); int RegisterAudioSessionNotification(IntPtr n);
    int UnregisterAudioSessionNotification(IntPtr n); int GetSessionIdentifier(out IntPtr s);
    int GetSessionInstanceIdentifier(out IntPtr s); int GetProcessId(out uint pid);
  }
  [Guid("87CE5498-68D6-44E5-9215-6DA47EF883D8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface ISimpleAudioVolume {
    int SetMasterVolume(float v, ref Guid g); int GetMasterVolume(out float v);
    int SetMute(bool m, ref Guid g); int GetMute(out bool m);
  }

  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  struct STARTUPINFO {
    public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
    public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
  [StructLayout(LayoutKind.Sequential)]
  struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
    public long PerProcessUserTimeLimit, PerJobUserTimeLimit; public uint LimitFlags;
    public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize; public uint ActiveProcessLimit;
    public UIntPtr Affinity; public uint PriorityClass, SchedulingClass;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct IO_COUNTERS { public ulong a, b, c, d, e, f; }
  [StructLayout(LayoutKind.Sequential)]
  struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    public JOBOBJECT_BASIC_LIMIT_INFORMATION Basic; public IO_COUNTERS Io;
    public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
  }

  public static class Host {
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr devmode, uint flags, uint access, IntPtr sa);
    [DllImport("user32.dll", SetLastError = true)] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern IntPtr CreateJobObjectW(IntPtr sa, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr job, int infoClass, ref JOBOBJECT_EXTENDED_LIMIT_INFORMATION info, int size);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool QueryInformationJobObject(IntPtr job, int infoClass, IntPtr info, int size, IntPtr returned);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags,
      IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr handle, uint ms);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr process, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr handle);

    const uint GENERIC_ALL = 0x10000000;
    const uint CREATE_SUSPENDED = 0x4;
    // Свои stdin/stdout/stderr игре не достаются: протокол помощника идёт по ним.
    const int STARTF_FLAGS = 0x1 | 0x100;
    // KILL_ON_JOB_CLOSE | DIE_ON_UNHANDLED_EXCEPTION: окна отчёта об ошибке
    // на скрытом столе никто не увидит, процесс просто завершается.
    const uint JOB_LIMITS = 0x2000 | 0x400;

    static readonly object OutLock = new object();
    static readonly Queue<string> Commands = new Queue<string>();
    static volatile bool InputClosed;
    static Guid Context = Guid.NewGuid();
    // Сессии, которые заглушили мы (pid:устройство): снимаем глушение только с них.
    static readonly HashSet<string> MutedByUs = new HashSet<string>();
    static readonly HashSet<string> SeenSessions = new HashSet<string>();
    // Устройства, на которых нашлись сессии перезапуска-сброса.
    static readonly HashSet<uint> ResetSeen = new HashSet<uint>();

    static string Json(string s) {
      var b = new StringBuilder("\"");
      foreach (char c in s ?? "") {
        if (c == '"' || c == '\\') { b.Append('\\').Append(c); }
        else if (c < 0x20 || c > 0x7e) { b.AppendFormat("\\u{0:x4}", (int)c); }
        else b.Append(c);
      }
      return b.Append('"').ToString();
    }

    static void Emit(string body) {
      lock (OutLock) { Console.Out.WriteLine("{" + body + "}"); Console.Out.Flush(); }
    }

    static void Error(string where, int code) {
      Emit("\"event\":\"error\",\"where\":" + Json(where) + ",\"code\":" + code);
    }

    // Простой разбор плоского JSON команды: строки и true/false.
    static Dictionary<string, string> Parse(string line) {
      var d = new Dictionary<string, string>();
      int i = 0;
      Func<string> str = () => {
        var b = new StringBuilder();
        i++;
        while (i < line.Length && line[i] != '"') {
          if (line[i] == '\\' && i + 1 < line.Length) {
            i++;
            char e = line[i];
            if (e == 'n') b.Append('\n'); else if (e == 't') b.Append('\t'); else if (e == 'r') b.Append('\r');
            else if (e == 'u' && i + 4 < line.Length) { b.Append((char)Convert.ToInt32(line.Substring(i + 1, 4), 16)); i += 4; }
            else b.Append(e);
          } else b.Append(line[i]);
          i++;
        }
        i++;
        return b.ToString();
      };
      while (i < line.Length) {
        if (line[i] != '"') { i++; continue; }
        string key = str();
        while (i < line.Length && (line[i] == ' ' || line[i] == ':')) i++;
        if (i >= line.Length) break;
        if (line[i] == '"') d[key] = str();
        else {
          int start = i;
          while (i < line.Length && line[i] != ',' && line[i] != '}') i++;
          d[key] = line.Substring(start, i - start).Trim();
        }
      }
      return d;
    }

    static string Next() {
      lock (Commands) { return Commands.Count > 0 ? Commands.Dequeue() : null; }
    }

    static List<uint> JobPids(IntPtr job) {
      var pids = new List<uint>();
      int size = 8 + 8 * 256;
      IntPtr buffer = Marshal.AllocHGlobal(size);
      try {
        if (QueryInformationJobObject(job, 3, buffer, size, IntPtr.Zero)) {
          int count = Marshal.ReadInt32(buffer, 4);
          for (int k = 0; k < count && k < 256; k++) pids.Add((uint)Marshal.ReadIntPtr(buffer, 8 + k * IntPtr.Size).ToInt64());
        }
      } finally { Marshal.FreeHGlobal(buffer); }
      return pids;
    }

    const int MUTE = 0, UNMUTE_ANY = 1;

    // Все активные устройства вывода: игра открывает сессии не только на
    // устройстве по умолчанию. Системная громкость и чужие сессии не трогаются.
    // MUTE — заглушить и запомнить свои (unmuteAll: заглушённые до нас — тоже
    // свои, это mute прошлой пачки или сбоя); UNMUTE_ANY — снять с любых сессий этих PID (сброс).
    static int Sessions(ICollection<uint> pids, int mode, bool unmuteAll) {
      int found = 0;
      var en = (IMMDeviceEnumerator)(new MMDeviceEnumeratorCo());
      IMMDeviceCollection col; en.EnumAudioEndpoints(0, 1, out col);
      uint n; col.GetCount(out n);
      Guid iid = typeof(IAudioSessionManager2).GUID;
      for (uint d = 0; d < n; d++) {
        IMMDevice dev; col.Item(d, out dev);
        object o; dev.Activate(ref iid, 23, IntPtr.Zero, out o);
        var mgr = (IAudioSessionManager2)o;
        IAudioSessionEnumerator se; mgr.GetSessionEnumerator(out se);
        int c; se.GetCount(out c);
        for (int k = 0; k < c; k++) {
          IAudioSessionControl2 s; se.GetSession(k, out s);
          uint pid; s.GetProcessId(out pid);
          if (pids.Contains(pid)) {
            found++;
            if (mode == UNMUTE_ANY) ResetSeen.Add(d);
            string key = pid + ":" + d;
            var vol = (ISimpleAudioVolume)s;
            bool was; vol.GetMute(out was);
            bool first = SeenSessions.Add(key);
            if (mode == MUTE) {
              if (!was) { vol.SetMute(true, ref Context); MutedByUs.Add(key); }
              else if (first && unmuteAll) MutedByUs.Add(key);
              if (first) Emit("\"event\":\"audio\",\"action\":\"muted\",\"pid\":" + pid + ",\"device\":" + d + ",\"was\":" + (was ? "true" : "false"));
            } else if (was) {
              vol.SetMute(false, ref Context);
              MutedByUs.Remove(key);
              Emit("\"event\":\"audio\",\"action\":\"unmuted\",\"pid\":" + pid + ",\"device\":" + d);
            }
          }
          Marshal.ReleaseComObject(s);
        }
        Marshal.ReleaseComObject(se); Marshal.ReleaseComObject(mgr); Marshal.ReleaseComObject(dev);
      }
      Marshal.ReleaseComObject(col); Marshal.ReleaseComObject(en);
      return found;
    }

    static bool SafeSessions(ICollection<uint> pids, int mode, bool unmuteAll) {
      try { Sessions(pids, mode, unmuteAll); return true; }
      catch (Exception e) { Emit("\"event\":\"error\",\"where\":\"audio\",\"message\":" + Json(e.Message)); return false; }
    }

    static PROCESS_INFORMATION Launch(string exe, string cmd, string cwd, string desktop, IntPtr job, out bool ok) {
      var si = new STARTUPINFO();
      si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
      si.lpDesktop = desktop;
      si.dwFlags = STARTF_FLAGS;
      si.wShowWindow = 1;
      PROCESS_INFORMATION pi;
      ok = CreateProcessW(exe, new StringBuilder(cmd), IntPtr.Zero, IntPtr.Zero, false, CREATE_SUSPENDED, IntPtr.Zero, cwd, ref si, out pi);
      if (!ok) { Error("CreateProcessW", Marshal.GetLastWin32Error()); return pi; }
      if (!AssignProcessToJobObject(job, pi.hProcess)) {
        // Без задания нет гарантии закрыть игру при сбое — не запускаем.
        Error("AssignProcessToJobObject", Marshal.GetLastWin32Error());
        TerminateProcess(pi.hProcess, 1);
        CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
        ok = false;
        return pi;
      }
      ResumeThread(pi.hThread);
      CloseHandle(pi.hThread);
      return pi;
    }

    static IntPtr NewJob() {
      IntPtr job = CreateJobObjectW(IntPtr.Zero, null);
      if (job == IntPtr.Zero) { Error("CreateJobObjectW", Marshal.GetLastWin32Error()); return job; }
      var limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
      limits.Basic.LimitFlags = JOB_LIMITS;
      if (!SetInformationJobObject(job, 9, ref limits, Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION)))) {
        Error("SetInformationJobObject", Marshal.GetLastWin32Error());
        CloseHandle(job);
        return IntPtr.Zero;
      }
      return job;
    }

    // Игра закрыта заглушённой: Windows запомнил mute для этого exe. Короткий
    // запуск той же игры (в самой игре звука нет, +volume 0) на том же столе:
    // дождаться её сессий на всех устройствах, где глушили (или ещё 1,5 с после
    // первой), снять глушение, закрыть. devices == null (resetOnly: какие устройства
    // глушили, уже не известно) — годится любая сессия. Возвращает true, если сброс не подтверждён.
    static bool ResetMute(string exe, string cmd, string cwd, string desktop, HashSet<uint> devices) {
      ResetSeen.Clear();
      IntPtr job = NewJob();
      if (job == IntPtr.Zero) { Emit("\"event\":\"audio-reset\",\"ok\":false"); return true; }
      bool ok;
      var pi = Launch(exe, cmd, cwd, desktop, job, out ok);
      if (ok) {
        var pids = new List<uint>();
        pids.Add((uint)pi.dwProcessId);
        var deadline = DateTime.UtcNow.AddSeconds(30);
        DateTime firstSeen = DateTime.MaxValue;
        while (DateTime.UtcNow < deadline && WaitForSingleObject(pi.hProcess, 0) != 0) {
          try {
            if (Sessions(pids, UNMUTE_ANY, false) > 0 && firstSeen == DateTime.MaxValue) firstSeen = DateTime.UtcNow;
          } catch (Exception) { }
          if ((devices != null && ResetSeen.IsSupersetOf(devices)) || (firstSeen != DateTime.MaxValue && DateTime.UtcNow > firstSeen.AddSeconds(1.5))) break;
          Thread.Sleep(100);
        }
        Thread.Sleep(300);
        TerminateJobObject(job, 1);
        WaitForSingleObject(pi.hProcess, 5000);
        CloseHandle(pi.hProcess);
      }
      CloseHandle(job);
      bool done = ok && (devices == null ? ResetSeen.Count > 0 : devices.Count > 0 && ResetSeen.IsSupersetOf(devices));
      Emit("\"event\":\"audio-reset\",\"ok\":" + (done ? "true" : "false"));
      return !done;
    }
    public static int Run() {
      var input = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false));
      var reader = new Thread(() => {
        try {
          string line;
          while ((line = input.ReadLine()) != null) lock (Commands) { Commands.Enqueue(line); }
        } catch (Exception) { }
        InputClosed = true;
      });
      reader.IsBackground = true;
      reader.Start();
      Emit("\"event\":\"ready\"");

      Dictionary<string, string> start = null;
      while (start == null) {
        string line = Next();
        if (line != null) {
          var cmd = Parse(line);
          string name;
          if (cmd.TryGetValue("cmd", out name) && name == "start") start = cmd;
          else if (name == "stop") { Emit("\"event\":\"stopped\",\"code\":null,\"pending\":false"); return 0; }
        } else if (InputClosed) return 0;
        else Thread.Sleep(20);
      }

      string exe = start["exe"], cwd = start["cwd"], cmdLine = start["commandLine"];
      string reset = start.ContainsKey("resetCommandLine") ? start["resetCommandLine"] : "";
      // Сброс mute запускает саму игру, даже если её запускает посредник (HL1: Injector).
      string resetExe = start.ContainsKey("resetExe") ? start["resetExe"] : exe;
      bool mute = start.ContainsKey("mute") && start["mute"] == "true";
      bool unmuteAll = start.ContainsKey("unmuteAll") && start["unmuteAll"] == "true";
      // followJob: запущенный процесс — посредник (Injector), он выходит сразу;
      // игра жива, пока в задании есть процессы.
      bool followJob = start.ContainsKey("followJob") && start["followJob"] == "true";
      // watchImage: имя образа без .exe (HL1: "hl"); такой процесс вне задания — событие foreign
      // (чужая игра на том же движке, у неё общий с нами ключ реестра).
      string watchImage = start.ContainsKey("watchImage") ? start["watchImage"] : "";
      var foreignSeen = new HashSet<uint>();
      int mySession = System.Diagnostics.Process.GetCurrentProcess().SessionId;
      int lastWatch = 0;
      string desktopName = "hlsr-render-" + System.Diagnostics.Process.GetCurrentProcess().Id + "-" + Environment.TickCount;

      IntPtr desktop = CreateDesktopW(desktopName, IntPtr.Zero, IntPtr.Zero, 0, GENERIC_ALL, IntPtr.Zero);
      if (desktop == IntPtr.Zero) { Error("CreateDesktopW", Marshal.GetLastWin32Error()); return 2; }
      // resetOnly: без рендера — только беззвучный сброс mute, который Windows запомнил после
      // прошлого рендера (флаг лаунчера): один запуск resetCommandLine на этом столе.
      if (start.ContainsKey("resetOnly") && start["resetOnly"] == "true") {
        Emit("\"event\":\"started\",\"pid\":0,\"desktop\":" + Json(desktopName));
        bool left = ResetMute(resetExe, reset, cwd, desktopName, null);
        for (int k = 0; k < 40 && !CloseDesktop(desktop); k++) Thread.Sleep(250);
        Emit("\"event\":\"stopped\",\"code\":0,\"alone\":false,\"pending\":" + (left ? "true" : "false"));
        return 0;
      }
      IntPtr job = NewJob();
      if (job == IntPtr.Zero) { CloseDesktop(desktop); return 2; }
      bool ok;
      var pi = Launch(exe, cmdLine, cwd, desktopName, job, out ok);
      if (!ok) { CloseHandle(job); CloseDesktop(desktop); return 2; }
      Emit("\"event\":\"started\",\"pid\":" + pi.dwProcessId + ",\"desktop\":" + Json(desktopName));

      bool exitedAlone = false;
      uint exitCode = 0;
      bool stopping = false;
      // Игра всегда закрывается заглушённой: живую игру не разглушаем, иначе
      // звук мог бы прозвучать. reset — снять запомненный mute беззвучным
      // перезапуском (конец рендера); без него (между пачками) следующая пачка
      // стартует уже заглушённой. Закрытый stdin — тоже конец, со сбросом.
      bool resetAfter = true;
      while (true) {
        string line;
        while ((line = Next()) != null) {
          var cmd = Parse(line);
          string name; cmd.TryGetValue("cmd", out name);
          if (name == "stop") { stopping = true; string r; resetAfter = !cmd.TryGetValue("reset", out r) || r != "false"; }
        }
        if (InputClosed) stopping = true;
        if (WaitForSingleObject(pi.hProcess, 0) == 0 && (!followJob || JobPids(job).Count == 0)) {
          GetExitCodeProcess(pi.hProcess, out exitCode);
          exitedAlone = !stopping;
          break;
        }
        if (stopping) break;
        if (mute) SafeSessions(JobPids(job), MUTE, unmuteAll);
        if (watchImage.Length > 0 && unchecked(Environment.TickCount - lastWatch) > 1000) {
          lastWatch = Environment.TickCount;
          try {
            // Сначала процессы, потом список задания: дочерний процесс игры, созданный
            // между двумя запросами, уже в задании и чужим не считается.
            var found = System.Diagnostics.Process.GetProcessesByName(watchImage);
            var mine = JobPids(job);
            // Пустой список — запрос задания не ответил (своя игра в нём есть всегда): не гадаем.
            // Игры других сеансов Windows (другой пользователь, свой HKCU) нам не мешают.
            foreach (var p in found) {
              uint id = (uint)p.Id;
              if (mine.Count > 0 && p.SessionId == mySession && !mine.Contains(id) && foreignSeen.Add(id)) Emit("\"event\":\"foreign\",\"pid\":" + id);
              p.Dispose();
            }
          } catch (Exception) { }
        }
        Thread.Sleep(50);
      }

      if (exitedAlone) Emit("\"event\":\"exited\",\"code\":" + exitCode);
      TerminateJobObject(job, 1);
      WaitForSingleObject(pi.hProcess, 5000);
      // Игра посредника должна закрыться до сброса: второй hl.exe не стартует рядом с первым.
      for (int k = 0; k < 100 && followJob && JobPids(job).Count > 0; k++) Thread.Sleep(50);
      if (!exitedAlone) GetExitCodeProcess(pi.hProcess, out exitCode);
      var devices = new HashSet<uint>();
      foreach (var key in MutedByUs) devices.Add(uint.Parse(key.Substring(key.IndexOf(':') + 1)));
      // Mute прошлой пачки или сбоя (unmuteAll), а сессий игра так и не открыла — он остался.
      bool pending = devices.Count > 0 || (unmuteAll && SeenSessions.Count == 0);
      // Сброс — только для устройств, где глушили сами: без них подтверждать нечего.
      if (devices.Count > 0 && resetAfter && reset.Length > 0) {
        MutedByUs.Clear();
        SeenSessions.Clear();
        pending = ResetMute(resetExe, reset, cwd, desktopName, devices);
      }
      CloseHandle(pi.hProcess);
      CloseHandle(job);
      for (int k = 0; k < 40 && !CloseDesktop(desktop); k++) Thread.Sleep(250);
      // pending: mute этой игры, возможно, остался у Windows — лаунчер запомнит и снимет позже.
      Emit("\"event\":\"stopped\",\"code\":" + exitCode + ",\"alone\":" + (exitedAlone ? "true" : "false") + ",\"pending\":" + (pending ? "true" : "false"));
      return 0;
    }
  }
}
'@
exit ([HlsrRender.Host]::Run())
