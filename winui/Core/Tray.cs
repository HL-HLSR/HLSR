using System.Runtime.InteropServices;

namespace HLSR.Core;

/// <summary>
/// Значок HLSR в области уведомлений на время игры (трей платформы: <c>host:hide-for-game</c>,
/// <c>host:show-after-game</c>). Shell_NotifyIcon на окне лаунчера; сообщения значка — через
/// сабкласс окна (comctl32 v6). Щелчок — показать окно, правая кнопка — меню «Показать HLSR / Выход».
/// </summary>
public sealed class Tray : IDisposable
{
    private const int WM_APP_TRAY = 0x8000 + 0x48;
    private const int WM_LBUTTONUP = 0x0202;
    private const int WM_LBUTTONDBLCLK = 0x0203;
    private const int WM_RBUTTONUP = 0x0205;
    private const int WM_CONTEXTMENU = 0x007B;
    private const int NIM_ADD = 0, NIM_DELETE = 2, NIM_SETVERSION = 4;
    private const int NIF_MESSAGE = 1, NIF_ICON = 2, NIF_TIP = 4;
    private const int NOTIFYICON_VERSION_4 = 4;
    private const uint MF_STRING = 0, TPM_RETURNCMD = 0x100, TPM_RIGHTBUTTON = 2;
    private const int CmdShow = 1, CmdQuit = 2;

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NOTIFYICONDATA
    {
        public int cbSize;
        public IntPtr hWnd;
        public int uID;
        public int uFlags;
        public int uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public int dwState;
        public int dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public int uVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public int dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)] private struct POINT { public int X, Y; }

    private delegate IntPtr SubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, UIntPtr id, UIntPtr data);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] private static extern bool Shell_NotifyIcon(int msg, ref NOTIFYICONDATA data);
    [DllImport("comctl32.dll")] private static extern bool SetWindowSubclass(IntPtr hWnd, SubclassProc proc, UIntPtr id, UIntPtr data);
    [DllImport("comctl32.dll")] private static extern bool RemoveWindowSubclass(IntPtr hWnd, SubclassProc proc, UIntPtr id);
    [DllImport("comctl32.dll")] private static extern IntPtr DefSubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr LoadImage(IntPtr hInst, string name, uint type, int cx, int cy, uint load);
    [DllImport("user32.dll")] private static extern IntPtr CreatePopupMenu();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool AppendMenu(IntPtr menu, uint flags, UIntPtr id, string text);
    [DllImport("user32.dll")] private static extern int TrackPopupMenuEx(IntPtr menu, uint flags, int x, int y, IntPtr hWnd, IntPtr tpm);
    [DllImport("user32.dll")] private static extern bool DestroyMenu(IntPtr menu);
    [DllImport("user32.dll")] private static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] private static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern bool DestroyIcon(IntPtr icon);

    private readonly IntPtr _hwnd;
    private readonly SubclassProc _proc;
    private readonly Action _show;
    private readonly Action _quit;
    private IntPtr _icon;
    private bool _visible;

    public Tray(IntPtr hwnd, Action show, Action quit)
    {
        _hwnd = hwnd;
        _show = show;
        _quit = quit;
        _proc = WndProc;
        SetWindowSubclass(_hwnd, _proc, (UIntPtr)0x484C5352, UIntPtr.Zero);
    }

    public bool Visible => _visible;

    public void ShowIcon()
    {
        if (_visible) return;
        if (_icon == IntPtr.Zero)
            _icon = LoadImage(IntPtr.Zero, Path.Combine(AppContext.BaseDirectory, "Assets", "hlsr.ico"), 1, 16, 16, 0x10);
        var data = Data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_APP_TRAY;
        data.hIcon = _icon;
        data.szTip = "HLSR";
        _visible = Shell_NotifyIcon(NIM_ADD, ref data);
        data.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIcon(NIM_SETVERSION, ref data);
    }

    public void HideIcon()
    {
        if (!_visible) return;
        var data = Data();
        Shell_NotifyIcon(NIM_DELETE, ref data);
        _visible = false;
    }

    private NOTIFYICONDATA Data() => new()
    {
        cbSize = Marshal.SizeOf<NOTIFYICONDATA>(),
        hWnd = _hwnd,
        uID = 1,
        szTip = "",
        szInfo = "",
        szInfoTitle = "",
    };

    private IntPtr WndProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, UIntPtr id, UIntPtr data)
    {
        if (msg == WM_APP_TRAY)
        {
            // NOTIFYICON_VERSION_4: событие — в младшем слове lParam.
            int ev = (int)((long)lParam & 0xFFFF);
            if (ev is WM_LBUTTONUP or WM_LBUTTONDBLCLK) _show();
            else if (ev is WM_RBUTTONUP or WM_CONTEXTMENU) Menu();
            return IntPtr.Zero;
        }
        return DefSubclassProc(hWnd, msg, wParam, lParam);
    }

    private void Menu()
    {
        var menu = CreatePopupMenu();
        AppendMenu(menu, MF_STRING, (UIntPtr)CmdShow, Loc.T("#L_TRAY_SHOW"));
        AppendMenu(menu, MF_STRING, (UIntPtr)CmdQuit, Loc.T("#L_TRAY_QUIT"));
        GetCursorPos(out var point);
        SetForegroundWindow(_hwnd);
        int cmd = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, point.X, point.Y, _hwnd, IntPtr.Zero);
        DestroyMenu(menu);
        if (cmd == CmdShow) _show();
        else if (cmd == CmdQuit) _quit();
    }

    public void Dispose()
    {
        HideIcon();
        RemoveWindowSubclass(_hwnd, _proc, (UIntPtr)0x484C5352);
        if (_icon != IntPtr.Zero) DestroyIcon(_icon);
    }
}
