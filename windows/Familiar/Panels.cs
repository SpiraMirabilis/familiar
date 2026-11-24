using System.Text.Json.Nodes;
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Foundation;
using Windows.UI;

namespace Familiar;

/// <summary>A pane of stats from the server (GMCP beip.stats): names with numbers, text or bars.</summary>
internal sealed class StatsPane : UserControl
{
    private readonly StackPanel rows = new() { Spacing = 8, Padding = new Thickness(12) };
    private readonly Grid root = new();

    public StatsPane()
    {
        root.Children.Add(new ScrollViewer { Content = rows });
        Content = root;
    }

    private static Brush Color(JsonNode? value, string key, Brush fallback)
    {
        var text = value.OptStr(key);
        if (text == "transparent") return new SolidColorBrush(Microsoft.UI.Colors.Transparent);
        return Json.Hex(text) is { } color ? new SolidColorBrush(color) : fallback;
    }

    public void Update(JsonNode? pane)
    {
        root.Background = Color(pane, "background", new SolidColorBrush(Microsoft.UI.Colors.Transparent));
        rows.Children.Clear();
        foreach (var stat in pane.Arr("values"))
        {
            var name = new TextBlock
            {
                Text = stat.Str("name"),
                Foreground = Color(stat, "name_color", Form.Brush("TextFillColorPrimaryBrush")),
                HorizontalAlignment = stat.Str("align") switch { "right" => HorizontalAlignment.Right, "center" => HorizontalAlignment.Center, _ => HorizontalAlignment.Left },
                TextTrimming = TextTrimming.CharacterEllipsis,
            };
            var kind = stat.Str("kind");
            if (kind == "none")
            {
                rows.Children.Add(name);
                continue;
            }
            var value = new TextBlock { Text = stat.Str("text"), Foreground = Color(stat, "value_color", Form.Brush("TextFillColorSecondaryBrush")), IsTextSelectionEnabled = kind == "text" };
            var line = new Grid { ColumnSpacing = 8 };
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            line.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            Grid.SetColumn(value, 1);
            line.Children.Add(name);
            line.Children.Add(value);
            if (kind != "bar")
            {
                rows.Children.Add(line);
                continue;
            }
            var colors = stat.Get("colors");
            var fraction = Math.Clamp(stat.Num("fraction"), 0, 1);
            var bar = new Grid { Height = 8 };
            bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(fraction, GridUnitType.Star) });
            bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1 - fraction, GridUnitType.Star) });
            var empty = new Rectangle { RadiusX = 4, RadiusY = 4, Fill = Color(colors, "empty", Form.Brush("ControlStrongFillColorDisabledBrush")) };
            Grid.SetColumnSpan(empty, 2);
            var fill = new Rectangle { RadiusX = 4, RadiusY = 4, Fill = Color(colors, "fill", Form.Brush("AccentFillColorDefaultBrush")) };
            bar.Children.Add(empty);
            bar.Children.Add(fill);
            if (!colors.IsNull("outline"))
            {
                var outline = new Rectangle { RadiusX = 4, RadiusY = 4, StrokeThickness = 1, Stroke = Color(colors, "outline", new SolidColorBrush(Microsoft.UI.Colors.Transparent)) };
                Grid.SetColumnSpan(outline, 2);
                bar.Children.Add(outline);
            }
            var both = new StackPanel { Spacing = 3 };
            both.Children.Add(line);
            both.Children.Add(bar);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(both, $"{stat.Str("name")}: {stat.Str("text")}");
            rows.Children.Add(both);
        }
    }
}

/// <summary>A map made of tiles from one picture (GMCP beip.tilemap), scaled to fit.</summary>
internal sealed class TilemapView : UserControl
{
    private readonly DrawingSurface canvas = new();
    private readonly Action<string> notice;
    private JsonNode? map;
    private CanvasBitmap? tiles;
    private string tilesUrl = "";
    private string? tilesPath;
    private bool loading;

    public TilemapView(Action<string> notice)
    {
        this.notice = notice;
        canvas.Draw += OnDraw;
        // A new canvas can't draw the old one's picture.
        canvas.Ready += () => tiles = null;
        Content = canvas;
    }

    /// <summary>Reads the tile picture for the canvas there is now.</summary>
    private async void LoadTiles(CanvasControl on, string path)
    {
        if (loading) return;
        loading = true;
        try
        {
            var loaded = await CanvasBitmap.LoadAsync(on, path);
            if (ReferenceEquals(canvas.Canvas, on) && tilesPath == path) tiles = loaded;
        }
        catch (Exception e)
        {
            tilesPath = null;
            notice($"Couldn’t read the map’s tiles: {e.Message}");
        }
        loading = false;
        canvas.Invalidate();
    }

    public async void Update(JsonNode? next)
    {
        map = next;
        var url = next.Str("tile_url");
        if (url != tilesUrl)
        {
            tilesUrl = url;
            try
            {
                var path = await Task.Run(() => Core.CallAnywhere("media.fetch", Json.Args(("url", url))).Str("path"));
                if (tilesUrl != url) return;
                tilesPath = path;
                tiles = null;
            }
            catch (Exception e)
            {
                notice($"Couldn’t get the map’s tiles: {e.Message}");
            }
        }
        canvas.Invalidate();
    }

    private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
    {
        if (tiles is null && tilesPath is not null) LoadTiles(sender, tilesPath);
        if (map is null || tiles is null) return;
        int tw = map.Int("tile_width"), th = map.Int("tile_height"), width = map.Int("width"), height = map.Int("height");
        if (tw <= 0 || th <= 0 || width <= 0 || height <= 0) return;
        var perRow = Math.Max(1, (int)tiles.SizeInPixels.Width / tw);
        var scale = (float)Math.Min(sender.ActualWidth / (width * tw), sender.ActualHeight / (height * th));
        var left = (float)(sender.ActualWidth - width * tw * scale) / 2;
        var top = (float)(sender.ActualHeight - height * th * scale) / 2;
        var list = map.Arr("tiles");
        for (var i = 0; i < list.Count; i++)
        {
            var tile = (int)(list[i]?.GetValue<double>() ?? 0);
            int sx = tile % perRow * tw, sy = tile / perRow * th;
            // The tile picture may not have this many tiles.
            if (sy + th > tiles.SizeInPixels.Height) continue;
            // A little overlap hides the seams scaling would leave between tiles.
            var to = new Rect(left + i % width * tw * scale, top + i / width * th * scale, tw * scale + 0.5, th * scale + 0.5);
            args.DrawingSession.DrawImage(tiles, to, new Rect(sx, sy, tw, th), 1, CanvasImageInterpolation.NearestNeighbor);
        }
    }
}

/// <summary>A puppet's tab: the lines that belong to it, and a box that speaks as it.</summary>
internal sealed class PuppetView : UserControl
{
    public OutputView Output { get; }
    public InputBox Input { get; }

    public PuppetView(Session session, string name, OutputBuffer buffer)
    {
        var settings = AppState.Settings;
        Output = new OutputView(buffer, session.Font);
        Input = new InputBox(session.Font, settings.Int("input_history_size"), settings.Bool("spellcheck")) { Margin = new Thickness(0, 8, 0, 0) };
        var card = new Border { CornerRadius = new CornerRadius(8), BorderThickness = new Thickness(1), BorderBrush = Form.Brush("CardStrokeColorDefaultBrush"), Background = new SolidColorBrush(AppState.Background), Child = Output };
        var root = new Grid { Padding = new Thickness(8) };
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        Grid.SetRow(Input, 1);
        root.Children.Add(card);
        root.Children.Add(Input);
        Content = root;

        Loaded += (_, _) => Input.FocusInput();
        Output.LinkClicked += session.FollowLink;
        Output.TypingStarted += Input.FocusInput;
        Input.Page += Output.ScrollPage;
        Input.Submitted += (text, _) =>
        {
            Output.ScrollToEnd();
            session.PuppetSubmit(name, text);
        };
        session.Changed += () =>
        {
            Output.Font = session.Font;
            Input.Font = session.Font;
        };
    }
}

/// <summary>A web page the server or a script opened (GMCP webview.open), in the side dock or
/// a window of its own. The page talks to the session through webview_bridge.js.</summary>
internal sealed class WebPane : UserControl
{
    private readonly WebView2 view = new();
    private readonly Task ready;
    private Window? window;
    private Action? onWindowClosed;

    public bool Docked { get; }
    private readonly int width, height;

    public WebPane(JsonNode open, Action<JsonNode> onMessage)
    {
        Docked = !open.IsNull("dock");
        width = (int)(open.OptNum("width") ?? 900);
        height = (int)(open.OptNum("height") ?? 700);
        Content = view;
        ready = Prepare(onMessage);
        Load(open);
    }

    private async Task Prepare(Action<JsonNode> onMessage)
    {
        await view.EnsureCoreWebView2Async();
        var core = view.CoreWebView2;
        // The bridge script was written for WebKit's message handlers; this gives it one that
        // posts through WebView2, and makes the BeipMU-style host object it sets up stick.
        var script = "if (window.top === window) {\n"
            + "const post = window.chrome.webview.postMessage.bind(window.chrome.webview);\n"
            + "window.webkit = { messageHandlers: { mu: { postMessage: post } } };\n"
            + AppState.Start.Str("webview_bridge_js")
            + "\ntry { Object.defineProperty(window.chrome.webview, 'hostObjects', { value: { client: window.mu.client, options: {} }, configurable: true }); } catch (e) {}\n}";
        await core.AddScriptToExecuteOnDocumentCreatedAsync(script);
        core.WebMessageReceived += (_, e) =>
        {
            if (JsonNode.Parse(e.WebMessageAsJson) is { } message) onMessage(message);
        };
        // Links that ask for a new window open in the browser.
        core.NewWindowRequested += (_, e) =>
        {
            e.Handled = true;
            Session.OpenUri(e.Uri);
        };
    }

    public async void Load(JsonNode open)
    {
        try
        {
            await ready;
            var core = view.CoreWebView2;
            if (open.OptStr("url") is { } url)
            {
                var headers = string.Join("\r\n", open.Arr("headers").Select(h => $"{h?[0]}: {h?[1]}"));
                core.NavigateWithWebResourceRequest(core.Environment.CreateWebResourceRequest(url, "GET", null, headers));
            }
            else if (open.OptStr("source") is { } source)
            {
                core.NavigateToString(source);
            }
        }
        catch (Exception e)
        {
            System.Diagnostics.Debug.WriteLine($"Familiar: loading a web view: {e.Message}");
        }
    }

    /// <summary>Hands the page an event: gmcp, receive, send or display.</summary>
    public async void Deliver(JsonNode message)
    {
        try
        {
            await ready;
            await view.CoreWebView2.ExecuteScriptAsync($"window.mu && window.mu._dispatch({message.ToJsonString()});");
        }
        catch (Exception)
        {
            // The page is gone.
        }
    }

    public void OpenWindow(string title, Action onClose)
    {
        onWindowClosed = onClose;
        window = new Window { Title = title, Content = this, SystemBackdrop = new Microsoft.UI.Xaml.Media.MicaBackdrop() };
        window.AppWindow.SetIcon(System.IO.Path.Combine(AppContext.BaseDirectory, "familiar.ico"));
        var scale = App.Window?.Content.XamlRoot?.RasterizationScale ?? 1;
        window.AppWindow.Resize(new Windows.Graphics.SizeInt32((int)(width * scale), (int)(height * scale)));
        window.Closed += (_, _) =>
        {
            window = null;
            view.Close();
            onWindowClosed?.Invoke();
            onWindowClosed = null;
        };
        window.Activate();
    }

    public void Present() => window?.Activate();

    public void Shut()
    {
        onWindowClosed = null;
        if (window is not null) window.Close();
        else view.Close();
    }
}
