using System.Text.Json.Nodes;
using CommunityToolkit.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Familiar;

/// <summary>A session on screen: its output, the status line under it, the input box, and a
/// dock at the side for what the world adds (spawn windows, stats, the map, web views).</summary>
internal sealed class SessionView : UserControl
{
    public Session Session { get; }
    public OutputView Output { get; }
    public InputBox Input { get; }

    private readonly TextBlock status = new() { Visibility = Visibility.Collapsed, Margin = new Thickness(4, 6, 4, 0), TextTrimming = TextTrimming.CharacterEllipsis };
    private readonly InfoBar dropped = new() { Severity = InfoBarSeverity.Warning, Title = "Disconnected", IsOpen = false, Margin = new Thickness(0, 8, 0, 0) };
    private bool dismissed;
    private readonly TabView dock = new() { VerticalAlignment = VerticalAlignment.Stretch, IsAddTabButtonVisible = false, TabWidthMode = TabViewWidthMode.SizeToContent, CanReorderTabs = true, CloseButtonOverlayMode = TabViewCloseButtonOverlayMode.OnPointerOver };
    private readonly ColumnDefinition dockColumn = new() { Width = new GridLength(0) };
    private readonly GridSplitter splitter = new() { Width = 12, Visibility = Visibility.Collapsed, ResizeBehavior = GridSplitter.GridResizeBehavior.PreviousAndNext };
    private readonly Dictionary<string, TabViewItem> panels = [];
    private readonly Dictionary<string, WebPane> webPanes = [];
    private readonly Dictionary<string, CodeEditorWindow> editors = [];
    private readonly List<OutputView> spawnViews = [];
    /// <summary>The LLM agent working on this world, in the dock.</summary>
    private AgentPane? agent;
    private double dockWidth = 340;

    /// <summary>Asks the window for a tab beside this session's (a puppet's). The last argument
    /// says whether to switch to it.</summary>
    public event Action<string, UIElement, bool>? TabRequested;
    public event Action<UIElement>? TabFocusRequested;

    public SessionView(Session session)
    {
        Session = session;
        var settings = AppState.Settings;
        Output = new OutputView(session.Output, session.Font);
        Input = new InputBox(session.Font, settings.Int("input_history_size"), settings.Bool("spellcheck"));
        status.Style = Form.Style("CaptionTextBlockStyle");
        status.Foreground = Form.Brush("TextFillColorSecondaryBrush");

        var main = new Grid();
        main.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        main.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        main.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        main.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Grid.SetRow(dropped, 1);
        Grid.SetRow(status, 2);
        Grid.SetRow(Input, 3);
        // When the connection ends, a bar says why and offers to reconnect.
        var reconnect = new Button { Content = "Reconnect" };
        reconnect.Click += (_, _) => session.Connect();
        dropped.ActionButton = reconnect;
        dropped.Closed += (_, _) => dismissed = true;
        main.Children.Add(dropped);
        Input.Margin = new Thickness(0, 8, 0, 0);
        main.Children.Add(Card(Output));
        main.Children.Add(status);
        main.Children.Add(Input);

        var root = new Grid { Padding = new Thickness(8) };
        root.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star), MinWidth = 240 });
        root.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        root.ColumnDefinitions.Add(dockColumn);
        Grid.SetColumn(splitter, 1);
        Grid.SetColumn(dock, 2);
        dock.Visibility = Visibility.Collapsed;
        root.Children.Add(main);
        root.Children.Add(splitter);
        root.Children.Add(dock);
        Content = root;
        dock.TabCloseRequested += (_, e) =>
        {
            if (e.Tab.Tag is string id && id.StartsWith("web:")) CloseWebView(id[4..]);
            else if (e.Tab.Tag is "agent") Session.AgentStop();
        };

        Output.FindClosed += Input.FocusInput;
        Output.LinkClicked += session.FollowLink;
        Output.GridSizeChanged += session.Resize;
        Output.TypingStarted += Input.FocusInput;
        Input.Submitted += (text, secret) =>
        {
            Output.ClearSelection();
            Output.ScrollToEnd();
            session.Submit(text, secret);
        };
        Input.Key = e =>
        {
            if (Keys.Chord(e) is not { } chord || !session.RunMacro(chord)) return false;
            Output.ScrollToEnd();
            return true;
        };
        Input.Page += Output.ScrollPage;
        Input.CopyOutput = () =>
        {
            if (!Output.HasSelection) return false;
            Output.Copy();
            return true;
        };
        session.PasswordMode += Input.SetPasswordMode;
        session.SetInput += text =>
        {
            Input.Text = text;
            Input.FocusInput();
        };
        session.Changed += Refresh;
        Refresh();
        if (session.Info.Bool("has_map")) ShowMap(select: false);
        session.Attach(Handle);
        // Whenever this tab comes on screen, typing goes to the input.
        Loaded += (_, _) => Input.FocusInput();
    }

    /// <summary>The output keeps the world's own colours; a rounded card sets it apart from the window.</summary>
    private static Border Card(UIElement child) => new()
    {
        CornerRadius = new CornerRadius(8),
        BorderThickness = new Thickness(1),
        BorderBrush = Form.Brush("CardStrokeColorDefaultBrush"),
        Background = new SolidColorBrush(AppState.Background),
        Child = child,
    };

    /// <summary>Picks up what may have changed in the session or the config.</summary>
    private void Refresh()
    {
        var settings = AppState.Settings;
        var font = Session.Font;
        if (!Session.Dropped) dismissed = false;
        dropped.Message = Session.DropReason ?? "The connection was closed.";
        dropped.IsOpen = Session.Dropped && !dismissed;
        status.Text = Session.StatusText;
        status.Visibility = Session.StatusText.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        Output.Font = font;
        foreach (var spawn in spawnViews) spawn.Font = font;
        Input.Font = font;
        Input.Spellcheck = settings.Bool("spellcheck");
        Input.HistorySize = settings.Int("input_history_size");
    }

    /// <summary>Closes the windows that belong to this session. The session itself is the caller's to close.</summary>
    public void Shut()
    {
        foreach (var pane in webPanes.Values.ToList()) pane.Shut();
        webPanes.Clear();
        foreach (var editor in editors.Values.ToList()) editor.Close();
        editors.Clear();
        agent?.Shut();
        agent = null;
    }

    // MARK: The side dock

    private TabViewItem ShowPanel(string id, string title, string glyph, Func<UIElement> make, bool select, bool closable = false)
    {
        if (!panels.TryGetValue(id, out var tab))
        {
            tab = new TabViewItem { Header = title, Tag = id, IsClosable = closable, IconSource = new FontIconSource { Glyph = glyph }, Content = make() };
            panels[id] = tab;
            dock.TabItems.Add(tab);
        }
        if (select || dock.SelectedItem is null) dock.SelectedItem = tab;
        if (dock.Visibility == Visibility.Collapsed)
        {
            dock.Visibility = splitter.Visibility = Visibility.Visible;
            dockColumn.Width = new GridLength(dockWidth);
            dockColumn.MinWidth = 180;
        }
        return tab;
    }

    private void RemovePanel(string id)
    {
        if (!panels.Remove(id, out var tab)) return;
        dock.TabItems.Remove(tab);
        if (dock.TabItems.Count > 0) return;
        dockWidth = Math.Max(180, dockColumn.ActualWidth);
        dockColumn.MinWidth = 0;
        dockColumn.Width = new GridLength(0);
        dock.Visibility = splitter.Visibility = Visibility.Collapsed;
    }

    private void ShowMap(bool select)
    {
        var tab = ShowPanel("map", "Map", "", () => new MapView(Session), select);
        (tab.Content as MapView)?.Reload();
    }

#if DEBUG
    internal string MapDebug => panels.TryGetValue("map", out var tab) && tab.Content is MapView map ? map.Debug : "no map";
#endif

    // MARK: What the engine says

    private void Handle(JsonNode? o)
    {
        switch (o.Str("t"))
        {
            case "spawn":
                var name = o.Str("name");
                ShowPanel($"spawn:{name}", name, "", () =>
                {
                    var view = new OutputView(Session.Spawns[name], Session.Font);
                    view.LinkClicked += Session.FollowLink;
                    view.TypingStarted += Input.FocusInput;
                    spawnViews.Add(view);
                    return new Border { Background = new SolidColorBrush(AppState.Background), Child = view };
                }, select: false);
                break;
            case "stats":
                var pane = o.Str("pane");
                if (o.IsNull("data")) RemovePanel($"stats:{pane}");
                else (ShowPanel($"stats:{pane}", pane, "", () => new StatsPane(), select: false).Content as StatsPane)?.Update(o.Get("data"));
                break;
            case "tilemap":
                var map = o.Str("name");
                var isNew = !panels.ContainsKey($"tilemap:{map}");
                (ShowPanel($"tilemap:{map}", map, "", () => new TilemapView(Session.Output.Notice), select: isNew).Content as TilemapView)?.Update(o.Get("map"));
                break;
            case "map_changed":
                if (panels.TryGetValue("map", out var existing)) (existing.Content as MapView)?.Reload();
                else if (Session.Map().Arr("rooms").Count > 0) ShowMap(select: false);
                break;
            case "show_map": ShowMap(select: true); break;
            case "open_puppet":
                var puppet = o.Str("name");
                if (!puppets.TryGetValue(puppet, out var view))
                {
                    puppets[puppet] = view = new PuppetView(Session, puppet, Session.Puppets[puppet]);
                    TabRequested?.Invoke(puppet, view, o.Bool("focus"));
                }
                else if (o.Bool("focus")) TabFocusRequested?.Invoke(view);
                break;
            case "edit": OpenEditor(o.Get("request")); break;
            case "compile_error":
                if (editors.TryGetValue(o.Str("reference"), out var editor)) editor.AddError(o.Int("line"), o.Str("message"));
                break;
            case "webview": WebView(o.Get("command") ?? new JsonObject()); break;
            case "agent_start":
                agent?.Shut();
                RemovePanel("agent");
                var started = new AgentPane(Session, o.Str("name"));
                agent = started;
                ShowPanel("agent", $"✦ {started.AgentName}", "\uE756", () => started, select: true, closable: true);
                started.FocusTerminal();
                break;
            case "agent_output": agent?.Write(o.Str("data")); break;
            case "agent_focus":
                if (agent is not null && panels.TryGetValue("agent", out var agentTab))
                {
                    dock.SelectedItem = agentTab;
                    agent.FocusTerminal();
                }
                break;
            case "agent_stop":
                RemovePanel("agent");
                agent?.Shut();
                agent = null;
                break;
            case "display": ToPages(Json.Args(("type", "display"), ("text", o.Get("text")))); break;
            case "gmcp": ToPages(Json.Args(("type", "gmcp"), ("package", o.Get("package")), ("data", o.Get("data")))); break;
            case "received": ToPages(Json.Args(("type", "receive"), ("text", o.Get("text")))); break;
            case "sent": ToPages(Json.Args(("type", "send"), ("text", o.Get("text")))); break;
        }
    }

    // MARK: Puppets

    private readonly Dictionary<string, PuppetView> puppets = [];

    /// <summary>The user closed a puppet's tab.</summary>
    public void PuppetClosed(UIElement view)
    {
        foreach (var (name, puppet) in puppets.ToList())
        {
            if (!ReferenceEquals(puppet, view)) continue;
            puppets.Remove(name);
            Session.PuppetClosed(name);
        }
    }

    // MARK: Editors

    private void OpenEditor(JsonNode? request)
    {
        if (request is null) return;
        var reference = request.Str("reference");
        // The server sending the same program again reuses its editor.
        if (request.Str("kind") != "local" && editors.TryGetValue(reference, out var existing))
        {
            existing.Reload(request);
            return;
        }
        var editor = new CodeEditorWindow(Session, request);
        editors[reference] = editor;
        editor.Closed += (_, _) =>
        {
            if (editors.TryGetValue(reference, out var current) && ReferenceEquals(current, editor)) editors.Remove(reference);
        };
        editor.Activate();
    }

    // MARK: Web views

    private void ToPages(JsonNode message)
    {
        foreach (var pane in webPanes.Values) pane.Deliver(message);
    }

    private void WebView(JsonNode command)
    {
        var id = command.Str("id");
        if (command.Str("op") == "close")
        {
            CloseWebView(id);
            return;
        }
        if (!command.Bool("from_server"))
        {
            OpenWebView(command);
            return;
        }
        var world = AppState.World(Session.WorldName);
        if (world is null) AskWebView(command, canRemember: false);
        // Not asked yet.
        else if (world.IsNull("allow_webviews")) AskWebView(command, canRemember: true);
        else if (world.Bool("allow_webviews")) OpenWebView(command);
        else Session.Output.Notice("Blocked a web view from the server (web views are off for this world).");
    }

    private async void AskWebView(JsonNode command, bool canRemember)
    {
        var target = command.OptStr("url") ?? "a page it sent";
        var text = new TextBlock { Text = $"{Session.Title} wants to open {target} inside Familiar. Pages it opens can send commands and read the output, like a script.", TextWrapping = TextWrapping.Wrap };
        var dialog = Form.Dialog(XamlRoot, "Open a Web View?", text, "Allow Once", "Block");
        if (canRemember) dialog.SecondaryButtonText = "Always Allow";
        var answer = await Form.Show(dialog);
        if (Session.IsClosed) return;
        if (answer == ContentDialogResult.Secondary)
        {
            var name = Session.WorldName;
            AppState.Update(config =>
            {
                if (config.Arr("world").FirstOrDefault(w => w.Str("name") == name) is JsonObject saved) saved["allow_webviews"] = true;
            });
        }
        if (answer is ContentDialogResult.Primary or ContentDialogResult.Secondary) OpenWebView(command);
        else Session.Output.Notice("Blocked a web view from the server.");
    }

    private void OpenWebView(JsonNode command)
    {
        var id = command.Str("id");
        if (webPanes.TryGetValue(id, out var pane))
        {
            pane.Load(command);
            if (pane.Docked) dock.SelectedItem = panels.GetValueOrDefault($"web:{id}");
            else pane.Present();
            return;
        }
        webPanes[id] = pane = new WebPane(command, message => PageMessage(id, message));
        Session.HasPages = true;
        if (pane.Docked) ShowPanel($"web:{id}", id, "", () => pane, select: true, closable: true);
        else pane.OpenWindow(id, () => Forget(id));
    }

    private void Forget(string id)
    {
        webPanes.Remove(id);
        Session.HasPages = webPanes.Count > 0;
    }

    private void CloseWebView(string id)
    {
        if (!webPanes.TryGetValue(id, out var pane)) return;
        Forget(id);
        if (pane.Docked) RemovePanel($"web:{id}");
        pane.Shut();
    }

    /// <summary>A request from a page's script (see webview_bridge.js).</summary>
    private void PageMessage(string id, JsonNode message)
    {
        var text = message.Str("text");
        switch (message.Str("op"))
        {
            case "send": Session.Call("session.send_text", Json.Args(("text", text), ("aliases", message.Bool("aliases")))); break;
            case "gmcp": Session.Call("session.gmcp", Json.Args(("package", message.Str("package")), ("data", message.Get("data")))); break;
            case "receive": Session.Call("session.inject", Json.Args(("text", text))); break;
            case "display":
                try
                {
                    Session.Output.AppendLine(Core.CallAnywhere("ansi.parse", Json.Args(("text", text))));
                }
                catch (CoreException)
                {
                    // Nothing to show.
                }
                break;
            case "close": CloseWebView(id); break;
        }
    }
}
