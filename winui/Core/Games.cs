using System.Text.Json.Nodes;

namespace HLSR.Core;

/// <summary>Игра в навигации (<c>games.config.js</c> и <c>game-availability</c>): порядок колонки 218, 220, 70.</summary>
public sealed record GameInfo(string LauncherId, string Name, string ShortName, string Hero, string Icon, string[] Builds);

public static class Games
{
    public static readonly GameInfo[] All =
    [
        new("218", "Half-Life 2: Ghosting", "HL2 Ghosting", "Assets/Art/hl2-ghosting-bg.png", "Assets/Icons/hl2.png", []),
        new("220", "Half-Life 2", "Half-Life 2", "Assets/Art/hero-hl2.jpg", "Assets/Icons/hl2.png", ["4260", "2707", "5288"]),
        new("70", "Half-Life", "Half-Life", "Assets/Art/hl1-bg.jpg", "Assets/Icons/hl1.png", []),
    ];

    public static GameInfo? Find(string? launcherId) => All.FirstOrDefault(g => g.LauncherId == launcherId);

    /// <summary>Сборка из id доставки: <c>hl2-4260</c> → <c>4260</c>.</summary>
    public static string? BuildOf(string? gameId) =>
        gameId is not null && gameId.StartsWith("hl2-", StringComparison.Ordinal) ? gameId[4..] : null;

    /// <summary>Подпись главной кнопки по <c>mainButton.state</c> (<c>download_bar.rs</c>, <c>BUTTONS</c>).</summary>
    public static (string Key, string Glyph) ButtonStyle(string? state) => state switch
    {
        "ready" => ("#L_B_PLAY", ""),
        "not-installed" => ("#L_B_INSTALL", ""),
        "episode-not-installed" => ("#L_B_INSTALL", ""),
        "episode-paused" or "update-paused" or "download-paused" => ("#L_B_RESUME", ""),
        "unavailable" => ("#L_B_UNAVAILABLE", ""),
        "checking-updates" or "verifying-update" => ("#L_B_CHECKING_UPDATES", ""),
        "update-available" => ("#L_B_UPDATE", ""),
        "downloading-update" or "downloading" => ("#L_B_PAUSE", ""),
        "applying-update" => ("#L_B_APPLYING_UPDATE", ""),
        "update-error" or "error" => ("#L_B_RETRY", ""),
        "starting-game" => ("#L_B_STARTING_GAME", ""),
        "game-running" => ("#L_B_GAME_RUNNING", ""),
        "stopping-game" => ("#L_B_STOPPING_GAME", ""),
        "installing" => ("#L_B_INSTALLING", ""),
        "busy" => ("#L_B_BUSY", ""),
        "repair-required" => ("#L_B_REPAIR", ""),
        "update-launcher-required" => ("#L_B_UPDATE_LAUNCHER", ""),
        _ => ("#L_B_PLAY", ""),
    };

    /// <summary>Причина недоступности → заголовок и пояснение (<c>shell_windows.rs</c>).</summary>
    public static (string Title, string Text) Reason(string? code) => code switch
    {
        "STEAM_NOT_RUNNING" => ("#L_UA_STEAM_NOT_RUNNING", "#L_UA_STEAM_NOT_RUNNING_T"),
        "STEAM_INTEGRATION_UNAVAILABLE" => ("#L_UA_STEAM_INTEGRATION", "#L_UA_STEAM_INTEGRATION_T"),
        "SOURCE_SDK_MISSING" or "SDK_MISSING" => ("#L_UA_SDK_MISSING", "#L_UA_SDK_MISSING_T"),
        "REQUIRED_LICENSE_MISSING" => ("#L_UA_LICENSE", "#L_UA_LICENSE_T"),
        "LICENSE_CHECK_UNAVAILABLE" => ("#L_UA_LICENSE_CHECK", "#L_UA_LICENSE_CHECK_T"),
        "GAME_NOT_INSTALLED" => ("#L_UA_NOT_INSTALLED", "#L_UA_NOT_INSTALLED_T"),
        "DELIVERY_CHANNEL_DISABLED" => ("#L_UA_CHANNEL", "#L_UA_CHANNEL_T"),
        "MANIFEST_UNAVAILABLE" => ("#L_UA_MANIFEST", "#L_UA_MANIFEST_T"),
        "MANIFEST_SIGNATURE_INVALID" => ("#L_UA_SIGNATURE", "#L_UA_SIGNATURE_T"),
        "UPDATE_REQUIRED" => ("#L_UA_UPDATE_REQUIRED", "#L_UA_UPDATE_REQUIRED_T"),
        "GAME_FILES_DAMAGED" => ("#L_UA_DAMAGED", "#L_UA_DAMAGED_T"),
        "LIBRARY_UNAVAILABLE" => ("#L_UA_LIBRARY", "#L_UA_LIBRARY_T"),
        "LIBRARY_READ_ONLY" => ("#L_UA_LIBRARY_RO", "#L_UA_LIBRARY_RO_T"),
        "INSUFFICIENT_DISK_SPACE" => ("#L_UA_SPACE", "#L_UA_SPACE_T"),
        "GAME_PROCESS_ALREADY_RUNNING" => ("#L_UA_RUNNING", "#L_UA_RUNNING_T"),
        "BUILD_METADATA_MISSING" => ("#L_UA_BUILD_META", "#L_UA_BUILD_META_MISSING_T"),
        "BUILD_METADATA_INVALID" => ("#L_UA_BUILD_META", "#L_UA_BUILD_META_INVALID_T"),
        "BUILD_METADATA_MISMATCH" => ("#L_UA_BUILD_META", "#L_UA_BUILD_META_MISMATCH_T"),
        "BUILD_METADATA_UNSUPPORTED" => ("#L_UA_BUILD_META_UNSUPPORTED", "#L_UA_BUILD_META_UNSUPPORTED_T"),
        "BUILD_REQUIRES_NEWER_LAUNCHER" => ("#L_UA_BUILD_NEWER_LAUNCHER", "#L_UA_BUILD_NEWER_LAUNCHER_T"),
        _ => ("#L_UA_UNKNOWN", "#L_UA_UNKNOWN_T"),
    };

    public static string Str(JsonNode? node, string key) =>
        node?[key] is JsonValue v && v.TryGetValue<string>(out var s) ? s : "";

    public static double Num(JsonNode? node, string key) =>
        node?[key] is JsonValue v && v.TryGetValue<double>(out var d) ? d : 0;

    public static bool Bool(JsonNode? node, string key) =>
        node?[key] is JsonValue v && v.TryGetValue<bool>(out var b) && b;

    public static string Size(double bytes)
    {
        if (bytes <= 0) return "0 " + Loc.TOr("#L_MB", "MB");
        double gb = bytes / 1024 / 1024 / 1024;
        return gb >= 1
            ? $"{gb:0.0} {Loc.TOr("#L_GB", "GB")}"
            : $"{bytes / 1024 / 1024:0} {Loc.TOr("#L_MB", "MB")}";
    }

    public static string Eta(double seconds)
    {
        var t = TimeSpan.FromSeconds(Math.Max(0, seconds));
        return t.TotalHours >= 1 ? $"{(int)t.TotalHours}:{t.Minutes:00}:{t.Seconds:00}" : $"{t.Minutes}:{t.Seconds:00}";
    }
}
