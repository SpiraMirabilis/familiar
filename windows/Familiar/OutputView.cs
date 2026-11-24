using System.Numerics;
using System.Text;
using Microsoft.Graphics.Canvas.Text;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.Foundation;
using Windows.System;
using Windows.UI;
using Windows.UI.Text;

namespace Familiar;

/// <summary>Shows an <see cref="OutputBuffer"/>: read-only, selectable, with clickable links.
///
/// The text is drawn with DirectWrite (through Win2D), and only the lines on screen are ever
/// laid out, so a long scrollback costs nothing. That means there is no total height to scroll
/// through: the position is a line and an offset into it, and the scroll bar counts lines.</summary>
internal sealed class OutputView : UserControl
{
    private const float Pad = 8;

    private readonly DrawingSurface canvas = new();
    private readonly ScrollBar bar = new() { Orientation = Orientation.Vertical, IndicatorMode = ScrollingIndicatorMode.MouseIndicator, HorizontalAlignment = HorizontalAlignment.Right, SmallChange = 1 };
    private readonly Dictionary<Line, (int Version, CanvasTextLayout Layout)> layouts = [];
    private readonly List<(int Index, float Y, float Height, CanvasTextLayout Layout)> visible = [];

    private OutputBuffer buffer;
    private FontSpec font;
    private CanvasTextFormat? format;
    private float lineHeight = 16, cellWidth = 8;

    /// <summary>Stay at the bottom as lines arrive. Off while the user has scrolled up.</summary>
    private bool follow = true;
    private int topLine;
    private float topOffset;
    private bool settingBar;

    // The selection, as (line, character) positions. Lines are named by the objects themselves,
    // so the selection survives old lines being trimmed away.
    private (Line Line, int Char)? anchor, caret;
    private bool selecting;
    private string? pressedLink;
    private (int Cols, int Rows) grid;

    public event Action<string>? LinkClicked;
    /// <summary>The visible size in character cells changed (for telnet NAWS).</summary>
    public event Action<int, int>? GridSizeChanged;
    /// <summary>A key that types something was pressed here: the input box should take the focus.</summary>
    public event Action? TypingStarted;

    public OutputView(OutputBuffer buffer, FontSpec font)
    {
        this.buffer = buffer;
        this.font = font;
        IsTabStop = true;
        UseSystemFocusVisuals = false;
        canvas.ClearColor = AppState.Background;
        var root = new Grid();
        root.Children.Add(canvas);
        root.Children.Add(bar);
        Content = root;

        canvas.Draw += OnDraw;
        canvas.Ready += Reset;
        canvas.SizeChanged += (_, _) =>
        {
            Reset();
            ReportGrid();
        };
        bar.Scroll += OnBarScroll;
        PointerPressed += OnPointerPressed;
        PointerMoved += OnPointerMoved;
        PointerReleased += OnPointerReleased;
        PointerWheelChanged += OnWheel;
        KeyDown += OnKeyDown;
        // The two can arrive out of order when a tab's content changes hands.
        Loaded += (_, _) => Attach();
        Unloaded += (_, _) =>
        {
            if (!IsLoaded) Detach();
        };
        Attach();

        var menu = new MenuFlyout();
        var copy = new MenuFlyoutItem { Text = "Copy", Icon = new SymbolIcon(Symbol.Copy), KeyboardAcceleratorTextOverride = "Ctrl+C" };
        copy.Click += (_, _) => Copy();
        var all = new MenuFlyoutItem { Text = "Select All", Icon = new SymbolIcon(Symbol.SelectAll) };
        all.Click += (_, _) => SelectAll();
        menu.Items.Add(copy);
        menu.Items.Add(all);
        menu.Opening += (_, _) => copy.IsEnabled = HasSelection;
        ContextFlyout = menu;
    }

    private void Attach()
    {
        Detach();
        buffer.Changed += OnChanged;
        buffer.Trimmed += OnTrimmed;
        buffer.LineAdded += Announce;
    }

    private void Detach()
    {
        buffer.Changed -= OnChanged;
        buffer.Trimmed -= OnTrimmed;
        buffer.LineAdded -= Announce;
    }

    public FontSpec Font
    {
        get => font;
        set
        {
            if (font == value) return;
            font = value;
            format?.Dispose();
            format = null;
            Reset();
            ReportGrid();
        }
    }

#if DEBUG
    internal string Debug => $"canvas {canvas.ActualWidth}x{canvas.ActualHeight} ready {canvas.ReadyToDraw} lines {buffer.Lines.Count} top {topLine}+{topOffset} follow {follow} visible {visible.Count} lineHeight {lineHeight} cell {cellWidth} draws {draws} last {lastDraw}";
    private int draws;
    private string lastDraw = "";
#endif

    // MARK: Layout

    private CanvasTextFormat Format => format ??= new CanvasTextFormat
    {
        FontFamily = font.Family,
        FontSize = font.Size,
        FontWeight = font.Bold ? Microsoft.UI.Text.FontWeights.Bold : Microsoft.UI.Text.FontWeights.Normal,
        FontStyle = font.Italic ? FontStyle.Italic : FontStyle.Normal,
        WordWrapping = CanvasWordWrapping.EmergencyBreak,
    };

    private float TextWidth => Math.Max(1, (float)canvas.ActualWidth - Pad * 2 - 12);
    private float ViewHeight => Math.Max(1, (float)canvas.ActualHeight - Pad * 2);

    /// <summary>Forgets every layout: the width or the font changed.</summary>
    private void Reset()
    {
        foreach (var (_, layout) in layouts.Values) layout.Dispose();
        layouts.Clear();
        visible.Clear();
        if (canvas.ReadyToDraw)
        {
            using var probe = new CanvasTextLayout(canvas.Canvas, "MMMMMMMMMM", Format, 10000, 0);
            cellWidth = (float)probe.LayoutBounds.Width / 10;
            lineHeight = (float)probe.LayoutBounds.Height;
        }
        canvas.Invalidate();
    }

    private CanvasTextLayout Layout(Line line)
    {
        if (layouts.TryGetValue(line, out var cached))
        {
            if (cached.Version == line.Version) return cached.Layout;
            cached.Layout.Dispose();
        }
        // An empty string would have no height.
        var layout = new CanvasTextLayout(canvas.Canvas, line.Text.Length == 0 ? " " : line.Text, Format, TextWidth, 0);
        foreach (var span in line.Spans)
        {
            layout.SetColor(span.Start, span.Length, span.Foreground);
            if (span.Bold) layout.SetFontWeight(span.Start, span.Length, Microsoft.UI.Text.FontWeights.Bold);
            if (span.Italic) layout.SetFontStyle(span.Start, span.Length, FontStyle.Italic);
            if (span.Underline || span.Link is not null) layout.SetUnderline(span.Start, span.Length, true);
            if (span.Strike) layout.SetStrikethrough(span.Start, span.Length, true);
        }
        layouts[line] = (line.Version, layout);
        return layout;
    }

    private float RowHeight(int index) => (float)Layout(buffer.Lines[index]).LayoutBounds.Height;

    /// <summary>Works out where the top of the view is when it's following the bottom.</summary>
    private void AnchorToBottom()
    {
        var lines = buffer.Lines;
        float height = 0;
        for (var i = lines.Count - 1; i >= 0; i--)
        {
            height += RowHeight(i);
            if (height >= ViewHeight)
            {
                topLine = i;
                topOffset = height - ViewHeight;
                return;
            }
        }
        topLine = 0;
        topOffset = 0;
    }

    private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
    {
        var ds = args.DrawingSession;
        var lines = buffer.Lines;
        visible.Clear();
        topLine = Math.Clamp(topLine, 0, Math.Max(0, lines.Count - 1));
        if (follow) AnchorToBottom();

        var selection = Selection();
        var highlight = SelectionColor();
        var bottom = (float)sender.ActualHeight;
        var y = Pad - topOffset;
        var used = new HashSet<Line>();
        for (var i = topLine; i < lines.Count && y < bottom; i++)
        {
            var line = lines[i];
            var layout = Layout(line);
            var height = (float)layout.LayoutBounds.Height;
            used.Add(line);
            foreach (var span in line.Spans)
            {
                if (span.Background is not { } background) continue;
                foreach (var region in layout.GetCharacterRegions(span.Start, span.Length))
                {
                    ds.FillRectangle(Offset(region.LayoutBounds, Pad, y), background);
                }
            }
            if (selection is { } s && i >= s.First && i <= s.Last)
            {
                var from = i == s.First ? s.FirstChar : 0;
                var to = i == s.Last ? s.LastChar : line.Text.Length;
                if (to > from)
                {
                    foreach (var region in layout.GetCharacterRegions(from, to - from)) ds.FillRectangle(Offset(region.LayoutBounds, Pad, y), highlight);
                }
                // The line break is part of the selection too.
                if (i < s.Last)
                {
                    var end = layout.GetCaretPosition(line.Text.Length, false);
                    ds.FillRectangle(Pad + end.X, y + height - lineHeight, cellWidth * 0.6f, lineHeight, highlight);
                }
            }
            ds.DrawTextLayout(layout, new Vector2(Pad, y), AppState.Foreground);
            visible.Add((i, y, height, layout));
            y += height;
        }

        // Only what's on screen stays laid out.
        if (layouts.Count > used.Count + 64)
        {
            foreach (var line in layouts.Keys.Where(l => !used.Contains(l)).ToList())
            {
                layouts[line].Layout.Dispose();
                layouts.Remove(line);
            }
        }
        UpdateBar();
#if DEBUG
        draws++;
        lastDraw = $"y {y} bottom {bottom} first {(visible.Count > 0 ? visible[0].Y : -1)}/{(visible.Count > 0 ? visible[0].Height : -1)}";
#endif
    }

    private static Rect Offset(Rect r, float x, float y) => new(r.X + x, r.Y + y, r.Width, r.Height);

    private Color SelectionColor()
    {
        var accent = Application.Current.Resources.TryGetValue("SystemAccentColor", out var value) && value is Color c ? c : Color.FromArgb(255, 0, 120, 215);
        return Color.FromArgb(110, accent.R, accent.G, accent.B);
    }

    // MARK: Scrolling

    private void UpdateBar()
    {
        var shown = Math.Max(1, visible.Count);
        settingBar = true;
        bar.Maximum = Math.Max(0, buffer.Lines.Count - shown);
        bar.ViewportSize = shown;
        bar.LargeChange = Math.Max(1, shown - 1);
        bar.Value = follow ? bar.Maximum : Math.Min(topLine, bar.Maximum);
        bar.Visibility = bar.Maximum > 0 ? Visibility.Visible : Visibility.Collapsed;
        settingBar = false;
    }

    private void OnBarScroll(object sender, ScrollEventArgs e)
    {
        if (settingBar) return;
        if (e.NewValue >= bar.Maximum - 0.5)
        {
            follow = true;
        }
        else
        {
            follow = false;
            topLine = (int)e.NewValue;
            topOffset = 0;
        }
        canvas.Invalidate();
    }

    private void ScrollBy(float dy)
    {
        var lines = buffer.Lines;
        if (lines.Count == 0 || !canvas.ReadyToDraw) return;
        if (follow) AnchorToBottom();
        follow = false;
        topLine = Math.Clamp(topLine, 0, lines.Count - 1);
        topOffset += dy;
        while (topOffset < 0 && topLine > 0)
        {
            topLine--;
            topOffset += RowHeight(topLine);
        }
        if (topOffset < 0) topOffset = 0;
        while (topLine < lines.Count - 1 && topOffset >= RowHeight(topLine))
        {
            topOffset -= RowHeight(topLine);
            topLine++;
        }
        // Does what's below the top still fill the view? If not, we're at the bottom.
        var below = -topOffset;
        var i = topLine;
        for (; i < lines.Count && below <= ViewHeight; i++) below += RowHeight(i);
        if (below <= ViewHeight) follow = true;
        canvas.Invalidate();
    }

    public void ScrollToEnd()
    {
        follow = true;
        canvas.Invalidate();
    }

    public void ScrollPage(bool up) => ScrollBy((up ? -1 : 1) * ViewHeight * 0.9f);

    private void OnWheel(object sender, PointerRoutedEventArgs e)
    {
        var delta = e.GetCurrentPoint(this).Properties.MouseWheelDelta;
        ScrollBy(-delta / 120f * 3 * lineHeight);
        e.Handled = true;
    }

    private void OnChanged() => canvas.Invalidate();

    private void OnTrimmed(int count)
    {
        topLine = Math.Max(0, topLine - count);
        var lines = buffer.Lines;
        if (anchor is { } a && !lines.Contains(a.Line)) anchor = caret = null;
        if (caret is { } c && !lines.Contains(c.Line)) anchor = caret = null;
    }

    private void ReportGrid()
    {
        if (!canvas.ReadyToDraw || cellWidth <= 0 || lineHeight <= 0) return;
        var now = (Math.Max(1, (int)(TextWidth / cellWidth)), Math.Max(1, (int)(ViewHeight / lineHeight)));
        if (now == grid || canvas.ActualWidth < 1) return;
        grid = now;
        GridSizeChanged?.Invoke(now.Item1, now.Item2);
    }

    // MARK: The mouse

    /// <summary>The line and character under a point, and whether the point is on the text itself.</summary>
    private ((Line Line, int Char) Position, bool Inside)? Hit(Point point)
    {
        if (visible.Count == 0) return null;
        var lines = buffer.Lines;
        foreach (var (index, y, height, layout) in visible)
        {
            if (point.Y >= y + height && index != visible[^1].Index) continue;
            if (index >= lines.Count) return null;
            var line = lines[index];
            if (point.Y < y) return ((line, 0), false);
            var inside = layout.HitTest((float)point.X - Pad, (float)point.Y - y, out var region, out var trailing);
            var at = Math.Min(region.CharacterIndex + (trailing ? region.CharacterCount : 0), line.Text.Length);
            if (point.Y >= y + height) return ((line, line.Text.Length), false);
            return ((line, at), inside && line.Text.Length > 0);
        }
        return null;
    }

    private string? LinkAt(Point point)
    {
        foreach (var (index, y, height, layout) in visible)
        {
            if (point.Y < y || point.Y >= y + height || index >= buffer.Lines.Count) continue;
            if (!layout.HitTest((float)point.X - Pad, (float)point.Y - y, out var region)) return null;
            return buffer.Lines[index].LinkAt(region.CharacterIndex);
        }
        return null;
    }

    private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(canvas);
        if (!point.Properties.IsLeftButtonPressed) return;
        Focus(FocusState.Pointer);
        pressedLink = LinkAt(point.Position);
        if (Hit(point.Position) is not { } hit) return;
        if (Keys.Shift && anchor is not null)
        {
            caret = hit.Position;
        }
        else
        {
            anchor = caret = hit.Position;
        }
        selecting = true;
        CapturePointer(e.Pointer);
        canvas.Invalidate();
    }

    private void OnPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(canvas).Position;
        if (selecting)
        {
            // Dragging past an edge scrolls.
            if (point.Y < 0) ScrollBy(-lineHeight);
            else if (point.Y > canvas.ActualHeight) ScrollBy(lineHeight);
            if (Hit(new Point(point.X, Math.Clamp(point.Y, 0, canvas.ActualHeight - 1))) is { } hit && hit.Position != caret)
            {
                caret = hit.Position;
                canvas.Invalidate();
            }
            return;
        }
        ProtectedCursor = InputSystemCursor.Create(LinkAt(point) is null ? InputSystemCursorShape.IBeam : InputSystemCursorShape.Hand);
    }

    private void OnPointerReleased(object sender, PointerRoutedEventArgs e)
    {
        if (!selecting) return;
        selecting = false;
        ReleasePointerCapture(e.Pointer);
        var link = LinkAt(e.GetCurrentPoint(canvas).Position);
        if (!HasSelection && link is not null && link == pressedLink) LinkClicked?.Invoke(link);
        pressedLink = null;
    }

    /// <summary>Whether a key types something: a letter, a digit, punctuation or a space.</summary>
    private static bool Types(VirtualKey key) =>
        key is (>= VirtualKey.A and <= VirtualKey.Z) or (>= VirtualKey.Number0 and <= VirtualKey.Number9) or (>= VirtualKey.NumberPad0 and <= VirtualKey.Divide) or VirtualKey.Space
        || (int)key is (>= 186 and <= 192) or (>= 219 and <= 223) or 226;

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        // Typing here belongs in the input box. Moving the focus there now, while the key is
        // still going down, means the character it makes is typed there like any other.
        if (!Keys.Ctrl && !Keys.Alt && Types(e.Key))
        {
            TypingStarted?.Invoke();
            return;
        }
        if (Keys.Ctrl && e.Key == VirtualKey.C)
        {
            Copy();
            e.Handled = true;
        }
        else if (Keys.Ctrl && e.Key == VirtualKey.A)
        {
            SelectAll();
            e.Handled = true;
        }
        else if (e.Key is VirtualKey.PageUp or VirtualKey.PageDown)
        {
            ScrollPage(e.Key == VirtualKey.PageUp);
            e.Handled = true;
        }
        else if (Keys.Ctrl && e.Key is VirtualKey.Home)
        {
            follow = false;
            topLine = 0;
            topOffset = 0;
            canvas.Invalidate();
            e.Handled = true;
        }
        else if (Keys.Ctrl && e.Key is VirtualKey.End)
        {
            ScrollToEnd();
            e.Handled = true;
        }
    }

    // MARK: The selection

    /// <summary>The selection in line order, as indexes into the buffer.</summary>
    private (int First, int FirstChar, int Last, int LastChar)? Selection()
    {
        if (anchor is not { } a || caret is not { } c || a == c) return null;
        var lines = buffer.Lines;
        int ai = IndexOf(lines, a.Line), ci = IndexOf(lines, c.Line);
        if (ai < 0 || ci < 0) return null;
        return (ai, a.Char).CompareTo((ci, c.Char)) <= 0 ? (ai, a.Char, ci, c.Char) : (ci, c.Char, ai, a.Char);
    }

    private static int IndexOf(IReadOnlyList<Line> lines, Line line)
    {
        // Selections are nearly always near the end.
        for (var i = lines.Count - 1; i >= 0; i--)
        {
            if (ReferenceEquals(lines[i], line)) return i;
        }
        return -1;
    }

    public bool HasSelection => Selection() is not null;

    public void SelectAll()
    {
        var lines = buffer.Lines;
        if (lines.Count == 0) return;
        anchor = (lines[0], 0);
        caret = (lines[^1], lines[^1].Text.Length);
        canvas.Invalidate();
    }

    public void ClearSelection()
    {
        anchor = caret = null;
        canvas.Invalidate();
    }

    public void Copy()
    {
        if (Selection() is not { } s) return;
        var lines = buffer.Lines;
        var text = new StringBuilder();
        for (var i = s.First; i <= s.Last; i++)
        {
            var line = lines[i].Text;
            var from = i == s.First ? Math.Min(s.FirstChar, line.Length) : 0;
            var to = i == s.Last ? Math.Min(s.LastChar, line.Length) : line.Length;
            text.Append(line, from, Math.Max(0, to - from));
            if (i < s.Last) text.Append("\r\n");
        }
        var package = new DataPackage();
        package.SetText(text.ToString());
        Clipboard.SetContent(package);
    }

    // MARK: Screen readers

    protected override AutomationPeer OnCreateAutomationPeer() => new Peer(this);

    /// <summary>Reads a new line aloud, if a screen reader is listening.</summary>
    private void Announce(string text)
    {
        if (text.Length == 0) return;
        // There's only a peer once something has asked for one.
        FrameworkElementAutomationPeer.FromElement(this)?.RaiseNotificationEvent(
            AutomationNotificationKind.ItemAdded, AutomationNotificationProcessing.All, text, "FamiliarOutput");
    }

    private sealed class Peer(OutputView owner) : FrameworkElementAutomationPeer(owner)
    {
        protected override string GetNameCore() => "Output";
        protected override string GetClassNameCore() => "OutputView";
        protected override AutomationControlType GetAutomationControlTypeCore() => AutomationControlType.Document;
        protected override bool IsContentElementCore() => true;
        protected override bool IsControlElementCore() => true;
    }
}
