using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.UI.Dispatching;

namespace HLSR.Core;

/// <summary>Ошибка канала: отклонённый промис <c>ipcRenderer.invoke</c>.</summary>
public sealed class BackendException(string channel, string message) : Exception($"{channel}: {message}")
{
    public string Channel { get; } = channel;
    public string Error { get; } = message;
}

/// <summary>
/// Службы HLSR (Rust, <c>hlsr_ffi.dll</c>): каналы окна как у <c>ipcRenderer</c> —
/// <see cref="InvokeAsync"/>, <see cref="On"/>, <see cref="SendSync"/>, хранилища окна.
/// События приходят из потоков tokio и переносятся в поток окна.
/// </summary>
public static class Backend
{
    private const string Dll = "hlsr_ffi";

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void EventCallback(IntPtr channel, IntPtr payload);

    [DllImport(Dll)] private static extern int hlsr_start(EventCallback callback);
    [DllImport(Dll)] private static extern void hlsr_invoke(ulong id, byte[] channel, byte[] args);
    [DllImport(Dll)] private static extern void hlsr_send(byte[] channel, byte[] args);
    [DllImport(Dll)] private static extern IntPtr hlsr_send_sync(byte[] channel, byte[] args);
    [DllImport(Dll)] private static extern IntPtr hlsr_store_read(byte[] name);
    [DllImport(Dll)] [return: MarshalAs(UnmanagedType.U1)] private static extern bool hlsr_store_write(byte[] name, byte[] data);
    [DllImport(Dll)] private static extern void hlsr_window_state(IntPtr hwnd, [MarshalAs(UnmanagedType.U1)] bool visible, [MarshalAs(UnmanagedType.U1)] bool focused);
    [DllImport(Dll)] private static extern void hlsr_shutdown(int exitCode);
    [DllImport(Dll)] private static extern void hlsr_free_string(IntPtr ptr);

    public const int StartOk = 0;
    public const int StartForwarded = 1;
    public const int StartFailed = 2;

    // Делегат держится, пока жив процесс: иначе GC соберёт его, а Rust позовёт пустоту.
    private static readonly EventCallback Callback = OnNativeEvent;
    private static readonly ConcurrentDictionary<ulong, TaskCompletionSource<JsonNode?>> Pending = new();
    private static readonly ConcurrentDictionary<ulong, string> PendingChannels = new();
    private static readonly Dictionary<string, List<Action<JsonNode?>>> Handlers = new();
    private static long _nextId;
    private static DispatcherQueue? _ui;

    public static string? StartupError { get; private set; }
    public static bool Running { get; private set; }

    /// <summary>Команда окну от служб (<c>host:reveal</c>, <c>host:quit</c>…).</summary>
    public static event Action<string, JsonNode?>? HostCommand;

    public static int Start(DispatcherQueue ui)
    {
        _ui = ui;
        int code = hlsr_start(Callback);
        Running = code == StartOk;
        return code;
    }

    public static void Shutdown()
    {
        if (!Running) return;
        Running = false;
        hlsr_shutdown(0);
        foreach (var pending in Pending.Values) pending.TrySetCanceled();
        Pending.Clear();
    }

    private static byte[] Utf8(string text) => System.Text.Encoding.UTF8.GetBytes(text + "\0");

    private static string ArgsJson(object?[] args)
    {
        var array = new JsonArray();
        foreach (var arg in args)
        {
            array.Add(arg switch
            {
                null => null,
                JsonNode node => node.DeepClone(),
                _ => JsonSerializer.SerializeToNode(arg),
            });
        }
        return array.ToJsonString();
    }

    private static string TakeString(IntPtr ptr)
    {
        if (ptr == IntPtr.Zero) return "null";
        try { return Marshal.PtrToStringUTF8(ptr) ?? "null"; }
        finally { hlsr_free_string(ptr); }
    }

    /// <summary><c>ipcRenderer.invoke(channel, ...args)</c>; ответ — в потоке окна.</summary>
    public static Task<JsonNode?> InvokeAsync(string channel, params object?[] args)
    {
        var id = (ulong)Interlocked.Increment(ref _nextId);
        var tcs = new TaskCompletionSource<JsonNode?>();
        Pending[id] = tcs;
        PendingChannels[id] = channel;
        hlsr_invoke(id, Utf8(channel), Utf8(ArgsJson(args)));
        return tcs.Task;
    }

    /// <summary><c>ipcRenderer.send(channel, ...args)</c>.</summary>
    public static void Send(string channel, params object?[] args) =>
        hlsr_send(Utf8(channel), Utf8(ArgsJson(args)));

    /// <summary><c>ipcRenderer.sendSync(channel, ...args)</c>.</summary>
    public static JsonNode? SendSync(string channel, params object?[] args) =>
        JsonNode.Parse(TakeString(hlsr_send_sync(Utf8(channel), Utf8(ArgsJson(args)))));

    /// <summary><c>store.read(name)</c>: <c>data</c>, если файл есть и разобран.</summary>
    public static JsonObject? StoreRead(string name)
    {
        var reply = JsonNode.Parse(TakeString(hlsr_store_read(Utf8(name))));
        return reply?["valid"]?.GetValue<bool>() == true ? reply["data"] as JsonObject : null;
    }

    public static bool StoreWrite(string name, JsonNode data) =>
        hlsr_store_write(Utf8(name), Utf8(data.ToJsonString()));

    public static void WindowState(IntPtr hwnd, bool visible, bool focused)
    {
        if (Running) hlsr_window_state(hwnd, visible, focused);
    }

    /// <summary><c>ipcRenderer.on(channel, handler)</c>; подписка снимается <c>Dispose</c>.</summary>
    public static IDisposable On(string channel, Action<JsonNode?> handler)
    {
        lock (Handlers)
        {
            if (!Handlers.TryGetValue(channel, out var list)) Handlers[channel] = list = new();
            list.Add(handler);
        }
        return new Subscription(channel, handler);
    }

    private sealed class Subscription(string channel, Action<JsonNode?> handler) : IDisposable
    {
        public void Dispose()
        {
            lock (Handlers)
            {
                if (Handlers.TryGetValue(channel, out var list)) list.Remove(handler);
            }
        }
    }

    private static void OnNativeEvent(IntPtr channelPtr, IntPtr payloadPtr)
    {
        // Строки живут только на время вызова: копируем сразу, в потоке tokio.
        var channel = Marshal.PtrToStringUTF8(channelPtr) ?? "";
        var payloadText = Marshal.PtrToStringUTF8(payloadPtr) ?? "null";
        JsonNode? payload;
        try { payload = JsonNode.Parse(payloadText); }
        catch (JsonException) { payload = null; }

        if (channel == "startup-error") StartupError = payload?.ToString();
        _ui?.TryEnqueue(() => Dispatch(channel, payload));
    }

    private static void Dispatch(string channel, JsonNode? payload)
    {
        if (channel == "reply")
        {
            var id = payload?["id"]?.GetValue<ulong>() ?? 0;
            if (!Pending.TryRemove(id, out var tcs)) return;
            PendingChannels.TryRemove(id, out var name);
            if (payload?["ok"]?.GetValue<bool>() == true) tcs.TrySetResult(payload["value"]);
            else tcs.TrySetException(new BackendException(name ?? "?", payload?["error"]?.ToString() ?? "error"));
            return;
        }
        if (channel.StartsWith("host:", StringComparison.Ordinal))
        {
            HostCommand?.Invoke(channel[5..], payload);
            return;
        }
        Action<JsonNode?>[] handlers;
        lock (Handlers)
        {
            handlers = Handlers.TryGetValue(channel, out var list) ? list.ToArray() : [];
        }
        foreach (var handler in handlers)
        {
            try { handler(payload); }
            catch (Exception e) { System.Diagnostics.Debug.WriteLine($"[{channel}] {e}"); }
        }
    }
}
