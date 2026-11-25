using System.Text.Json.Nodes;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml.Input;
using Windows.System;
using Windows.UI.Core;

namespace Familiar;

/// <summary>Key presses as the names the engine's macros use ("F5", "KP_8", "Ctrl+K"). They're
/// GDK's names, so macros made in the GTK client work here.</summary>
internal static class Keys
{
    private static readonly Dictionary<VirtualKey, string> Named = new()
    {
        [VirtualKey.Enter] = "Return", [VirtualKey.Tab] = "Tab", [VirtualKey.Escape] = "Escape", [VirtualKey.Back] = "BackSpace",
        [VirtualKey.Delete] = "Delete", [VirtualKey.Insert] = "Insert", [VirtualKey.Home] = "Home", [VirtualKey.End] = "End",
        [VirtualKey.PageUp] = "Page_Up", [VirtualKey.PageDown] = "Page_Down", [VirtualKey.Up] = "Up", [VirtualKey.Down] = "Down",
        [VirtualKey.Left] = "Left", [VirtualKey.Right] = "Right", [VirtualKey.Space] = "space", [VirtualKey.Pause] = "Pause",
        [VirtualKey.Add] = "KP_Add", [VirtualKey.Subtract] = "KP_Subtract", [VirtualKey.Multiply] = "KP_Multiply",
        [VirtualKey.Divide] = "KP_Divide", [VirtualKey.Decimal] = "KP_Decimal", [VirtualKey.Clear] = "KP_Begin",
        [(VirtualKey)188] = "comma", [(VirtualKey)190] = "period", [(VirtualKey)191] = "slash", [(VirtualKey)186] = "semicolon",
        [(VirtualKey)222] = "apostrophe", [(VirtualKey)219] = "bracketleft", [(VirtualKey)221] = "bracketright",
        [(VirtualKey)220] = "backslash", [(VirtualKey)189] = "minus", [(VirtualKey)187] = "equal", [(VirtualKey)192] = "grave",
    };

    /// <summary>What the keypad's keys are called with Num Lock off. Windows reports them as the
    /// navigation keys, told apart from the real ones by not being "extended" keys.</summary>
    private static readonly Dictionary<VirtualKey, string> KeypadNavigation = new()
    {
        [VirtualKey.Home] = "KP_Home", [VirtualKey.End] = "KP_End", [VirtualKey.PageUp] = "KP_Page_Up", [VirtualKey.PageDown] = "KP_Page_Down",
        [VirtualKey.Up] = "KP_Up", [VirtualKey.Down] = "KP_Down", [VirtualKey.Left] = "KP_Left", [VirtualKey.Right] = "KP_Right",
        [VirtualKey.Insert] = "KP_Insert", [VirtualKey.Delete] = "KP_Delete",
    };

    public static bool IsDown(VirtualKey key) => InputKeyboardSource.GetKeyStateForCurrentThread(key).HasFlag(CoreVirtualKeyStates.Down);

    public static bool Ctrl => IsDown(VirtualKey.Control);
    public static bool Shift => IsDown(VirtualKey.Shift);
    public static bool Alt => IsDown(VirtualKey.Menu);

    /// <summary>The key's name, or null for something that isn't a key of its own (a modifier).</summary>
    public static string? Name(VirtualKey key, bool extended)
    {
        if (key == VirtualKey.Enter && extended) return "KP_Enter";
        if (!extended && KeypadNavigation.TryGetValue(key, out var keypad)) return keypad;
        if (Named.TryGetValue(key, out var name)) return name;
        if (key is >= VirtualKey.A and <= VirtualKey.Z) return ((char)('a' + (key - VirtualKey.A))).ToString();
        if (key is >= VirtualKey.Number0 and <= VirtualKey.Number9) return ((char)('0' + (key - VirtualKey.Number0))).ToString();
        if (key is >= VirtualKey.NumberPad0 and <= VirtualKey.NumberPad9) return $"KP_{key - VirtualKey.NumberPad0}";
        if (key is >= VirtualKey.F1 and <= VirtualKey.F24) return $"F{key - VirtualKey.F1 + 1}";
        return null;
    }

    public static string? Name(KeyRoutedEventArgs e) => Name(e.Key, e.KeyStatus.IsExtendedKey);

    /// <summary>The chord as arguments for the engine's session.macro.</summary>
    public static JsonObject? Chord(KeyRoutedEventArgs e)
    {
        var key = Name(e);
        if (key is null) return null;
        return Json.Args(("key", key), ("ctrl", Ctrl), ("alt", Alt || e.KeyStatus.IsMenuKeyDown), ("shift", Shift));
    }

    /// <summary>The chord as text for the config ("Ctrl+Shift+F5"), for the macro editor.</summary>
    public static string? Text(KeyRoutedEventArgs e)
    {
        var key = Name(e);
        if (key is null) return null;
        var parts = new List<string>();
        if (Ctrl) parts.Add("Ctrl");
        if (Alt || e.KeyStatus.IsMenuKeyDown) parts.Add("Alt");
        if (Shift) parts.Add("Shift");
        parts.Add(key.Length == 1 ? key.ToUpperInvariant() : key);
        return string.Join("+", parts);
    }
}

/// <summary>What was typed before, for Up and Down in the input box.</summary>
internal sealed class History(int limit)
{
    private readonly List<string> entries = [];
    private int? position;
    /// <summary>What was being typed when the user started going back.</summary>
    private string draft = "";

    public int Limit { get; set; } = Math.Max(limit, 1);

    /// <summary>Oldest first.</summary>
    public IReadOnlyList<string> Entries => entries;

    public void Push(string line)
    {
        position = null;
        if (line.Length == 0 || (entries.Count > 0 && entries[^1] == line)) return;
        entries.Add(line);
        if (entries.Count > Limit) entries.RemoveRange(0, entries.Count - Limit);
    }

    public string? Older(string current)
    {
        if (entries.Count == 0) return null;
        if (position is null)
        {
            draft = current;
            position = entries.Count - 1;
        }
        else if (position > 0) position--;
        else return null;
        return entries[position.Value];
    }

    public string? Newer()
    {
        if (position is not { } p) return null;
        if (p + 1 < entries.Count)
        {
            position = p + 1;
            return entries[p + 1];
        }
        position = null;
        return draft;
    }

    public void Reset() => position = null;
}
