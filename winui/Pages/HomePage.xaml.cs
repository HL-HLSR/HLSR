using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>Главная: объявления HLSR, рекорды, уведомления, онлайн по играм.</summary>
public sealed partial class HomePage : Page
{
    private readonly StackPanel _content = new() { Spacing = 8, MaxWidth = 900, HorizontalAlignment = HorizontalAlignment.Left };

    public HomePage()
    {
        InitializeComponent();
        PageTitle.Text = Loc.T("#L_HOME_TITLE");
        var refresh = new Button { Content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children = { new FontIcon { Glyph = "", FontSize = 14 }, new TextBlock { Text = Loc.T("#L_HOME_REFRESH") } } } };
        refresh.Click += async (_, _) => await Load();
        Body.Children.Add(refresh);
        Body.Children.Add(_content);
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e) => await Load();

    private async Task Load()
    {
        _content.Children.Clear();
        var form = new Form();
        _content.Children.Add(form.Panel);
        form.Caption(Loc.T("#L_HOME_LOADING"));
        JsonNode? news = null, notes = null, online = null;
        try
        {
            news = await Backend.InvokeAsync("public-news-get");
            notes = await Backend.InvokeAsync("notifications-list");
            online = await Backend.InvokeAsync("public-online-get");
        }
        catch (BackendException) { }
        form.Panel.Children.Clear();

        // Уведомления.
        var items = (notes?["items"] as JsonArray ?? []).OfType<JsonObject>().ToList();
        form.Header(Loc.T("#L_N_TITLE"), first: true);
        if (items.Count == 0) form.Caption(Loc.T("#L_N_EMPTY"));
        foreach (var note in items.Take(10))
        {
            var code = Games.Str(note, "code");
            var level = Games.Str(note, "level");
            var glyph = level switch { "error" => "", "warning" => "", _ => "" };
            var when = Games.Num(note, "ts") > 0 ? DateTimeOffset.FromUnixTimeMilliseconds((long)Games.Num(note, "ts")).LocalDateTime.ToString("g") : "";
            var row = Form.ListRow(glyph, Loc.TOr("#L_N_" + code, code), string.Join(" · ", new[] { Games.Str(note, "detail"), when }.Where(s => s.Length > 0)));
            row.Background = (Brush)Application.Current.Resources["W10ListLowBrush"];
            form.Add(row);
        }
        if (items.Count > 0)
        {
            form.Row(
                Btn(Loc.Code == "ru" ? "Прочитать все" : "Mark all read", async () => { await Backend.InvokeAsync("notifications-mark-all-read"); await Load(); }),
                Btn(Loc.Code == "ru" ? "Очистить" : "Clear", async () => { await Backend.InvokeAsync("notifications-clear"); await Load(); }));
        }

        // Онлайн.
        if (online?["games"] is JsonObject { Count: > 0 } games)
        {
            form.Header(Loc.Code == "ru" ? "Сейчас играют" : "Playing now");
            foreach (var (id, count) in games) form.Body($"{id}: {count}");
        }

        // Лента.
        var feed = (news?["items"] as JsonArray ?? []).OfType<JsonObject>().ToList();
        form.Header(Loc.T("#L_HOME_POSTS"));
        if (Games.Str(news, "status") == "error" && feed.Count == 0) form.Caption(Loc.T("#L_HOME_FAILED"));
        else if (Games.Str(news, "status") == "stale") form.Caption(Loc.T("#L_HOME_STALE"));
        foreach (var post in feed.Where(i => Games.Str(i, "kind") == "post"))
            form.Add(Post(post));

        var records = feed.Where(i => Games.Str(i, "kind") is "record" or "run").ToList();
        if (records.Count > 0)
        {
            form.Header(Loc.T("#L_HOME_RECORDS"));
            foreach (var r in records)
            {
                var runners = string.Join(", ", (r["runners"] as JsonArray ?? []).Select(x => x?.ToString()));
                var time = TimeSpan.FromSeconds(Games.Num(r, "seconds"));
                var title = $"{Games.Str(r, "game")} · {Games.Str(r, "category")} {Games.Str(r, "subcategory")}".Trim();
                var row = Form.ListRow(Games.Str(r, "kind") == "record" ? "" : "", title,
                    $"{runners} · {(int)time.TotalHours}:{time.Minutes:00}:{time.Seconds:00}.{time.Milliseconds:000}",
                    Form.IconButton("", Games.Str(r, "url"), () => _ = Windows.System.Launcher.LaunchUriAsync(new Uri(Games.Str(r, "url")))));
                row.Background = (Brush)Application.Current.Resources["W10ListLowBrush"];
                form.Add(row);
            }
        }
    }

    private static Button Btn(string text, Func<Task> click)
    {
        var b = new Button { Content = text };
        b.Click += async (_, _) => { try { await click(); } catch (BackendException) { } };
        return b;
    }

    /// <summary>Объявление: карточка Windows 10 — картинка, заголовок, текст, автор и дата.</summary>
    private static UIElement Post(JsonObject post)
    {
        var card = new StackPanel { Spacing = 6, Padding = new Thickness(16), Background = (Brush)Application.Current.Resources["W10ListLowBrush"] };
        if ((post["images"] as JsonArray)?.FirstOrDefault() is JsonObject image && Uri.TryCreate(Games.Str(image, "url"), UriKind.Absolute, out var uri))
            card.Children.Add(new Image { Source = new BitmapImage(uri), MaxHeight = 280, Stretch = Stretch.UniformToFill, HorizontalAlignment = HorizontalAlignment.Left });
        card.Children.Add(new TextBlock { Text = Games.Str(post, "title"), Style = Form.Style("W10SubtitleText"), TextWrapping = TextWrapping.Wrap });
        var date = DateTimeOffset.TryParse(Games.Str(post, "date"), out var d) ? d.LocalDateTime.ToString("D") : "";
        card.Children.Add(new TextBlock { Text = $"{Games.Str(post, "author")} · {date}", Style = Form.Style("W10CaptionText") });
        // Markdown показывается простым текстом: **жирный** и ссылки без разметки.
        var body = Games.Str(post, "body").Replace("**", "").Replace("__", "");
        card.Children.Add(new TextBlock { Text = body, Style = Form.Style("W10BodyText"), IsTextSelectionEnabled = true });
        return card;
    }
}
