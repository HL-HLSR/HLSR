using HLSR.Core;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace HLSR;

public partial class App : Application
{
    public static MainWindow? Window { get; private set; }

    public App()
    {
        AppDomain.CurrentDomain.UnhandledException += (_, e) => CrashLog(e.ExceptionObject?.ToString());
        try
        {
            InitializeComponent();
        }
        catch (Exception e)
        {
            CrashLog(e.ToString());
            throw;
        }
        UnhandledException += (_, e) =>
        {
            CrashLog(e.Exception?.ToString() + Environment.NewLine + e.Message);
            e.Handled = true;
        };
        DebugSettings.XamlResourceReferenceFailed += (_, e) => CrashLog("XAML resource: " + e.Message);
        DebugSettings.BindingFailed += (_, e) => CrashLog("Binding: " + e.Message);
    }

    /// <summary>Сбой интерфейса — в <c>%TEMP%\hlsr-winui-crash.log</c>.</summary>
    public static void CrashLog(string? text)
    {
        try
        {
            File.AppendAllText(Path.Combine(Path.GetTempPath(), "hlsr-winui-crash.log"),
                $"[{DateTime.Now:O}] {text}{Environment.NewLine}");
        }
        catch (IOException) { }
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var ui = DispatcherQueue.GetForCurrentThread();
        int code = Backend.Start(ui);
        if (code == Backend.StartForwarded)
        {
            // Уже запущен другой HLSR: он сам покажет своё окно.
            Exit();
            return;
        }

        var settings = Backend.StoreRead("settings");
        int language = settings?["config"]?["language"]?.GetValue<int>() ?? 1;
        Loc.Load(language);

        try
        {
            Window = new MainWindow(code == Backend.StartOk ? null : Backend.StartupError ?? "startup error");
            Window.Activate();
        }
        catch (Exception e)
        {
            CrashLog(e.ToString());
            throw;
        }
    }
}
