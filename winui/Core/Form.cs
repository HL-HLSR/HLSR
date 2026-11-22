using System.Text.Json.Nodes;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace HLSR.Core;

/// <summary>
/// Формы в духе «Параметров» Windows 10: заголовок раздела (20 px), под ним контролы с
/// подписью сверху, переключатели «Вкл./Откл.», пояснения мелким серым текстом.
/// Значения читаются и пишутся в узел JSON (<see cref="JsonObject"/>).
/// </summary>
public sealed class Form
{
    public StackPanel Panel { get; } = new() { Spacing = 8, MaxWidth = 640, HorizontalAlignment = HorizontalAlignment.Left };

    /// <summary>Что-то поменяли: родитель включает «Сохранить».</summary>
    public event Action? Changed;

    private void Notify() => Changed?.Invoke();

    public static Style Style(string key) => (Style)Application.Current.Resources[key];

    public static string OnText => Loc.Code == "ru" ? "Вкл." : "On";
    public static string OffText => Loc.Code == "ru" ? "Откл." : "Off";

    public TextBlock Header(string text, bool first = false)
    {
        var block = new TextBlock { Text = text, Style = Style("W10SubtitleText"), Margin = new Thickness(0, first ? 0 : 24, 0, 4) };
        Panel.Children.Add(block);
        return block;
    }

    public TextBlock Caption(string text)
    {
        var block = new TextBlock { Text = text, Style = Style("W10CaptionText") };
        Panel.Children.Add(block);
        return block;
    }

    public TextBlock Body(string text)
    {
        var block = new TextBlock { Text = text, Style = Style("W10BodyText") };
        Panel.Children.Add(block);
        return block;
    }

    public T Add<T>(T element) where T : UIElement
    {
        Panel.Children.Add(element);
        return element;
    }

    private static JsonNode? Get(JsonObject node, string key) => node[key];

    public ToggleSwitch Toggle(JsonObject node, string key, string label, string? hint = null, Action<bool>? after = null)
    {
        var toggle = new ToggleSwitch
        {
            Header = label,
            OnContent = OnText,
            OffContent = OffText,
            IsOn = Get(node, key) is JsonValue v && v.TryGetValue<bool>(out var b) && b,
        };
        toggle.Toggled += (_, _) =>
        {
            node[key] = toggle.IsOn;
            after?.Invoke(toggle.IsOn);
            Notify();
        };
        Panel.Children.Add(toggle);
        if (!string.IsNullOrEmpty(hint)) Caption(hint).Margin = new Thickness(0, -6, 0, 4);
        return toggle;
    }

    /// <summary>Выпадающий список; <paramref name="items"/> — (значение JSON, подпись).</summary>
    public ComboBox Combo(JsonObject node, string key, string label, IEnumerable<(JsonNode? Value, string Label)> items, Action? after = null)
    {
        var box = new ComboBox { Header = label, MinWidth = 296 };
        var current = Get(node, key)?.ToJsonString() ?? "null";
        foreach (var (value, text) in items)
        {
            var item = new ComboBoxItem { Content = text, Tag = value?.DeepClone() };
            box.Items.Add(item);
            if ((value?.ToJsonString() ?? "null") == current) box.SelectedItem = item;
        }
        if (box.SelectedItem is null && box.Items.Count > 0) box.SelectedIndex = 0;
        box.SelectionChanged += (_, _) =>
        {
            if (box.SelectedItem is ComboBoxItem item)
            {
                node[key] = (item.Tag as JsonNode)?.DeepClone();
                after?.Invoke();
                Notify();
            }
        };
        Panel.Children.Add(box);
        return box;
    }

    /// <summary>Ползунок Windows 10 с подписью и значением справа.</summary>
    public Slider Slider(JsonObject node, string key, string label, double min, double max, double step = 1, Func<double, string>? format = null)
    {
        double value = Get(node, key) is JsonValue v && v.TryGetValue<double>(out var d) ? d : min;
        var slider = new Slider
        {
            Minimum = min,
            Maximum = max,
            StepFrequency = step,
            SmallChange = step,
            Value = Math.Clamp(value, min, max),
            Width = 296,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        var title = new TextBlock { Text = label, FontSize = 15 };
        var shown = new TextBlock { Style = Style("W10CaptionText"), FontSize = 14, Margin = new Thickness(12, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };
        string Show(double x) => format?.Invoke(x) ?? (step < 1 ? x.ToString("0.##") : x.ToString("0"));
        shown.Text = Show(slider.Value);
        slider.ValueChanged += (_, e) =>
        {
            shown.Text = Show(e.NewValue);
            node[key] = step < 1 ? Math.Round(e.NewValue, 2) : Math.Round(e.NewValue);
            Notify();
        };
        var row = new StackPanel { Orientation = Orientation.Horizontal };
        row.Children.Add(slider);
        row.Children.Add(shown);
        Panel.Children.Add(new StackPanel { Spacing = 0, Children = { title, row } });
        return slider;
    }

    public TextBox Text(JsonObject node, string key, string label, string? placeholder = null, double width = 420)
    {
        var box = new TextBox
        {
            Header = label,
            Text = Get(node, key)?.ToString() ?? "",
            PlaceholderText = placeholder ?? "",
            Width = width,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        box.TextChanged += (_, _) =>
        {
            node[key] = box.Text;
            Notify();
        };
        Panel.Children.Add(box);
        return box;
    }

    /// <summary>Цвет <c>{r, g, b}</c>: образец + кнопка с ColorPicker Windows 10 во всплывающем окне.</summary>
    public Button Color(JsonObject node, string key, string label)
    {
        Windows.UI.Color Read()
        {
            var c = Get(node, key) as JsonObject;
            byte Ch(string n) => (byte)Math.Clamp((int)Games.Num(c, n), 0, 255);
            return Windows.UI.Color.FromArgb(255, Ch("r"), Ch("g"), Ch("b"));
        }
        var swatch = new Border { Width = 24, Height = 24, BorderThickness = new Thickness(1), BorderBrush = (Brush)Application.Current.Resources["W10BaseMediumBrush"] };
        var hex = new TextBlock { VerticalAlignment = VerticalAlignment.Center, FontFamily = new FontFamily("Consolas") };
        void Show(Windows.UI.Color color)
        {
            swatch.Background = new SolidColorBrush(color);
            hex.Text = $"#{color.R:X2}{color.G:X2}{color.B:X2}";
        }
        Show(Read());
        var picker = new ColorPicker
        {
            Color = Read(),
            IsAlphaEnabled = false,
            IsMoreButtonVisible = false,
            IsColorSliderVisible = true,
            IsColorChannelTextInputVisible = true,
            IsHexInputVisible = true,
        };
        picker.ColorChanged += (_, e) =>
        {
            node[key] = new JsonObject { ["r"] = e.NewColor.R, ["g"] = e.NewColor.G, ["b"] = e.NewColor.B };
            Show(e.NewColor);
            Notify();
        };
        var button = new Button
        {
            Content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10, Children = { swatch, hex } },
            Flyout = new Flyout { Content = picker },
        };
        var title = new TextBlock { Text = label, FontSize = 15 };
        Panel.Children.Add(new StackPanel { Spacing = 6, Children = { title, button } });
        return button;
    }

    public Button Button(string text, Action onClick, bool accent = false)
    {
        var button = new Button { Content = text, MinWidth = 120 };
        if (accent) button.Style = Style("AccentButtonStyle");
        button.Click += (_, _) => onClick();
        Panel.Children.Add(button);
        return button;
    }

    public StackPanel Row(params UIElement[] children)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        foreach (var child in children) row.Children.Add(child);
        Panel.Children.Add(row);
        return row;
    }

    /// <summary>Строка списка Windows 10: иконка, две строки текста, справа — действия.</summary>
    public static Grid ListRow(string glyph, string title, string? subtitle, params UIElement[] actions)
    {
        var grid = new Grid { ColumnSpacing = 14, Padding = new Thickness(12, 8, 8, 8) };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var icon = new FontIcon { Glyph = glyph, FontSize = 20, VerticalAlignment = VerticalAlignment.Center };
        var text = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
        text.Children.Add(new TextBlock { Text = title, FontSize = 15, TextTrimming = TextTrimming.CharacterEllipsis });
        if (!string.IsNullOrEmpty(subtitle))
            text.Children.Add(new TextBlock { Text = subtitle, Style = Style("W10CaptionText"), TextTrimming = TextTrimming.CharacterEllipsis, TextWrapping = TextWrapping.NoWrap });
        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        foreach (var a in actions) buttons.Children.Add(a);
        Grid.SetColumn(text, 1);
        Grid.SetColumn(buttons, 2);
        grid.Children.Add(icon);
        grid.Children.Add(text);
        grid.Children.Add(buttons);
        return grid;
    }

    /// <summary>Кнопка-иконка Windows 10 (как в списках «Параметров»).</summary>
    public static Button IconButton(string glyph, string tip, Action onClick)
    {
        var button = new Button { Content = new FontIcon { Glyph = glyph, FontSize = 16 }, Width = 40, Height = 36, Padding = new Thickness(0) };
        ToolTipService.SetToolTip(button, tip);
        button.Click += (_, _) => onClick();
        return button;
    }

    public static TextBlock Bold(string text) => new() { Text = text, FontSize = 15, FontWeight = FontWeights.SemiBold };
}
