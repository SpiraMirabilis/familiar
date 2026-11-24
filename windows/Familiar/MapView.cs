using System.Numerics;
using System.Text.Json.Nodes;
using Microsoft.Graphics.Canvas.Geometry;
using Microsoft.Graphics.Canvas.Text;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Windows.Foundation;
using Windows.UI;

namespace Familiar;

/// <summary>The world's map: rooms as boxes joined by their exits. It fills in as you move, if
/// the game sends GMCP Room.Info. Drag to pan or to move a room, scroll to zoom, double-click a
/// room to walk there.</summary>
internal sealed class MapView : UserControl
{
    private sealed record Room(long Id, string Name, string Area, double X, double Y, double W, double H, Color? Color, List<(string Command, long? To)> Exits)
    {
        public Vector2 Center => new((float)(X + W / 2), (float)(Y + H / 2));
        public bool Contains(Vector2 p) => p.X >= X && p.X <= X + W && p.Y >= Y && p.Y <= Y + H;
    }

    private static readonly Dictionary<string, (int X, int Y)> Directions = new(StringComparer.OrdinalIgnoreCase)
    {
        ["n"] = (0, -1), ["north"] = (0, -1), ["s"] = (0, 1), ["south"] = (0, 1), ["e"] = (1, 0), ["east"] = (1, 0), ["w"] = (-1, 0), ["west"] = (-1, 0),
        ["ne"] = (1, -1), ["northeast"] = (1, -1), ["nw"] = (-1, -1), ["northwest"] = (-1, -1), ["se"] = (1, 1), ["southeast"] = (1, 1), ["sw"] = (-1, 1), ["southwest"] = (-1, 1),
    };

    private readonly Session session;
    private readonly DrawingSurface canvas = new();
    private readonly ComboBox areas = new() { MinWidth = 120, PlaceholderText = "Area" };
    private readonly AutoSuggestBox search = new() { PlaceholderText = "Find a room", QueryIcon = new SymbolIcon(Symbol.Find), MinWidth = 120 };
    private readonly ToggleButton follow = new() { Content = new FontIcon { Glyph = "", FontSize = 14 }, IsChecked = true };

    private Dictionary<long, Room> rooms = [];
    private long? current, selected;
    private double gridX = 144, gridY = 72;
    private string area = "";
    private float zoom = 1;
    private Vector2 origin;
    private bool placed, fillingAreas;

    // A drag: of the view, or of a room.
    private Vector2? dragFrom;
    private Vector2 dragOrigin;
    private Room? dragRoom;
    private bool dragged;

    public MapView(Session session)
    {
        this.session = session;
        var center = new Button { Content = new FontIcon { Glyph = "", FontSize = 14 } };
        var import = new Button { Content = new FontIcon { Glyph = "", FontSize = 14 } };
        ToolTipService.SetToolTip(center, "Show where you are");
        ToolTipService.SetToolTip(follow, "Keep where you are in view");
        ToolTipService.SetToolTip(import, "Import a BeipMU map (.atlas)");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(center, "Show where you are");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(follow, "Follow");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(import, "Import a map");
        var bar = new Grid { ColumnSpacing = 6, Padding = new Thickness(8, 6, 8, 6) };
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        for (var i = 0; i < 3; i++) bar.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        UIElement[] items = [areas, search, center, follow, import];
        for (var i = 0; i < items.Length; i++)
        {
            Grid.SetColumn((FrameworkElement)items[i], i);
            bar.Children.Add(items[i]);
        }
        var root = new Grid();
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        Grid.SetRow(canvas, 1);
        root.Children.Add(bar);
        root.Children.Add(canvas);
        Content = root;

        canvas.Draw += OnDraw;
        canvas.PointerPressed += OnPressed;
        canvas.PointerMoved += OnMoved;
        canvas.PointerReleased += OnReleased;
        canvas.PointerWheelChanged += OnWheel;
        canvas.DoubleTapped += (_, e) =>
        {
            if (RoomAt(e.GetPosition(canvas)) is { } room) session.Call("map.walk", Json.Args(("room", room.Id)));
        };
        canvas.RightTapped += (_, e) => ShowMenu(e.GetPosition(canvas));
        ActualThemeChanged += (_, _) => canvas.Invalidate();
        areas.SelectionChanged += (_, _) =>
        {
            if (fillingAreas || areas.SelectedItem is not string chosen) return;
            area = chosen;
            Fit();
        };
        center.Click += (_, _) => CenterOnCurrent();
        follow.Click += (_, _) =>
        {
            if (follow.IsChecked == true) CenterOnCurrent();
        };
        search.QuerySubmitted += (_, _) => Find();
        import.Click += async (_, _) =>
        {
            if (await Form.PickFile(".atlas") is { } path) session.Call("map.import", Json.Args(("path", path)));
        };
    }

#if DEBUG
    internal string Debug => $"size {canvas.ActualWidth}x{canvas.ActualHeight} canvas {canvas.Canvas is not null} ready {canvas.ReadyToDraw} loaded {canvas.IsLoaded}/{IsLoaded} draws {draws} rooms {rooms.Count} shown {Shown.Count()} area '{area}' origin {origin} zoom {zoom} placed {placed}";
    private int draws;
#endif

    /// <summary>Reads the map again from the engine.</summary>
    public void Reload()
    {
        var map = session.Map();
        gridX = map.OptNum("grid_x") ?? gridX;
        gridY = map.OptNum("grid_y") ?? gridY;
        rooms = map.Arr("rooms").Select(r => new Room(
            r.Long("id"), r.Str("name"), r.Str("area"), r.Num("x"), r.Num("y"), r.OptNum("w") ?? 96, r.OptNum("h") ?? 32, Json.Hex(r.OptStr("color")),
            r.Arr("exits").Select(e => (e.Str("command"), e.IsNull("to") ? (long?)null : e.Long("to"))).ToList())).ToDictionary(r => r.Id);
        var was = current;
        current = map.IsNull("current") ? null : map.Long("current");
        if (selected is { } s && !rooms.ContainsKey(s)) selected = null;

        fillingAreas = true;
        var names = map.Arr("areas").Select(a => a?.ToString() ?? "").ToList();
        areas.Items.Clear();
        foreach (var name in names) areas.Items.Add(name);
        var here = current is { } id && rooms.TryGetValue(id, out var room) ? room : null;
        // Following where you are takes the map to its area.
        if (here is not null && (follow.IsChecked == true || !names.Contains(area))) area = here.Area;
        else if (!names.Contains(area)) area = names.FirstOrDefault() ?? "";
        areas.SelectedItem = area;
        areas.Visibility = names.Count > 1 ? Visibility.Visible : Visibility.Collapsed;
        fillingAreas = false;

        if (!placed) Fit();
        else if (follow.IsChecked == true && current != was) CenterOnCurrent();
        canvas.Invalidate();
    }

    private IEnumerable<Room> Shown => rooms.Values.Where(r => r.Area == area);

    // MARK: Where the view is

    private Vector2 ToMap(Point p) => new Vector2((float)p.X, (float)p.Y) / zoom + origin;

    private void CenterOn(Vector2 point)
    {
        origin = point - new Vector2((float)canvas.ActualWidth, (float)canvas.ActualHeight) / (2 * zoom);
        canvas.Invalidate();
    }

    private void CenterOnCurrent()
    {
        if (current is not { } id || !rooms.TryGetValue(id, out var room)) return;
        if (room.Area != area)
        {
            area = room.Area;
            fillingAreas = true;
            areas.SelectedItem = area;
            fillingAreas = false;
        }
        CenterOn(room.Center);
    }

    /// <summary>Shows where you are, or failing that the middle of the area.</summary>
    private void Fit()
    {
        if (canvas.ActualWidth < 1) return;
        var shown = Shown.ToList();
        if (shown.Count == 0) return;
        placed = true;
        if (current is { } id && rooms.TryGetValue(id, out var here) && here.Area == area) CenterOn(here.Center);
        else CenterOn(new Vector2((float)(shown.Min(r => r.X) + shown.Max(r => r.X + r.W)) / 2, (float)(shown.Min(r => r.Y) + shown.Max(r => r.Y + r.H)) / 2));
    }

    private Room? RoomAt(Point point)
    {
        var p = ToMap(point);
        return Shown.LastOrDefault(r => r.Contains(p));
    }

    private void Find()
    {
        var text = search.Text.Trim();
        if (text.Length == 0) return;
        // The engine's answer is a list of rooms or of their numbers.
        var first = session.Call("map.search", Json.Args(("text", text))).Items().FirstOrDefault();
        long? id = first is JsonObject ? first.Long("id") : first is JsonValue v && v.TryGetValue<long>(out var n) ? n : null;
        if (id is not { } found || !rooms.TryGetValue(found, out var room)) return;
        selected = found;
        area = room.Area;
        fillingAreas = true;
        areas.SelectedItem = area;
        fillingAreas = false;
        follow.IsChecked = false;
        CenterOn(room.Center);
    }

    // MARK: Drawing

    private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
    {
#if DEBUG
        draws++;
#endif
        var ds = args.DrawingSession;
        var dark = ActualTheme == ElementTheme.Dark;
        var text = dark ? Color.FromArgb(255, 235, 235, 235) : Color.FromArgb(255, 26, 26, 26);
        var fill = dark ? Color.FromArgb(255, 56, 61, 77) : Color.FromArgb(255, 219, 227, 242);
        var line = dark ? Color.FromArgb(255, 140, 145, 158) : Color.FromArgb(255, 115, 120, 133);
        var accent = Application.Current.Resources.TryGetValue("SystemAccentColor", out var value) && value is Color c ? c : Color.FromArgb(255, 0, 120, 215);
        if (!placed) Fit();

        var shown = Shown.ToList();
        if (shown.Count == 0)
        {
            using var note = new CanvasTextFormat { FontSize = 13, HorizontalAlignment = CanvasHorizontalAlignment.Center, VerticalAlignment = CanvasVerticalAlignment.Center, WordWrapping = CanvasWordWrapping.Wrap };
            ds.DrawText("No rooms yet.\nThe map fills in as you move if the game sends GMCP Room.Info; otherwise use /map addroom \"Name\", or right-click to add rooms.",
                new Rect(16, 0, Math.Max(1, sender.ActualWidth - 32), sender.ActualHeight), Color.FromArgb(160, text.R, text.G, text.B), note);
            return;
        }

        ds.Transform = Matrix3x2.CreateTranslation(-origin) * Matrix3x2.CreateScale(zoom);
        var stroke = 1.5f / Math.Max(zoom, 0.5f);
        // Exits go under the rooms. One to a room elsewhere (or not mapped yet) gets a short stub.
        foreach (var room in shown)
        {
            foreach (var (command, to) in room.Exits)
            {
                if (to is { } id && rooms.TryGetValue(id, out var target) && target.Area == area)
                {
                    // Each two-way pair is drawn once.
                    if (target.Id < room.Id && target.Exits.Any(e => e.To == room.Id)) continue;
                    ds.DrawLine(room.Center, target.Center, line, stroke);
                }
                else if (Directions.TryGetValue(command, out var d))
                {
                    ds.DrawLine(room.Center, room.Center + new Vector2((float)(d.X * gridX * 0.45), (float)(d.Y * gridY * 0.45)), line, stroke);
                }
            }
        }
        using var format = new CanvasTextFormat
        {
            FontSize = 11, HorizontalAlignment = CanvasHorizontalAlignment.Center, VerticalAlignment = CanvasVerticalAlignment.Center,
            WordWrapping = CanvasWordWrapping.NoWrap, TrimmingGranularity = CanvasTextTrimmingGranularity.Character, TrimmingSign = CanvasTrimmingSign.Ellipsis,
        };
        foreach (var room in shown)
        {
            var box = new Rect(room.X, room.Y, room.W, room.H);
            var isCurrent = room.Id == current;
            var colour = isCurrent ? accent : room.Color ?? fill;
            ds.FillRoundedRectangle(box, 6, 6, colour);
            if (room.Id == selected) ds.DrawRoundedRectangle(box, 6, 6, Color.FromArgb(255, 245, 194, 18), 2.5f / Math.Max(zoom, 0.5f));
            else ds.DrawRoundedRectangle(box, 6, 6, line, 1 / Math.Max(zoom, 0.5f));
            // Text that can be read on whatever the room's colour is.
            var bright = (colour.R * 299 + colour.G * 587 + colour.B * 114) / 1000 > 140;
            ds.DrawText(room.Name, new Rect(room.X + 4, room.Y, Math.Max(1, room.W - 8), room.H), bright ? Color.FromArgb(255, 26, 26, 26) : Color.FromArgb(255, 245, 245, 245), format);
        }
    }

    // MARK: The mouse

    private void OnPressed(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(canvas);
        if (!point.Properties.IsLeftButtonPressed) return;
        dragFrom = new Vector2((float)point.Position.X, (float)point.Position.Y);
        dragOrigin = origin;
        dragRoom = RoomAt(point.Position);
        dragged = false;
        selected = dragRoom?.Id;
        canvas.CapturePointer(e.Pointer);
        canvas.Invalidate();
    }

    private void OnMoved(object sender, PointerRoutedEventArgs e)
    {
        if (dragFrom is not { } from) return;
        var point = e.GetCurrentPoint(canvas).Position;
        var delta = (new Vector2((float)point.X, (float)point.Y) - from) / zoom;
        if (!dragged && delta.Length() * zoom < 4) return;
        dragged = true;
        if (dragRoom is { } room)
        {
            rooms[room.Id] = rooms[room.Id] with { X = room.X + delta.X, Y = room.Y + delta.Y };
        }
        else
        {
            origin = dragOrigin - delta;
            follow.IsChecked = false;
        }
        canvas.Invalidate();
    }

    private void OnReleased(object sender, PointerRoutedEventArgs e)
    {
        if (dragFrom is null) return;
        dragFrom = null;
        canvas.ReleasePointerCapture(e.Pointer);
        if (dragged && dragRoom is { } room && rooms.TryGetValue(room.Id, out var moved))
        {
            session.Call("map.move", Json.Args(("room", room.Id), ("x", moved.X), ("y", moved.Y)));
        }
        dragRoom = null;
    }

    private void OnWheel(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(canvas);
        var at = ToMap(point.Position);
        zoom = Math.Clamp(zoom * MathF.Pow(1.1f, point.Properties.MouseWheelDelta / 120f), 0.2f, 4f);
        // The point under the pointer stays under it.
        origin = at - new Vector2((float)point.Position.X, (float)point.Position.Y) / zoom;
        canvas.Invalidate();
        e.Handled = true;
    }

    private void ShowMenu(Point point)
    {
        var menu = new MenuFlyout();
        void Item(string text, Action act)
        {
            var item = new MenuFlyoutItem { Text = text };
            item.Click += (_, _) => act();
            menu.Items.Add(item);
        }
        if (RoomAt(point) is { } room)
        {
            selected = room.Id;
            canvas.Invalidate();
            Item("Walk Here", () => session.Call("map.walk", Json.Args(("room", room.Id))));
            Item("I’m Here", () => session.Call("map.set_current", Json.Args(("room", room.Id))));
            menu.Items.Add(new MenuFlyoutSeparator());
            Item("Rename…", async () =>
            {
                if (await AskName("Rename the Room", room.Name) is { } name) session.Call("map.rename", Json.Args(("room", room.Id), ("name", name)));
            });
            Item("Delete", () => session.Call("map.delete", Json.Args(("room", room.Id))));
        }
        else
        {
            var at = ToMap(point);
            Item("Add a Room Here…", async () =>
            {
                if (await AskName("New Room", "") is { } name) session.Call("map.add", Json.Args(("name", name), ("area", area), ("x", at.X - 48), ("y", at.Y - 16)));
            });
        }
        menu.ShowAt(canvas, point);
    }

    private async Task<string?> AskName(string title, string name)
    {
        var box = new TextBox { Text = name, Header = "Name", MinWidth = 300 };
        box.SelectAll();
        if (await Form.Show(Form.Dialog(XamlRoot, title, box, "OK")) != ContentDialogResult.Primary) return null;
        var entered = box.Text.Trim();
        return entered.Length == 0 ? null : entered;
    }
}
