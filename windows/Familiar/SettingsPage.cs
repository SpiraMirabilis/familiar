using System.Text.Json.Nodes;
using CommunityToolkit.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Familiar;

/// <summary>The settings, as a tab. Every change is saved as it's made, as in Windows' own
/// Settings.</summary>
internal sealed class SettingsPage : UserControl
{
    private readonly StackPanel panel = new() { Spacing = 4, MaxWidth = 760, Padding = new Thickness(24, 28, 24, 24) };

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
        Card("", "Speak lines as they arrive", "Reads the output aloud with the system voice. Ctrl+. stops it.", Toggle("speak_output"));
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

        Content = new ScrollViewer { Content = panel };
    }

    private void Section(string title) => panel.Children.Add(new TextBlock { Text = title, Style = Form.Style("BodyStrongTextBlockStyle"), Margin = new Thickness(1, 20, 0, 4) });

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
