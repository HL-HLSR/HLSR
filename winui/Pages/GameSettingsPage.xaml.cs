using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>
/// Настройки игры (<c>GameSettingsModal</c> окна GPUI): разделы Pivot — запуск, персонализация,
/// конфиги, клавиатура, демо, призраки, версия; набор зависит от игры (<c>sections.rs</c>).
/// Запуск и клавиатура сохраняются целиком (<c>game-settings-save</c>), персонализация —
/// со <c>scope: "personalization"</c>.
/// </summary>
public sealed partial class GameSettingsPage : Page
{
    private const string Ghosting = "218";
    private const string Hl1 = "70";

    private GameInfo _game = Games.All[0];
    private JsonObject _settings = new();
    private JsonObject? _reply;
    private string _gameId = "";
    private bool _dirty;
    private bool _personalDirty;

    public GameSettingsPage()
    {
        InitializeComponent();
        SaveButton.Content = Loc.T("#L_SAVE");
        ResetButton.Content = Loc.T("#L_RESET");
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _game = Games.Find(e.Parameter as string) ?? Games.All[0];
        PageTitle.Text = $"{Loc.T("#L_GM_SETTINGS")} · {_game.Name}";
        await Load();
    }

    private string Section => (Tabs.SelectedItem as PivotItem)?.Tag as string ?? "";

    private async Task Load(string? keepSection = null)
    {
        keepSection ??= Section;
        try
        {
            var status = await Backend.InvokeAsync("game-delivery-get-status", _game.LauncherId);
            _gameId = Games.Str(status, "gameId");
            _reply = await Backend.InvokeAsync("game-settings-get", _game.LauncherId) as JsonObject;
            _settings = _reply?["settings"]?.DeepClone() as JsonObject ?? new JsonObject();
            _settings["personalization"] ??= new JsonObject();
            _settings["bxt"] ??= new JsonObject();
            _settings["packs"] ??= new JsonObject();
            _settings["binds"] ??= new JsonObject();
            _settings["liveSplitOverlay"] ??= new JsonObject();

            Tabs.Items.Clear();
            bool goldsrc = _game.LauncherId == Hl1, ghosting = _game.LauncherId == Ghosting;
            var presets = await Backend.InvokeAsync("winui-bind-presets", Games.Str(_reply, "bindsGameId"));

            if (goldsrc || ghosting) AddTab("personal", "#L_GS_S_PERSONAL", BuildPersonal());
            AddTab("configs", "#L_GS_S_CONFIGS", await BuildConfigs());
            if (presets is JsonObject catalog) AddTab("keyboard", "#L_GS_S_KEYBOARD", BuildKeyboard(catalog));
            AddTab("launch", "#L_GS_S_LAUNCH", BuildLaunch());
            if (goldsrc || ghosting) AddTab("demos", "#L_GS_S_DEMOS", await BuildDemos());
            if (ghosting) AddTab("ghosts", "#L_GS_S_GHOSTS", await BuildGhosts());
            if (ghosting) AddTab("version", "#L_GS_S_VERSION", BuildVersion(status));

            var keep = Tabs.Items.OfType<PivotItem>().FirstOrDefault(i => (string)i.Tag == keepSection);
            Tabs.SelectedItem = keep ?? Tabs.Items.FirstOrDefault();
            _dirty = _personalDirty = false;
            UpdateFooter();
        }
        catch (BackendException ex)
        {
            FooterStatus.Text = ex.Error;
        }
    }

    private void AddTab(string id, string titleKey, UIElement content)
    {
        Tabs.Items.Add(new PivotItem
        {
            Header = Loc.T(titleKey),
            Tag = id,
            Content = new ScrollViewer
            {
                VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                Content = new Border { Padding = new Thickness(12, 12, 36, 36), Child = content },
            },
        });
    }

    private void MarkDirty(bool personal)
    {
        if (personal) _personalDirty = true; else _dirty = true;
        UpdateFooter();
    }

    private void UpdateFooter()
    {
        bool saves = Section is "launch" or "personal" or "keyboard";
        Footer.Visibility = saves ? Visibility.Visible : Visibility.Collapsed;
        SaveButton.IsEnabled = Section == "personal" ? _personalDirty : _dirty;
        if (SaveButton.IsEnabled) FooterStatus.Text = "";
    }

    private void OnTabChanged(object sender, SelectionChangedEventArgs e) => UpdateFooter();

    private async void OnSave(object sender, RoutedEventArgs e)
    {
        bool personal = Section == "personal";
        try
        {
            var result = await Backend.InvokeAsync("game-settings-save", _game.LauncherId, _settings,
                personal ? "personalization" : null, new JsonObject());
            if (Games.Str(result, "status") is "error")
            {
                var error = result?["error"];
                var code = Games.Str(error, "code");
                var message = Games.Str(error, "message");
                await Dialogs.Message(Loc.T("#L_GM_SETTINGS"), message.Length > 0 ? message : Loc.T("#L_GS_CFG_SAVE_FAILED", code));
                return;
            }
            if (personal) _personalDirty = false; else _dirty = false;
            FooterStatus.Text = Loc.T("#L_GS_SAVED");
            SaveButton.IsEnabled = false;
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(Loc.T("#L_GM_SETTINGS"), ex.Error);
        }
    }

    private async void OnReset(object sender, RoutedEventArgs e)
    {
        if (!await Dialogs.Confirm(Loc.T("#L_RESET"), Loc.T("#L_GS_X_RESET_HINT"), Loc.T("#L_RESET"))) return;
        try
        {
            await Backend.InvokeAsync("game-settings-reset", _game.LauncherId, new JsonObject());
            await Load();
            FooterStatus.Text = Loc.T("#L_GS_SAVED");
        }
        catch (BackendException ex)
        {
            await Dialogs.Message(Loc.T("#L_RESET"), ex.Error);
        }
    }

    // ---- Запуск ------------------------------------------------------------------------------

    private static readonly (string Key, string Label, string Hint)[] SourceToggles =
    [
        ("console", "#L_GS_CONSOLE", "-console"),
        ("novid", "#L_GS_NOVID", "-novid"),
        ("borderless", "#L_GS_BORDERLESS", "-windowed -noborder"),
        ("dx9", "#L_GS_DX9", "-dx9"),
        ("nocrashdialog", "#L_GS_NOCRASHDIALOG", "-nocrashdialog"),
        ("noasync", "#L_GS_NOASYNC", "-noasync"),
        ("noaafonts", "#L_GS_NOAAFONTS", "-noaafonts"),
        ("high", "#L_GS_HIGH", "-high"),
    ];

    private static readonly (string Key, string Label, string Hint)[] GoldSrcToggles =
    [
        ("rinput", "#L_GS_RINPUT", "#L_GS_RINPUT_HINT"),
        ("rinputMenuFix", "#L_GS_RINPUT_MENUFIX", "#L_GS_RINPUT_MENUFIX_HINT"),
        ("windowed", "#L_GS_WINDOWED", "-windowed"),
        ("console", "#L_GS_CONSOLE", "-console"),
        ("developer", "#L_GS_DEVELOPER", "-dev"),
        ("condebug", "#L_GS_CONDEBUG", "-condebug"),
        ("nojoy", "#L_GS_NOJOY", "-nojoy"),
        ("noaafonts", "#L_GS_NOAAFONTS", "-noaafonts"),
    ];

    private static readonly (int W, int H)[] Resolutions =
    [
        (640, 480), (800, 600), (1024, 768), (1280, 960), (1440, 1080), (1280, 720), (1366, 768),
        (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160), (1680, 1050), (1920, 1200),
    ];

    private static readonly int[] RefreshRates = [60, 75, 100, 120, 144, 165, 180, 240, 360];

    private UIElement BuildLaunch()
    {
        var form = new Form();
        form.Changed += () => MarkDirty(false);
        bool goldsrc = _game.LauncherId == Hl1;

        form.Header(Loc.T("#L_GS_S_LAUNCH"), first: true);
        ToggleSwitch? menuFix = null;
        foreach (var (key, label, hint) in goldsrc ? GoldSrcToggles : SourceToggles)
        {
            var text = hint.StartsWith('#') ? Loc.T(hint) : hint;
            var toggle = form.Toggle(_settings, key, Loc.T(label), text,
                key == "rinput" ? on => { if (menuFix is not null) menuFix.IsEnabled = on; } : null);
            if (key == "rinputMenuFix") menuFix = toggle;
        }
        if (menuFix is not null) menuFix.IsEnabled = Games.Bool(_settings, "rinput");
        form.Toggle(_settings, "excludeCpu0", Loc.T("#L_GS_NO_CPU0"), Loc.TOr("#L_GS_NO_CPU0_HINT", ""));
        form.Toggle(_settings, "startTimer", Loc.T("#L_GS_START_TIMER"), Loc.TOr("#L_GS_START_TIMER_HINT", ""));

        var vulkan = _reply?["vulkan"];
        if (vulkan is JsonValue vk && vk.TryGetValue<bool>(out var hasDxvk))
        {
            var toggle = form.Toggle(_settings, "vulkan", Loc.T("#L_GS_VULKAN"), hasDxvk ? Loc.TOr("#L_GS_VULKAN_HINT", "") : Loc.T("#L_GS_VULKAN_MISSING"));
            toggle.IsEnabled = hasDxvk;
        }

        form.Header(Loc.T("#L_GS_RESOLUTION"));
        // Разрешение — пара width/height: отдельный узел-посредник.
        var res = new JsonObject { ["v"] = $"{Games.Num(_settings, "width"):0}x{Games.Num(_settings, "height"):0}" };
        var resItems = new List<(JsonNode?, string)> { ("0x0", Loc.T("#L_GS_RESOLUTION_AUTO")) };
        resItems.AddRange(Resolutions.Select(r => ((JsonNode?)$"{r.W}x{r.H}", $"{r.W} × {r.H}")));
        form.Combo(res, "v", Loc.T("#L_GS_RESOLUTION"), resItems, () =>
        {
            var parts = (res["v"]?.ToString() ?? "0x0").Split('x');
            _settings["width"] = int.Parse(parts[0]);
            _settings["height"] = int.Parse(parts[1]);
        });
        var rates = new List<(JsonNode?, string)> { (0, Loc.T("#L_GS_RESOLUTION_AUTO")) };
        rates.AddRange(RefreshRates.Select(r => ((JsonNode?)r, Loc.T("#L_GS_HZ", r))));
        form.Combo(_settings, "refreshRate", Loc.T("#L_GS_REFRESH_RATE"), rates);

        if (_reply?["languages"] is JsonArray { Count: > 0 } languages)
        {
            var items = new List<(JsonNode?, string)> { ("auto", Loc.T("#L_GS_LANG_AUTO")) };
            items.AddRange(languages.Select(l => ((JsonNode?)l!.ToString(), l!.ToString())));
            form.Combo(_settings, "language", Loc.T("#L_GS_LANG"), items);
        }

        form.Header(Loc.T("#L_GS_ARGS"));
        form.Text(_settings, "launchArgs", Loc.T("#L_GS_ARGS"), "+fps_max 1000");
        return form.Panel;
    }

    // ---- Персонализация ----------------------------------------------------------------------

    private UIElement BuildPersonal()
    {
        var form = new Form();
        form.Changed += () => MarkDirty(true);
        var p = (JsonObject)_settings["personalization"]!;
        bool goldsrc = _game.LauncherId == Hl1;

        form.Header(Loc.T("#L_GS_P_HUD_TITLE"), first: true);
        if (goldsrc) form.Caption(Loc.T("#L_GS_P_HUD_HINT_GOLDSRC"));
        form.Toggle(p, "hudEnabled", Loc.T("#L_GS_P_HUD"));
        form.Color(p, "hudColor", Loc.T("#L_GS_P_PICK"));
        form.Slider(p, "hudAlpha", Loc.T("#L_GS_P_ALPHA"), -1, 255, 1, x => x < 0 ? Loc.T("#L_GS_P_AUTO") : x.ToString("0"));
        form.Toggle(p, "hudAffectsCrosshair", Loc.T("#L_GS_P_AFFECTS_CROSS"));
        form.Toggle(p, "hideQuickInfo", Loc.T("#L_GS_P_HIDE_QUICKINFO"));

        form.Header(Loc.T("#L_GS_P_CROSS_TITLE"));
        form.Toggle(p, "crossEnabled", Loc.T("#L_GS_P_CROSS"));
        form.Combo(p, "crossPreset", Loc.T("#L_GS_P_PRESETS"),
        [
            ("standard", Loc.T("#L_GS_P_PR_STD")), ("dot", Loc.T("#L_GS_P_PR_DOT")), ("classic", Loc.T("#L_GS_P_PR_CLASSIC")),
            ("circle", Loc.T("#L_GS_P_PR_CIRCLE")), ("crossdot", Loc.T("#L_GS_P_PR_CROSSDOT")),
        ]);
        form.Color(p, "crossColor", Loc.T("#L_GS_P_CROSS_COLOR"));
        form.Slider(p, "crossAlpha", Loc.T("#L_GS_P_ALPHA_C"), 0, 255);
        form.Slider(p, "crossSize", Loc.T("#L_GS_P_SIZE"), 0, 50);
        form.Slider(p, "crossGap", Loc.T("#L_GS_P_GAP"), 0, 30);
        form.Slider(p, "crossThickness", Loc.T("#L_GS_P_THICK"), 1, 10);
        form.Slider(p, "crossDotSize", Loc.T("#L_GS_P_DOT"), 0, 10);
        form.Slider(p, "crossCircleRadius", Loc.T("#L_GS_P_CIRCLE"), 0, 30);
        form.Toggle(p, "crossOutline", Loc.T("#L_GS_P_OUTLINE"));
        form.Slider(p, "crossOutlineThickness", Loc.T("#L_GS_P_OUTLINE_THICK"), 1, 3);
        form.Caption(Loc.T("#L_GS_P_LINES"));
        foreach (var (key, label) in new[] { ("crossTopLine", "↑"), ("crossBottomLine", "↓"), ("crossLeftLine", "←"), ("crossRightLine", "→") })
            form.Toggle(p, key, $"{Loc.T("#L_GS_P_LINES")} {label}");
        form.Toggle(p, "crossHideDefault", Loc.T("#L_GS_P_HIDE_DEF"));

        if (goldsrc)
        {
            var bxt = (JsonObject)_settings["bxt"]!;
            form.Header(Loc.T("#L_GS_H_S_HUD"));
            foreach (var (on, x, y, label) in new[]
            {
                ("timer", "timerX", "timerY", "#L_GS_H_TIMER"),
                ("speedometer", "speedometerX", "speedometerY", "#L_GS_H_SPEEDOMETER"),
                ("jumpspeed", "jumpspeedX", "jumpspeedY", "#L_GS_H_JUMPSPEED"),
                ("viewangles", "viewanglesX", "viewanglesY", "#L_GS_H_VIEWANGLES"),
            })
            {
                form.Toggle(bxt, on, Loc.T(label));
                form.Slider(bxt, x, Loc.T("#L_GS_H_POS_X"), 0, 100);
                form.Slider(bxt, y, Loc.T("#L_GS_H_POS_Y"), 0, 100);
            }
            form.Toggle(bxt, "jumpdistance", Loc.T("#L_GS_H_JUMPDISTANCE"));
            form.Slider(bxt, "precision", Loc.T("#L_GS_H_PRECISION"), 0, 6);
            form.Toggle(bxt, "incorrectFps", Loc.T("#L_GS_H_BAD_FPS"));

            form.Header(Loc.T("#L_GS_H_S_VIEW"));
            form.Slider(bxt, "fov", Loc.T("#L_GS_H_FOV"), 60, 130);
            form.Toggle(bxt, "fixWidescreenFov", Loc.T("#L_GS_H_WIDESCREEN"));
            form.Slider(bxt, "viewmodelFov", Loc.T("#L_GS_H_VM_FOV"), 0, 130, 1, x => x == 0 ? Loc.T("#L_GS_P_AUTO") : x.ToString("0"));
            form.Slider(bxt, "viewmodelForward", Loc.T("#L_GS_H_VM_FORWARD"), -10, 10, 0.5);
            form.Slider(bxt, "viewmodelRight", Loc.T("#L_GS_H_VM_RIGHT"), -10, 10, 0.5);
            form.Slider(bxt, "viewmodelUp", Loc.T("#L_GS_H_VM_UP"), -10, 10, 0.5);
            form.Toggle(bxt, "removeViewmodel", Loc.T("#L_GS_H_VM_REMOVE"));
            form.Toggle(bxt, "viewmodelSemitransparent", Loc.T("#L_GS_H_VM_SEMI"));
            form.Toggle(bxt, "viewmodelBobAngled", Loc.T("#L_GS_H_VM_BOB"));
            form.Toggle(bxt, "viewmodelNoIdle", Loc.T("#L_GS_H_VM_NO_IDLE"));
            form.Toggle(bxt, "viewmodelNoEquip", Loc.T("#L_GS_H_VM_NO_EQUIP"));
            form.Toggle(bxt, "hideCrosshair", Loc.T("#L_GS_H_HIDE_CROSS"));

            var packs = (JsonObject)_settings["packs"]!;
            var catalog = _reply?["packs"];
            form.Header(Loc.T("#L_GS_H_S_PACKS"));
            IEnumerable<(JsonNode?, string)> PackItems(string kind) =>
                new (JsonNode?, string)[] { ("default", Loc.T("#L_GS_H_PACK_NONE")) }.Concat(
                    (catalog?["packs"] as JsonArray ?? []).Where(x => Games.Str(x, "kind") == kind)
                        .Select(x => ((JsonNode?)Games.Str(x, "id"), $"{Games.Str(x, "title")} — {Games.Str(x, "author")}")));
            form.Combo(packs, "hud", Loc.T("#L_GS_H_PACK_HUD"), PackItems("hud"));
            form.Combo(packs, "crosshair", Loc.T("#L_GS_H_PACK_CROSS"), PackItems("crosshair"));
            form.Combo(packs, "changerShape", Loc.T("#L_GS_H_CHANGER_SHAPE"),
                (catalog?["changerShapes"] as JsonArray ?? []).Select(s => ((JsonNode?)s!.ToString(), s!.ToString())));
            form.Combo(packs, "changerColor", Loc.T("#L_GS_H_CHANGER_COLOR"),
                (catalog?["changerColors"] as JsonArray ?? []).Select(c => ((JsonNode?)c!.ToString(),
                    Loc.TOr("#L_GS_H_COLOR_" + c!.ToString().ToUpperInvariant(), c.ToString()))));
            if (catalog?["sourcePage"]?.ToString() is { } page)
                form.Add(new HyperlinkButton { Content = "speedrun.com/hl1/resources", NavigateUri = new Uri(page), Padding = new Thickness(0) });
        }

        var overlay = (JsonObject)_settings["liveSplitOverlay"]!;
        form.Header("LiveSplit");
        form.Toggle(overlay, "enabled", Loc.TOr("#L_LSO_ENABLED", Loc.Code == "ru" ? "Таймер LiveSplit поверх игры" : "LiveSplit timer over the game"));
        form.Slider(overlay, "x", Loc.T("#L_GS_H_POS_X"), 0, 100);
        form.Slider(overlay, "y", Loc.T("#L_GS_H_POS_Y"), 0, 100);
        form.Slider(overlay, "height", Loc.Code == "ru" ? "Высота, %" : "Height, %", 10, 100);
        form.Slider(overlay, "opacity", Loc.T("#L_GS_P_ALPHA_C"), 10, 100);
        return form.Panel;
    }

    // ---- Конфиги -----------------------------------------------------------------------------

    private async Task<UIElement> BuildConfigs()
    {
        var root = new Grid { ColumnSpacing = 16, MinHeight = 480 };
        root.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(280) });
        root.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });

        var list = await Backend.InvokeAsync("game-cfg-files-list", _game.LauncherId, null);
        if (Games.Str(list, "status") != "ok")
        {
            var form = new Form();
            form.Body(Games.Str(list, "code") == "NOT_INSTALLED" ? Loc.T("#L_GS_NOT_INSTALLED") : Loc.T("#L_GS_CFG_MISSING"));
            return form.Panel;
        }

        var files = new ListView { SelectionMode = ListViewSelectionMode.Single };
        var player = Games.Str(list, "player");
        foreach (var file in list?["files"] as JsonArray ?? [])
        {
            var path = Games.Str(file, "path");
            files.Items.Add(new ListViewItem
            {
                Tag = path,
                Content = new StackPanel
                {
                    Children =
                    {
                        new TextBlock { Text = path, FontWeight = path == player ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal },
                        new TextBlock { Text = Games.Size(Games.Num(file, "size")).Replace("0 ", "< 1 "), Style = Form.Style("W10CaptionText") },
                    },
                },
            });
        }

        var editor = new TextBox
        {
            AcceptsReturn = true,
            TextWrapping = TextWrapping.NoWrap,
            FontFamily = new FontFamily("Consolas"),
            FontSize = 14,
            MinHeight = 420,
            IsSpellCheckEnabled = false,
        };
        ScrollViewer.SetHorizontalScrollBarVisibility(editor, ScrollBarVisibility.Auto);
        ScrollViewer.SetVerticalScrollBarVisibility(editor, ScrollBarVisibility.Auto);
        var title = Form.Bold("");
        var status = new TextBlock { Style = Form.Style("W10CaptionText"), VerticalAlignment = VerticalAlignment.Center };
        var save = new Button { Content = Loc.T("#L_SAVE"), Style = Form.Style("AccentButtonStyle"), IsEnabled = false, MinWidth = 120 };
        var folder = new Button { Content = Loc.T("#L_GS_OPEN_FOLDER") };
        JsonNode? meta = null;
        string? current = null;
        bool loading = false;

        files.SelectionChanged += async (_, _) =>
        {
            if (files.SelectedItem is not ListViewItem { Tag: string path }) return;
            try
            {
                var file = await Backend.InvokeAsync("game-cfg-file-read", _game.LauncherId, null, path);
                if (Games.Str(file, "status") != "ok") { status.Text = Games.Str(file, "code"); return; }
                loading = true;
                current = path;
                meta = file;
                title.Text = path;
                editor.Text = Games.Str(file, "content");
                save.IsEnabled = false;
                status.Text = "";
                loading = false;
            }
            catch (BackendException ex) { status.Text = ex.Error; }
        };
        editor.TextChanged += (_, _) =>
        {
            if (loading || current is null) return;
            save.IsEnabled = true;
            status.Text = Loc.T("#L_GS_CFG_EDITED", current);
        };
        save.Click += async (_, _) =>
        {
            if (current is null) return;
            var options = new JsonObject
            {
                ["encoding"] = Games.Str(meta, "encoding"),
                ["eol"] = Games.Str(meta, "eol"),
                ["mtime"] = meta?["mtime"]?.DeepClone(),
            };
            try
            {
                var written = await Backend.InvokeAsync("game-cfg-file-write", _game.LauncherId, null, current, editor.Text.Replace("\r", "\n").Replace("\n\n", "\n"), options);
                if (Games.Str(written, "status") != "ok")
                {
                    status.Text = Games.Str(written, "code") == "CONFIG_CHANGED" ? Loc.T("#L_GS_CFG_CHANGED") : Loc.T("#L_GS_CFG_SAVE_FAILED", Games.Str(written, "code"));
                    return;
                }
                if (meta is JsonObject m) m["mtime"] = written?["mtime"]?.DeepClone();
                save.IsEnabled = false;
                status.Text = Loc.T("#L_GS_SAVED");
            }
            catch (BackendException ex) { status.Text = ex.Error; }
        };
        folder.Click += async (_, _) =>
        {
            try { await Backend.InvokeAsync("game-config-open-folder", _game.LauncherId, null); } catch (BackendException) { }
        };

        var left = new StackPanel { Spacing = 8 };
        left.Children.Add(Form.Bold(Loc.T("#L_GS_S_CONFIGS")));
        left.Children.Add(new ScrollViewer { Content = files, MaxHeight = 520 });
        left.Children.Add(folder);
        var right = new StackPanel { Spacing = 8 };
        right.Children.Add(title);
        right.Children.Add(editor);
        right.Children.Add(new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, Children = { save, status } });
        Grid.SetColumn(right, 1);
        root.Children.Add(left);
        root.Children.Add(right);
        if (files.Items.Count > 0) files.SelectedIndex = 0;
        return root;
    }

    // ---- Клавиатура --------------------------------------------------------------------------

    private UIElement BuildKeyboard(JsonObject catalog)
    {
        var form = new Form();
        var binds = (JsonObject)_settings["binds"]!;
        string Label(JsonNode? loc) => Games.Str(loc, Loc.Code);
        var presets = (catalog["presets"] as JsonArray ?? []).OfType<JsonObject>().ToList();
        var categories = (catalog["categories"] as JsonArray ?? []).OfType<JsonObject>().ToList();
        string Describe(string command) =>
            presets.FirstOrDefault(p => string.Equals(Games.Str(p, "command"), command, StringComparison.OrdinalIgnoreCase)) is { } preset
                ? Label(preset["label"]) : "";

        form.Header(Loc.T("#L_GS_S_KEYBOARD"), first: true);
        form.Caption(Loc.Code == "ru"
            ? "Бинды лаунчера дописываются в cfg игры при сохранении. Имена клавиш — как в консоли игры: f, space, mouse4, kp_enter."
            : "Launcher binds are written into the game cfg on save. Key names are as in the game console: f, space, mouse4, kp_enter.");

        var list = new StackPanel { Spacing = 2 };
        void Render()
        {
            list.Children.Clear();
            foreach (var (key, value) in binds.OrderBy(b => b.Key))
            {
                var command = Games.Str(value, "command");
                var subtitle = Describe(command);
                var row = Form.ListRow("", $"{key.ToUpperInvariant()}  →  {command}", subtitle,
                    Form.IconButton("", Loc.T("#L_GS_X_RESET_HINT"), () =>
                    {
                        binds.Remove(key);
                        MarkDirty(false);
                        Render();
                    }));
                row.Background = (Brush)Application.Current.Resources["W10ListLowBrush"];
                list.Children.Add(row);
            }
            if (binds.Count == 0) list.Children.Add(new TextBlock { Text = Loc.Code == "ru" ? "Своих биндов пока нет" : "No launcher binds yet", Style = Form.Style("W10CaptionText") });
        }
        Render();
        form.Add(list);

        form.Header(Loc.Code == "ru" ? "Назначить клавишу" : "Assign a key");
        var keyBox = new TextBox { Header = Loc.Code == "ru" ? "Клавиша" : "Key", Width = 140, PlaceholderText = "f" };
        var category = new ComboBox { Header = Loc.Code == "ru" ? "Категория" : "Category", MinWidth = 200 };
        foreach (var c in categories) category.Items.Add(new ComboBoxItem { Content = Label(c["title"]), Tag = Games.Str(c, "id") });
        var command = new ComboBox { Header = Loc.Code == "ru" ? "Команда" : "Command", MinWidth = 320, IsEditable = true };
        category.SelectionChanged += (_, _) =>
        {
            command.Items.Clear();
            var id = (category.SelectedItem as ComboBoxItem)?.Tag as string;
            foreach (var p in presets.Where(p => Games.Str(p, "category") == id))
                command.Items.Add(new ComboBoxItem { Content = $"{Label(p["label"])}  ·  {Games.Str(p, "command")}", Tag = Games.Str(p, "command") });
        };
        if (category.Items.Count > 0) category.SelectedIndex = 0;
        var add = new Button { Content = Loc.Code == "ru" ? "Назначить" : "Assign", Style = Form.Style("AccentButtonStyle"), VerticalAlignment = VerticalAlignment.Bottom, MinWidth = 120 };
        add.Click += (_, _) =>
        {
            var key = keyBox.Text.Trim().ToLowerInvariant();
            var cmd = (command.SelectedItem as ComboBoxItem)?.Tag as string ?? command.Text?.Trim();
            if (key.Length == 0 || string.IsNullOrEmpty(cmd)) return;
            var previous = binds[key] as JsonObject;
            binds[key] = new JsonObject { ["command"] = cmd, ["base"] = Games.Str(previous, "base") };
            keyBox.Text = "";
            MarkDirty(false);
            Render();
        };
        form.Add(new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children = { keyBox, category } });
        form.Add(new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children = { command, add } });
        return form.Panel;
    }

    // ---- Демо --------------------------------------------------------------------------------

    private async Task<UIElement> BuildDemos()
    {
        var form = new Form();
        var reply = await Backend.InvokeAsync("game-demos-list", _game.LauncherId);
        form.Header(Loc.T("#L_GS_S_DEMOS"), first: true);
        if (Games.Str(reply, "status") != "ok")
        {
            form.Body(Loc.T("#L_GS_NOT_INSTALLED"));
            return form.Panel;
        }
        var demos = (reply?["demos"] as JsonArray ?? []).OfType<JsonObject>().OrderByDescending(d => Games.Num(d, "mtimeMs")).ToList();
        form.Caption(Loc.T("#L_GS_DEMOS_COUNT", demos.Count));
        var list = new ListView { SelectionMode = ListViewSelectionMode.Multiple, MaxHeight = 460 };
        foreach (var demo in demos)
        {
            var when = DateTimeOffset.FromUnixTimeMilliseconds((long)Games.Num(demo, "mtimeMs")).LocalDateTime;
            list.Items.Add(new ListViewItem
            {
                Tag = demo,
                Content = Form.ListRow("", Games.Str(demo, "name"), $"{Games.Str(demo, "dir")} · {Games.Size(Games.Num(demo, "size"))} · {when:g}"),
            });
        }
        form.Add(list);
        var pack = new Button { Content = Loc.Code == "ru" ? "Упаковать выбранные в ZIP" : "Pack selected into ZIP", MinWidth = 120 };
        var toTool = new Button { Content = Loc.Code == "ru" ? "Открыть в «Демо»" : "Open in Demo tool" };
        var progress = new ProgressBar { Width = 296, Visibility = Visibility.Collapsed, Maximum = 100 };
        var events = Backend.On("game-demos-pack-progress", e => progress.Value = Games.Num(e, "percent"));
        Unloaded += (_, _) => events.Dispose();
        pack.Click += async (_, _) =>
        {
            var picked = new JsonArray(list.SelectedItems.OfType<ListViewItem>().Select(i => (JsonNode)new JsonObject
            {
                ["dir"] = Games.Str(i.Tag as JsonObject, "dir"),
                ["name"] = Games.Str(i.Tag as JsonObject, "name"),
            }).ToArray());
            if (picked.Count == 0) return;
            progress.Visibility = Visibility.Visible;
            try
            {
                var result = await Backend.InvokeAsync("game-demos-pack", _game.LauncherId, picked);
                if (Games.Str(result, "status") == "ok")
                    await Backend.InvokeAsync("game-demos-archive-reveal", Games.Str(result?["archive"], "name"));
                else
                    await Dialogs.Message(Loc.T("#L_GS_S_DEMOS"), Games.Str(result, "code"));
            }
            catch (BackendException ex) { await Dialogs.Message(Loc.T("#L_GS_S_DEMOS"), ex.Error); }
            progress.Visibility = Visibility.Collapsed;
        };
        toTool.Click += async (_, _) =>
        {
            var paths = new JsonArray(list.SelectedItems.OfType<ListViewItem>()
                .Select(i => (JsonNode)Path.Combine(Games.Str(i.Tag as JsonObject, "dir"), Games.Str(i.Tag as JsonObject, "name"))).ToArray());
            if (paths.Count == 0) return;
            try { await Backend.InvokeAsync("demo-add-paths", paths); App.Window?.Navigate("demo"); }
            catch (BackendException ex) { await Dialogs.Message(Loc.T("#L_DEMO_TITLE"), ex.Error); }
        };
        form.Row(pack, toTool);
        form.Add(progress);
        return form.Panel;
    }

    // ---- Призраки (Ghosting) -----------------------------------------------------------------

    private async Task<UIElement> BuildGhosts()
    {
        var form = new Form();
        form.Header(Loc.T("#L_GS_S_GHOSTS"), first: true);
        var reply = await Backend.InvokeAsync("game-runs-list", _game.LauncherId);
        var status = Games.Str(reply, "status");
        if (status == "not-installed") { form.Body(Loc.T("#L_GS_NOT_INSTALLED")); return form.Panel; }
        if (status == "error") { form.Body(Loc.T("#L_GS_RUNS_ERROR", Games.Str(reply, "code"))); return form.Panel; }
        var items = (reply?["items"] as JsonArray ?? []).OfType<JsonObject>().ToList();
        form.Caption(Loc.T("#L_GS_GHOSTS_COUNT", items.Count));
        var list = new StackPanel { Spacing = 2 };
        foreach (var item in items)
        {
            var name = Games.Str(item, "name");
            Grid? row = null;
            row = Form.ListRow("", Games.Str(item, "displayName"),
                $"{Games.Size(Games.Num(item, "size"))} · {(DateTimeOffset.TryParse(Games.Str(item, "modifiedAt"), out var at) ? at.LocalDateTime.ToString("g") : "")}",
                Form.IconButton("", Loc.T("#L_GS_REVEAL"), async () =>
                {
                    try { await Backend.InvokeAsync("game-runs-reveal", _game.LauncherId, name); } catch (BackendException) { }
                }),
                Form.IconButton("", Loc.T("#L_GS_GHOST_DELETE"), async () =>
                {
                    if (!await Dialogs.Confirm(Loc.T("#L_GS_GHOST_DELETE"), Loc.T("#L_GS_GHOST_DELETE_CONFIRM"), Loc.T("#L_GS_GHOST_DELETE_YES"))) return;
                    var result = await Backend.InvokeAsync("game-runs-delete", _game.LauncherId, name);
                    if (Games.Str(result, "status") == "error") await Dialogs.Message(Loc.T("#L_GS_S_GHOSTS"), Loc.T("#L_GS_GHOST_DELETE_FAILED"));
                    else list.Children.Remove(row!);
                }));
            row.Background = (Brush)Application.Current.Resources["W10ListLowBrush"];
            list.Children.Add(row);
        }
        form.Add(list);
        form.Button(Loc.T("#L_GS_OPEN_RUNS"), async () =>
        {
            try { await Backend.InvokeAsync("game-runs-open-folder", _game.LauncherId); } catch (BackendException) { }
        });
        return form.Panel;
    }

    // ---- Версия (Ghosting) -------------------------------------------------------------------

    private UIElement BuildVersion(JsonNode? status)
    {
        var form = new Form();
        form.Header(Loc.T("#L_GS_S_VERSION"), first: true);
        var meta = status?["buildMetadata"];
        var grid = new Grid { ColumnSpacing = 32, RowSpacing = 8 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        void Row(string label, string value)
        {
            if (string.IsNullOrEmpty(value)) return;
            int r = grid.RowDefinitions.Count;
            grid.RowDefinitions.Add(new RowDefinition());
            var l = new TextBlock { Text = label, Style = Form.Style("W10CaptionText"), FontSize = 14 };
            var v = new TextBlock { Text = value, FontSize = 14, IsTextSelectionEnabled = true, TextWrapping = TextWrapping.Wrap };
            Grid.SetRow(l, r); Grid.SetRow(v, r); Grid.SetColumn(v, 1);
            grid.Children.Add(l); grid.Children.Add(v);
        }
        Row(Loc.T("#L_GS_V_NAME"), Games.Str(meta, "name").Length > 0 ? Games.Str(meta, "name") : _game.Name);
        Row(Loc.T("#L_GS_V_GAME"), Games.Str(status, "installedVersion"));
        Row(Loc.T("#L_GS_V_BUILD"), Games.Str(meta, "buildId").Length > 0 ? Games.Str(meta, "buildId") : _gameId);
        Row(Loc.T("#L_GS_V_BUILT"), Games.Str(status, "buildDate"));
        Row(Loc.T("#L_GS_V_INSTALLED"), DateTimeOffset.TryParse(Games.Str(status, "installedAt"), out var at) ? at.LocalDateTime.ToString("g") : "");
        Row(Loc.T("#L_GS_V_MINLAUNCHER"), Games.Str(meta, "minLauncherVersion"));
        var trust = Games.Str(status, "metadataState") switch
        {
            "trusted" => Loc.T("#L_GS_TRUST_TRUSTED"),
            "legacy" => Loc.T("#L_GS_TRUST_LEGACY"),
            "repair-required" => Loc.T("#L_GS_TRUST_REPAIR_REQUIRED"),
            "launcher-update-required" => Loc.T("#L_GS_TRUST_LAUNCHER_UPDATE_REQUIRED"),
            "not-installed" => Loc.T("#L_GS_NOT_INSTALLED"),
            var other => other,
        };
        Row(Loc.T("#L_GS_V_TRUST"), trust);
        form.Add(grid);
        return form.Panel;
    }
}
