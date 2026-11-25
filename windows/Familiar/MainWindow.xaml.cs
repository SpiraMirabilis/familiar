using System.Runtime.InteropServices;
using System.Text.Json.Nodes;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Familiar;

/// <summary>The window: a tab for each open world, in the title bar.</summary>
public sealed partial class MainWindow : Window
{
    [LibraryImport("user32")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetForegroundWindow(nint window);

    [LibraryImport("user32", EntryPoint = "PostMessageW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool PostMessage(nint window, uint message, nint wParam, nint lParam);

    private bool active = true;
    private bool closing;

    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(DragRegion);
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "familiar.ico"));
        Root.Loaded += (_, _) =>
        {
            var scale = Root.XamlRoot.RasterizationScale;
            // Where it was last time, or a sensible size the first time.
            if (!WindowPlacement.Restore(AppWindow)) AppWindow.Resize(new Windows.Graphics.SizeInt32((int)(1040 * scale), (int)(720 * scale)));
            // Remembered a moment after it stops moving, so it survives however the app ends.
            var settle = DispatcherQueue.CreateTimer();
            settle.Interval = TimeSpan.FromMilliseconds(600);
            settle.IsRepeating = false;
            settle.Tick += (_, _) => WindowPlacement.Save(AppWindow);
            AppWindow.Changed += (_, e) =>
            {
                if (!e.DidPositionChange && !e.DidSizeChange && !e.DidPresenterChange) return;
                settle.Stop();
                settle.Start();
            };
            // Room for the minimise, maximise and close buttons, which sit over the tab strip.
            Footer.Padding = new Thickness(0, 0, AppWindow.TitleBar.RightInset / scale, 0);
        };
        Activated += (_, e) =>
        {
            active = e.WindowActivationState != WindowActivationState.Deactivated;
            if (active) Showing?.Session.Seen();
        };
        AppWindow.Closing += OnClosing;

        // Every shortcut goes through the keymap (keymap.rs), before keyboard macros. They're
        // caught on the way down, before the tab strip (which moves between tabs' buttons with
        // the arrow keys) or the input box can take them.
        Root.PreviewKeyDown += (_, e) =>
        {
            if (Shortcuts.Match("window", e) is not { } action) return;
            e.Handled = true;
            Run(action);
        };
        OpenConnect();
    }

    /// <summary>The output of the tab that's showing: a session's or a puppet's.</summary>
    private OutputView? ShownOutput => (Tabs.SelectedItem as TabViewItem)?.Content switch
    {
        SessionView view => view.Output,
        PuppetView puppet => puppet.Output,
        _ => null,
    };

    private SessionView? Showing => (Tabs.SelectedItem as TabViewItem)?.Content as SessionView;

    /// <summary>The input box of the tab that's showing: a session's or a puppet's.</summary>
    private InputBox? ShownInput => (Tabs.SelectedItem as TabViewItem)?.Content switch
    {
        SessionView view => view.Input,
        PuppetView puppet => puppet.Input,
        _ => null,
    };

    private IEnumerable<SessionView> Views => Tabs.TabItems.OfType<TabViewItem>().Select(t => t.Content).OfType<SessionView>();

    // MARK: Sessions

    /// <summary>Opens a world, in `replace` (the Connect tab it was chosen from) or a new tab.</summary>
    internal async void OpenSession(JsonNode world, string? character, TabViewItem? replace = null)
    {
        Session session;
        try
        {
            session = new Session(world, character);
        }
        catch (CoreException e)
        {
            await AppState.Alert(Root.XamlRoot, $"Couldn’t open {world.Str("name")}", e.Message);
            return;
        }
        var view = new SessionView(session);
        // Always a tab of its own: a tab view doesn't show content that changes under the tab
        // it's showing, so the Connect tab is swapped for this one rather than reused.
        var tab = new TabViewItem { Content = view };
        // A puppet gets a tab of its own, after its session's.
        view.TabRequested += (title, content, focus) =>
        {
            var puppet = new TabViewItem { Header = title, IconSource = new FontIconSource { Glyph = "\uE77B" }, Content = content, Tag = view };
            Tabs.TabItems.Insert(Math.Min(Tabs.TabItems.IndexOf(tab) + 1 + Tabs.TabItems.OfType<TabViewItem>().Count(t => ReferenceEquals(t.Tag, view)), Tabs.TabItems.Count), puppet);
            if (focus) Tabs.SelectedItem = puppet;
        };
        view.TabFocusRequested += content => Tabs.SelectedItem = Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => ReferenceEquals(t.Content, content));
        session.IsShowing = () => active && ReferenceEquals(Tabs.SelectedItem, tab);
        session.Changed += () => Describe(tab, session);
        Describe(tab, session);
        var at = replace is null ? -1 : Tabs.TabItems.IndexOf(replace);
        if (at >= 0) Tabs.TabItems.Insert(at, tab);
        else Tabs.TabItems.Add(tab);
        Tabs.SelectedItem = tab;
        if (at >= 0) Tabs.TabItems.Remove(replace);
        UpdateTitle();
        DispatcherQueue.TryEnqueue(view.Input.FocusInput);
        session.Connect();
    }

    /// <summary>A new tab to choose a world in, or the one that's already open.</summary>
    private void OpenConnect()
    {
        if (Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => t.Content is ConnectPage) is { } existing)
        {
            Tabs.SelectedItem = existing;
            return;
        }
        var page = new ConnectPage();
        var tab = new TabViewItem { Header = "Connect", IconSource = new FontIconSource { Glyph = "\uE774" }, Content = page };
        page.Chosen += (world, character) => OpenSession(world, character, tab);
        Tabs.TabItems.Add(tab);
        Tabs.SelectedItem = tab;
    }

    private void NextTab(int step)
    {
        var count = Tabs.TabItems.Count;
        if (count > 1) Tabs.SelectedIndex = (Tabs.SelectedIndex + step + count) % count;
    }

    /// <summary>Familiar was started again: come to the front, and open the world it was asked for.</summary>
    internal async void Reopened(string[] arguments)
    {
        if (AppWindow.Presenter is Microsoft.UI.Windowing.OverlappedPresenter { State: Microsoft.UI.Windowing.OverlappedPresenterState.Minimized } presenter) presenter.Restore();
        SetForegroundWindow(WinRT.Interop.WindowNative.GetWindowHandle(this));
        Activate();
        await OpenNamed(arguments);
    }

    /// <summary>Opens a world where the Connect tab is, if that's all there is, as choosing it there would.</summary>
    internal void OpenInPlace(JsonNode world, string? character)
    {
        var connect = Tabs.TabItems.Count == 1 ? Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => t.Content is ConnectPage) : null;
        OpenSession(world, character, connect);
    }

    /// <summary>"Familiar WORLD [CHARACTER]" connects to a saved world, as the other clients do.</summary>
    internal async Task OpenNamed(string[] arguments)
    {
        if (arguments.Length == 0) return;
        if (AppState.World(arguments[0]) is { } world)
        {
            OpenInPlace(world, arguments.ElementAtOrDefault(1));
        }
        else
        {
            await Alert($"There’s no saved world called {arguments[0]}", "Choose a world to connect to instead.");
        }
    }

    /// <summary>The tab's title and the icon that says how its connection is.</summary>
    private void Describe(TabViewItem tab, Session session)
    {
        tab.Header = session.Title;
        var glyph = session.State switch
        {
            _ when session.Important => "\uE735",
            _ when session.Unread => "\uE91F",
            ConnectionState.Connecting => "\uE895",
            ConnectionState.Disconnected => "\uF384",
            _ => "\uE774",
        };
        tab.IconSource = new FontIconSource { Glyph = glyph };
        // The tab says what the icon only hints at.
        var world = session.Info.Get("world");
        var note = session.State switch
        {
            ConnectionState.Connecting => "Connecting…",
            ConnectionState.Connected => session.Logging ? "Logging" : "Connected",
            _ => session.DropReason is { } reason ? $"Disconnected: {reason}" : "Disconnected",
        };
        ToolTipService.SetToolTip(tab, $"{world.Str("host")}:{world.Int("port")} — {note}");
        if (ReferenceEquals(Tabs.SelectedItem, tab)) UpdateTitle();
    }

    private void UpdateTitle()
    {
        Title = Showing is { } view ? $"{view.Session.Title} – Familiar" : "Familiar";
    }

    private void CloseTab(TabViewItem tab)
    {
        if (tab.Content is SessionView view)
        {
            // Its puppets' tabs go with it.
            foreach (var puppet in Tabs.TabItems.OfType<TabViewItem>().Where(t => ReferenceEquals(t.Tag, view)).ToList()) Tabs.TabItems.Remove(puppet);
            view.Shut();
            view.Session.Close();
        }
        else if (tab.Tag is SessionView owner && tab.Content is UIElement puppetView)
        {
            owner.PuppetClosed(puppetView);
        }
        Tabs.TabItems.Remove(tab);
        // Closing the last world leaves somewhere to choose another; closing that closes the window.
        if (Tabs.TabItems.Count == 0)
        {
            if (tab.Content is SessionView) OpenConnect();
            else Close();
        }
        UpdateTitle();
    }

    private void OnTabCloseRequested(TabView sender, TabViewTabCloseRequestedEventArgs args) => CloseTab(args.Tab);

    private void OnTabSelected(object sender, SelectionChangedEventArgs e)
    {
        UpdateTitle();
        if ((Tabs.SelectedItem as TabViewItem)?.Content is PuppetView puppet) DispatcherQueue.TryEnqueue(puppet.Input.FocusInput);
        if (Showing is not { } view) return;
        view.Session.Seen();
        // Once the tab's content is on screen.
        DispatcherQueue.TryEnqueue(view.Input.FocusInput);
    }

    private async void OnClosing(Microsoft.UI.Windowing.AppWindow sender, Microsoft.UI.Windowing.AppWindowClosingEventArgs args)
    {
        var connected = Views.Where(v => v.Session.State == ConnectionState.Connected).ToList();
        if (connected.Count > 0 && !closing)
        {
            args.Cancel = true;
            var dialog = new ContentDialog
            {
                XamlRoot = Root.XamlRoot,
                Title = connected.Count == 1 ? $"Disconnect from {connected[0].Session.Title} and close?" : $"Disconnect from {connected.Count} worlds and close?",
                PrimaryButtonText = "Close",
                CloseButtonText = "Cancel",
                DefaultButton = ContentDialogButton.Primary,
            };
            try
            {
                if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
            }
            catch (Exception)
            {
                // Another dialog is open; leave the window as it is.
                return;
            }
            closing = true;
            Close();
            return;
        }
        // Saves each world's map.
        foreach (var view in Views.ToList())
        {
            view.Shut();
            view.Session.Close();
        }
    }

    // MARK: Actions

    /// <summary>Runs one of the client's commands (an action in keymap.rs), from a key, the
    /// menu or the command palette.</summary>
    internal async void Run(string action)
    {
        var view = Showing;
        var session = view?.Session;
        switch (action)
        {
            case "connect": OpenConnect(); break;
            case "new-world": await WorldDialog.Edit(Root.XamlRoot, NewWorld(), isNew: true); break;
            case "edit-world": await EditWorld(session); break;
            case "reconnect": session?.Connect(); break;
            case "disconnect": session?.Disconnect(); break;
            case "close-tab":
                if (Tabs.SelectedItem is TabViewItem tab) CloseTab(tab);
                break;
            case "next-tab": NextTab(1); break;
            case "prev-tab": NextTab(-1); break;
            case ['t', 'a', 'b', '-', var digit] when digit is >= '1' and <= '9':
                if (digit - '1' < Tabs.TabItems.Count) Tabs.SelectedIndex = digit - '1';
                break;
            case "find": ShownOutput?.ShowFind(); break;
            case "find-next": ShownOutput?.FindAgain(1); break;
            case "find-previous": ShownOutput?.FindAgain(-1); break;
            case "clear": session?.Output.Clear(); break;
            case "toggle-log": session?.ToggleLog(); break;
            case "open-logs": OpenFolder(AppState.Start.Str("log_dir")); break;
            case "toggle-sidebar": view?.ToggleDock(); break;
            case "next-panel": view?.StepPanel(1); break;
            case "prev-panel": view?.StepPanel(-1); break;
            case "show-map": view?.ShowMap(select: true); break;
            case "palette": await ShowPalette(); break;
            case "history-search": await SearchHistory(); break;
            case "rules": OnRules(); break;
            case "reopen-editor": view?.ReopenEditor(); break;
            case "restart-scripts": session?.RestartScripts(); break;
            case "stop-speech":
                if (session is not null) session.StopSounds();
                else Core.Send("speech.stop");
                break;
            case "import-beipmu": await SettingsPage.ImportBeipMU(Root.XamlRoot); break;
            case "import-map": await ImportMap(view); break;
            case "preferences": ShowSettings(); break;
            case "keyboard": ShowSettings(keyboard: true); break;
            case "open-settings-folder": OpenFolder(AppState.Start.Str("config_dir")); break;
            case "help": Session.OpenUri("https://github.com/SpiraMirabilis/familiar#readme"); break;
            case "about": await ShowAbout(); break;
            // As the close button does, so open worlds are asked about first.
            case "quit": PostMessage(WinRT.Interop.WindowNative.GetWindowHandle(this), 0x0010, 0, 0); break;
        }
    }

    private static JsonNode NewWorld() => AppState.Defaults.Get("world")?.DeepClone() ?? new JsonObject();

    /// <summary>The world of the tab that's showing; one that isn't saved (a quick connection) is
    /// offered as a new one to save.</summary>
    private async Task EditWorld(Session? session)
    {
        if (session is null) await WorldDialog.Edit(Root.XamlRoot, NewWorld(), isNew: true);
        else if (session.Info.Bool("saved") && AppState.World(session.WorldName) is { } saved) await WorldDialog.Edit(Root.XamlRoot, saved, isNew: false);
        else await WorldDialog.Edit(Root.XamlRoot, session.Info.Get("world")?.DeepClone() ?? NewWorld(), isNew: true);
    }

    private async Task ImportMap(SessionView? view)
    {
        if (view is null)
        {
            await Alert("Open a world first", "A map belongs to a world: connect to it, then import its map.");
            return;
        }
        if (await Form.PickFile(".atlas") is not { } path) return;
        view.Session.Call("map.import", Json.Args(("path", path)));
        view.ShowMap(select: true);
    }

    private static void OpenFolder(string folder)
    {
        Directory.CreateDirectory(folder);
        Session.OpenUri(folder);
    }

    /// <summary>The app's own client commands, besides the engine's.</summary>
    private static readonly (string Usage, string About)[] AppCommands =
    [
        ("/agent [<name>|stop|list]", "run an LLM agent (claude, codex, agy, grok) beside the world, with the world's tools"),
    ];

    /// <summary>Everything the app can do, the saved worlds, the tabs and the client commands, to search.</summary>
    private async Task ShowPalette()
    {
        var items = new List<Picker.Item>();
        foreach (var action in Shortcuts.Actions)
        {
            var id = action.Str("id");
            // Tab N is under Tabs below, for the tabs there are.
            if (action.Str("scope") != "window" || id == "palette" || id.StartsWith("tab-")) continue;
            items.Add(new("Commands", action.Str("label"), Shortcuts.Label(id), () => Run(id)));
        }
        foreach (var world in AppState.Worlds)
        {
            var address = $"{world.Str("host")}:{world.Int("port")}";
            var characters = world.Arr("character");
            if (characters.Count == 0) items.Add(new("Worlds", world.Str("name"), address, () => OpenInPlace(world!, null)));
            foreach (var character in characters)
            {
                var name = character.Str("name");
                items.Add(new("Worlds", $"{name} @ {world.Str("name")}", address, () => OpenInPlace(world!, name)));
            }
        }
        foreach (var (tab, i) in Tabs.TabItems.OfType<TabViewItem>().Select((t, i) => (t, i)))
        {
            items.Add(new("Tabs", $"Switch to {tab.Header}", i < 9 ? Shortcuts.Label($"tab-{i + 1}") : "", () => Tabs.SelectedItem = tab));
        }
        // [usage, description] pairs.
        static string At(JsonNode? pair, int i) => pair.Items().ElementAtOrDefault(i) is JsonValue v && v.TryGetValue<string>(out var text) ? text : "";
        var commands = AppState.Start.Arr("commands").Select(c => (Usage: At(c, 0), About: At(c, 1)));
        foreach (var (usage, about) in commands.Concat(AppCommands))
        {
            items.Add(new("Client commands", usage, about, () => InsertCommand(usage)));
        }
        await Picker.Show(Root.XamlRoot, "Search commands, worlds and tabs", items, "Nothing matches.");
    }

    /// <summary>A client command in the input box, ready for its arguments: "/map " for
    /// "/map [go|here] ...", as the terminal client's palette does.</summary>
    private void InsertCommand(string usage)
    {
        if (ShownInput is not { } input) return;
        var literal = new List<string>();
        var more = false;
        foreach (var (token, i) in usage.Split(' ').Select((t, i) => (t, i)))
        {
            if (i > 0 && (token.IndexOfAny(['<', '[', '|']) >= 0 || token == "..."))
            {
                more = true;
                break;
            }
            var cut = token.IndexOf('<') is var at and >= 0 ? token[..at] : token;
            literal.Add(cut);
            if (cut.Length < token.Length) break;
        }
        input.Text = string.Join(' ', literal) + (more ? " " : "");
        input.FocusInput();
    }

    /// <summary>Searches what was typed in this tab, newest first; the one chosen goes in the
    /// input box, to send or change.</summary>
    private async Task SearchHistory()
    {
        if (ShownInput is not { } input) return;
        var items = input.PastCommands.Reverse().Distinct().Select(line => new Picker.Item("History", line.Replace('\n', '⏎'), "", () =>
        {
            input.Text = line;
            input.FocusInput();
        })).ToList();
        await Picker.Show(Root.XamlRoot, "Search what you’ve typed", items, "Nothing typed here yet.");
        input.FocusInput();
    }

    // MARK: The menu

    private void OnAddTab(TabView sender, object args) => OpenConnect();

    private void OnMenuItem(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string action }) Run(action);
    }

    private void OnMenuOpening(object sender, object e)
    {
        var session = Showing?.Session;
        ReconnectItem.IsEnabled = session is not null;
        DisconnectItem.IsEnabled = session?.IsConnected == true;
        ClearItem.IsEnabled = session is not null;
        FindItem.IsEnabled = ShownOutput is not null;
        LogItem.IsEnabled = session is not null;
        LogItem.IsChecked = session?.Logging == true;
        // The keys as they are now: the user may have changed them.
        foreach (var item in ((MenuFlyout)sender).Items.OfType<MenuFlyoutItem>())
        {
            if (item.Tag is string action) item.KeyboardAcceleratorTextOverride = Shortcuts.Label(action);
        }
    }

    /// <summary>The settings tab, at the keyboard shortcuts if `keyboard`.</summary>
    internal void ShowSettings(bool keyboard = false)
    {
        if (Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => t.Content is SettingsPage) is not { } tab)
        {
            tab = new TabViewItem { Header = "Settings", IconSource = new FontIconSource { Glyph = "" }, Content = new SettingsPage() };
            Tabs.TabItems.Add(tab);
        }
        Tabs.SelectedItem = tab;
        if (keyboard) (tab.Content as SettingsPage)?.ShowKeyboard();
    }

    private void OnRules()
    {
        if (Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => t.Content is RulesPage) is not { } tab)
        {
            // It opens on the rules of the world being looked at.
            var session = Showing?.Session;
            var page = new RulesPage(session?.Info.Bool("saved") == true ? session.WorldName : null, session?.Info.OptStr("character"));
            tab = new TabViewItem { Header = "Triggers, Aliases & Macros", IconSource = new FontIconSource { Glyph = "" }, Content = page };
            Tabs.TabItems.Add(tab);
        }
        Tabs.SelectedItem = tab;
    }

    private async Task ShowAbout()
    {
        var panel = new StackPanel { Spacing = 4 };
        panel.Children.Add(new TextBlock { Text = $"Version {AppState.Start.Str("version")}" });
        panel.Children.Add(new TextBlock { Text = "A client for MUDs, MUSHes, MUCKs and MOOs.", TextWrapping = TextWrapping.Wrap });
        panel.Children.Add(new HyperlinkButton { Content = "github.com/SpiraMirabilis/familiar", NavigateUri = new Uri("https://github.com/SpiraMirabilis/familiar"), Padding = new Thickness(0), Margin = new Thickness(0, 8, 0, 0) });
        await Form.Show(new ContentDialog { XamlRoot = Root.XamlRoot, Title = "Familiar", Content = panel, CloseButtonText = "Close", DefaultButton = ContentDialogButton.Close });
    }

    internal Task Alert(string title, string detail) => AppState.Alert(Root.XamlRoot, title, detail);

    internal void WhenReady(Action action)
    {
        if (Root.IsLoaded) action();
        else Root.Loaded += (_, _) => action();
    }

    internal SessionView? CurrentView => Showing;
    internal OutputView? CurrentOutput => ShownOutput;
    internal SettingsPage? CurrentSettings => (Tabs.SelectedItem as TabViewItem)?.Content as SettingsPage;
    internal RulesPage? ShowRules()
    {
        OnRules();
        return (Tabs.SelectedItem as TabViewItem)?.Content as RulesPage;
    }
}
