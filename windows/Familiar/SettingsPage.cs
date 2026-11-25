using System.Text.Json.Nodes;
using CommunityToolkit.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Familiar;

/// <summary>The settings, as a tab. Every change is saved as it's made, as in Windows' own
/// Settings.</summary>
internal sealed class SettingsPage : UserControl
{
    private readonly StackPanel panel = new() { Spacing = 4, MaxWidth = 760, Padding = new Thickness(24, 28, 24, 24) };
    private readonly StackPanel shortcuts = new() { Spacing = 4 };
    private readonly Button resetAll = new() { Content = "Reset All…" };
    private readonly TextBlock keyboardHeading;
    private readonly ScrollViewer scroller;
    private bool scrollToKeyboard;
    /// <summary>The add-a-key button that's waiting for a key, if one is.</summary>
    private Button? recorder;
    /// <summary>Each action's add-a-key button.</summary>
    private readonly Dictionary<string, Button> adders = [];

    public SettingsPage()
    {
        panel.Children.Add(new TextBlock { Text = "Settings", Style = Form.Style("TitleTextBlockStyle"), Margin = new Thickness(0, 0, 0, 8) });

        Section("Appearance");
        var font = new FontPicker(AppState.Settings.Str("font"));
        font.Changed += () => Set("font", font.Description);
        Card("", "Font", "Worlds can have their own, in their settings.", font);
        Card("", "Lines of scrollback", "How much of the output is kept to scroll back through.", Number("scrollback_lines", 100, 1000000, 1000));

        Section("Input");
        Card("", "Show what I send in the output", null, Toggle("local_echo"));
        Card("", "Check spelling as I type", null, Toggle("spellcheck"));
        Card("", "Commands to remember", "For going back through what you typed with the Up key.", Number("input_history_size", 1, 100000, 100));

        Section("Output");
        Card("", "Show pictures under links to images", "Fetching a picture someone posted tells that site your IP address.", Toggle("inline_images"));
        Card("", "Speak lines as they arrive", Shortcuts.Label("stop-speech") is { Length: > 0 } stop ? $"Reads the output aloud with the system voice. {stop} stops it." : "Reads the output aloud with the system voice.", Toggle("speak_output"));
        Card("", "Wait for a prompt", "Milliseconds of silence before an unfinished line is shown as a prompt.", Number("prompt_timeout_ms", 0, 60000, 50));

        Section("Logs");
        Card("", "Put the time before each line", null, Toggle("log_timestamps"));
        var logs = new Button { Content = "Open Folder" };
        logs.Click += (_, _) => OpenFolder(AppState.Start.Str("log_dir"));
        Card("", "Log folder", AppState.Start.Str("log_dir"), logs);

        Section("Files");
        var import = new Button { Content = "Import…" };
        import.Click += async (_, _) => await ImportBeipMU(XamlRoot);
        Card("", "Import from BeipMU", "Brings in the worlds, characters, triggers and aliases from BeipMU’s Config.txt.", import);
        var folder = new Button { Content = "Open Folder" };
        folder.Click += (_, _) => OpenFolder(AppState.Start.Str("config_dir"));
        Card("", "Settings file", AppState.Start.Str("config_path"), folder);

        keyboardHeading = Section("Keyboard shortcuts");
        resetAll.Click += async (_, _) => await ResetAll();
        Card("\uE765", "Keys for Familiar’s commands", "Add a key with +, then press it. A shortcut comes before a keyboard macro on the same key.", resetAll);
        panel.Children.Add(shortcuts);

        Content = scroller = new ScrollViewer { Content = panel };
        shortcuts.SizeChanged += (_, _) =>
        {
            // Once the scroller knows how tall the page is now.
            if (scrollToKeyboard) DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, ScrollToKeyboard);
        };
        // The shortcuts are filled in each time the page is shown, since macros (which they can
        // clash with) are changed elsewhere.
        Loaded += (_, _) => FillShortcuts();
        Unloaded += (_, _) => StopRecording();
    }

    private TextBlock Section(string title)
    {
        var heading = new TextBlock { Text = title, Style = Form.Style("BodyStrongTextBlockStyle"), Margin = new Thickness(1, 20, 0, 4) };
        panel.Children.Add(heading);
        return heading;
    }

    /// <summary>Scrolls to the keyboard shortcuts, now or once they're laid out.</summary>
    public void ShowKeyboard()
    {
        if (IsLoaded && shortcuts.Children.Count > 0) ScrollToKeyboard();
        else scrollToKeyboard = true;
    }

    private void ScrollToKeyboard()
    {
        scrollToKeyboard = false;
        var top = keyboardHeading.TransformToVisual(panel).TransformPoint(default).Y;
        scroller.ChangeView(null, Math.Max(0, top - 12), null, disableAnimation: true);
    }

    // MARK: Keyboard shortcuts

    private void FillShortcuts(string? focus = null)
    {
        shortcuts.Children.Clear();
        adders.Clear();
        string? group = null;
        foreach (var action in Shortcuts.Actions)
        {
            if (action.Str("group") != group)
            {
                group = action.Str("group");
                shortcuts.Children.Add(new TextBlock { Text = group, Style = Form.Style("CaptionTextBlockStyle"), Foreground = Form.Brush("TextFillColorSecondaryBrush"), Margin = new Thickness(1, 12, 0, 2) });
            }
            var (keys, add) = KeyEditor(action);
            adders[action.Str("id")] = add;
            shortcuts.Children.Add(new SettingsCard { Header = action.Str("label"), Description = Clashes(action), Content = keys });
        }
        resetAll.IsEnabled = Shortcuts.Actions.Any(a => a.Bool("changed"));
        // Back where the user was, after a change.
        if (focus is not null && adders.TryGetValue(focus, out var again)) again.Focus(FocusState.Programmatic);
    }

#if DEBUG
    /// <summary>Starts recording a key for an action, as clicking its + does (for tests).</summary>
    internal void Record(string action)
    {
        if (adders.TryGetValue(action, out var add)) StartRecording(add);
    }
#endif

    /// <summary>What else uses an action's keys: other actions, and keyboard macros (which
    /// don't run while a shortcut has their key).</summary>
    private static string Clashes(JsonNode? action)
    {
        var notes = new List<string>();
        foreach (var key in action.Arr("keys"))
        {
            var name = Shortcuts.Pretty(key.Str("key"));
            var others = key.Arr("clashes").Select(c => c?.GetValue<string>()).ToList();
            // Labels like "Connect…" end a sentence already.
            if (others.Count > 0) notes.Add($"{name} is also: {string.Join(", ", others)}".TrimEnd('…') + ".");
            var macros = key.Arr("macros").Select(m => $"Macro: {m?.GetValue<string>()}").ToList();
            if (macros.Count > 0) notes.Add($"{name} is also a keyboard macro ({string.Join("; ", macros)}), which won’t run while this shortcut has the key.");
        }
        return string.Join(" ", notes);
    }

    /// <summary>An action's keys, each with a button to remove it; a button to add one; and one
    /// to go back to the defaults, if they've been changed.</summary>
    private (StackPanel Keys, Button Add) KeyEditor(JsonNode? action)
    {
        var id = action.Str("id");
        var keys = action.Arr("keys").Select(k => k.Str("key")).ToList();
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        foreach (var key in action.Arr("keys"))
        {
            var text = key.Str("key");
            var clashing = key.Arr("clashes").Count > 0 || key.Arr("macros").Count > 0;
            var remove = new Button { Content = new FontIcon { Glyph = "", FontSize = 10 }, Padding = new Thickness(4), MinWidth = 0, MinHeight = 0, Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent), BorderThickness = new Thickness(0) };
            var label = $"Remove {Shortcuts.Pretty(text)}";
            ToolTipService.SetToolTip(remove, label);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(remove, label);
            remove.Click += (_, _) => SetKeys(id, keys.Where(k => k != text).ToList());
            var chip = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 2 };
            chip.Children.Add(new TextBlock { Text = Shortcuts.Pretty(text), VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(8, 0, 0, 0) });
            chip.Children.Add(remove);
            row.Children.Add(new Border
            {
                Child = chip,
                CornerRadius = new CornerRadius(4),
                BorderThickness = new Thickness(1),
                BorderBrush = Form.Brush(clashing ? "SystemFillColorCautionBrush" : "ControlStrokeColorDefaultBrush"),
                Background = Form.Brush("ControlFillColorSecondaryBrush"),
                Padding = new Thickness(0, 2, 2, 2),
            });
        }
        if (keys.Count == 0) row.Children.Add(new TextBlock { Text = "No key", Foreground = Form.Brush("TextFillColorSecondaryBrush"), VerticalAlignment = VerticalAlignment.Center });

        var add = new Button { Content = new FontIcon { Glyph = "", FontSize = 12 }, MinWidth = 36 };
        ToolTipService.SetToolTip(add, "Add a key");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(add, $"Add a key for {action.Str("label")}");
        add.Click += (_, _) => StartRecording(add);
        // While recording, the next key pressed (with its modifiers) is the new one.
        add.PreviewKeyDown += (_, e) =>
        {
            if (!ReferenceEquals(recorder, add)) return;
            e.Handled = true;
            if (e.Key == VirtualKey.Escape)
            {
                StopRecording();
                return;
            }
            // A modifier on its own: wait for the rest.
            if (Keys.Text(e) is not { } pressed) return;
            StopRecording();
            if (!keys.Contains(pressed)) SetKeys(id, [.. keys, pressed]);
        };
        // Going somewhere else in the window stops it (not the window losing the focus: the
        // button has it again when it's back).
        add.LostFocus += (_, _) =>
        {
            if (ReferenceEquals(recorder, add) && FocusManager.GetFocusedElement(XamlRoot) is { } now && !ReferenceEquals(now, add)) StopRecording();
        };
        row.Children.Add(add);

        if (action.Bool("changed"))
        {
            var reset = new Button { Content = new FontIcon { Glyph = "", FontSize = 12 }, MinWidth = 36 };
            var defaults = action.Arr("defaults").Select(d => Shortcuts.Pretty(d?.GetValue<string>() ?? "")).ToList();
            var tip = defaults.Count == 0 ? "Reset to no key" : $"Reset to {string.Join(", ", defaults)}";
            ToolTipService.SetToolTip(reset, tip);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(reset, tip);
            reset.Click += (_, _) => SetKeys(id, null);
            row.Children.Add(reset);
        }
        return (row, add);
    }

    private void StartRecording(Button add)
    {
        StopRecording();
        recorder = add;
        Shortcuts.Recording = true;
        add.Content = "Press a key…";
        add.Focus(FocusState.Programmatic);
    }

    private void StopRecording()
    {
        if (recorder is { } add) add.Content = new FontIcon { Glyph = "", FontSize = 12 };
        recorder = null;
        Shortcuts.Recording = false;
    }

    /// <summary>Gives an action these keys (null: its defaults). Saved straight away; keys
    /// that clash are allowed, and shown, so they can be sorted out next.</summary>
    private async void SetKeys(string id, List<string>? keys)
    {
        try
        {
            Core.Call("keys.set", Json.Args(("id", id), ("keys", keys is null ? null : new JsonArray([.. keys.Select(k => (JsonNode?)k)]))));
        }
        catch (CoreException e)
        {
            await AppState.Alert(XamlRoot, "The shortcut wasn’t changed", e.Message);
        }
        // The engine saved the config: the app's copy has to have the new keys, or the next
        // change to another setting would put the old ones back.
        AppState.ReloadFromEngine();
        FillShortcuts(focus: id);
    }

    private async Task ResetAll()
    {
        if (!await Form.Confirm(XamlRoot, "Reset all keyboard shortcuts?", "Every shortcut goes back to the keys it came with.", "Reset All")) return;
        foreach (var action in Shortcuts.Actions.Where(a => a.Bool("changed")).ToList()) Core.Send("keys.set", Json.Args(("id", action.Str("id")), ("keys", null)));
        AppState.ReloadFromEngine();
        FillShortcuts();
    }

    private void Card(string glyph, string title, string? description, object control)
    {
        panel.Children.Add(new SettingsCard { Header = title, Description = description ?? "", HeaderIcon = new FontIcon { Glyph = glyph }, Content = control });
    }

    private static async void Set(string key, JsonNode value)
    {
        var problem = AppState.Update(config =>
        {
            if (config["settings"] is not JsonObject settings) config["settings"] = settings = [];
            settings[key] = value;
        });
        if (problem is not null) await AppState.Alert(App.Window?.Content.XamlRoot, "Your settings weren’t saved", problem);
    }

    private static ToggleSwitch Toggle(string key)
    {
        var toggle = new ToggleSwitch { IsOn = AppState.Settings.Bool(key) };
        toggle.Toggled += (_, _) => Set(key, toggle.IsOn);
        return toggle;
    }

    private static NumberBox Number(string key, double min, double max, double step)
    {
        var box = new NumberBox { Value = AppState.Settings.Num(key), Minimum = min, Maximum = max, SmallChange = step, LargeChange = step * 10, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact, MinWidth = 140 };
        box.ValueChanged += (_, _) =>
        {
            if (!double.IsNaN(box.Value)) Set(key, (long)box.Value);
        };
        return box;
    }

    private static void OpenFolder(string folder)
    {
        Directory.CreateDirectory(folder);
        Session.OpenUri(folder);
    }

    /// <summary>Asks for BeipMU's Config.txt and brings in what's in it.</summary>
    public static async Task ImportBeipMU(XamlRoot root)
    {
        if (await Form.PickFile(".txt") is not { } path) return;
        JsonNode? result;
        try
        {
            result = Core.Call("import.beipmu", Json.Args(("path", path)));
        }
        catch (CoreException e)
        {
            await AppState.Alert(root, "Couldn’t Import", e.Message);
            return;
        }
        AppState.ReloadFromEngine();
        var body = new StackPanel { Spacing = 8 };
        body.Children.Add(new TextBlock { Text = result.Str("body"), TextWrapping = TextWrapping.Wrap });
        var warnings = result.Arr("warnings");
        if (warnings.Count > 0)
        {
            body.Children.Add(Form.Heading("What couldn’t be carried over"));
            foreach (var warning in warnings) body.Children.Add(new TextBlock { Text = $"• {warning}", TextWrapping = TextWrapping.Wrap });
        }
        await Form.Show(Form.Dialog(root, result.Str("heading"), new ScrollViewer { Content = body }, close: "OK"));
    }
}
