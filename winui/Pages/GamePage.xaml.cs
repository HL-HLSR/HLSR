using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>
/// Страница игры: арт, главная кнопка (<c>mainButton</c> статуса доставки), загрузка,
/// причина недоступности, меню управления игрой, выбор сборки HL2.
/// </summary>
public sealed partial class GamePage : Page
{
    private static readonly string[] Cancellable =
        ["preparing", "downloading", "paused", "verifying", "extracting", "exporting", "importing"];

    private GameInfo _game = Games.All[0];
    private JsonNode? _status;
    private IDisposable? _statusEvents;
    private bool _busy;
    private bool _buildsFilling;

    public GamePage()
    {
        InitializeComponent();
        CancelText.Text = Loc.T("#L_CANCEL");
        MenuFolder.Text = Loc.T("#L_GM_FOLDER");
        MenuScreenshots.Text = Loc.T("#L_GM_SCREENSHOTS");
        MenuShortcut.Text = Loc.T("#L_GM_SHORTCUT");
        MenuUpdates.Text = Loc.TOr("#L_GM_CHECK_UPDATES", Loc.Code == "ru" ? "Проверить обновления" : "Check for updates");
        MenuVerify.Text = Loc.T("#L_GM_VERIFY");
        MenuRepair.Text = Loc.T("#L_GM_REPAIR");
        MenuUninstall.Text = Loc.T("#L_GM_UNINSTALL");
        InfoHeader.Text = Loc.T("#L_GS_CFG_BUILD");
        SettingsText.Text = Loc.T("#L_GM_SETTINGS");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(SettingsButton, Loc.T("#L_GM_SETTINGS"));
        MenuExportBuild.Text = Loc.T("#L_GM_EXPORT_BUILD");
        MenuExportSettings.Text = Loc.T("#L_GM_EXPORT_SETTINGS");
        MenuImportBuild.Text = Loc.T("#L_GM_IMPORT_BUILD");
        EpisodesHeader.Text = Loc.Code == "ru" ? "Эпизоды" : "Episodes";
        ToolTipService.SetToolTip(MoreButton, Loc.T("#L_GM_TITLE"));
        Unloaded += (_, _) => _statusEvents?.Dispose();
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _game = Games.Find(e.Parameter as string) ?? Games.All[0];
        GameTitle.Text = _game.Name;
        Hero.Source = new BitmapImage(new Uri("ms-appx:///" + _game.Hero));
        _statusEvents = Backend.On("game-delivery-status", OnStatusEvent);
        FillBuilds();
        Render();
        await Refresh();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        _statusEvents?.Dispose();
        _statusEvents = null;
    }

    private string GameId => Games.Str(_status, "gameId");

    private async Task Refresh()
    {
        try
        {
            var status = await Backend.InvokeAsync("game-delivery-get-status", _game.LauncherId);
            if (status is JsonObject) _status = status;
        }
        catch (BackendException ex)
        {
            ShowNotice("", Loc.T("#L_UA_UNKNOWN"), ex.Error);
        }
        SyncBuild();
        Render();
        await RenderEpisodes();
    }

    private void OnStatusEvent(JsonNode? status)
    {
        if (Games.Str(status, "launcherId") != _game.LauncherId) return;
        // У HL2 три сборки: событие чужой сборки страницу не трогает.
        if (_status is not null && Games.Str(status, "gameId") != GameId) return;
        _status = status;
        Render();
    }

    // ---- Сборки HL2 ----------------------------------------------------------------------

    private void FillBuilds()
    {
        _buildsFilling = true;
        BuildBox.Items.Clear();
        BuildBox.Visibility = _game.Builds.Length > 1 ? Visibility.Visible : Visibility.Collapsed;
        foreach (var build in _game.Builds) BuildBox.Items.Add(new ComboBoxItem { Content = $"{Loc.T("#L_GS_CFG_BUILD")} {build}", Tag = build });
        _buildsFilling = false;
    }

    private void SyncBuild()
    {
        var build = Games.BuildOf(GameId);
        _buildsFilling = true;
        BuildBox.SelectedItem = BuildBox.Items.OfType<ComboBoxItem>().FirstOrDefault(i => (string)i.Tag == build);
        _buildsFilling = false;
    }

    private async void OnBuildChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_buildsFilling || BuildBox.SelectedItem is not ComboBoxItem { Tag: string build }) return;
        if (Games.BuildOf(GameId) == build) return;
        try
        {
            var result = await Backend.InvokeAsync("game-select-build", _game.LauncherId, build);
            var status = Games.Str(result, "status");
            if (status == "busy")
            {
                await Dialogs.Message(Loc.T("#L_GS_CFG_BUILD"), Loc.T("#L_D_BUILD_LOCKED"));
                SyncBuild();
                return;
            }
            if (result?["delivery"] is JsonObject delivery) _status = delivery.DeepClone();
            else await Refresh();
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(Loc.T("#L_GS_CFG_BUILD"), ex.Error);
        }
        SyncBuild();
        Render();
    }

    // ---- Вид -------------------------------------------------------------------------------

    private void Render()
    {
        var button = _status?["mainButton"];
        var buttonState = button is null ? "ready" : Games.Str(button, "state");
        var (key, glyph) = Games.ButtonStyle(buttonState);
        MainText.Text = Loc.T(key);
        MainGlyph.Glyph = glyph;
        MainButton.IsEnabled = !_busy && (button is null || button["enabled"]?.GetValue<bool>() != false)
            && Games.Str(button, "action") != "none";

        var state = Games.Str(_status, "state");
        bool installed = !string.IsNullOrEmpty(Games.Str(_status, "installedVersion"));
        bool running = _status?["process"]?["running"]?.GetValue<bool>() == true;

        // Подзаголовок: версия и размер.
        var version = installed ? Games.Str(_status, "installedVersion") : Games.Str(_status, "version");
        var total = Games.Num(_status, "totalBytes");
        var parts = new List<string>();
        if (version.Length > 0) parts.Add((installed ? Loc.T("#L_D_READY") : Loc.T("#L_D_NOT_INSTALLED")) + " · " + version);
        if (total > 0 && !installed) parts.Add(Games.Size(total));
        if (running) parts.Add(Loc.T("#L_P_RUNNING"));
        GameSubtitle.Text = string.Join("   ", parts);

        RenderProgress(state);
        RenderNotice(buttonState);
        RenderInfo(installed);

        CancelButton.Visibility = Cancellable.Contains(state) ? Visibility.Visible : Visibility.Collapsed;
        bool idle = !Cancellable.Contains(state) && !running;
        MenuFolder.IsEnabled = installed;
        MenuScreenshots.IsEnabled = installed;
        MenuShortcut.IsEnabled = installed;
        MenuUpdates.IsEnabled = installed && idle;
        MenuVerify.IsEnabled = installed && idle;
        MenuRepair.IsEnabled = installed && idle;
        MenuUninstall.IsEnabled = installed && idle;
        MenuExportBuild.IsEnabled = installed && idle;
        MenuExportSettings.IsEnabled = installed && idle;
        MenuImportBuild.IsEnabled = idle;
        BuildBox.IsEnabled = idle;
    }

    private void RenderProgress(string state)
    {
        bool show = Cancellable.Contains(state) || _status?["mainButton"]?["progress"]?.GetValue<bool>() == true;
        ProgressPanel.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        if (!show) return;

        var percent = Math.Clamp(Games.Num(_status, "percent"), 0, 100);
        ProgressTitle.Text = state switch
        {
            "preparing" => Loc.T("#L_D_PREPARING"),
            "downloading" => Loc.T("#L_D_DOWNLOADING"),
            "paused" => Loc.T("#L_D_PAUSED"),
            "verifying" => Loc.T("#L_D_VERIFYING"),
            "extracting" => Loc.T("#L_D_EXTRACTING"),
            "finalizing" => Loc.T("#L_D_FINALIZING"),
            "exporting" => Loc.T("#L_D_EXPORTING"),
            "importing" => Loc.T("#L_D_IMPORTING"),
            "repairing" => Loc.T("#L_D_REPAIRING"),
            "checking" => Loc.T("#L_D_CHECKING"),
            "applying" => Loc.T("#L_D_APPLYING"),
            _ => Loc.T("#L_D_DOWNLOADING"),
        };
        ProgressPercent.Text = $"{percent:0}%";
        Progress.IsIndeterminate = state == "preparing";
        Progress.Value = percent;
        Progress.ShowPaused = state == "paused";

        var details = new List<string>();
        double done = Games.Num(_status, "bytesDownloaded"), total = Games.Num(_status, "totalBytes");
        if (total > 0) details.Add($"{Games.Size(done)} / {Games.Size(total)}");
        double speed = Games.Num(_status, "speedBps");
        if (speed > 0 && state == "downloading") details.Add(Loc.T("#L_D_SPEED", (speed / 1024 / 1024).ToString("0.0")));
        double eta = Games.Num(_status, "etaSeconds");
        if (eta > 0 && state == "downloading") details.Add(Loc.T("#L_D_ETA", Games.Eta(eta)));
        double checkedFiles = Games.Num(_status, "checked"), totalChecked = Games.Num(_status, "totalChecked");
        if (totalChecked > 0 && state is "verifying" or "checking") details.Add($"{checkedFiles:0} / {totalChecked:0} {Loc.T("#L_D_FILES_UNIT")}");
        ProgressDetails.Text = string.Join(" · ", details);
    }

    private void RenderNotice(string buttonState)
    {
        var error = _status?["error"];
        if (error is JsonObject)
        {
            var message = Games.Str(error, "message");
            if (message.Length == 0) message = Games.Str(error, "code");
            ShowNotice("", Loc.T("#L_B_RETRY"), message);
            return;
        }
        if (buttonState == "unavailable")
        {
            var code = Games.Str(_status?["mainButton"], "reason");
            if (code.Length == 0) code = Games.Str(_status, "unavailableReason");
            var (title, text) = Games.Reason(code);
            ShowNotice("", Loc.T(title), Loc.T(text));
            return;
        }
        Notice.Visibility = Visibility.Collapsed;
    }

    private void ShowNotice(string glyph, string title, string text)
    {
        NoticeGlyph.Glyph = glyph;
        NoticeTitle.Text = title;
        NoticeText.Text = text;
        NoticeText.Visibility = string.IsNullOrEmpty(text) ? Visibility.Collapsed : Visibility.Visible;
        Notice.Visibility = Visibility.Visible;
    }

    private void RenderInfo(bool installed)
    {
        InfoGrid.Children.Clear();
        InfoGrid.RowDefinitions.Clear();
        void Row(string label, string value)
        {
            if (string.IsNullOrEmpty(value)) return;
            int row = InfoGrid.RowDefinitions.Count;
            InfoGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var l = new TextBlock { Text = label, Style = (Style)Application.Current.Resources["W10CaptionText"], FontSize = 14 };
            var v = new TextBlock { Text = value, FontSize = 14, IsTextSelectionEnabled = true, TextWrapping = TextWrapping.Wrap };
            Grid.SetRow(l, row);
            Grid.SetRow(v, row);
            Grid.SetColumn(v, 1);
            InfoGrid.Children.Add(l);
            InfoGrid.Children.Add(v);
        }

        Row(Loc.T("#L_GS_V_NAME"), _game.Name + (Games.BuildOf(GameId) is { } b ? $" ({b})" : ""));
        Row(Loc.T("#L_GS_V_BUILD"), GameId);
        Row(Loc.T("#L_BUILD_VERSION"), Games.Str(_status, "version"));
        if (installed)
        {
            Row(Loc.T("#L_BUILD_INSTALLED"), Games.Str(_status, "installedVersion"));
            var at = Games.Str(_status, "installedAt");
            if (DateTimeOffset.TryParse(at, out var when)) Row(Loc.T("#L_BUILD_UPDATED"), when.LocalDateTime.ToString("g"));
        }
        var total = Games.Num(_status, "totalBytes");
        if (total > 0) Row(Loc.TOr("#L_S_SIZE", Loc.Code == "ru" ? "Размер" : "Size"), Games.Size(total));
        var date = Games.Str(_status, "buildDate");
        if (date.Length > 0) Row(Loc.T("#L_GS_V_BUILT"), date);
    }

    // ---- Действия --------------------------------------------------------------------------

    private async void OnMainButton(object sender, RoutedEventArgs e)
    {
        var button = _status?["mainButton"];
        var action = button is null ? "play" : Games.Str(button, "action");
        _busy = true;
        Render();
        try
        {
            switch (action)
            {
                case "play":
                    await Play();
                    break;
                case "stop":
                    await Check(await Backend.InvokeAsync("game-process-stop", GameId));
                    break;
                case "install":
                    await Install();
                    break;
                case "update":
                    await Check(await Backend.InvokeAsync("game-menu-apply-update", GameId));
                    break;
                case "pause":
                    await Backend.InvokeAsync("game-delivery-pause");
                    break;
                case "resume":
                case "retry":
                    if (Games.Str(_status, "operation") == "update")
                        await Check(await Backend.InvokeAsync("game-menu-apply-update", GameId));
                    else
                        await Install();
                    break;
                case "repair":
                    await Check(await Backend.InvokeAsync("game-menu-repair", GameId));
                    break;
                case "explain":
                    var (title, text) = Games.Reason(Games.Str(button, "reason"));
                    await Dialogs.Message(Loc.T("#L_UA_TITLE"), Loc.T(title) + "\n\n" + Loc.T(text));
                    break;
            }
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(_game.Name, ex.Error);
        }
        finally
        {
            _busy = false;
            await Refresh();
        }
    }

    private async Task Play()
    {
        var result = await Backend.InvokeAsync("game-process-launch", GameId, new JsonObject());
        if (Games.Str(result, "status") == "already-running")
        {
            await Dialogs.Message(_game.Name, Loc.T("#L_P_ALREADY_RUNNING"));
            return;
        }
        await Check(result);
    }

    private async Task Install()
    {
        if (_status is not null && !Games.Bool(_status, "installable"))
        {
            var (title, text) = Games.Reason(Games.Str(_status, "unavailableReason"));
            await Dialogs.Message(Loc.T("#L_D_UNAVAILABLE"), Loc.T(title) + "\n\n" + Loc.T(text));
            return;
        }
        await Check(await Backend.InvokeAsync("game-delivery-install", _game.LauncherId));
    }

    /// <summary>Ответ операции: <c>{status: "error", error: {code, message}}</c> — сообщение.</summary>
    private async Task Check(JsonNode? result)
    {
        if (result is null || Games.Str(result, "status") != "error") return;
        var error = result["error"];
        var message = Games.Str(error, "message");
        var code = Games.Str(error, "code");
        if (message.Length == 0)
        {
            var (title, text) = Games.Reason(code);
            message = Loc.T(title) + "\n\n" + Loc.T(text);
        }
        await Dialogs.Message(_game.Name, message);
    }

    private async void OnCancel(object sender, RoutedEventArgs e)
    {
        try { await Backend.InvokeAsync("game-delivery-cancel", GameId); }
        catch (BackendException ex) { await Dialogs.Message(_game.Name, ex.Error); }
        await Refresh();
    }

    private async void OnMenu(object sender, RoutedEventArgs e)
    {
        if (sender is not MenuFlyoutItem { Tag: string channel }) return;
        try
        {
            var result = await Backend.InvokeAsync(channel, GameId);
            await Check(result);
            switch (channel)
            {
                case "game-menu-create-shortcut" when Games.Str(result, "status") != "error":
                    await Dialogs.Message(_game.Name, Loc.T("#L_GM_SHORTCUT_DONE"));
                    break;
                case "game-menu-check-updates":
                    var available = result?["update"]?["availableVersion"]?.ToString() ?? Games.Str(result, "availableVersion");
                    var update = result?["update"]?["updateAvailable"]?.GetValue<bool>() == true || Games.Bool(result, "updateAvailable");
                    await Dialogs.Message(_game.Name, update ? Loc.T("#L_GM_UPDATE_AVAILABLE", available) : Loc.T("#L_GM_NO_UPDATES"));
                    break;
                case "game-menu-verify-files" when Games.Str(result, "status") != "error":
                    var damaged = result?["damaged"] as JsonArray;
                    await Dialogs.Message(_game.Name, damaged is { Count: > 0 }
                        ? Loc.T("#L_D_FILES_DAMAGED_N", damaged.Count)
                        : Loc.T("#L_GM_NO_DAMAGE"));
                    break;
            }
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(_game.Name, ex.Error);
        }
        await Refresh();
    }

    private async void OnUninstall(object sender, RoutedEventArgs e)
    {
        bool go = await Dialogs.Confirm(Loc.T("#L_C_UNINSTALL_TITLE"),
            $"{_game.Name}\n\n{Loc.T("#L_GM_UNINSTALL_NOTE")}", Loc.T("#L_C_UNINSTALL"));
        if (!go) return;
        try
        {
            var result = await Backend.InvokeAsync("game-menu-uninstall", GameId, new JsonObject { ["confirmed"] = true });
            await Check(result);
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(_game.Name, ex.Error);
        }
        await Refresh();
    }

    // ---- Настройки игры, эпизоды, перенос сборки ---------------------------------------------

    private void OnGameSettings(object sender, RoutedEventArgs e) =>
        App.Window?.OpenPage(typeof(GameSettingsPage), _game.LauncherId);

    private static readonly (string Id, string Name)[] Episodes =
        [("ep1", "Half-Life 2: Episode One"), ("ep2", "Half-Life 2: Episode Two")];

    private async Task RenderEpisodes()
    {
        EpisodesPanel.Visibility = _game.LauncherId == "218" ? Visibility.Visible : Visibility.Collapsed;
        if (_game.LauncherId != "218") return;
        JsonNode? status;
        try { status = await Backend.InvokeAsync("game-episodes-status"); }
        catch (BackendException) { return; }
        EpisodesList.Children.Clear();
        bool ghostingInstalled = !string.IsNullOrEmpty(Games.Str(_status, "installedVersion"));
        foreach (var (id, name) in Episodes)
        {
            var ep = status?[id];
            bool installed = Games.Bool(ep, "installed");
            var subtitle = installed
                ? $"{Loc.T("#L_D_READY")} · {Games.Str(ep, "version")}"
                : Games.Bool(ep, "paused") ? Loc.T("#L_D_PAUSED")
                : Games.Num(ep, "size") > 0 ? Games.Size(Games.Num(ep, "size")) : Loc.T("#L_D_NOT_INSTALLED");
            var action = installed
                ? Form.IconButton("", Loc.T("#L_GM_UNINSTALL_EPISODE"), async () =>
                {
                    if (!await Dialogs.Confirm(Loc.T("#L_GM_UNINSTALL_EPISODE"), Loc.T("#L_GM_EPISODE_REMOVE", name) + "\n\n" + Loc.T("#L_GM_EPISODE_REMOVE_NOTE"), Loc.T("#L_C_UNINSTALL"))) return;
                    await Check(await Backend.InvokeAsync("game-episode-remove", id));
                    await RenderEpisodes();
                })
                : Form.IconButton("", Loc.T("#L_B_INSTALL"), async () =>
                {
                    await Check(await Backend.InvokeAsync("game-episode-install", id));
                    await RenderEpisodes();
                });
            action.IsEnabled = ghostingInstalled && (installed || Games.Bool(ep, "available") || Games.Bool(ep, "paused"));
            var row = Form.ListRow("", name, subtitle, action);
            row.Background = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources["W10ListLowBrush"];
            EpisodesList.Children.Add(row);
        }
    }

    /// <summary>Экспорт и импорт сборки (<c>.hlsrpack</c>): вопрос — <c>confirmation-required</c> с токеном.</summary>
    private async void OnTransfer(object sender, RoutedEventArgs e)
    {
        if (sender is not MenuFlyoutItem { Tag: string kind }) return;
        try
        {
            var result = kind switch
            {
                "export" => await Backend.InvokeAsync("game-build-export", GameId),
                "export-settings" => await Backend.InvokeAsync("game-build-export-settings", GameId),
                _ => await Backend.InvokeAsync("game-build-import"),
            };
            if (Games.Str(result, "status") == "confirmation-required")
            {
                var text = Games.Str(result, "kind") == "pack"
                    ? $"{Games.Str(result, "installedVersion")} → {Games.Str(result, "version")}"
                    : string.Join("\n", (result?["modified"] as JsonArray ?? []).Select(x => x?.ToString()));
                if (!await Dialogs.Confirm(kind == "import" ? Loc.T("#L_GM_IMPORT_BUILD") : Loc.T("#L_GM_EXPORT_BUILD"), text, Loc.T("#L_SAVE"))) return;
                result = await Backend.InvokeAsync(kind == "import" ? "game-build-import-confirm" : "game-build-export-confirm", Games.Str(result, "token"));
            }
            switch (Games.Str(result, "status"))
            {
                case "ok":
                    await Dialogs.Message(_game.Name, kind == "import" ? Loc.T("#L_D_IMPORT_DONE") : Loc.T("#L_D_EXPORT_DONE"));
                    break;
                case "error":
                    await Dialogs.Message(_game.Name, Games.Str(result, "code"));
                    break;
            }
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(_game.Name, ex.Error);
        }
        await Refresh();
    }
}
