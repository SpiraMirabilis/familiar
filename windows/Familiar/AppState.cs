using System.Text.Json.Nodes;
using Microsoft.Graphics.Canvas.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.UI;

namespace Familiar;

/// <summary>What the whole app shares: the config (owned by the engine; this is a copy), the
/// open sessions, and what the engine told us at startup.</summary>
internal static class AppState
{
    /// <summary>The config as JSON, in the shape of config.toml. Change it with <see cref="Save"/>.</summary>
    public static JsonNode Config { get; private set; } = new JsonObject();
    public static JsonNode Start { get; private set; } = new JsonObject();
    public static Color Foreground { get; private set; } = Color.FromArgb(255, 204, 204, 204);
    public static Color Background { get; private set; } = Color.FromArgb(255, 0, 0, 0);
    public static readonly Dictionary<long, Session> Sessions = [];

    /// <summary>Raised after the config changes, however it changed.</summary>
    public static event Action? ConfigChanged;

    public static JsonNode? Settings => Config.Get("settings");
    public static IList<JsonNode?> Worlds => Config.Arr("world");
    public static JsonNode? Defaults => Start.Get("defaults");

    /// <summary>Returns what went wrong, if the app should say something before carrying on.</summary>
    public static (string Title, string Detail)? Launch()
    {
        Core.Start();
        Core.Wake += id =>
        {
            if (Sessions.TryGetValue(id, out var session)) session.Poll();
        };
        try
        {
            Start = Core.Call("start") ?? new JsonObject();
        }
        catch (CoreException e)
        {
            return ("Familiar couldn’t start its engine", e.Message);
        }
        Config = Start.Get("config")?.DeepClone() ?? new JsonObject();
        Foreground = Json.Hex(Start.Get("palette").OptStr("foreground")) ?? Foreground;
        Background = Json.Hex(Start.Get("palette").OptStr("background")) ?? Background;
        if (Start.OptStr("load_error") is { } problem)
        {
            return ("Your settings couldn’t be read", $"{problem}\n\nFamiliar is using the defaults and won’t save over your file. Fix it and start Familiar again.");
        }
        return null;
    }

    /// <summary>Replaces the config: saves it and has every open session pick it up. Returns
    /// what went wrong, if something did.</summary>
    public static string? Save(JsonNode config)
    {
        Config = config;
        string? problem = null;
        try
        {
            Core.Call("config.set", Json.Args(("config", config)));
        }
        catch (CoreException e)
        {
            problem = e.Message;
        }
        foreach (var session in Sessions.Values.ToList()) session.ConfigChanged();
        ConfigChanged?.Invoke();
        return problem;
    }

    /// <summary>Changes one thing in the config and saves.</summary>
    public static string? Update(Action<JsonNode> change)
    {
        var config = Config.DeepClone();
        change(config);
        return JsonNode.DeepEquals(config, Config) ? null : Save(config);
    }

    /// <summary>The engine changed the config itself (a script set a variable, /images on).</summary>
    public static void ReloadFromEngine()
    {
        if (Core.Send("config.get") is { } fresh)
        {
            Config = fresh;
            ConfigChanged?.Invoke();
        }
    }

    public static JsonNode? World(string name) => Worlds.FirstOrDefault(w => w.Str("name") == name);

    /// <summary>The font for a world: its own if it has one, otherwise the one in the settings.</summary>
    public static FontSpec FontFor(string world)
    {
        var own = World(world).Str("font").Trim();
        return FontSpec.Parse(own.Length == 0 ? Settings.Str("font") : own);
    }

    public static async Task Alert(XamlRoot? root, string title, string detail)
    {
        if (root is null) return;
        var dialog = new ContentDialog { XamlRoot = root, Title = title, Content = new TextBlock { Text = detail, TextWrapping = TextWrapping.Wrap }, CloseButtonText = "OK", DefaultButton = ContentDialogButton.Close };
        try
        {
            await dialog.ShowAsync();
        }
        catch (Exception)
        {
            // Another dialog is already open; only one can be shown at a time.
        }
    }
}

/// <summary>A font as the config stores it. Fonts are written the way the GTK client writes
/// them ("Fira Mono Bold 12"), so every client can share a config.</summary>
internal readonly record struct FontSpec(string Family, double Points, bool Bold, bool Italic)
{
    public const double DefaultPoints = 11;

    /// <summary>The size in the units Windows lays text out in (96 to the inch).</summary>
    public float Size => (float)(Points * 96.0 / 72.0);

    private static readonly Lazy<HashSet<string>> Installed = new(() => new HashSet<string>(CanvasTextFormat.GetSystemFontFamilies(), StringComparer.OrdinalIgnoreCase));

    /// <summary>The fixed-width font Windows comes with: Cascadia Mono on Windows 11, Consolas before.</summary>
    public static string SystemMonospace => Installed.Value.Contains("Cascadia Mono") ? "Cascadia Mono" : "Consolas";

    public static FontSpec Parse(string description)
    {
        var words = description.Split(' ', StringSplitOptions.RemoveEmptyEntries).ToList();
        var points = DefaultPoints;
        if (words.Count > 0 && double.TryParse(words[^1].Replace("px", ""), System.Globalization.CultureInfo.InvariantCulture, out var n) && n > 0)
        {
            points = n;
            words.RemoveAt(words.Count - 1);
        }
        var name = string.Join(' ', words);
        // The generic names a config written on Linux uses.
        if (name.Length == 0 || name.Equals("monospace", StringComparison.OrdinalIgnoreCase) || name.Equals("mono", StringComparison.OrdinalIgnoreCase))
        {
            return new FontSpec(SystemMonospace, points, false, false);
        }
        if (Installed.Value.Contains(name)) return new FontSpec(name, points, false, false);
        // "Fira Mono Bold": a family followed by style words.
        bool bold = false, italic = false;
        while (words.Count > 1)
        {
            var last = words[^1].ToLowerInvariant();
            words.RemoveAt(words.Count - 1);
            if (last is "bold" or "semibold" or "semi-bold") bold = true;
            if (last is "italic" or "oblique") italic = true;
            var candidate = string.Join(' ', words);
            if (Installed.Value.Contains(candidate)) return new FontSpec(candidate, points, bold, italic);
        }
        return new FontSpec(SystemMonospace, points, false, false);
    }

    public static string Describe(string family, double points)
    {
        var n = points == Math.Round(points) ? ((int)points).ToString() : points.ToString("0.#", System.Globalization.CultureInfo.InvariantCulture);
        return family.Length == 0 ? $"Monospace {n}" : $"{family} {n}";
    }
}
