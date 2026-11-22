using System.Text.Json.Nodes;
using HLSR.Core;
using HLSR.Pages;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Animation;
using Windows.Graphics;

namespace HLSR;

public sealed partial class MainWindow : Window
{
    private readonly IntPtr _hwnd;
    private bool _focused = true;
    private bool _closing;
    private readonly Tray _tray;

    public MainWindow(string? startupError)
    {
        InitializeComponent();
        _hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(TitleBar);
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "hlsr.ico"));
        AppWindow.Resize(new SizeInt32(1180, 760));
        Win10Caption();
        Root.ActualThemeChanged += (_, _) => Win10Caption();

        PaneTitle.Text = "HLSR";
        NavFolder.Content = Loc.Code == "ru" ? "Папка с играми" : "Games folder";
        NavigationViewItem Item(string tag, string text, IconElement icon) =>
            new() { Content = text, Tag = tag, Icon = icon };
        FontIcon Glyph(string glyph) => new() { Glyph = glyph };
        Nav.MenuItems.Add(Item("home", Loc.Code == "ru" ? "Главная" : "Home", Glyph("")));
        Nav.MenuItems.Add(new NavigationViewItemHeader { Content = Loc.Code == "ru" ? "Игры" : "Games" });
        foreach (var game in Games.All)
            Nav.MenuItems.Add(Item(game.LauncherId, game.ShortName,
                new BitmapIcon { UriSource = new Uri("ms-appx:///" + game.Icon), ShowAsMonochrome = false }));
        Nav.MenuItems.Add(new NavigationViewItemSeparator());
        Nav.MenuItems.Add(Item("tools", Loc.Code == "ru" ? "Инструменты" : "Tools", Glyph("")));
        Nav.MenuItems.Add(Item("demo", Loc.T("#L_DEMO_TITLE"), Glyph("")));
        Nav.MenuItems.Add(Item("profile", Loc.Code == "ru" ? "Профиль" : "Profile", Glyph("")));
        ContentFrame.Navigated += (_, _) =>
        {
            Nav.IsBackButtonVisible = ContentFrame.CanGoBack ? NavigationViewBackButtonVisible.Visible : NavigationViewBackButtonVisible.Collapsed;
        };
        Nav.BackRequested += (_, _) =>
        {
            if (ContentFrame.CanGoBack) ContentFrame.GoBack();
        };

        // Пункт настроек NavigationView создаёт при загрузке и подписывает языком ОС.
        Nav.Loaded += (_, _) =>
        {
            if (Nav.SettingsItem is NavigationViewItem settings)
            {
                settings.Content = Loc.T("#UI_SETTINGS");
                // Анимированная шестерёнка — из Windows 11; в Windows 10 — глиф MDL2 E713.
                settings.Icon = new FontIcon { Glyph = "\uE713", FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Segoe MDL2 Assets") };
                Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(settings, Loc.T("#UI_SETTINGS"));
            }
        };

        if (startupError is not null)
        {
            Nav.Visibility = Visibility.Collapsed;
            StartupFailure.Visibility = Visibility.Visible;
            StartupFailureText.Text = startupError;
        }
        else
        {
            Nav.SelectedItem = Nav.MenuItems.OfType<NavigationViewItem>().First(i => (string)i.Tag == Games.All[0].LauncherId);
        }

        _tray = new Tray(_hwnd, Reveal, Close);
        Backend.HostCommand += OnHostCommand;
        Activated += (_, e) =>
        {
            _focused = e.WindowActivationState != WindowActivationState.Deactivated;
            ReportState();
        };
        AppWindow.Changed += (_, e) =>
        {
            if (e.DidPresenterChange || e.DidVisibilityChange || e.DidSizeChange) ReportState();
        };
        Closed += (_, _) =>
        {
            _closing = true;
            Backend.HostCommand -= OnHostCommand;
            Backend.WindowState(IntPtr.Zero, false, false);
            _tray.Dispose();
            Backend.Shutdown();
        };
        ReportState();
    }

    /// <summary>Кнопки заголовка в цветах Windows 10: прозрачные, наведение — BaseLow.</summary>
    private void Win10Caption()
    {
        var bar = AppWindow.TitleBar;
        bool dark = Root.ActualTheme == ElementTheme.Dark;
        var fg = dark ? Colors.White : Colors.Black;
        bar.ButtonBackgroundColor = Colors.Transparent;
        bar.ButtonInactiveBackgroundColor = Colors.Transparent;
        bar.ButtonForegroundColor = fg;
        bar.ButtonHoverBackgroundColor = dark ? ColorHelper.FromArgb(0x33, 0xFF, 0xFF, 0xFF) : ColorHelper.FromArgb(0x1A, 0, 0, 0);
        bar.ButtonHoverForegroundColor = fg;
        bar.ButtonPressedBackgroundColor = dark ? ColorHelper.FromArgb(0x66, 0xFF, 0xFF, 0xFF) : ColorHelper.FromArgb(0x33, 0, 0, 0);
        bar.ButtonPressedForegroundColor = fg;
        bar.ButtonInactiveForegroundColor = dark ? ColorHelper.FromArgb(0x66, 0xFF, 0xFF, 0xFF) : ColorHelper.FromArgb(0x66, 0, 0, 0);
    }

    private bool Minimized => AppWindow.Presenter is OverlappedPresenter p && p.State == OverlappedPresenterState.Minimized;

    private void ReportState()
    {
        if (_closing) return;
        Backend.WindowState(_hwnd, AppWindow.IsVisible && !Minimized, _focused);
    }

    private bool _syncingNav;

    private void OnNavSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (_syncingNav) return;
        if (args.IsSettingsSelected)
        {
            ContentFrame.Navigate(typeof(SettingsPage), null, new EntranceNavigationTransitionInfo());
            return;
        }
        if (args.SelectedItem is NavigationViewItem { Tag: string id }) Open(id);
    }

    private void Open(string id)
    {
        var transition = new EntranceNavigationTransitionInfo();
        switch (id)
        {
            case "home": ContentFrame.Navigate(typeof(HomePage), null, transition); break;
            case "tools": ContentFrame.Navigate(typeof(ToolsPage), null, transition); break;
            case "demo": ContentFrame.Navigate(typeof(DemoPage), null, transition); break;
            case "profile": ContentFrame.Navigate(typeof(ProfilePage), null, transition); break;
            default:
                ContentFrame.Navigate(typeof(GamePage), id, transition);
                Backend.InvokeAsync("game-delivery-selected", id).ContinueWith(_ => { });
                break;
        }
    }

    /// <summary>Перейти к разделу навигации (<c>home</c>, <c>demo</c>, id игры…).</summary>
    public void Navigate(string id)
    {
        var item = Nav.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(i => (string)i.Tag == id);
        if (item is not null) Nav.SelectedItem = item;
    }

    /// <summary>Открыть страницу внутри раздела (настройки игры): «Назад» вернёт обратно.</summary>
    public void OpenPage(Type page, object? parameter) =>
        ContentFrame.Navigate(page, parameter, new DrillInNavigationTransitionInfo());

    private async void OnOpenGamesFolder(object sender, Microsoft.UI.Xaml.Input.TappedRoutedEventArgs e)
    {
        try { await Backend.InvokeAsync("open-games-folder"); }
        catch (BackendException) { }
    }

    private void Reveal()
    {
        _tray.HideIcon();
        if (AppWindow.Presenter is OverlappedPresenter p && p.State == OverlappedPresenterState.Minimized) p.Restore();
        AppWindow.Show();
        Activate();
    }

    private void OnHostCommand(string command, JsonNode? payload)
    {
        switch (command)
        {
            case "reveal":
            case "bring-forward":
            case "show-after-game":
                Reveal();
                break;
            case "hide-for-game":
                // На время игры — в трей: окно прячется, значок возвращает его.
                _tray.ShowIcon();
                AppWindow.Hide();
                break;
            case "destroy-tray":
                _tray.HideIcon();
                break;
            case "window-command":
                var what = payload?.ToString();
                if (AppWindow.Presenter is OverlappedPresenter presenter)
                {
                    if (what == "minimize") presenter.Minimize();
                    else if (what == "maximize") presenter.Maximize();
                    else if (what == "restore") presenter.Restore();
                }
                if (what == "close") Close();
                if (what == "fullscreen") AppWindow.SetPresenter(AppWindowPresenterKind.FullScreen);
                break;
            case "quit":
                Close();
                break;
        }
        ReportState();
    }
}
