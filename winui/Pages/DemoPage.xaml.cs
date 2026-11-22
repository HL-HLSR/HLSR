using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>
/// «Демо»: рендер демок HL1 и Ghosting в видео — FFmpeg и bxt-rs, список демок, параметры
/// экспорта, ход задания, сохранённые видео.
/// </summary>
public sealed partial class DemoPage : Page
{
    private readonly StackPanel _content = new() { Spacing = 8, MaxWidth = 760, HorizontalAlignment = HorizontalAlignment.Left };
    private readonly List<IDisposable> _events = new();
    private readonly ProgressBar _jobProgress = new() { Width = 420, HorizontalAlignment = HorizontalAlignment.Left, Maximum = 100 };
    private readonly TextBlock _jobText = new() { FontSize = 15 };
    private readonly StackPanel _job = new() { Spacing = 6, Visibility = Visibility.Collapsed };
    private string? _jobId;
    private JsonObject _settings = new();

    public DemoPage()
    {
        InitializeComponent();
        PageTitle.Text = Loc.T("#L_DEMO_TITLE");
        Body.Children.Add(_content);
        var cancel = new Button { Content = Loc.T("#L_CANCEL") };
        cancel.Click += async (_, _) => { try { await Backend.InvokeAsync("demo-cancel", _jobId); } catch (BackendException) { } };
        _job.Children.Add(_jobText);
        _job.Children.Add(_jobProgress);
        _job.Children.Add(cancel);
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _events.Add(Backend.On("demo-job-progress", p =>
        {
            _job.Visibility = Visibility.Visible;
            _jobId = Games.Str(p, "jobId").Length > 0 ? Games.Str(p, "jobId") : _jobId;
            _jobProgress.Value = Games.Num(p, "percent");
            _jobText.Text = $"{Loc.T("#L_DEMO_EXPORT_RUNNING")} · {Games.Num(p, "percent"):0}%";
        }));
        _events.Add(Backend.On("demo-job-done", async p =>
        {
            _job.Visibility = Visibility.Collapsed;
            await Dialogs.Message(Loc.T("#L_DEMO_TITLE"), Loc.T("#L_DEMO_EXPORT_DONE_TOAST", Games.Str(p, "name")));
            await Load();
        }));
        _events.Add(Backend.On("demo-job-error", async p =>
        {
            _job.Visibility = Visibility.Collapsed;
            var code = Games.Str(p?["error"], "code");
            if (code != "CANCELLED") await Dialogs.Message(Loc.T("#L_DEMO_TITLE"), Loc.T("#L_DEMO_EXPORT_FAILED_TOAST", Loc.TOr("#L_DEMO_ERR_" + code, code)));
        }));
        _events.Add(Backend.On("demo-ffmpeg-progress", ev => { _ = Load(); }));
        _events.Add(Backend.On("demo-bxtrs-progress", ev => { _ = Load(); }));
        await Load();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        foreach (var ev in _events) ev.Dispose();
        _events.Clear();
    }

    private bool _loading;

    private async Task Load()
    {
        if (_loading) return;
        _loading = true;
        try
        {
            var ffmpeg = (await Backend.InvokeAsync("demo-ffmpeg-status"))?["ffmpeg"];
            var bxtrs = (await Backend.InvokeAsync("demo-bxtrs-status"))?["bxtrs"];
            var list = await Backend.InvokeAsync("demo-list");
            var settings = await Backend.InvokeAsync("demo-settings-get");
            var rendered = await Backend.InvokeAsync("demo-rendered-list");
            _settings = settings?["settings"]?.DeepClone() as JsonObject ?? new JsonObject();

            _content.Children.Clear();
            var form = new Form();
            _content.Children.Add(form.Panel);

            // Компоненты.
            form.Header(Loc.Code == "ru" ? "Компоненты" : "Components", first: true);
            Component(form, ffmpeg, "FFmpeg", Loc.T("#L_DEMO_FFMPEG_TITLE", Games.Str(ffmpeg, "pinnedVersion")),
                Loc.T("#L_DEMO_FFMPEG_TEXT", Games.Size(Games.Num(ffmpeg, "sizeBytes"))), Loc.T("#L_DEMO_FFMPEG_INSTALL"), "demo-ffmpeg-install");
            Component(form, bxtrs, "bxt-rs", Loc.T("#L_DEMO_BXTRS_TITLE"),
                Loc.T("#L_DEMO_BXTRS_TEXT", Games.Size(Games.Num(bxtrs, "sizeBytes"))), Loc.T("#L_DEMO_BXTRS_INSTALL"), "demo-bxtrs-install");

            // Демки.
            form.Header(Loc.Code == "ru" ? "Демки" : "Demos");
            var items = (list?["items"] as JsonArray ?? []).OfType<JsonObject>().ToList();
            if (items.Count == 0)
            {
                form.Body(Loc.T("#L_DEMO_EMPTY_TITLE"));
                form.Caption(Loc.T("#L_DEMO_EMPTY_TEXT"));
            }
            bool ready = Games.Str(ffmpeg, "state") is "installed" or "ready" or "ok";
            foreach (var item in items)
            {
                var id = Games.Str(item, "id");
                var when = Games.Num(item, "mtimeMs") > 0 ? DateTimeOffset.FromUnixTimeMilliseconds((long)Games.Num(item, "mtimeMs")).LocalDateTime.ToString("g") : "";
                var export = Form.IconButton("", Loc.T("#L_DEMO_EXPORT"), () => _ = Export(id));
                export.IsEnabled = ready && _job.Visibility == Visibility.Collapsed;
                var row = Form.ListRow("", Games.Str(item, "name"), string.Join(" · ", new[] { Games.Str(item, "game"), when }.Where(s => s.Length > 0)),
                    export,
                    Form.IconButton("", Loc.T("#L_DEMO_RENDERED_REVEAL"), async () => await Safe("demo-reveal", id)),
                    Form.IconButton("", Loc.T("#L_DEMO_REMOVE"), async () => { await Safe("demo-remove", id); await Load(); }));
                row.Background = (Brush)Application.Current.Resources["W10ListLowBrush"];
                form.Add(row);
            }
            form.Button(Loc.T("#L_DEMO_ADD"), async () => { await Safe("demo-pick-files"); await Load(); });
            form.Add(_job);

            // Параметры экспорта.
            form.Header(Loc.T("#L_DEMO_EXPORT"));
            form.Combo(_settings, "height", Loc.Code == "ru" ? "Качество" : "Quality",
                new (JsonNode?, string)[] { (720, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "2160p (4K)") });
            form.Combo(_settings, "fps", "FPS", new (JsonNode?, string)[] { (30, "30"), (60, "60"), (120, "120") });
            form.Combo(_settings, "preset", Loc.Code == "ru" ? "Профиль кодирования" : "Encoding preset",
                new (JsonNode?, string)[] { ("quality", Loc.Code == "ru" ? "Качество" : "Quality"), ("balanced", Loc.Code == "ru" ? "Баланс" : "Balanced"), ("small", Loc.Code == "ru" ? "Маленький файл" : "Small file") });
            form.Toggle(_settings, "audio", Loc.Code == "ru" ? "Звук" : "Audio");
            form.Toggle(_settings, "hud", "HUD");
            form.Toggle(_settings, "keyhud", Loc.Code == "ru" ? "Нажатия клавиш" : "Key HUD");
            var outDir = Games.Str(settings, "outputDir");
            form.Caption((Loc.Code == "ru" ? "Папка видео: " : "Video folder: ") + outDir);
            form.Button(Loc.T("#L_DEMO_EX_FOLDER_CHANGE"), async () => { await Safe("demo-output-pick"); await Load(); });

            // Сохранённые видео.
            form.Header(Loc.Code == "ru" ? "Сохранённые видео" : "Saved videos");
            var videos = (rendered?["items"] as JsonArray ?? []).OfType<JsonObject>().ToList();
            if (videos.Count == 0) form.Caption(Loc.T("#L_DEMO_RENDERED_EMPTY_TITLE"));
            foreach (var video in videos)
            {
                var name = Games.Str(video, "name");
                var row = Form.ListRow("", name, Games.Size(Games.Num(video, "size")),
                    Form.IconButton("", Loc.T("#L_DEMO_PREVIEW"), async () => await Safe("demo-rendered-open", name)),
                    Form.IconButton("", Loc.T("#L_DEMO_RENDERED_REVEAL"), async () => await Safe("demo-rendered-reveal", name)),
                    Form.IconButton("", Loc.T("#L_DEMO_RENDERED_DELETE"), async () =>
                    {
                        if (await Dialogs.Confirm(Loc.T("#L_DEMO_RENDERED_DELETE_TITLE"), Loc.T("#L_DEMO_RENDERED_DELETE_TEXT"), Loc.T("#L_DEMO_RENDERED_DELETE_OK")))
                        {
                            await Safe("demo-rendered-delete", name);
                            await Load();
                        }
                    }));
                row.Background = (Brush)Application.Current.Resources["W10ListLowBrush"];
                form.Add(row);
            }
        }
        catch (BackendException ex)
        {
            _content.Children.Add(new TextBlock { Text = ex.Error });
        }
        finally { _loading = false; }
    }

    private void Component(Form form, JsonNode? status, string name, string title, string text, string install, string channel)
    {
        var state = Games.Str(status, "state");
        if (state is "installed" or "ready" or "ok")
        {
            var row = Form.ListRow("", name, Games.Str(status, "version"));
            form.Add(row);
            return;
        }
        form.Body(title);
        form.Caption(text);
        if (state is "installing" or "downloading")
        {
            form.Add(new ProgressBar { Width = 296, HorizontalAlignment = HorizontalAlignment.Left, Value = Games.Num(status, "percent") });
            return;
        }
        if (Games.Bool(status, "installable") || status?["installable"] is null)
            form.Button(install, async () => { await Safe(channel); await Load(); }, accent: true);
    }

    private async Task Export(string id)
    {
        try
        {
            var plan = await Backend.InvokeAsync("demo-plan", id, _settings);
            if (Games.Str(plan, "status") == "error")
            {
                await Dialogs.Message(Loc.T("#L_DEMO_TITLE"), Loc.TOr("#L_DEMO_ERR_" + Games.Str(plan, "code"), Games.Str(plan, "code")));
                return;
            }
            _job.Visibility = Visibility.Visible;
            _jobText.Text = Loc.T("#L_DEMO_EXPORT_RUNNING");
            _jobProgress.Value = 0;
            var started = await Backend.InvokeAsync("demo-export-start", id, _settings);
            _jobId = Games.Str(started, "jobId");
        }
        catch (BackendException ex)
        {
            _job.Visibility = Visibility.Collapsed;
            await Dialogs.Message(Loc.T("#L_DEMO_TITLE"), Loc.TOr("#L_DEMO_ERR_" + ex.Error, ex.Error));
        }
    }

    private static async Task Safe(string channel, params object?[] args)
    {
        try { await Backend.InvokeAsync(channel, args); }
        catch (BackendException ex) { await Dialogs.Message(Loc.T("#L_DEMO_TITLE"), Loc.TOr("#L_DEMO_ERR_" + ex.Error, ex.Error)); }
    }
}
