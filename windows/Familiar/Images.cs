using System.Runtime.InteropServices.WindowsRuntime;
using Microsoft.Graphics.Canvas;
using Windows.Storage.Streams;

namespace Familiar;

/// <summary>Downloads and remembers pictures (avatars, inline images). UI thread only.</summary>
internal static class Images
{
    private const int MaxCached = 256;
    private const int MaxBytes = 20 * 1024 * 1024;
    private static readonly Dictionary<string, CanvasBitmap?> Cache = [];
    private static readonly Queue<string> Order = [];
    private static readonly Dictionary<string, List<Action>> Waiting = [];
    private static readonly Lazy<HttpClient> Http = new(() =>
    {
        var client = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
        client.DefaultRequestHeaders.UserAgent.ParseAdd("Familiar/1.0 (MU* client; +https://github.com/SpiraMirabilis/familiar)");
        return client;
    });

    private static void Store(string url, CanvasBitmap? bitmap)
    {
        if (!Cache.ContainsKey(url))
        {
            Order.Enqueue(url);
            while (Cache.Count >= MaxCached && Order.TryDequeue(out var oldest))
            {
                if (Cache.Remove(oldest, out var oldBitmap))
                {
                    oldBitmap?.Dispose();
                }
            }
        }
        Cache[url] = bitmap;
    }

    /// <summary>The picture if it's here already. If not, it's fetched, and `loaded` is called
    /// once it has arrived (not if it can't be had).</summary>
    public static CanvasBitmap? Get(string url, Action loaded)
    {
        if (Cache.TryGetValue(url, out var bitmap)) return bitmap;
        if (Waiting.TryGetValue(url, out var callbacks))
        {
            if (!callbacks.Contains(loaded)) callbacks.Add(loaded);
            return null;
        }
        if (!Uri.TryCreate(url, UriKind.Absolute, out var address) || address.Scheme is not ("http" or "https"))
        {
            Store(url, null);
            return null;
        }
        Waiting[url] = [loaded];
        Load(url, address);
        return null;
    }

    private static async Task<byte[]?> DownloadCapped(Uri address)
    {
        using var response = await Http.Value.GetAsync(address, HttpCompletionOption.ResponseHeadersRead);
        response.EnsureSuccessStatusCode();
        if (response.Content.Headers.ContentLength > MaxBytes) return null;
        using var body = await response.Content.ReadAsStreamAsync();
        using var ms = new MemoryStream();
        var buffer = new byte[16 * 1024];
        int total = 0;
        int read;
        while ((read = await body.ReadAsync(buffer)) > 0)
        {
            total += read;
            if (total > MaxBytes) return null;
            ms.Write(buffer, 0, read);
        }
        return ms.ToArray();
    }

    private static async void Load(string url, Uri address)
    {
        CanvasBitmap? bitmap = null;
        try
        {
            if (await DownloadCapped(address) is { } bytes)
            {
                using var stream = new InMemoryRandomAccessStream();
                await stream.WriteAsync(bytes.AsBuffer());
                stream.Seek(0);
                bitmap = await CanvasBitmap.LoadAsync(CanvasDevice.GetSharedDevice(), stream);
            }
        }
        catch (Exception e)
        {
            System.Diagnostics.Debug.WriteLine($"Familiar: couldn't get the picture {url}: {e.Message}");
        }
        Store(url, bitmap);
        if (!Waiting.Remove(url, out var callbacks) || bitmap is null) return;
        foreach (var callback in callbacks) callback();
    }
}
