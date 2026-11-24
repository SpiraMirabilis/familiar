using System.Text.Json.Nodes;
using CommunityToolkit.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Familiar;

/// <summary>Triggers, aliases, keyboard macros and variables, as a tab. Each set belongs to a
/// scope: everywhere, one world, or one of its characters. Changes are saved as they're made.</summary>
internal sealed class RulesPage : UserControl
{
    /// <summary>Where a set of rules lives. Both null means everywhere.</summary>
    private sealed record Scope(string? World, string? Character)
    {
        public string Label => (World, Character) switch
        {
            (null, _) => "Every world",
            (_, null) => World,
            _ => $"{Character} @ {World}",
        };
    }

    private static readonly (string Key, string Title, string Singular)[] Kinds =
    [
        ("trigger", "Triggers", "Trigger"), ("alias", "Aliases", "Alias"), ("macro", "Macros", "Macro"), ("variables", "Variables", "Variable"),
    ];

    private readonly ComboBox scopes = new() { MinWidth = 220 };
    private readonly SelectorBar kinds = new();
    private readonly StackPanel rows = new() { Spacing = 4 };
    private readonly Button add = new() { Style = Form.Style("AccentButtonStyle") };
    private readonly TextBlock hint = Form.Caption("");
    private readonly Button examples = new() { Content = "Add Examples" };
    private readonly InfoBar notice = new() { IsOpen = false, Severity = InfoBarSeverity.Informational };
    private bool filling;

    public RulesPage(string? world, string? character)
    {
        var title = new TextBlock { Text = "Triggers, Aliases & Macros", Style = Form.Style("TitleTextBlockStyle") };
        foreach (var (key, name, _) in Kinds) kinds.Items.Add(new SelectorBarItem { Text = name, Tag = key });
        kinds.SelectedItem = kinds.Items[0];
        var bar = new Grid { ColumnSpacing = 12 };
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        ToolTipService.SetToolTip(examples, "Adds a few triggers, aliases and macros to learn from and change. Triggers and aliases start switched off.");
        // Beside the title, as it's about all four kinds at once.
        examples.HorizontalAlignment = HorizontalAlignment.Right;
        examples.VerticalAlignment = VerticalAlignment.Center;
        var header = new Grid();
        header.Children.Add(title);
        header.Children.Add(examples);
        examples.Click += (_, _) => AddExamples();
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(scopes, "Applies to");
        ToolTipService.SetToolTip(scopes, "Where these apply: every world, one world, or one character");
        Grid.SetColumn(scopes, 1);
        Grid.SetColumn(add, 2);
        bar.Children.Add(kinds);
        bar.Children.Add(scopes);
        bar.Children.Add(add);

        var panel = new StackPanel { Spacing = 12, MaxWidth = 760, Padding = new Thickness(24, 28, 24, 24) };
        panel.Children.Add(header);
        panel.Children.Add(bar);
        panel.Children.Add(hint);
        panel.Children.Add(notice);
        panel.Children.Add(rows);
        Content = new ScrollViewer { Content = panel };

        FillScopes(new Scope(world, AppState.World(world ?? "").Arr("character").Any(c => c.Str("name") == character) ? character : null));
        scopes.SelectionChanged += (_, _) =>
        {
            if (!filling) Fill();
        };
        kinds.SelectionChanged += (_, _) => Fill();
        add.Click += (_, _) => Edit(-1);
        AppState.ConfigChanged += OnConfigChanged;
        Unloaded += (_, _) =>
        {
            if (!IsLoaded) AppState.ConfigChanged -= OnConfigChanged;
        };
        Loaded += (_, _) =>
        {
            AppState.ConfigChanged -= OnConfigChanged;
            AppState.ConfigChanged += OnConfigChanged;
        };
        Fill();
    }

    private void OnConfigChanged()
    {
        FillScopes(Current);
        Fill();
    }

    private Scope Current => (scopes.SelectedItem as ComboBoxItem)?.Tag as Scope ?? new Scope(null, null);
    private string Kind => (kinds.SelectedItem as SelectorBarItem)?.Tag as string ?? "trigger";

    private void FillScopes(Scope select)
    {
        filling = true;
        scopes.Items.Clear();
        var all = new List<Scope> { new(null, null) };
        foreach (var world in AppState.Worlds)
        {
            all.Add(new Scope(world.Str("name"), null));
            all.AddRange(world.Arr("character").Select(c => new Scope(world.Str("name"), c.Str("name"))));
        }
        foreach (var scope in all) scopes.Items.Add(new ComboBoxItem { Content = scope.Label, Tag = scope });
        var at = all.IndexOf(select);
        if (at < 0) at = all.IndexOf(select with { Character = null });
        scopes.SelectedIndex = Math.Max(at, 0);
        filling = false;
    }

    /// <summary>The object in the config that holds a scope's rules, made if `create` is set.</summary>
    private static JsonObject? RulesOf(JsonNode config, Scope scope, bool create)
    {
        JsonObject? owner = config as JsonObject;
        if (scope.World is not null)
        {
            owner = config.Arr("world").FirstOrDefault(w => w.Str("name") == scope.World) as JsonObject;
            if (scope.Character is not null) owner = owner.Arr("character").FirstOrDefault(c => c.Str("name") == scope.Character) as JsonObject;
        }
        if (owner is null) return null;
        if (owner["rules"] is JsonObject rules) return rules;
        if (!create) return null;
        owner["rules"] = rules = [];
        return rules;
    }

    private IList<JsonNode?> List => RulesOf(AppState.Config, Current, false).Arr(Kind);
    private JsonObject? Variables => RulesOf(AppState.Config, Current, false)?["variables"] as JsonObject;

    /// <summary>Changes this scope's rules of the kind showing, and saves.</summary>
    private async void Change(Action<JsonObject> change)
    {
        var scope = Current;
        var problem = AppState.Update(config =>
        {
            if (RulesOf(config, scope, true) is { } rules) change(rules);
        });
        if (problem is not null) await AppState.Alert(XamlRoot, "Your settings weren’t saved", problem);
    }

    private static JsonArray ArrayOf(JsonObject rules, string kind)
    {
        if (rules[kind] is JsonArray list) return list;
        rules[kind] = list = [];
        return list;
    }

    /// <summary>Adds the engine's examples to the scope that's showing: every kind at once, since
    /// they refer to each other (an alias and a trigger use the example variable).</summary>
    internal async void AddExamples()
    {
        var scope = Current;
        var where = scope switch
        {
            { World: null } => Json.Args(("kind", "global")),
            { Character: null } => Json.Args(("kind", "world"), ("world", scope.World)),
            _ => Json.Args(("kind", "character"), ("world", scope.World), ("character", scope.Character)),
        };
        try
        {
            var reply = Core.Call("rules.add_examples", Json.Args(("scope", where)));
            AppState.ReloadFromEngine();
            notice.Title = reply.Int("added") > 0 ? "Examples added" : "Nothing to add";
            notice.Message = reply.Str("message") + (reply.Int("added") > 0 ? " Look on each page: Triggers, Aliases, Macros and Variables." : "");
            notice.IsOpen = true;
        }
        catch (CoreException e)
        {
            await AppState.Alert(XamlRoot, "Couldn’t add the examples", e.Message);
        }
    }

    // MARK: The list

    private void Fill()
    {
        var kind = Kind;
        var names = Kinds.First(k => k.Key == kind);
        add.Content = $"Add {names.Singular}";
        hint.Text = kind switch
        {
            "trigger" => "Triggers act on lines from the world, in this order: every world’s first, then the world’s, then the character’s.",
            "alias" => "Aliases change what you type before it’s sent.",
            "macro" => "Macros send a command when you press a key.",
            _ => "Variables are written %name% in triggers, aliases and macros. Scripts can read and set them.",
        };
        rows.Children.Clear();
        if (kind == "variables")
        {
            foreach (var (name, value) in Variables?.ToList() ?? []) rows.Children.Add(VariableRow(name, value?.ToString() ?? ""));
        }
        else
        {
            var list = List;
            for (var i = 0; i < list.Count; i++) rows.Children.Add(Row(kind, i, list[i], list.Count));
        }
        if (rows.Children.Count == 0)
        {
            rows.Children.Add(new TextBlock
            {
                Text = $"No {names.Title.ToLowerInvariant()} here yet.",
                Foreground = Form.Brush("TextFillColorSecondaryBrush"),
                HorizontalAlignment = HorizontalAlignment.Center,
                Margin = new Thickness(0, 32, 0, 0),
            });
        }
    }

    private static (string Title, string Summary) Describe(string kind, JsonNode? rule)
    {
        switch (kind)
        {
            case "trigger":
                var parts = new List<string>();
                if (rule.Bool("regex")) parts.Add("regex");
                if (rule.Bool("gag")) parts.Add("hide");
                if (!rule.IsNull("highlight")) parts.Add("highlight");
                if (!rule.IsNull("replace")) parts.Add("replace");
                if (rule.Str("send").Split('\n').FirstOrDefault(l => l.Length > 0) is { } first) parts.Add($"send “{first}”");
                if (rule.Str("spawn").Length > 0) parts.Add($"→ {rule.Str("spawn")}");
                if (rule.Str("sound").Length > 0) parts.Add("sound");
                if (rule.Bool("notify")) parts.Add("notify");
                if (rule.Bool("important")) parts.Add("mark the tab");
                if (rule.Str("script").Length > 0) parts.Add($"{rule.Str("script")}()");
                if (rule.Str("speak").Length > 0) parts.Add("speak");
                if (rule.Bool("stop")) parts.Add("stop");
                var name = rule.Str("name");
                return (name.Length > 0 ? name : rule.Str("pattern"), (name.Length > 0 ? $"{rule.Str("pattern")} · " : "") + string.Join(", ", parts));
            case "alias":
                return (rule.Str("pattern"), "→ " + rule.Str("replacement").Replace("\n", " ⏎ "));
            default:
                return (rule.Str("key"), (rule.Bool("insert") ? "types " : "sends ") + rule.Str("command").Replace("\n", " ⏎ "));
        }
    }

    private SettingsCard Row(string kind, int index, JsonNode? rule, int count)
    {
        var (title, summary) = Describe(kind, rule);
        var controls = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
        if (kind != "macro")
        {
            var enabled = new ToggleSwitch { IsOn = rule.Bool("enabled"), OnContent = "", OffContent = "", MinWidth = 0 };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(enabled, "Enabled");
            enabled.Toggled += (_, _) => Change(rules => ArrayOf(rules, kind)[index]!["enabled"] = enabled.IsOn);
            controls.Children.Add(enabled);
        }
        var menu = new MenuFlyout();
        void Item(string text, Symbol icon, bool on, Action act)
        {
            var item = new MenuFlyoutItem { Text = text, Icon = new SymbolIcon(icon), IsEnabled = on };
            item.Click += (_, _) => act();
            menu.Items.Add(item);
        }
        void Move(int to) => Change(rules =>
        {
            var list = ArrayOf(rules, kind);
            var moved = list[index];
            list.RemoveAt(index);
            list.Insert(to, moved);
        });
        Item("Edit…", Symbol.Edit, true, () => Edit(index));
        Item("Duplicate", Symbol.Copy, true, () => Change(rules => ArrayOf(rules, kind).Insert(index + 1, rule!.DeepClone())));
        Item("Move Up", Symbol.Up, index > 0, () => Move(index - 1));
        Item("Move Down", Symbol.Download, index < count - 1, () => Move(index + 1));
        menu.Items.Add(new MenuFlyoutSeparator());
        Item("Delete", Symbol.Delete, true, () => Change(rules =>
        {
            var list = ArrayOf(rules, kind);
            list.RemoveAt(index);
            if (list.Count == 0) rules.Remove(kind);
        }));
        var more = new Button { Content = new SymbolIcon(Symbol.More), Flyout = menu, Style = Form.Style("CommandBarFlyoutEllipsisButtonStyle"), Width = 36, Height = 32 };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(more, "More");
        controls.Children.Add(more);

        var card = new SettingsCard { Header = title.Length == 0 ? "(empty)" : title, Description = summary, Content = controls, IsClickEnabled = true, IsActionIconVisible = false, ContextFlyout = menu };
        card.Click += (_, _) => Edit(index);
        return card;
    }

    private SettingsCard VariableRow(string name, string value)
    {
        var remove = new Button { Content = new SymbolIcon(Symbol.Delete), Style = Form.Style("CommandBarFlyoutEllipsisButtonStyle"), Width = 36, Height = 32 };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(remove, $"Delete {name}");
        remove.Click += (_, _) => Change(rules =>
        {
            if (rules["variables"] is not JsonObject variables) return;
            variables.Remove(name);
            if (variables.Count == 0) rules.Remove("variables");
        });
        var card = new SettingsCard { Header = name, Description = value, Content = remove, IsClickEnabled = true, IsActionIconVisible = false };
        card.Click += (_, _) => EditVariable(name, value);
        return card;
    }

    // MARK: Editing

    internal void Edit(int index)
    {
        switch (Kind)
        {
            case "trigger": EditTrigger(index); break;
            case "alias": EditAlias(index); break;
            case "macro": EditMacro(index); break;
            default: EditVariable(null, ""); break;
        }
    }

    /// <summary>Shows an editor, and puts the rule in the list (at `index`, or at the end) if
    /// it's saved. `check` says what's wrong with the rule, if anything is.</summary>
    private async void ShowEditor(string kind, int index, string title, JsonObject draft, UIElement form, Func<string?> check)
    {
        var problem = new TextBlock { Foreground = Form.Brush("SystemFillColorCriticalBrush"), TextWrapping = TextWrapping.Wrap, Visibility = Visibility.Collapsed };
        var body = new StackPanel { Spacing = 10, Width = 460 };
        body.Children.Add(form);
        body.Children.Add(problem);
        var dialog = Form.Dialog(XamlRoot, title, new ScrollViewer { Content = body, Padding = new Thickness(0, 0, 16, 0) }, "Save");
        dialog.Resources["ContentDialogMaxWidth"] = 560.0;
        dialog.PrimaryButtonClick += (_, e) =>
        {
            if (check() is not { } message) return;
            problem.Text = message;
            problem.Visibility = Visibility.Visible;
            e.Cancel = true;
        };
        if (await Form.Show(dialog) != ContentDialogResult.Primary) return;
        Change(rules =>
        {
            var list = ArrayOf(rules, kind);
            if (index >= 0 && index < list.Count) list[index] = draft.DeepClone();
            else list.Add(draft.DeepClone());
        });
    }

    private static StackPanel Stack(double spacing, params UIElement[] children)
    {
        var panel = new StackPanel { Spacing = spacing };
        foreach (var child in children) panel.Children.Add(child);
        return panel;
    }

    private static string? PatternProblem(JsonObject rule)
    {
        if (rule.Str("pattern").Length == 0) return "Give it a pattern to match.";
        try
        {
            return Core.CallAnywhere("rules.check_pattern", Json.Args(("rule", rule)))?.GetValue<string>() is { } wrong ? $"Invalid pattern: {wrong}" : null;
        }
        catch (CoreException e)
        {
            return $"Invalid pattern: {e.Message}";
        }
    }

    /// <summary>The pattern box and its options, shared by triggers and aliases.</summary>
    private static UIElement PatternEditor(JsonObject rule, string header, string wholeWordLabel, Action changed)
    {
        CheckBox? word = null;
        var regex = Form.Check(rule, "regex", "Regular expression", changed: () =>
        {
            word!.IsEnabled = !rule.Bool("regex");
            changed();
        });
        word = Form.Check(rule, "whole_word", wholeWordLabel, changed: changed);
        word.IsEnabled = !rule.Bool("regex");
        var options = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        options.Children.Add(regex);
        options.Children.Add(Form.Check(rule, "case_sensitive", "Match case", changed: changed));
        options.Children.Add(word);
        var pattern = Form.Text(rule, "pattern", header, changed: changed);
        Form.Mono(pattern);
        return Stack(4, pattern, options);
    }

    /// <summary>A box to try a line in, with what the rule does to it shown underneath.</summary>
    private static (UIElement View, Action Refresh) Tester(string header, string placeholder, Func<string, string> run)
    {
        var input = new TextBox { Header = header, PlaceholderText = placeholder };
        var result = Form.Caption("");
        void Refresh() => result.Text = input.Text.Length == 0 ? "" : run(input.Text);
        input.TextChanged += (_, _) => Refresh();
        return (Stack(4, input, result), Refresh);
    }

    private void EditTrigger(int index)
    {
        var draft = (index >= 0 ? List[index] : AppState.Defaults.Get("trigger"))?.DeepClone() as JsonObject ?? [];
        Action refresh = () => { };
        var (tester, run) = Tester("Try it on a line", "A line from the world", line =>
        {
            try
            {
                var reply = Core.CallAnywhere("rules.test_trigger", Json.Args(("rule", draft), ("text", line)));
                return reply.Bool("matched") ? reply.Str("result") : "No match";
            }
            catch (CoreException e)
            {
                return $"Invalid pattern: {e.Message}";
            }
        });
        refresh = run;

        var form = new StackPanel { Spacing = 10 };
        form.Children.Add(Form.Text(draft, "name", "Name", "Optional"));
        form.Children.Add(PatternEditor(draft, "When a line contains", "Whole words only", () => refresh()));

        form.Children.Add(Form.Heading("Change the line"));
        CheckBox? gagLog = null;
        var gag = Form.Check(draft, "gag", "Hide the line", changed: () =>
        {
            gagLog!.IsEnabled = draft.Bool("gag");
            refresh();
        });
        gagLog = Form.Check(draft, "gag_log", "Leave it out of the log too");
        gagLog.IsEnabled = draft.Bool("gag");
        form.Children.Add(Stack(0, gag, gagLog));
        form.Children.Add(HighlightEditor(draft, () => refresh()));
        var replaces = new CheckBox { Content = "Replace what matched with", IsChecked = !draft.IsNull("replace") };
        var replacement = new TextBox { Text = draft.Str("replace"), PlaceholderText = "Nothing (deletes it)", IsEnabled = replaces.IsChecked == true };
        void SetReplace()
        {
            replacement.IsEnabled = replaces.IsChecked == true;
            if (replaces.IsChecked == true) draft["replace"] = replacement.Text;
            else draft.Remove("replace");
            refresh();
        }
        replaces.Checked += (_, _) => SetReplace();
        replaces.Unchecked += (_, _) => SetReplace();
        replacement.TextChanged += (_, _) => SetReplace();
        form.Children.Add(Stack(4, replaces, replacement));

        form.Children.Add(Form.Heading("Then"));
        var send = Form.Text(draft, "send", "Send", "Commands, one per line. $1 is the first group, %name% a variable.", () => refresh());
        send.AcceptsReturn = true;
        send.TextWrapping = TextWrapping.Wrap;
        Form.Mono(send);
        send.Text = draft.Str("send").Replace('\n', '\r');
        send.TextChanged += (_, _) => draft["send"] = send.Text.Replace("\r\n", "\n").Replace('\r', '\n');
        form.Children.Add(send);
        form.Children.Add(Form.Text(draft, "spawn", "Copy the line to a side window called", "Optional"));
        var sound = Form.Text(draft, "sound", "Play a sound", "A sound file");
        var browse = new Button { Content = "Browse…", VerticalAlignment = VerticalAlignment.Bottom };
        browse.Click += async (_, _) =>
        {
            if (await Form.PickFile(".wav", ".mp3", ".ogg", ".flac") is { } path) sound.Text = path;
        };
        var soundRow = new Grid { ColumnSpacing = 8 };
        soundRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        soundRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(browse, 1);
        soundRow.Children.Add(sound);
        soundRow.Children.Add(browse);
        form.Children.Add(soundRow);
        form.Children.Add(Form.Text(draft, "speak", "Speak aloud", "$0 is the whole line"));
        form.Children.Add(Form.Text(draft, "script", "Call a script function", "Its name"));
        form.Children.Add(Stack(0,
            Form.Check(draft, "notify", "Show a notification"),
            Form.Check(draft, "important", "Mark the tab as needing attention"),
            Form.Check(draft, "stop", "Don’t run later triggers on the line")));
        form.Children.Add(tester);

        ShowEditor("trigger", index, index >= 0 ? "Edit Trigger" : "New Trigger", draft, form, () =>
        {
            foreach (var key in new[] { "name", "spawn", "sound", "speak", "script" }) draft[key] = draft.Str(key).Trim();
            return PatternProblem(draft);
        });
    }

    private static UIElement HighlightEditor(JsonObject rule, Action changed)
    {
        var on = new CheckBox { Content = "Highlight it", IsChecked = !rule.IsNull("highlight") };
        var draft = rule["highlight"]?.DeepClone() as JsonObject ?? [];
        var options = new StackPanel { Spacing = 6, Margin = new Thickness(28, 0, 0, 0) };
        void Apply()
        {
            options.Visibility = on.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            if (on.IsChecked == true) rule["highlight"] = draft.DeepClone();
            else rule.Remove("highlight");
            changed();
        }
        var colors = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 16 };
        colors.Children.Add(new ColorChoice("Text colour", draft, "fg", Color.FromArgb(255, 0xff, 0xd7, 0x5f), Apply));
        colors.Children.Add(new ColorChoice("Background", draft, "bg", Color.FromArgb(255, 0x5f, 0x00, 0x00), Apply));
        var styles = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12 };
        styles.Children.Add(Form.Check(draft, "bold", "Bold", changed: Apply));
        styles.Children.Add(Form.Check(draft, "italic", "Italic", changed: Apply));
        styles.Children.Add(Form.Check(draft, "underline", "Underline", changed: Apply));
        options.Children.Add(colors);
        options.Children.Add(styles);
        options.Children.Add(Stack(0,
            Form.Check(draft, "whole_line", "The whole line, not just what matched", changed: Apply),
            Form.Check(draft, "groups", "Only the pattern’s groups, if it has any", changed: Apply)));
        options.Visibility = on.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
        on.Checked += (_, _) => Apply();
        on.Unchecked += (_, _) => Apply();
        return Stack(4, on, options);
    }

    private void EditAlias(int index)
    {
        var draft = (index >= 0 ? List[index] : AppState.Defaults.Get("alias"))?.DeepClone() as JsonObject ?? [];
        Action refresh = () => { };
        var (tester, run) = Tester("Try it on something typed", "What you’d type", input =>
        {
            try
            {
                var reply = Core.CallAnywhere("rules.test_alias", Json.Args(("rule", draft), ("text", input)));
                return reply.Bool("matched") ? string.Join(" ⏎ ", reply.Arr("lines").Select(l => l?.ToString())) : "No match, so it’s sent as typed";
            }
            catch (CoreException e)
            {
                return $"Invalid pattern: {e.Message}";
            }
        });
        refresh = run;

        var form = new StackPanel { Spacing = 10 };
        form.Children.Add(PatternEditor(draft, "When I type", "Whole words only", () => refresh()));
        form.Children.Add(Form.Caption("Plain text matches the first word of what you type. A regular expression can match anywhere."));
        var replacement = new TextBox
        {
            Header = "Send instead",
            PlaceholderText = "Commands, one per line. $1 is the first argument, $* all of them.",
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
            Text = draft.Str("replacement").Replace('\n', '\r'),
        };
        replacement.TextChanged += (_, _) =>
        {
            draft["replacement"] = replacement.Text.Replace("\r\n", "\n").Replace('\r', '\n');
            refresh();
        };
        form.Children.Add(Form.Mono(replacement));
        CheckBox? stop = null;
        var substitute = Form.Check(draft, "substitute", "Replace only what matched, wherever it appears", "Then later aliases still apply, as in BeipMU", () =>
        {
            stop!.IsEnabled = draft.Bool("substitute");
            refresh();
        });
        stop = Form.Check(draft, "stop", "Don’t apply later aliases after this one");
        stop.IsEnabled = draft.Bool("substitute");
        form.Children.Add(Stack(0, substitute, stop));
        form.Children.Add(tester);
        ShowEditor("alias", index, index >= 0 ? "Edit Alias" : "New Alias", draft, form, () => PatternProblem(draft));
    }

    private void EditMacro(int index)
    {
        var draft = index >= 0 ? List[index]?.DeepClone() as JsonObject ?? [] : new JsonObject { ["key"] = "", ["command"] = "" };
        var form = new StackPanel { Spacing = 10 };
        var key = Form.Text(draft, "key", "Key", "Click here and press the key");
        // Pressing a key here records it, rather than typing its letter.
        key.PreviewKeyDown += (_, e) =>
        {
            if (e.Key is Windows.System.VirtualKey.Tab || Keys.Text(e) is not { } chord) return;
            key.Text = chord;
            key.Select(chord.Length, 0);
            e.Handled = true;
        };
        form.Children.Add(key);
        form.Children.Add(Form.Caption("A key on its own or with Ctrl, Alt and Shift: F5, Ctrl+K, Shift+KP_8 (the keypad’s 8)."));
        form.Children.Add(Form.Text(draft, "command", "Command"));
        form.Children.Add(Form.Check(draft, "insert", "Put it in the input box instead of sending it"));
        ShowEditor("macro", index, index >= 0 ? "Edit Macro" : "New Macro", draft, form, () =>
        {
            if (Core.CallAnywhere("keys.parse", Json.Args(("text", draft.Str("key"))))?.GetValue<string>() is not { } chord) return "That isn’t a key Familiar knows. Click the box and press the key you want.";
            draft["key"] = chord;
            return draft.Str("command").Length == 0 ? "Give it a command to send." : null;
        });
    }

    private async void EditVariable(string? original, string value)
    {
        var draft = new JsonObject { ["name"] = original ?? "", ["value"] = value };
        var problem = new TextBlock { Foreground = Form.Brush("SystemFillColorCriticalBrush"), TextWrapping = TextWrapping.Wrap, Visibility = Visibility.Collapsed };
        var form = Stack(10, Form.Text(draft, "name", "Name"), Form.Text(draft, "value", "Value"), problem);
        form.Width = 400;
        var dialog = Form.Dialog(XamlRoot, original is null ? "New Variable" : "Edit Variable", form, "Save");
        dialog.PrimaryButtonClick += (_, e) =>
        {
            var name = draft.Str("name").Trim();
            var message = name.Length == 0 ? "Give the variable a name." : name != original && Variables?.ContainsKey(name) == true ? $"There’s already a variable called {name}." : null;
            if (message is null) return;
            problem.Text = message;
            problem.Visibility = Visibility.Visible;
            e.Cancel = true;
        };
        if (await Form.Show(dialog) != ContentDialogResult.Primary) return;
        Change(rules =>
        {
            if (rules["variables"] is not JsonObject variables) rules["variables"] = variables = [];
            if (original is not null) variables.Remove(original);
            variables[draft.Str("name").Trim()] = draft.Str("value");
        });
    }
}

/// <summary>A colour that can also be "none": a check box, and a swatch that opens a picker.</summary>
internal sealed class ColorChoice : StackPanel
{
    public ColorChoice(string label, JsonObject o, string key, Color fallback, Action changed)
    {
        Orientation = Orientation.Horizontal;
        Spacing = 4;
        var current = Json.Hex(o.OptStr(key));
        var on = new CheckBox { Content = label, IsChecked = current is not null, MinWidth = 0 };
        var swatch = new Border { Width = 28, Height = 18, CornerRadius = new CornerRadius(3), Background = new SolidColorBrush(current ?? fallback) };
        var picker = new ColorPicker { Color = current ?? fallback, IsAlphaEnabled = false, IsMoreButtonVisible = true, IsColorSpectrumVisible = true };
        var button = new Button { Content = swatch, Padding = new Thickness(5), Flyout = new Flyout { Content = picker }, IsEnabled = current is not null, VerticalAlignment = VerticalAlignment.Center };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(button, $"Choose the {label.ToLowerInvariant()}");
        void Apply()
        {
            button.IsEnabled = on.IsChecked == true;
            var c = picker.Color;
            swatch.Background = new SolidColorBrush(c);
            if (on.IsChecked == true) o[key] = $"#{c.R:x2}{c.G:x2}{c.B:x2}";
            else o.Remove(key);
            changed();
        }
        on.Checked += (_, _) => Apply();
        on.Unchecked += (_, _) => Apply();
        picker.ColorChanged += (_, _) => Apply();
        Children.Add(on);
        Children.Add(button);
    }
}
