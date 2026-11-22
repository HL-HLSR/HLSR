using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>Инструменты (LiveSplit): установка, запуск со сплитами игры, закрытие, папка, удаление.</summary>
public sealed partial class ToolsPage : Page
{
    private readonly StackPanel _content = new() { Spacing = 8, MaxWidth = 720, HorizontalAlignment = HorizontalAlignment.Left };
    private IDisposable? _events;

    public ToolsPage()
    {
        InitializeComponent();
        PageTitle.Text = Loc.Code == "ru" ? "Инструменты" : "Tools";
        Body.Children.Add(_content);
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _events = Backend.On("tool-status", ev => { _ = Load(); });
        await Load();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e) => _events?.Dispose();

    private bool _loading;

    private async Task Load()
    {
        if (_loading) return;
        _loading = true;
        try
        {
            var tools = await Backend.InvokeAsync("tool-list") as JsonArray ?? [];
            _content.Children.Clear();
            foreach (var tool in tools.OfType<JsonObject>()) _content.Children.Add(await Card(tool));
        }
        catch (BackendException ex)
        {
            _content.Children.Add(new TextBlock { Text = ex.Error });
        }
        finally { _loading = false; }
    }

    private async Task<UIElement> Card(JsonObject tool)
    {
        var id = Games.Str(tool, "toolId");
        var state = Games.Str(tool, "state");
        bool installed = state is "installed" or "update-available" || Games.Str(tool, "version").Length > 0;
        bool running = Games.Bool(tool, "running");
        var form = new Form();
        form.Header(Games.Str(tool, "title"), first: true);
        form.Caption(Loc.T(Games.Str(tool, "subtitle")));
        form.Body(Loc.T(Games.Str(tool, "description")));
        if (installed) form.Caption($"{Loc.T("#L_BUILD_VERSION")}: {Games.Str(tool, "version")}");
        else form.Caption(Loc.T("#L_TOOL_SIZE", Games.Size(Games.Num(tool, "sizeBytes"))));

        if (state is "installing" or "downloading")
        {
            form.Caption(Loc.T("#L_TOOL_INSTALLING"));
            form.Add(new ProgressBar { Width = 296, HorizontalAlignment = HorizontalAlignment.Left, Value = Games.Num(tool, "percent") });
        }

        if (!installed)
        {
            form.Button(Loc.T("#L_TOOL_INSTALL"), () => Run("tool-install", id), accent: true);
            return form.Panel;
        }

        // Сплиты: игра для запуска.
        var game = new ComboBox { Header = Loc.T("#L_TOOL_LIVESPLIT_GAME"), MinWidth = 296 };
        foreach (var g in Games.All) game.Items.Add(new ComboBoxItem { Content = g.Name, Tag = g.LauncherId });
        game.SelectedIndex = 0;
        if (id == "livesplit") form.Add(game);

        var run = new Button { Content = running ? Loc.T("#L_B_GAME_RUNNING") : Loc.T("#L_TOOL_RUN"), Style = Form.Style("AccentButtonStyle"), MinWidth = 120 };
        run.Click += async (_, _) =>
        {
            if (running) { await Run("tool-stop", id); return; }
            // tool-launch(toolId, сплиты, игра, параметры): сплиты пусто — заготовка игры по умолчанию.
            await Run("tool-launch", id, "", (game.SelectedItem as ComboBoxItem)?.Tag as string, new JsonObject());
        };
        var buttons = new List<UIElement> { run };
        if (Games.Bool(tool, "updateAvailable"))
        {
            var update = new Button { Content = Loc.T("#L_TOOL_UPDATE") };
            update.Click += async (_, _) => await Run("tool-install", id);
            buttons.Add(update);
        }
        var folder = new Button { Content = Loc.T("#L_TOOL_FOLDER") };
        folder.Click += async (_, _) => await Run("tool-open-folder", id);
        buttons.Add(folder);
        if (Games.Bool(tool, "hasHelp"))
        {
            var help = new Button { Content = Loc.T("#L_TOOL_HELP") };
            help.Click += async (_, _) => await Run("tool-open-help", id);
            buttons.Add(help);
        }
        var remove = new Button { Content = Loc.T("#L_TOOL_REMOVE") };
        remove.Click += async (_, _) =>
        {
            if (await Dialogs.Confirm(Loc.T("#L_TOOL_REMOVE"), Loc.T("#L_TOOL_REMOVE_TEXT"), Loc.T("#L_TOOL_REMOVE")))
                await Run("tool-remove", id);
        };
        buttons.Add(remove);
        form.Row(buttons.ToArray());
        await Task.CompletedTask;
        return form.Panel;
    }

    private async Task Run(string channel, params object?[] args)
    {
        try
        {
            var result = await Backend.InvokeAsync(channel, args);
            if (Games.Str(result, "status") == "error" || result?["error"] is JsonObject)
            {
                var error = result?["error"];
                var code = error is JsonObject ? Games.Str(error, "code") : Games.Str(result, "code");
                await Dialogs.Message("LiveSplit", Loc.TOr("#L_TOOL_ERR_" + code, code));
            }
        }
        catch (BackendException ex)
        {
            await Dialogs.Message("LiveSplit", ex.Error);
        }
        await Load();
    }
}
