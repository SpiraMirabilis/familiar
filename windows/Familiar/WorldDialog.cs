using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Familiar;

/// <summary>Edits a saved world: its server, its characters and their puppets.</summary>
internal static class WorldDialog
{
    /// <summary>Shows the editor for a copy of `world`. Returns the edited world once it's
    /// been saved, or null if the user cancelled.</summary>
    public static async Task<JsonNode?> Edit(XamlRoot root, JsonNode world, bool isNew)
    {
        var draft = (JsonObject)world.DeepClone();
        var original = isNew ? null : world.Str("name");
        var problem = new TextBlock { Foreground = Form.Brush("SystemFillColorCriticalBrush"), TextWrapping = TextWrapping.Wrap, Visibility = Visibility.Collapsed };

        var form = new StackPanel { Spacing = 10, Width = 460 };
        form.Children.Add(Form.Text(draft, "name", "Name"));
        var address = new Grid { ColumnSpacing = 8 };
        address.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        address.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var port = Form.Number(draft, "port", "Port", 1, 65535);
        Grid.SetColumn(port, 1);
        address.Children.Add(Form.Text(draft, "host", "Host", "example.org"));
        address.Children.Add(port);
        form.Children.Add(address);
        CheckBox? verify = null;
        var tls = Form.Check(draft, "tls", "Use TLS", changed: () => verify!.IsEnabled = draft.Bool("tls"));
        verify = Form.Check(draft, "verify_certificate", "Verify the server’s certificate", "Turn off for servers with self-signed certificates");
        verify.IsEnabled = draft.Bool("tls");
        form.Children.Add(Stack(tls, verify,
            Form.Check(draft, "bouncer", "Connect through the bouncer", "The bouncer set up in config.toml keeps the character connected while you’re away"),
            Form.Check(draft, "mcp", "Use MCP if the server offers it")));
        var encoding = Form.Choice("Encoding", new[] { "Auto", "UTF-8", "Windows-1252", "CP437" }.Select(e => (e, e)), draft.Str("encoding"), v => draft["encoding"] = v);
        ToolTipService.SetToolTip(encoding, "Auto reads UTF-8 and falls back to Windows-1252");
        form.Children.Add(encoding);

        form.Children.Add(Form.Heading("Programs and scripts"));
        var language = Form.Choice("Program language", new[] { ("MOO", ""), ("MUF", "muf"), ("Lua", "lua") }, draft.Str("program_language"), v => draft["program_language"] = v);
        ToolTipService.SetToolTip(language, "For the code editor (MCP simpleedit and /edit)");
        form.Children.Add(language);
        var script = Form.Text(draft, "script", "Script file", "Optional");
        var choose = new Button { Content = "Browse…", VerticalAlignment = VerticalAlignment.Bottom };
        choose.Click += async (_, _) =>
        {
            if (await Form.PickFile(".js") is { } path) script.Text = path;
        };
        var scriptRow = new Grid { ColumnSpacing = 8 };
        scriptRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        scriptRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(choose, 1);
        scriptRow.Children.Add(script);
        scriptRow.Children.Add(choose);
        form.Children.Add(scriptRow);
        var web = draft.IsNull("allow_webviews") ? "ask" : draft.Bool("allow_webviews") ? "allow" : "block";
        form.Children.Add(Form.Choice("Web views from the server", new[] { ("Ask", "ask"), ("Allow", "allow"), ("Block", "block") }, web, v =>
        {
            if (v == "ask") draft.Remove("allow_webviews");
            else draft["allow_webviews"] = v == "allow";
        }));

        form.Children.Add(Form.Heading("Appearance"));
        var ownFont = new CheckBox { Content = "Use a different font for this world", IsChecked = draft.Str("font").Trim().Length > 0 };
        var font = new FontPicker(draft.Str("font").Trim().Length > 0 ? draft.Str("font") : AppState.Settings.Str("font")) { Visibility = ownFont.IsChecked == true ? Visibility.Visible : Visibility.Collapsed };
        void SetFont()
        {
            font.Visibility = ownFont.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;
            draft["font"] = ownFont.IsChecked == true ? font.Description : "";
        }
        ownFont.Checked += (_, _) => SetFont();
        ownFont.Unchecked += (_, _) => SetFont();
        font.Changed += SetFont;
        form.Children.Add(ownFont);
        form.Children.Add(font);

        form.Children.Add(Form.Heading("Characters"));
        form.Children.Add(Form.Caption("A character’s login command is sent right after connecting. It’s stored in plain text."));
        var characters = new StackPanel { Spacing = 6 };
        if (draft["character"] is not JsonArray list) draft["character"] = list = [];
        foreach (var character in list.OfType<JsonObject>().ToList()) characters.Children.Add(CharacterEditor(list, character, characters));
        form.Children.Add(characters);
        var add = new Button { Content = "Add Character" };
        add.Click += (_, _) =>
        {
            var character = new JsonObject { ["name"] = "" };
            list.Add(character);
            var editor = CharacterEditor(list, character, characters);
            editor.IsExpanded = true;
            characters.Children.Add(editor);
        };
        form.Children.Add(add);
        form.Children.Add(problem);

        var dialog = Form.Dialog(root, isNew ? "New World" : $"Edit {original}", new ScrollViewer { Content = form, Padding = new Thickness(0, 0, 16, 0) }, "Save");
        dialog.Resources["ContentDialogMaxWidth"] = 560.0;
        dialog.PrimaryButtonClick += (_, e) =>
        {
            var message = Check(draft, original) ?? Save(draft, original);
            if (message is null) return;
            problem.Text = message;
            problem.Visibility = Visibility.Visible;
            e.Cancel = true;
        };
        return await Form.Show(dialog) == ContentDialogResult.Primary ? AppState.World(draft.Str("name")) : null;
    }

    private static StackPanel Stack(params UIElement[] children)
    {
        var panel = new StackPanel();
        foreach (var child in children) panel.Children.Add(child);
        return panel;
    }

    private static string? Check(JsonObject draft, string? original)
    {
        var name = draft.Str("name").Trim();
        draft["name"] = name;
        draft["host"] = draft.Str("host").Trim();
        if (name.Length == 0) return "Give the world a name.";
        if (draft.Str("host").Length == 0) return "Give the world a host to connect to.";
        if (name != original && AppState.World(name) is not null) return $"There’s already a world called {name}.";
        var characters = (JsonArray)draft["character"]!;
        foreach (var character in characters.OfType<JsonObject>().ToList())
        {
            character["name"] = character.Str("name").Trim();
            if (character.Str("name").Length == 0)
            {
                if (character.Str("login").Trim().Length > 0) return "Give every character a name.";
                characters.Remove(character);
                continue;
            }
            if (character["puppet"] is not JsonArray puppets) continue;
            foreach (var puppet in puppets.OfType<JsonObject>().Where(p => p.Str("name").Trim().Length == 0).ToList()) puppets.Remove(puppet);
            if (puppets.Count == 0) character.Remove("puppet");
        }
        if (characters.OfType<JsonObject>().GroupBy(c => c.Str("name")).Any(g => g.Count() > 1)) return "Two characters have the same name.";
        if (characters.Count == 0) draft.Remove("character");
        return null;
    }

    private static string? Save(JsonObject draft, string? original) => AppState.Update(config =>
    {
        if (config["world"] is not JsonArray worlds) config["world"] = worlds = [];
        var at = original is null ? -1 : worlds.ToList().FindIndex(w => w.Str("name") == original);
        if (at >= 0) worlds[at] = draft.DeepClone();
        else worlds.Add(draft.DeepClone());
    });

    private static Expander CharacterEditor(JsonArray list, JsonObject character, StackPanel owner)
    {
        var expander = new Expander { HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch };
        void Title() => expander.Header = character.Str("name").Length == 0 ? "New Character" : character.Str("name");
        Title();
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(Form.Text(character, "name", "Name", changed: Title));
        panel.Children.Add(Form.Text(character, "login", "Login command", "connect name password"));

        var puppets = new StackPanel { Spacing = 6 };
        if (character["puppet"] is not JsonArray puppetList) character["puppet"] = puppetList = [];
        foreach (var puppet in puppetList.OfType<JsonObject>().ToList()) puppets.Children.Add(PuppetEditor(puppetList, puppet, puppets));
        panel.Children.Add(puppets);

        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        var addPuppet = new Button { Content = "Add Puppet" };
        ToolTipService.SetToolTip(addPuppet, "A puppet is a second character played through this one; its lines get a tab of their own.");
        addPuppet.Click += (_, _) =>
        {
            var puppet = AppState.Defaults.Get("puppet")?.DeepClone() as JsonObject ?? [];
            puppetList.Add(puppet);
            var editor = PuppetEditor(puppetList, puppet, puppets);
            editor.IsExpanded = true;
            puppets.Children.Add(editor);
        };
        var remove = new Button { Content = "Delete Character" };
        remove.Click += (_, _) =>
        {
            list.Remove(character);
            owner.Children.Remove(expander);
        };
        buttons.Children.Add(addPuppet);
        buttons.Children.Add(remove);
        panel.Children.Add(buttons);
        expander.Content = panel;
        return expander;
    }

    private static Expander PuppetEditor(JsonArray list, JsonObject puppet, StackPanel owner)
    {
        var expander = new Expander { HorizontalAlignment = HorizontalAlignment.Stretch, HorizontalContentAlignment = HorizontalAlignment.Stretch };
        void Title() => expander.Header = puppet.Str("name").Length == 0 ? "New Puppet" : $"Puppet: {puppet.Str("name")}";
        Title();
        var panel = new StackPanel { Spacing = 10 };
        panel.Children.Add(Form.Text(puppet, "name", "Name", changed: Title));
        panel.Children.Add(Form.Text(puppet, "receive_prefix", "Lines start with"));
        panel.Children.Add(Stack(
            Form.Check(puppet, "regex", "That’s a regular expression"),
            Form.Check(puppet, "hide_prefix", "Hide it in the puppet’s tab")));
        panel.Children.Add(Form.Text(puppet, "send_prefix", "Put before what’s typed", "@force puppet="));
        panel.Children.Add(Stack(
            Form.Check(puppet, "remove_accidental_prefix", "Don’t add it twice if it was typed"),
            Form.Check(puppet, "auto_open", "Open the tab when a line arrives"),
            Form.Check(puppet, "open_on_connect", "Open the tab on connecting")));
        var remove = new Button { Content = "Delete Puppet" };
        remove.Click += (_, _) =>
        {
            list.Remove(puppet);
            owner.Children.Remove(expander);
        };
        panel.Children.Add(remove);
        expander.Content = panel;
        return expander;
    }
}
