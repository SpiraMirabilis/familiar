using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Familiar;

/// <summary>What a new tab shows: the saved worlds, the built-in directory of public ones, and
/// a box that searches both or takes an address.</summary>
internal sealed class ConnectPage : UserControl
{
    private sealed record Row(string Id, JsonNode World, string? Character, string Title, string Detail, bool Saved);

    private readonly AutoSuggestBox search = new() { PlaceholderText = "Search worlds, or type host:port", QueryIcon = new SymbolIcon(Symbol.Find) };
    private readonly ListView list = new() { SelectionMode = ListViewSelectionMode.Single };
    private readonly Button connect = new() { Content = "Connect", Style = Form.Style("AccentButtonStyle"), MinWidth = 96 };
    private readonly Button edit = new() { Content = "Edit…" };
    private readonly Button delete = new() { Content = "Delete" };
    private static IList<JsonNode?>? directory;

    /// <summary>The user chose a world (and perhaps a character) to open in this tab.</summary>
    public event Action<JsonNode, string?>? Chosen;

    public ConnectPage()
    {
        directory ??= Core.CallAnywhere("directory").Items();
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(list, "Worlds");

        var title = new TextBlock { Text = "Connect", Style = Form.Style("TitleTextBlockStyle") };
        var create = new Button { Content = "New World…", HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Center };
        var header = new Grid();
        header.Children.Add(title);
        header.Children.Add(create);

        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right };
        buttons.Children.Add(delete);
        buttons.Children.Add(edit);
        buttons.Children.Add(connect);

        // The list sits on a card, as lists do in Windows 11's own apps.
        var card = new Border
        {
            Child = list,
            CornerRadius = new CornerRadius(8),
            BorderThickness = new Thickness(1),
            BorderBrush = Form.Brush("CardStrokeColorDefaultBrush"),
            Background = Form.Brush("CardBackgroundFillColorDefaultBrush"),
            Padding = new Thickness(0, 4, 0, 4),
        };
        var root = new Grid { MaxWidth = 760, Padding = new Thickness(24, 28, 24, 24), RowSpacing = 16 };
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Grid.SetRow(search, 1);
        Grid.SetRow(card, 2);
        Grid.SetRow(buttons, 3);
        root.Children.Add(header);
        root.Children.Add(search);
        root.Children.Add(card);
        root.Children.Add(buttons);
        Content = root;

        search.TextChanged += (_, _) => Fill();
        search.QuerySubmitted += (_, _) => Open(Selected ?? FirstRow);
        search.PreviewKeyDown += (_, e) =>
        {
            // Down from the box goes into the list.
            if (e.Key != VirtualKey.Down || list.Items.Count == 0) return;
            if (list.SelectedItem is null) Select(FirstRow?.Id);
            (list.SelectedItem as ListViewItem)?.Focus(FocusState.Keyboard);
            e.Handled = true;
        };
        list.SelectionChanged += (_, _) => UpdateButtons();
        list.DoubleTapped += (_, _) => Open(Selected);
        list.PreviewKeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Enter)
            {
                Open(Selected);
                e.Handled = true;
            }
            else if (e.Key == VirtualKey.Delete && Selected is { Saved: true })
            {
                Delete();
                e.Handled = true;
            }
        };
        connect.Click += (_, _) => Open(Selected ?? FirstRow);
        edit.Click += (_, _) => Edit();
        delete.Click += (_, _) => Delete();
        create.Click += async (_, _) =>
        {
            // What's typed in the box is a start, if it's an address.
            var world = Quick()?.World ?? AppState.Defaults.Get("world")?.DeepClone() ?? new JsonObject();
            if (await WorldDialog.Edit(XamlRoot, world, isNew: true) is { } saved)
            {
                search.Text = "";
                Fill();
                Select($"saved:{saved.Str("name")}");
            }
        };
        AppState.ConfigChanged += Fill;
        Unloaded += (_, _) =>
        {
            if (!IsLoaded) AppState.ConfigChanged -= Fill;
        };
        Loaded += (_, _) =>
        {
            AppState.ConfigChanged -= Fill;
            AppState.ConfigChanged += Fill;
            search.Focus(FocusState.Programmatic);
        };
        Fill();
    }

    private Row? Selected => (list.SelectedItem as ListViewItem)?.Tag as Row;
    private Row? FirstRow => list.Items.OfType<ListViewItem>().Select(i => i.Tag).OfType<Row>().FirstOrDefault();

    private static string Address(JsonNode? world) => $"{world.Str("host")}:{world.Int("port")}" + (world.Bool("tls") ? " · TLS" : "");

    private bool Matches(params string[] fields)
    {
        var words = search.Text.ToLowerInvariant().Split(' ', StringSplitOptions.RemoveEmptyEntries);
        return words.All(word => fields.Any(f => f.Contains(word, StringComparison.OrdinalIgnoreCase)));
    }

    /// <summary>What's typed, if it's an address to connect to.</summary>
    private Row? Quick()
    {
        var query = search.Text.Trim();
        if (query.Length == 0 || Core.CallAnywhere("address.parse", Json.Args(("text", query))) is not { } world) return null;
        var host = world.Str("host");
        if (!host.Contains('.') && host != "localhost" && !query.Contains(':')) return null;
        return new Row("quick", world, null, $"Connect to {world.Str("name")}", "", false);
    }

    private void Fill()
    {
        var selected = Selected?.Id;
        list.Items.Clear();
        if (Quick() is { } quick)
        {
            Section("Quick connect");
            Add(quick);
        }
        var saved = new List<Row>();
        foreach (var world in AppState.Worlds)
        {
            if (world is null || !Matches(world.Str("name"), world.Str("host"))) continue;
            var name = world.Str("name");
            var characters = world.Arr("character").Select(c => c.Str("name")).Where(c => c.Length > 0).ToList();
            // A world with characters is opened as one of them.
            if (characters.Count == 0) saved.Add(new Row($"saved:{name}", world, null, name, "", true));
            foreach (var character in characters) saved.Add(new Row($"saved:{name}/{character}", world, character, $"{character} @ {name}", "", true));
        }
        if (saved.Count > 0) Section("My worlds");
        saved.ForEach(Add);
        var listed = (directory ?? []).Where(e => Matches([e.Str("name"), e.Str("host"), e.Str("description"), .. e.Arr("keywords").Select(k => k?.ToString() ?? "")])).ToList();
        if (listed.Count > 0) Section("Directory");
        foreach (var entry in listed)
        {
            var world = entry.Get("world") ?? new JsonObject();
            // A world with a guest account is opened as the guest.
            Add(new Row($"dir:{entry.Str("name")}", world, world.Arr("character").FirstOrDefault().OptStr("name"), world.Str("name"), entry.Str("description").Split('\n')[0].Trim(), false));
        }
        Select(selected);
        if (list.SelectedItem is null && search.Text.Trim().Length > 0) Select(FirstRow?.Id);
        UpdateButtons();
    }

    private void Section(string title)
    {
        list.Items.Add(new ListViewItem
        {
            Content = new TextBlock { Text = title, Style = Form.Style("BodyStrongTextBlockStyle") },
            IsHitTestVisible = false,
            IsTabStop = false,
            AllowFocusOnInteraction = false,
            MinHeight = 32,
            Padding = new Thickness(16, 10, 16, 2),
        });
    }

    private void Add(Row row)
    {
        var grid = new Grid { ColumnSpacing = 12, Padding = new Thickness(0, 7, 0, 7) };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var text = new StackPanel();
        text.Children.Add(new TextBlock { Text = row.Title, TextTrimming = TextTrimming.CharacterEllipsis });
        if (row.Detail.Length > 0)
        {
            var detail = Form.Caption(row.Detail);
            detail.TextWrapping = TextWrapping.NoWrap;
            detail.TextTrimming = TextTrimming.CharacterEllipsis;
            text.Children.Add(detail);
        }
        var address = Form.Caption(Address(row.World));
        address.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(address, 1);
        grid.Children.Add(text);
        grid.Children.Add(address);
        var item = new ListViewItem { Content = grid, Tag = row, HorizontalContentAlignment = HorizontalAlignment.Stretch };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(item, $"{row.Title}, {Address(row.World)}");
        if (row.Saved)
        {
            var menu = new MenuFlyout();
            var open = new MenuFlyoutItem { Text = "Connect" };
            open.Click += (_, _) => Open(row);
            var change = new MenuFlyoutItem { Text = "Edit…", Icon = new SymbolIcon(Symbol.Edit) };
            change.Click += (_, _) =>
            {
                list.SelectedItem = item;
                Edit();
            };
            var remove = new MenuFlyoutItem { Text = "Delete", Icon = new SymbolIcon(Symbol.Delete) };
            remove.Click += (_, _) =>
            {
                list.SelectedItem = item;
                Delete();
            };
            menu.Items.Add(open);
            menu.Items.Add(change);
            menu.Items.Add(new MenuFlyoutSeparator());
            menu.Items.Add(remove);
            item.ContextFlyout = menu;
        }
        list.Items.Add(item);
    }

    private void Select(string? id)
    {
        if (id is null) return;
        list.SelectedItem = list.Items.OfType<ListViewItem>().FirstOrDefault(i => i.Tag is Row row && row.Id == id);
    }

    private void UpdateButtons()
    {
        var row = Selected;
        connect.IsEnabled = row is not null || FirstRow is not null;
        edit.Visibility = delete.Visibility = row is { Saved: true } ? Visibility.Visible : Visibility.Collapsed;
    }

    private void Open(Row? row)
    {
        if (row is not null) Chosen?.Invoke(row.World, row.Character);
    }

    private async void Edit()
    {
        if (Selected is not { Saved: true } row) return;
        await WorldDialog.Edit(XamlRoot, row.World, isNew: false);
    }

    private async void Delete()
    {
        if (Selected is not { Saved: true } row) return;
        var name = row.World.Str("name");
        if (!await Form.Confirm(XamlRoot, $"Delete {name}?", "Its characters, triggers, aliases and other settings are deleted with it. Its map and logs are kept.", "Delete")) return;
        AppState.Update(config =>
        {
            if (config["world"] is not JsonArray worlds) return;
            foreach (var world in worlds.Where(w => w.Str("name") == name).ToList()) worlds.Remove(world);
            if (worlds.Count == 0) ((JsonObject)config).Remove("world");
        });
    }
}
