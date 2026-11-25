using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Familiar;

/// <summary>The client's own keyboard shortcuts. The engine keeps them (keymap.rs: every action,
/// its default keys, the user's changes); this asks it which action a key press runs, and what
/// to show in menus.</summary>
internal static class Shortcuts
{
    private static IList<JsonNode?>? actions;

    /// <summary>Set while the Keyboard settings are recording a key, so it doesn't also run something.</summary>
    public static bool Recording { get; set; }

    static Shortcuts()
    {
        // Keys and keyboard macros (which the settings show clashes with) live in the config.
        AppState.ConfigChanged += () => actions = null;
    }

    /// <summary>Every action this app has, in order ("keys.actions": id, label, group, scope,
    /// keys with what else is on them, defaults, changed).</summary>
    public static IList<JsonNode?> Actions => actions ??= Core.Send("keys.actions").Items();

    /// <summary>The action a key press runs in "window" or "editor", if any.</summary>
    public static string? Match(string scope, KeyRoutedEventArgs e)
    {
        if (Recording || Keys.Chord(e) is not { } chord) return null;
        chord["scope"] = scope;
        return Core.Send("keys.match", chord) is JsonValue value && value.TryGetValue<string>(out var id) ? id : null;
    }

    public static JsonNode? Action(string id) => Actions.FirstOrDefault(a => a.Str("id") == id);

    /// <summary>An action's first key as it's written on a menu ("Ctrl+,"), or "" if it has none.</summary>
    public static string Label(string id) => Action(id).Arr("keys").FirstOrDefault().Str("key") is { Length: > 0 } key ? Pretty(key) : "";

    /// <summary>An action's label with its key after it, for a tooltip: "Send (Ctrl+S)".</summary>
    public static string Tip(string text, string id) => Label(id) is { Length: > 0 } key ? $"{text} ({key})" : text;

    private static readonly Dictionary<string, string> Names = new(StringComparer.OrdinalIgnoreCase)
    {
        ["comma"] = ",", ["period"] = ".", ["slash"] = "/", ["semicolon"] = ";", ["apostrophe"] = "'", ["bracketleft"] = "[",
        ["bracketright"] = "]", ["backslash"] = "\\", ["minus"] = "-", ["equal"] = "=", ["grave"] = "`", ["Page_Up"] = "PgUp",
        ["Page_Down"] = "PgDn", ["Return"] = "Enter", ["Escape"] = "Esc", ["BackSpace"] = "Backspace", ["space"] = "Space",
        ["KP_Enter"] = "Num Enter", ["KP_Add"] = "Num +", ["KP_Subtract"] = "Num -", ["KP_Multiply"] = "Num *", ["KP_Divide"] = "Num /",
        ["KP_Decimal"] = "Num .",
    };

    /// <summary>A key as the config spells it ("Ctrl+Page_Down") as Windows writes keys ("Ctrl+PgDn").</summary>
    public static string Pretty(string key)
    {
        // "Ctrl++" is Ctrl and the plus key.
        var cut = key.EndsWith("++") ? key.Length - 1 : key.LastIndexOf('+') + 1;
        var name = key[cut..];
        if (Names.TryGetValue(name, out var pretty)) name = pretty;
        else if (name.StartsWith("KP_")) name = $"Num {name[3..].Replace('_', ' ')}";
        return key[..cut].Replace("Cmd+", "Win+") + name;
    }
}

/// <summary>A search box over a list, in a dialog: the command palette and the input history.
/// Type to filter; Up and Down choose, Enter takes it, Escape closes.</summary>
internal static class Picker
{
    public sealed record Item(string Group, string Label, string Detail, Action Run);

    /// <summary>Shows the items and, once the dialog has gone, runs the one chosen (another
    /// dialog can't open while this one is).</summary>
    public static async Task Show(XamlRoot root, string placeholder, IReadOnlyList<Item> items, string empty)
    {
        var search = new TextBox { PlaceholderText = placeholder };
        var list = new ListView { SelectionMode = ListViewSelectionMode.Single, Height = 380, IsItemClickEnabled = true };
        var none = new TextBlock { Text = empty, Margin = new Thickness(12, 8, 12, 8), Foreground = Form.Brush("TextFillColorSecondaryBrush"), Visibility = Visibility.Collapsed };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(search, placeholder);
        var body = new StackPanel { Spacing = 8, Width = 520 };
        body.Children.Add(search);
        body.Children.Add(none);
        body.Children.Add(list);
        var dialog = Form.Dialog(root, "", body, close: "");
        dialog.Resources["ContentDialogMaxWidth"] = 640.0;
        dialog.Title = null;
        Item? chosen = null;

        void Fill()
        {
            list.Items.Clear();
            var query = search.Text.Trim();
            var shown = query.Length == 0
                ? items.ToList()
                : items.Select((item, i) => (item, i, score: Fuzzy.Score(query, item.Label) ?? Fuzzy.Score(query, item.Detail) - 40))
                    .Where(m => m.score is not null).OrderByDescending(m => m.score).ThenBy(m => m.i).Select(m => m.item).ToList();
            string? group = null;
            foreach (var item in shown)
            {
                // The groups have headings until a search mixes them up.
                if (query.Length == 0 && item.Group != group && items.Any(i => i.Group != item.Group))
                {
                    group = item.Group;
                    var heading = new TextBlock { Text = group, Style = Form.Style("CaptionTextBlockStyle"), Foreground = Form.Brush("TextFillColorSecondaryBrush") };
                    list.Items.Add(new ListViewItem { Content = heading, IsHitTestVisible = false, IsTabStop = false, MinHeight = 28, Padding = new Thickness(12, 8, 12, 0) });
                }
                var row = new Grid { ColumnSpacing = 16 };
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                var detail = new TextBlock { Text = item.Detail, Foreground = Form.Brush("TextFillColorSecondaryBrush"), TextTrimming = TextTrimming.CharacterEllipsis, MaxWidth = 260, VerticalAlignment = VerticalAlignment.Center };
                Grid.SetColumn(detail, 1);
                row.Children.Add(new TextBlock { Text = item.Label, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center });
                row.Children.Add(detail);
                list.Items.Add(new ListViewItem { Content = row, Tag = item });
            }
            list.SelectedItem = Rows().FirstOrDefault();
            none.Visibility = shown.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        }

        // The rows that can be chosen (not the headings).
        List<ListViewItem> Rows() => list.Items.OfType<ListViewItem>().Where(r => r.Tag is Item).ToList();

        void Move(int by)
        {
            var rows = Rows();
            if (rows.Count == 0) return;
            var at = list.SelectedItem is ListViewItem selected ? rows.IndexOf(selected) : -1;
            list.SelectedItem = rows[Math.Clamp(at + by, 0, rows.Count - 1)];
            list.ScrollIntoView(list.SelectedItem);
        }

        void Choose(object? row)
        {
            if (row is not ListViewItem { Tag: Item item }) return;
            chosen = item;
            dialog.Hide();
        }

        search.TextChanged += (_, _) => Fill();
        search.PreviewKeyDown += (_, e) =>
        {
            switch (e.Key)
            {
                case VirtualKey.Down: Move(1); break;
                case VirtualKey.Up: Move(-1); break;
                case VirtualKey.PageDown: Move(8); break;
                case VirtualKey.PageUp: Move(-8); break;
                case VirtualKey.Enter: Choose(list.SelectedItem); break;
                case VirtualKey.Escape: dialog.Hide(); break;
                default: return;
            }
            e.Handled = true;
        };
        list.ItemClick += (_, e) => Choose(Rows().FirstOrDefault(r => ReferenceEquals(r, e.ClickedItem) || ReferenceEquals(r.Content, e.ClickedItem)));
        dialog.Opened += (_, _) => search.Focus(FocusState.Programmatic);
        Fill();
        await Form.Show(dialog);
        chosen?.Run();
    }
}

/// <summary>A small fuzzy matcher for the pickers, as the terminal client's (fuzzy.rs): the
/// query's characters must appear in order. Matches at word starts and runs of consecutive
/// characters score higher.</summary>
internal static class Fuzzy
{
    /// <summary>The score, or null if `query` doesn't match.</summary>
    public static long? Score(string query, string text)
    {
        if (query.Length == 0) return 0;
        var lower = text.ToLowerInvariant();
        if (lower.Length != text.Length) return lower.Contains(query.ToLowerInvariant()) ? 1 : null;
        long score = 0;
        var at = 0;
        int? previous = null, first = null;
        foreach (var q in query.ToLowerInvariant())
        {
            if (q == ' ') continue;
            var found = lower.IndexOf(q, at);
            if (found < 0) return null;
            first ??= found;
            score += 10;
            if (found == 0 || !char.IsLetterOrDigit(text[found - 1])) score += 20;
            if (previous == found - 1) score += 15;
            score -= found - at;
            previous = found;
            at = found + 1;
        }
        if (first == 0) score += 30;
        // Shorter texts win ties.
        return score - text.Length / 8;
    }
}
