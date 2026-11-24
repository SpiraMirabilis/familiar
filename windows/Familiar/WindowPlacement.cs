using System.Text.Json.Nodes;
using Microsoft.UI.Windowing;
using Windows.Graphics;

namespace Familiar;

/// <summary>Remembers where the window was and how big, between runs. It's kept beside the
/// logs and maps rather than in config.toml, since it belongs to this PC and its screens, not
/// to the settings the clients share.</summary>
internal static class WindowPlacement
{
    private static string File => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "familiar", "window.json");

    /// <summary>Puts the window back where it was. False if there's nothing remembered, or it
    /// was somewhere that isn't on a screen any more.</summary>
    public static bool Restore(AppWindow window)
    {
        try
        {
            var saved = JsonNode.Parse(System.IO.File.ReadAllText(File));
            var rect = new RectInt32(saved.Int("x"), saved.Int("y"), saved.Int("width"), saved.Int("height"));
            if (rect.Width < 300 || rect.Height < 200) return false;
            // A screen that's gone (a laptop off its dock) mustn't leave the window out of reach.
            var area = DisplayArea.GetFromRect(rect, DisplayAreaFallback.None)?.WorkArea;
            if (area is not { } work) return false;
            var overlapX = Math.Min(rect.X + rect.Width, work.X + work.Width) - Math.Max(rect.X, work.X);
            var overlapY = Math.Min(rect.Y + rect.Height, work.Y + work.Height) - Math.Max(rect.Y, work.Y);
            if (overlapX < 120 || overlapY < 80) return false;
            window.MoveAndResize(rect);
            if (saved.Bool("maximized") && window.Presenter is OverlappedPresenter presenter) presenter.Maximize();
            return true;
        }
        catch (Exception)
        {
            // Nothing saved yet, or not something we wrote.
            return false;
        }
    }

    private static RectInt32? normal;

    /// <summary>Notes where the window is now. The size kept is the one it has when it isn't
    /// maximised, so un-maximising next time gives that back.</summary>
    public static void Save(AppWindow window)
    {
        if (window.Presenter is not OverlappedPresenter presenter || presenter.State == OverlappedPresenterState.Minimized) return;
        var maximized = presenter.State == OverlappedPresenterState.Maximized;
        if (!maximized) normal = new RectInt32(window.Position.X, window.Position.Y, window.Size.Width, window.Size.Height);
        if (normal is not { } rect) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(File)!);
            var saved = new JsonObject { ["x"] = rect.X, ["y"] = rect.Y, ["width"] = rect.Width, ["height"] = rect.Height, ["maximized"] = maximized };
            System.IO.File.WriteAllText(File, saved.ToJsonString());
        }
        catch (Exception e)
        {
            System.Diagnostics.Debug.WriteLine($"Familiar: couldn't remember the window's place: {e.Message}");
        }
    }
}
