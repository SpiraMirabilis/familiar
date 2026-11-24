using System.Reflection;
using System.Text.Json.Nodes;
using System.Xml.Linq;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using TextControlBoxNS;
using Windows.System;

namespace Familiar;

/// <summary>Highlighting for the languages MU* programs are written in, made from the same
/// language files the GTK client uses (crates/gtk/data/language-specs).</summary>
internal static class CodeLanguages
{
    public static readonly (string Label, string Id)[] All = [("MOO", "moo"), ("MUF", "muf"), ("Lua", "lua"), ("Plain text", "text")];

    // Each style in the language files, as a light and a dark colour.
    private static readonly Dictionary<string, (string Light, string Dark, bool Bold, bool Italic)> Styles = new()
    {
        ["comment"] = ("#6a737d", "#8b949e", false, true), ["string"] = ("#0a7a3e", "#7ee787", false, false), ["bytes"] = ("#0a7a3e", "#7ee787", false, false),
        ["keyword"] = ("#af00db", "#d2a8ff", true, false), ["builtin"] = ("#005cc5", "#79c0ff", false, false), ["variable"] = ("#b35900", "#ffa657", false, false),
        ["object"] = ("#098658", "#b5cea8", false, false), ["sysref"] = ("#098658", "#b5cea8", false, false), ["colour"] = ("#098658", "#b5cea8", false, false),
        ["integer"] = ("#098658", "#b5cea8", false, false), ["float"] = ("#098658", "#b5cea8", false, false), ["error-code"] = ("#d73a49", "#ff7b72", false, false),
        ["type-name"] = ("#267f99", "#4ec9b0", false, false), ["directive"] = ("#6f42c1", "#d2a8ff", false, false),
    };

    private static readonly Dictionary<string, SyntaxHighlightLanguage?> Made = [];

    /// <summary>The highlighting for a language, or null for one we have no rules for.</summary>
    public static SyntaxHighlightLanguage? For(string id)
    {
        if (Made.TryGetValue(id, out var made)) return made;
        using var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream($"{id}.lang");
        if (stream is null) return Made[id] = null;
        var rules = new List<SyntaxHighlights>();
        var last = new List<SyntaxHighlights>();
        var muf = id == "muf";
        foreach (var context in XDocument.Load(stream).Descendants("context"))
        {
            if ((string?)context.Attribute("style-ref") is not { } style || !Styles.TryGetValue(style, out var look)) continue;
            string? pattern = null;
            var words = context.Elements("keyword").Select(k => k.Value).ToList();
            if (words.Count > 0)
            {
                // MUF's words can hold any punctuation, so they end at white space, not at a word boundary.
                pattern = muf ? $@"(?<!\S)(?:{string.Join('|', words)})(?!\S)" : $@"\b(?:{string.Join('|', words)})\b";
            }
            else if (context.Element("match") is { } match) pattern = match.Value;
            else if (context.Element("start") is { } start && context.Element("end") is { } end)
            {
                pattern = start.Value == "\"" ? "\"(?:\\\\.|[^\"\\\\])*\"?" : $"{start.Value}[\\s\\S]*?{end.Value}";
            }
            if (pattern is null) continue;
            var rule = new SyntaxHighlights(pattern, look.Light, look.Dark, look.Bold, look.Italic, false);
            // Comments and strings go last, so they win over what's inside them.
            (style is "comment" or "string" or "bytes" ? last : rules).Add(rule);
        }
        return Made[id] = new SyntaxHighlightLanguage { Name = id, Highlights = [.. rules, .. last] };
    }
}

/// <summary>A window for editing a program from the world (MCP simpleedit, /edit): highlighted,
/// indented as you type, with the compiler's errors listed underneath.</summary>
internal sealed class CodeEditorWindow : Window
{
    private const string Unit = "  ";

    private readonly Session session;
    private readonly TextControlBox editor = new();
    private readonly ListView errors = new() { MaxHeight = 140, SelectionMode = ListViewSelectionMode.Single, Visibility = Visibility.Collapsed };
    private readonly TextBox find = new() { PlaceholderText = "Find", MinWidth = 220 };
    private readonly StackPanel findBar = new() { Orientation = Orientation.Horizontal, Spacing = 4, Padding = new Thickness(12, 0, 12, 8), Visibility = Visibility.Collapsed };
    private readonly ComboBox languages;
    private JsonNode request;
    private string language;
    private string saved = "";
    private bool closing;

    public CodeEditorWindow(Session session, JsonNode request)
    {
        this.session = session;
        this.request = request.DeepClone();
        language = Language(request);
        SystemBackdrop = new Microsoft.UI.Xaml.Media.MicaBackdrop();
        ExtendsContentIntoTitleBar = false;
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "familiar.ico"));
        var scale = App.Window?.Content.XamlRoot?.RasterizationScale ?? 1;
        AppWindow.Resize(new Windows.Graphics.SizeInt32((int)(860 * scale), (int)(680 * scale)));

        var bar = new CommandBar { DefaultLabelPosition = CommandBarDefaultLabelPosition.Right, HorizontalAlignment = HorizontalAlignment.Left, Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent) };
        bar.PrimaryCommands.Add(Command("Send", Symbol.Send, "Ctrl+S", Send));
        bar.PrimaryCommands.Add(Command("Re-indent", Symbol.AlignLeft, "Ctrl+Shift+I", Reindent));
        bar.PrimaryCommands.Add(Command("Find", Symbol.Find, "Ctrl+F", ShowFind));
        languages = Form.Choice(null, CodeLanguages.All, language, id =>
        {
            language = id;
            ApplyLanguage();
        });
        languages.VerticalAlignment = VerticalAlignment.Center;
        languages.Margin = new Thickness(0, 0, 12, 0);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(languages, "Language");
        var top = new Grid();
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(languages, 1);
        top.Children.Add(bar);
        top.Children.Add(languages);

        var previous = new Button { Content = new SymbolIcon(Symbol.Up) };
        var next = new Button { Content = new SymbolIcon(Symbol.Download) };
        var done = new Button { Content = new SymbolIcon(Symbol.Cancel) };
        ToolTipService.SetToolTip(previous, "Previous (Shift+F3)");
        ToolTipService.SetToolTip(next, "Next (F3)");
        previous.Click += (_, _) => editor.FindPrevious();
        next.Click += (_, _) => editor.FindNext();
        done.Click += (_, _) => HideFind();
        find.TextChanged += (_, _) => Search();
        find.KeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Enter)
            {
                if (Keys.Shift) editor.FindPrevious();
                else editor.FindNext();
                e.Handled = true;
            }
            else if (e.Key == VirtualKey.Escape)
            {
                HideFind();
                e.Handled = true;
            }
        };
        findBar.Children.Add(find);
        findBar.Children.Add(previous);
        findBar.Children.Add(next);
        findBar.Children.Add(done);

        var font = session.Font;
        editor.FontFamily = new Microsoft.UI.Xaml.Media.FontFamily(font.Family);
        editor.FontSize = (int)Math.Round(font.Size);
        editor.UseSpacesInsteadTabs = true;
        editor.NumberOfSpacesForTab = Unit.Length;
        editor.ShowLineNumbers = true;
        editor.PreviewKeyDown += OnEditorKey;
        errors.SelectionChanged += (_, _) =>
        {
            if (errors.SelectedItem is ListViewItem { Tag: int line }) GoTo(line);
        };

        var root = new Grid();
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Grid.SetRow(findBar, 1);
        Grid.SetRow(editor, 2);
        Grid.SetRow(errors, 3);
        root.Children.Add(top);
        root.Children.Add(findBar);
        root.Children.Add(editor);
        root.Children.Add(errors);
        Content = root;

        Accelerator(root, VirtualKey.S, VirtualKeyModifiers.Control, Send);
        Accelerator(root, VirtualKey.I, VirtualKeyModifiers.Control | VirtualKeyModifiers.Shift, Reindent);
        Accelerator(root, VirtualKey.F, VirtualKeyModifiers.Control, ShowFind);
        Accelerator(root, VirtualKey.F3, VirtualKeyModifiers.None, () => editor.FindNext());
        Accelerator(root, VirtualKey.F3, VirtualKeyModifiers.Shift, () => editor.FindPrevious());

        editor.Loaded += _ =>
        {
            ApplyLanguage();
            Load();
        };
        AppWindow.Closing += OnClosing;
    }

    private static AppBarButton Command(string label, Symbol icon, string keys, Action act)
    {
        var button = new AppBarButton { Label = label, Icon = new SymbolIcon(icon) };
        ToolTipService.SetToolTip(button, $"{label} ({keys})");
        button.Click += (_, _) => act();
        return button;
    }

    private static void Accelerator(UIElement on, VirtualKey key, VirtualKeyModifiers modifiers, Action act)
    {
        var accelerator = new KeyboardAccelerator { Key = key, Modifiers = modifiers };
        accelerator.Invoked += (_, e) =>
        {
            act();
            e.Handled = true;
        };
        on.KeyboardAccelerators.Add(accelerator);
    }

    /// <summary>The language a request is in: its own, or the world's, or MOO.</summary>
    private static string Language(JsonNode request)
    {
        var id = (request.OptStr("language") ?? "").ToLowerInvariant();
        return id.Length == 0 ? "moo" : CodeLanguages.All.Any(l => l.Id == id) ? id : "text";
    }

    private string Text => editor.GetText().Replace("\r\n", "\n").Replace('\r', '\n');

    private void Load()
    {
        saved = request.Str("content").Replace("\r\n", "\n");
        editor.LoadText(saved);
        editor.ClearUndoRedoHistory();
        ClearErrors();
        Title = $"{request.Str("name")} – Familiar";
    }

    /// <summary>The server sent the program again.</summary>
    public void Reload(JsonNode again)
    {
        request = again.DeepClone();
        Load();
        Activate();
    }

    private void ApplyLanguage()
    {
        var rules = CodeLanguages.For(language);
        editor.SyntaxHighlighting = rules;
        editor.EnableSyntaxHighlighting = rules is not null;
    }

    private void Send()
    {
        ClearErrors();
        saved = Text;
        session.SaveEdit(request, saved);
    }

    private void Reindent()
    {
        JsonNode? reply;
        try
        {
            reply = Core.CallAnywhere("code.reindent", Json.Args(("text", Text), ("language", language), ("unit", Unit)));
        }
        catch (CoreException)
        {
            return;
        }
        if (reply is not JsonValue value || !value.TryGetValue<string>(out var text) || text == Text) return;
        var line = editor.CurrentLineIndex;
        editor.SetText(text);
        editor.GoToLine(Math.Min(line, editor.NumberOfLines - 1));
    }

    /// <summary>Enter with indentation rules: re-indent the finished line, then indent the new one.</summary>
    private void OnEditorKey(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter || Keys.Ctrl || Keys.Shift || Keys.Alt || editor.HasSelection) return;
        var index = editor.CurrentLineIndex;
        var line = editor.GetLineText(index);
        var at = Math.Min(editor.CursorPosition.CharacterPosition, line.Length);
        var before = string.Join('\n', Enumerable.Range(0, index).Select(editor.GetLineText).Append(line[..at]));
        JsonNode? reply;
        try
        {
            reply = Core.CallAnywhere("code.on_enter", Json.Args(("before", before), ("language", language)));
        }
        catch (CoreException)
        {
            return;
        }
        // A language without rules: Enter does what it always does.
        if (reply is null) return;
        e.Handled = true;
        var current = string.Concat(Enumerable.Repeat(Unit, reply.Int("current")));
        var next = string.Concat(Enumerable.Repeat(Unit, reply.Int("next")));
        editor.ExecuteActionGroup(() =>
        {
            editor.SetLineText(index, current + line[..at].TrimStart());
            editor.AddLine(index + 1, next + line[at..].TrimStart());
        });
        editor.SetCursorPosition(index + 1, next.Length, true, true);
    }

    // MARK: Finding

    private void ShowFind()
    {
        findBar.Visibility = Visibility.Visible;
        if (editor.HasSelection && !editor.SelectedText.Contains('\n')) find.Text = editor.SelectedText;
        find.Focus(FocusState.Programmatic);
        find.SelectAll();
        Search();
    }

    private void HideFind()
    {
        findBar.Visibility = Visibility.Collapsed;
        editor.EndSearch();
        editor.Focus(FocusState.Programmatic);
    }

    private void Search()
    {
        if (find.Text.Length == 0) editor.EndSearch();
        else editor.BeginSearch(find.Text, false, false);
    }

    // MARK: Errors

    private void ClearErrors()
    {
        errors.Items.Clear();
        errors.Visibility = Visibility.Collapsed;
    }

    /// <summary>The compiler didn't like a line (they're numbered from 1).</summary>
    public void AddError(int line, string message)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        row.Children.Add(new FontIcon { Glyph = "", FontSize = 14, Foreground = Form.Brush("SystemFillColorCriticalBrush") });
        row.Children.Add(new TextBlock { Text = line > 0 ? $"Line {line}: {message}" : message, TextWrapping = TextWrapping.Wrap });
        errors.Items.Add(new ListViewItem { Content = row, Tag = line });
        errors.Visibility = Visibility.Visible;
        if (errors.Items.Count == 1) GoTo(line);
        Activate();
    }

    private void GoTo(int line)
    {
        if (line < 1 || line > editor.NumberOfLines) return;
        editor.GoToLine(line - 1);
        editor.ScrollLineToCenter(line - 1);
        editor.Focus(FocusState.Programmatic);
    }

    // MARK: Closing

    private async void OnClosing(Microsoft.UI.Windowing.AppWindow sender, Microsoft.UI.Windowing.AppWindowClosingEventArgs args)
    {
        if (closing || session.IsClosed || Text == saved) return;
        args.Cancel = true;
        var dialog = Form.Dialog(Content.XamlRoot, $"Send your changes to {request.Str("name")}?", new TextBlock { Text = "They’re lost if you don’t.", TextWrapping = TextWrapping.Wrap }, "Send", "Cancel");
        dialog.SecondaryButtonText = "Discard";
        var answer = await Form.Show(dialog);
        if (answer == ContentDialogResult.None) return;
        if (answer == ContentDialogResult.Primary) Send();
        closing = true;
        Close();
    }
}
