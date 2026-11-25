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

        Accelerator(VirtualKey.O, VirtualKeyModifiers.Control, () => OnConnect(this, new RoutedEventArgs()));
        Accelerator(VirtualKey.R, VirtualKeyModifiers.Control, () => Showing?.Session.Connect());
        Accelerator(VirtualKey.D, VirtualKeyModifiers.Control | VirtualKeyModifiers.Shift, () => Showing?.Session.Disconnect());
        Accelerator(VirtualKey.L, VirtualKeyModifiers.Control | VirtualKeyModifiers.Shift, () => Showing?.Session.Output.Clear());
        Accelerator(VirtualKey.G, VirtualKeyModifiers.Control | VirtualKeyModifiers.Shift, () => Showing?.Session.ToggleLog());
        Accelerator(VirtualKey.W, VirtualKeyModifiers.Control, () =>
        {
            if (Tabs.SelectedItem is TabViewItem tab) CloseTab(tab);
        });
        Accelerator((VirtualKey)190, VirtualKeyModifiers.Control, () => Showing?.Session.StopSounds());
        Accelerator((VirtualKey)188, VirtualKeyModifiers.Control, () => OnSettings(this, new RoutedEventArgs()));
        Accelerator(VirtualKey.T, VirtualKeyModifiers.Control, () => OnRules(this, new RoutedEventArgs()));
        Accelerator(VirtualKey.F, VirtualKeyModifiers.Control, () => ShownOutput?.ShowFind());
        Accelerator(VirtualKey.Tab, VirtualKeyModifiers.Control, () => NextTab(1));
        Accelerator(VirtualKey.Tab, VirtualKeyModifiers.Control | VirtualKeyModifiers.Shift, () => NextTab(-1));
        // Alt+← and → switch tabs, as in the terminal client. They're caught on the way down,
        // before the tab strip (which moves between tabs' buttons with the arrow keys) or
        // anything else can take them; unused, Windows only beeps at them.
        Root.PreviewKeyDown += (_, e) =>
        {
            if (e.Key is not (VirtualKey.Left or VirtualKey.Right) || !(Keys.Alt || e.KeyStatus.IsMenuKeyDown) || Keys.Ctrl || Keys.Shift) return;
            NextTab(e.Key == VirtualKey.Right ? 1 : -1);
            e.Handled = true;
        };
        OpenConnect();
    }

    private void Accelerator(VirtualKey key, VirtualKeyModifiers modifiers, Action action)
    {
        var accelerator = new KeyboardAccelerator { Key = key, Modifiers = modifiers };
        accelerator.Invoked += (_, e) =>
        {
            action();
            e.Handled = true;
        };
        Root.KeyboardAccelerators.Add(accelerator);
    }

    /// <summary>The output of the tab that's showing: a session's or a puppet's.</summary>
    private OutputView? ShownOutput => (Tabs.SelectedItem as TabViewItem)?.Content switch
    {
        SessionView view => view.Output,
        PuppetView puppet => puppet.Output,
        _ => null,
    };

    private SessionView? Showing => (Tabs.SelectedItem as TabViewItem)?.Content as SessionView;

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

    // MARK: The menu

    private void OnAddTab(TabView sender, object args) => OpenConnect();
    private void OnConnect(object sender, RoutedEventArgs e) => OpenConnect();

    private void OnSettings(object sender, RoutedEventArgs e)
    {
        if (Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => t.Content is SettingsPage) is not { } tab)
        {
            tab = new TabViewItem { Header = "Settings", IconSource = new FontIconSource { Glyph = "\uE713" }, Content = new SettingsPage() };
            Tabs.TabItems.Add(tab);
        }
        Tabs.SelectedItem = tab;
    }

    private void OnRules(object sender, RoutedEventArgs e)
    {
        if (Tabs.TabItems.OfType<TabViewItem>().FirstOrDefault(t => t.Content is RulesPage) is not { } tab)
        {
            // It opens on the rules of the world being looked at.
            var session = Showing?.Session;
            var page = new RulesPage(session?.Info.Bool("saved") == true ? session.WorldName : null, session?.Info.OptStr("character"));
            tab = new TabViewItem { Header = "Triggers, Aliases & Macros", IconSource = new FontIconSource { Glyph = "\uE945" }, Content = page };
            Tabs.TabItems.Add(tab);
        }
        Tabs.SelectedItem = tab;
    }

    private async void OnImport(object sender, RoutedEventArgs e) => await SettingsPage.ImportBeipMU(Root.XamlRoot);

    private async void OnShortcuts(object sender, RoutedEventArgs e)
    {
        (string Keys, string What)[] shortcuts =
        [
            ("Ctrl+O", "Connect to a world"), ("Ctrl+R", "Reconnect"), ("Ctrl+Shift+D", "Disconnect"), ("Ctrl+W", "Close the tab"),
            ("Ctrl+Tab", "Next tab"), ("Ctrl+F", "Find in the output"), ("Ctrl+Shift+L", "Clear the output"), ("Ctrl+Shift+G", "Log to a file"), ("Ctrl+.", "Stop speaking"),
            ("Ctrl+T", "Triggers, aliases and macros"), ("Ctrl+,", "Settings"), ("Up, Down", "Earlier and later commands"), ("Shift+Enter", "A new line in the input"),
            ("Page Up, Page Down", "Scroll the output"), ("Escape", "Clear the input"),
        ];
        var grid = new Grid { ColumnSpacing = 24, RowSpacing = 8, MinWidth = 360 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        foreach (var (keys, what) in shortcuts)
        {
            grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            var key = new TextBlock { Text = keys, Style = Form.Style("BodyStrongTextBlockStyle") };
            var text = new TextBlock { Text = what };
            Grid.SetRow(key, grid.RowDefinitions.Count - 1);
            Grid.SetRow(text, grid.RowDefinitions.Count - 1);
            Grid.SetColumn(text, 1);
            grid.Children.Add(key);
            grid.Children.Add(text);
        }
        await Form.Show(Form.Dialog(Root.XamlRoot, "Keyboard Shortcuts", grid, close: "Close"));
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
    }

    private void OnReconnect(object sender, RoutedEventArgs e) => Showing?.Session.Connect();
    private void OnDisconnect(object sender, RoutedEventArgs e) => Showing?.Session.Disconnect();
    private void OnFind(object sender, RoutedEventArgs e) => ShownOutput?.ShowFind();
    private void OnClear(object sender, RoutedEventArgs e) => Showing?.Session.Output.Clear();
    private void OnToggleLog(object sender, RoutedEventArgs e) => Showing?.Session.ToggleLog();
    private void OnStopSpeaking(object sender, RoutedEventArgs e) => Core.Send("speech.stop");

    private void OnOpenLogFolder(object sender, RoutedEventArgs e)
    {
        var folder = AppState.Start.Str("log_dir");
        Directory.CreateDirectory(folder);
        Session.OpenUri(folder);
    }

    private async void OnAbout(object sender, RoutedEventArgs e)
    {
        var panel = new StackPanel { Spacing = 4 };
        panel.Children.Add(new TextBlock { Text = $"Version {AppState.Start.Str("version")}" });
        panel.Children.Add(new TextBlock { Text = "A client for MUDs, MUSHes, MUCKs and MOOs.", TextWrapping = TextWrapping.Wrap });
        panel.Children.Add(new HyperlinkButton { Content = "github.com/SpiraMirabilis/familiar", NavigateUri = new Uri("https://github.com/SpiraMirabilis/familiar"), Padding = new Thickness(0), Margin = new Thickness(0, 8, 0, 0) });
        var dialog = new ContentDialog { XamlRoot = Root.XamlRoot, Title = "Familiar", Content = panel, CloseButtonText = "Close", DefaultButton = ContentDialogButton.Close };
        try
        {
            await dialog.ShowAsync();
        }
        catch (Exception)
        {
            // Another dialog is already open.
        }
    }

    internal Task Alert(string title, string detail) => AppState.Alert(Root.XamlRoot, title, detail);

    internal void WhenReady(Action action)
    {
        if (Root.IsLoaded) action();
        else Root.Loaded += (_, _) => action();
    }

    internal SessionView? CurrentView => Showing;
    internal OutputView? CurrentOutput => ShownOutput;
    internal void ShowSettings() => OnSettings(this, new RoutedEventArgs());
    internal RulesPage? ShowRules()
    {
        OnRules(this, new RoutedEventArgs());
        return (Tabs.SelectedItem as TabViewItem)?.Content as RulesPage;
    }
}
