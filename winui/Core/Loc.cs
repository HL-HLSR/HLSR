using System.Text.Json;

namespace HLSR.Core;

/// <summary>
/// Строки интерфейса — те же <c>locales/ru.json</c> и <c>en.json</c>, что у лаунчера на Rust;
/// подстановки <c>%0%</c>, <c>%1%</c>…
/// </summary>
public static class Loc
{
    private static Dictionary<string, string> _strings = new();
    private static Dictionary<string, string> _fallback = new();

    /// <summary>Код языка: <c>ru</c> или <c>en</c>.</summary>
    public static string Code { get; private set; } = "ru";

    /// <summary><c>settings.json</c>: <c>config.language</c> — 0 английский, 1 русский.</summary>
    public static void Load(int language)
    {
        Code = language == 0 ? "en" : "ru";
        _strings = Read(Code);
        _fallback = Code == "en" ? _strings : Read("en");
    }

    private static Dictionary<string, string> Read(string code)
    {
        var path = Path.Combine(AppContext.BaseDirectory, "Assets", "Locales", code + ".json");
        var result = new Dictionary<string, string>();
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        foreach (var item in doc.RootElement.EnumerateObject())
        {
            result[item.Name] = item.Value.ValueKind switch
            {
                JsonValueKind.String => item.Value.GetString() ?? "",
                // Варианты по ОС: `{windows, linux}`.
                JsonValueKind.Object when item.Value.TryGetProperty("windows", out var win) => win.GetString() ?? "",
                _ => item.Value.ToString(),
            };
        }
        return result;
    }

    public static string T(string key, params object?[] args)
    {
        if (!_strings.TryGetValue(key, out var text) && !_fallback.TryGetValue(key, out text)) text = key;
        for (int i = 0; i < args.Length; i++) text = text.Replace($"%{i}%", args[i]?.ToString() ?? "");
        return text;
    }

    /// <summary>Подпись, если ключ есть, иначе запасной текст.</summary>
    public static string TOr(string key, string fallback) =>
        _strings.ContainsKey(key) || _fallback.ContainsKey(key) ? T(key) : fallback;
}
