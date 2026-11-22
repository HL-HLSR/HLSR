using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>Настройки лаунчера: язык, папка библиотеки, лимит скорости, Discord, обновление.</summary>
public sealed partial class SettingsPage : Page
{
    private static readonly int[] SpeedLimits = [0, 1, 2, 5, 10, 20, 50, 100];

    private bool _loading;
    private IDisposable? _updaterEvents;
    private JsonNode? _updater;

    public SettingsPage()
    {
        InitializeComponent();
        PageTitle.Text = Loc.T("#UI_SETTINGS");
        GeneralHeader.Text = Loc.T("#L_S_GENERAL");
        LanguageBox.Header = Loc.T("#L_S_LANGUAGE");
        LanguageHint.Text = Loc.T("#RESTART_APP");
        DownloadsHeader.Text = Loc.T("#L_S_DOWNLOADS");
        PathLabel.Text = Loc.T("#L_S_INSTALL_PATH");
        BrowseButton.Content = Loc.T("#L_S_BROWSE");
        SpeedBox.Header = Loc.T("#L_S_SPEED_LIMIT");
        foreach (var limit in SpeedLimits)
            SpeedBox.Items.Add(new ComboBoxItem { Content = limit == 0 ? Loc.T("#L_S_NO_LIMIT") : Loc.T("#L_S_MBPS", limit), Tag = limit });
        IntegrationsHeader.Text = Loc.T("#L_S_INTEGRATIONS");
        DiscordSwitch.Header = Loc.T("#L_S_DISCORD_RPC");
        DiscordSwitch.OnContent = Loc.Code == "ru" ? "Вкл." : "On";
        DiscordSwitch.OffContent = Loc.Code == "ru" ? "Откл." : "Off";
        AboutHeader.Text = Loc.T("#L_S_ABOUT");
        LogsButton.Content = Loc.T("#L_S_OPEN_LOGS");
        SiteLink.Content = Loc.T("#L_S_WEBSITE");
        GitHubLink.Content = Loc.T("#L_S_GITHUB");
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _loading = true;
        LanguageBox.SelectedIndex = Loc.Code == "ru" ? 0 : 1;
        VersionText.Text = Loc.T("#L_U_CURRENT", Backend.SendSync("app-version")?.ToString() ?? "");
        _updaterEvents = Backend.On("updater-state", state => { _updater = state; RenderUpdater(); });

        try
        {
            await LoadLibrary();
            var download = await Backend.InvokeAsync("download-settings-get");
            var limit = (int)Games.Num(download, "speedLimit");
            SpeedBox.SelectedItem = SpeedBox.Items.OfType<ComboBoxItem>().FirstOrDefault(i => (int)i.Tag == limit) ?? SpeedBox.Items[0];
            var discord = await Backend.InvokeAsync("discord-rpc-get");
            DiscordSwitch.IsOn = Games.Bool(discord, "enabled");
            RenderDiscord(discord);
            _updater = await Backend.InvokeAsync("updater-get-state");
            RenderUpdater();
            BuildExtras();
        }
        catch (BackendException ex)
        {
            PathInfo.Text = ex.Error;
        }
        _loading = false;
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e) => _updaterEvents?.Dispose();

    private async Task LoadLibrary()
    {
        var library = await Backend.InvokeAsync("get-library-path");
        PathBox.Text = Games.Str(library, "path");
        var free = Games.Num(library, "freeSpaceMb");
        PathInfo.Text = free > 0 ? (Loc.Code == "ru" ? "Свободно: " : "Free: ") + Games.Size(free * 1024 * 1024) : "";
    }

    private void OnLanguageChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loading || LanguageBox.SelectedItem is not ComboBoxItem { Tag: string code }) return;
        var settings = Backend.StoreRead("settings") ?? new JsonObject();
        if (settings["config"] is not JsonObject config) settings["config"] = config = new JsonObject();
        config["language"] = code == "en" ? 0 : 1;
        Backend.StoreWrite("settings", settings);
        _ = Backend.InvokeAsync("updater-set-language", code);
        LanguageHint.Visibility = code == Loc.Code ? Visibility.Collapsed : Visibility.Visible;
    }

    private async void OnBrowse(object sender, RoutedEventArgs e)
    {
        var picker = new Windows.Storage.Pickers.FolderPicker();
        picker.FileTypeFilter.Add("*");
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(App.Window));
        var folder = await picker.PickSingleFolderAsync();
        if (folder is null) return;
        try
        {
            var result = await Backend.InvokeAsync("set-library-path", folder.Path);
            if (result?["ok"]?.GetValue<bool>() != true)
            {
                var code = Games.Str(result, "code");
                var text = code switch
                {
                    "BUSY" => Loc.T("#L_S_PATH_BUSY"),
                    "NO_ACCESS" or "NOT_WRITABLE" => Loc.T("#L_S_PATH_NO_ACCESS"),
                    _ => Loc.T("#L_S_PATH_INVALID") + (code.Length > 0 ? $" ({code})" : ""),
                };
                await Dialogs.Message(Loc.T("#L_S_INSTALL_PATH"), text);
            }
            await LoadLibrary();
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(Loc.T("#L_S_INSTALL_PATH"), ex.Error);
        }
    }

    private async void OnSpeedChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loading || SpeedBox.SelectedItem is not ComboBoxItem { Tag: int limit }) return;
        try
        {
            var result = await Backend.InvokeAsync("download-settings-set", limit);
            if (result?["ok"]?.GetValue<bool>() != true) await Dialogs.Message(Loc.T("#L_S_SPEED_LIMIT"), Loc.T("#L_S_SPEED_SAVE_FAILED"));
        }
        catch (BackendException)
        {
            await Dialogs.Message(Loc.T("#L_S_SPEED_LIMIT"), Loc.T("#L_S_SPEED_SAVE_FAILED"));
        }
    }

    private async void OnDiscordToggled(object sender, RoutedEventArgs e)
    {
        if (_loading) return;
        try { RenderDiscord(await Backend.InvokeAsync("discord-rpc-set", DiscordSwitch.IsOn)); }
        catch (BackendException ex) { DiscordHint.Text = ex.Error; }
    }

    private void RenderDiscord(JsonNode? state)
    {
        var status = Games.Str(state, "state") switch
        {
            "connected" => Loc.T("#L_S_DISCORD_RPC_CONNECTED"),
            "connecting" => Loc.T("#L_S_DISCORD_RPC_CONNECTING"),
            "unavailable" => Loc.T("#L_S_DISCORD_RPC_UNAVAILABLE"),
            "error" => Loc.T("#L_S_DISCORD_RPC_ERROR"),
            _ => Games.Bool(state, "enabled") ? "" : Loc.T("#L_S_DISCORD_RPC_OFF"),
        };
        DiscordHint.Text = Loc.T("#L_S_DISCORD_RPC_HINT") + (status.Length > 0 ? $"\n{status}" : "");
    }

    private void RenderUpdater()
    {
        var state = Games.Str(_updater, "state");
        var version = Games.Str(_updater, "version");
        var progress = _updater?["progress"];
        UpdaterProgress.Visibility = state == "downloading" ? Visibility.Visible : Visibility.Collapsed;
        UpdaterProgress.Value = Games.Num(progress, "percent");
        UpdaterButton.IsEnabled = Games.Bool(_updater, "enabled") || state is "available" or "downloaded";
        (UpdaterText.Text, UpdaterButton.Content) = state switch
        {
            "checking" => (Loc.T("#L_U_CHECKING"), Loc.T("#L_U_DOWNLOAD")),
            "available" => (Loc.T("#L_U_AVAILABLE", version), Loc.T("#L_U_DOWNLOAD")),
            "downloading" => (Loc.T("#L_U_DOWNLOADING"), Loc.T("#L_CANCEL")),
            "downloaded" or "ready" => (Loc.T("#L_U_READY", version), Loc.T("#L_U_RESTART_INSTALL")),
            "error" => (Loc.T("#L_U_ERROR"), CheckLabel),
            _ => (Games.Bool(_updater, "enabled") ? "" : Loc.T("#L_U_ERR_CONFIG"), CheckLabel),
        };
        if (state == "checking") UpdaterButton.IsEnabled = false;
    }

    private static string CheckLabel => Loc.Code == "ru" ? "Проверить наличие обновлений" : "Check for updates";

    private async void OnUpdater(object sender, RoutedEventArgs e)
    {
        var state = Games.Str(_updater, "state");
        var channel = state switch
        {
            "available" => "updater-download",
            "downloading" => "updater-cancel",
            "downloaded" or "ready" => "updater-install-now",
            _ => "updater-check",
        };
        try
        {
            var reply = await Backend.InvokeAsync(channel);
            if (reply is JsonObject o && o.ContainsKey("state")) _updater = reply;
        }
        catch (BackendException ex)
        {
            UpdaterText.Text = ex.Error;
        }
        RenderUpdater();
    }

    private async void OnLogs(object sender, RoutedEventArgs e)
    {
        try { await Backend.InvokeAsync("open-logs-folder"); }
        catch (BackendException) { }
    }

    // ---- Во время игры, диагностика ---------------------------------------------------------

    private readonly StackPanel _checks = new() { Spacing = 2 };

    private void BuildExtras()
    {
        ExtraHost.Children.Clear();
        var form = new Form();
        form.Panel.Margin = new Thickness(0, 20, 0, 0);
        ExtraHost.Children.Add(form.Panel);

        form.Header(Loc.T("#L_S_WHILE_PLAYING"), first: true);
        var prefs = Backend.StoreRead("launcher-prefs") ?? new JsonObject();
        prefs["hideWhilePlaying"] ??= true;
        form.Toggle(prefs, "hideWhilePlaying", Loc.T("#L_S_HIDE_PLAYING"), Loc.T("#L_S_HIDE_PLAYING_HINT"), on =>
        {
            Backend.StoreWrite("launcher-prefs", prefs);
            _ = Backend.InvokeAsync("tray-hide-while-playing-set", on);
        });

        form.Header(Loc.T("#L_S_DIAGNOSTICS"));
        var run = new Button { Content = Loc.T("#L_DG_RUN"), Style = Form.Style("AccentButtonStyle"), MinWidth = 120 };
        var export = new Button { Content = Loc.T("#L_S_EXPORT_DIAGNOSTICS") };
        var status = new TextBlock { Style = Form.Style("W10CaptionText"), VerticalAlignment = VerticalAlignment.Center };
        JsonArray? last = null;
        run.Click += async (_, _) =>
        {
            run.IsEnabled = false;
            status.Text = Loc.T("#L_DG_RUNNING");
            try
            {
                var result = await Backend.InvokeAsync("diagnostics-run");
                last = result?["checks"] as JsonArray;
                RenderChecks(last);
                status.Text = "";
            }
            catch (BackendException ex) { status.Text = ex.Error; }
            run.IsEnabled = true;
        };
        export.Click += async (_, _) =>
        {
            try
            {
                var result = await Backend.InvokeAsync("export-diagnostics-zip", last is null ? null : new JsonObject { ["checks"] = last.DeepClone() });
                var st = Games.Str(result, "status");
                status.Text = st switch
                {
                    "ok" => Loc.T("#L_S_EXPORT_DIAGNOSTICS_OK"),
                    "canceled" or "cancelled" => Loc.T("#L_DG_EXPORT_CANCELLED"),
                    _ => Loc.T("#L_S_EXPORT_DIAGNOSTICS_FAIL"),
                };
            }
            catch (BackendException) { status.Text = Loc.T("#L_S_EXPORT_DIAGNOSTICS_FAIL"); }
        };
        form.Row(run, export, status);
        form.Add(_checks);
    }

    private void RenderChecks(JsonArray? checks)
    {
        _checks.Children.Clear();
        foreach (var group in (checks ?? []).OfType<JsonObject>().GroupBy(c => Games.Str(c, "group")))
        {
            _checks.Children.Add(new TextBlock
            {
                Text = Loc.TOr("#L_DG_G_" + group.Key.ToUpperInvariant(), group.Key),
                Style = Form.Style("W10BaseText"),
                Margin = new Thickness(0, 12, 0, 4),
            });
            foreach (var check in group)
            {
                var st = Games.Str(check, "status");
                var glyph = st switch { "ok" => "", "warn" => "", "error" or "fail" => "", _ => "" };
                var id = Games.Str(check, "id");
                var title = Loc.TOr("#L_DG_" + id.ToUpperInvariant().Replace('-', '_'), id);
                var detailKey = Games.Str(check, "detailKey");
                var args = (check["detailArgs"] as JsonArray ?? []).Select(a => (object?)a?.ToString()).ToArray();
                var detail = detailKey.Length > 0 ? Loc.T(detailKey, args) : Games.Str(check, "detail");
                var code = Games.Str(check, "code");
                if (code.Length > 0 && st != "ok") detail = string.Join(" · ", new[] { detail, Loc.TOr("#L_DGH_" + code, code) }.Where(x => x.Length > 0));
                _checks.Children.Add(Form.ListRow(glyph, title, detail));
            }
        }
    }
}
