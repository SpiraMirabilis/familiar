using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json.Nodes;
using Microsoft.UI.Dispatching;

namespace Familiar;

public sealed class CoreException(string message) : Exception(message);

/// <summary>The Rust engine (familiar_bridge.dll). Everything but <see cref="CallAnywhere"/>
/// must be used on the UI thread.</summary>
internal static unsafe partial class Core
{
    [LibraryImport("familiar_bridge")]
    private static partial void familiar_init(delegate* unmanaged[Cdecl]<ulong, void> wake);

    [LibraryImport("familiar_bridge", StringMarshalling = StringMarshalling.Utf8)]
    private static partial nint familiar_call(string method, string? args);

    [LibraryImport("familiar_bridge")]
    private static partial void familiar_free(nint reply);

    private static DispatcherQueue? queue;

    /// <summary>Raised on the UI thread when a session has network traffic or a timer waiting.</summary>
    public static event Action<long>? Wake;

    /// <summary>Call once, on the UI thread, before anything else.</summary>
    public static void Start()
    {
        queue = DispatcherQueue.GetForCurrentThread();
        familiar_init(&OnWake);
    }

    // The engine calls this from its network threads.
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    private static void OnWake(ulong session)
    {
        queue?.TryEnqueue(() => Wake?.Invoke((long)session));
    }

    private static JsonNode? Run(string method, JsonNode? args)
    {
        var raw = familiar_call(method, args?.ToJsonString());
        if (raw == 0) throw new CoreException($"{method}: no reply");
        try
        {
            var reply = JsonNode.Parse(Marshal.PtrToStringUTF8(raw)!)!;
            if (reply["error"] is { } error) throw new CoreException(error.GetValue<string>());
            // Detached, so it can be put into another document (the config, say).
            return reply["ok"]?.DeepClone();
        }
        finally
        {
            familiar_free(raw);
        }
    }

    public static JsonNode? Call(string method, JsonNode? args = null)
    {
        if (queue is not null && !queue.HasThreadAccess) throw new CoreException($"{method} must be called on the UI thread");
        return Run(method, args);
    }

    /// <summary>For calls that can't sensibly fail (a session we know is open). A failure is a
    /// bug, so it's logged rather than shown.</summary>
    public static JsonNode? Send(string method, JsonNode? args = null)
    {
        try
        {
            return Call(method, args);
        }
        catch (CoreException e)
        {
            System.Diagnostics.Debug.WriteLine($"Familiar: {method} failed: {e.Message}");
            return null;
        }
    }

    /// <summary>The few methods the engine allows on any thread (media.fetch, directory,
    /// ansi.parse, address.parse, code.*, rules.*, keys.parse).</summary>
    public static JsonNode? CallAnywhere(string method, JsonNode? args = null) => Run(method, args);
}
