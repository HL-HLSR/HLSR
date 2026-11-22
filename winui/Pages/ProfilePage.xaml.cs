using System.Text.Json.Nodes;
using HLSR.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;

namespace HLSR.Pages;

/// <summary>Профиль HLSR: вход через Steam, часы и запуски, достижения, друзья.</summary>
public sealed partial class ProfilePage : Page
{
    private readonly StackPanel _content = new() { Spacing = 8, MaxWidth = 760, HorizontalAlignment = HorizontalAlignment.Left };
    private readonly List<IDisposable> _events = new();

    public ProfilePage()
    {
        InitializeComponent();
        PageTitle.Text = Loc.T("#L_RUNNER_TITLE");
        Body.Children.Add(_content);
    }

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        _events.Add(Backend.On("runner-profile-state", ev => { _ = Load(); }));
        _events.Add(Backend.On("runner-login-success", ev => { _ = Load(); }));
        await Load();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        foreach (var ev in _events) ev.Dispose();
        _events.Clear();
    }

    private async Task Load()
    {
        JsonNode? state, friends;
        try
        {
            state = await Backend.InvokeAsync("runner-state");
            friends = await Backend.InvokeAsync("friends-list");
        }
        catch (BackendException ex)
        {
            _content.Children.Clear();
            _content.Children.Add(new TextBlock { Text = ex.Error });
            return;
        }
        _content.Children.Clear();
        var form = new Form();
        _content.Children.Add(form.Panel);

        bool signedIn = Games.Bool(state?["account"], "signedIn");
        var status = Games.Str(state, "status");
        form.Header(Loc.Code == "ru" ? "Учётная запись" : "Account", first: true);
        if (signedIn)
        {
            var nick = Games.Str(state, "nick");
            form.Add(Form.ListRow("", nick.Length > 0 ? nick : Games.Str(state?["account"], "personaName"),
                Games.Str(state?["account"], "steamId64")));
            form.Row(
                Btn(Loc.T("#L_RUNNER_LOGOUT_SHORT"), "runner-logout"),
                Btn(Loc.T("#L_RUNNER_LOGOUT_ALL_SHORT"), "runner-logout-all"));
            form.Caption(Loc.T("#L_RUNNER_LOGOUT_HINT"));
        }
        else if (status is "pending" or "waiting" or "logging-in")
        {
            form.Body(Loc.Code == "ru" ? "Подтвердите вход в браузере…" : "Confirm sign-in in your browser…");
            form.Add(new ProgressBar { IsIndeterminate = true, Width = 296, HorizontalAlignment = HorizontalAlignment.Left });
            form.Add(Btn(Loc.T("#L_CANCEL"), "runner-login-cancel"));
        }
        else
        {
            form.Body(Loc.Code == "ru"
                ? "Войдите, чтобы сохранять часы и достижения на сервере HLSR и видеть друзей."
                : "Sign in to keep your hours and achievements on the HLSR server and see your friends.");
            form.Add(Btn(Loc.T("#L_RUNNER_LOGIN"), "runner-login", true, accent: true));
            if (Games.Str(state, "error").Length > 0) form.Caption(Games.Str(state, "error"));
        }

        // Статистика.
        var stats = state?["stats"];
        form.Header(Loc.Code == "ru" ? "Статистика" : "Statistics");
        var hours = TimeSpan.FromSeconds(Games.Num(stats, "totalSeconds"));
        form.Add(Form.ListRow("", Loc.Code == "ru" ? "Время в играх" : "Time played", $"{(int)hours.TotalHours} ч {hours.Minutes} мин".Replace("ч", Loc.Code == "ru" ? "ч" : "h").Replace("мин", Loc.Code == "ru" ? "мин" : "min")));
        form.Add(Form.ListRow("", Loc.Code == "ru" ? "Запусков" : "Launches", $"{Games.Num(stats, "launches"):0}"));
        form.Add(Form.ListRow("", Loc.Code == "ru" ? "Игр сыграно" : "Games played", $"{Games.Num(stats, "gamesPlayed"):0}"));
        var streak = state?["streak"];
        if (streak is JsonObject)
            form.Add(Form.ListRow("", Loc.Code == "ru" ? "Дней подряд" : "Day streak", $"{Games.Num(streak, "days"):0} / {Games.Num(streak, "best"):0}"));

        // Достижения.
        var summary = state?["summary"];
        form.Header($"{(Loc.Code == "ru" ? "Достижения" : "Achievements")} · {Games.Num(summary, "unlocked"):0}/{Games.Num(summary, "total"):0}");
        form.Add(new ProgressBar { Width = 420, HorizontalAlignment = HorizontalAlignment.Left, Value = Games.Num(summary, "percent") });
        foreach (var ach in (state?["achievements"] as JsonArray ?? []).OfType<JsonObject>())
        {
            bool unlocked = Games.Bool(ach, "unlocked");
            var title = Games.Str(ach, "title");
            var desc = Games.Str(ach, "desc").Length > 0 ? Games.Str(ach, "desc") : Games.Str(ach, "description");
            var args = (ach["descArgs"] as JsonArray ?? []).Select(a => (object?)a?.ToString()).ToArray();
            var row = Form.ListRow(unlocked ? "" : "", Loc.T(title), desc.Length > 0 ? Loc.T(desc, args) : null);
            row.Opacity = unlocked ? 1 : 0.6;
            form.Add(row);
        }

        // Друзья.
        form.Header(Loc.T("#L_RUNNER_FRIENDS_HLSR"));
        var list = (friends?["friends"] as JsonArray ?? []).OfType<JsonObject>().ToList();
        if (list.Count == 0) form.Caption(Loc.Code == "ru" ? "Пока никого." : "Nobody yet.");
        foreach (var friend in list)
        {
            var name = Games.Str(friend, "nick");
            if (name.Length == 0) name = Games.Str(friend, "personaName");
            var online = friend["presence"]?["online"]?.GetValue<bool>() == true;
            form.Add(Form.ListRow(online ? "" : "", name, online ? Games.Str(friend["presence"], "gameId") : null));
        }
    }

    private Button Btn(string text, string channel, object? arg = null, bool accent = false)
    {
        var button = new Button { Content = text, MinWidth = 120 };
        if (accent) button.Style = Form.Style("AccentButtonStyle");
        button.Click += async (_, _) =>
        {
            try
            {
                var result = arg is null ? await Backend.InvokeAsync(channel) : await Backend.InvokeAsync(channel, arg);
                if (Games.Str(result, "status") == "error")
                    await Dialogs.Message(Loc.T("#L_RUNNER_TITLE"), Games.Str(result?["error"], "code") is { Length: > 0 } c ? c : Games.Str(result, "code"));
            }
            catch (BackendException ex) { await Dialogs.Message(Loc.T("#L_RUNNER_TITLE"), ex.Error); }
            await Load();
        };
        return button;
    }
}
