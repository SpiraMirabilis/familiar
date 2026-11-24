using System.Text.Json.Nodes;
using Windows.UI;

namespace Familiar;

/// <summary>A run of one style within a line. Offsets count UTF-16 units, as .NET strings do.</summary>
internal sealed record Span(int Start, int Length, Color Foreground, Color? Background = null, bool Bold = false, bool Italic = false, bool Underline = false, bool Strike = false, string? Link = null);

internal sealed class Line(string text, List<Span> spans)
{
    public string Text { get; private set; } = text;
    public List<Span> Spans { get; } = spans;
    /// <summary>Bumped when the line's text changes, so a view knows to lay it out again.</summary>
    public int Version { get; private set; }

    public void Append(string more, Color color)
    {
        Spans.Add(new Span(Text.Length, more.Length, color));
        Text += more;
        Version++;
    }

    public string? LinkAt(int index)
    {
        foreach (var span in Spans)
        {
            if (span.Link is not null && index >= span.Start && index < span.Start + span.Length) return span.Link;
        }
        return null;
    }
}

/// <summary>The text of one output area (a world's main output, a spawn window, a puppet's
/// tab): the lines themselves, the prompt at the bottom, and the scrollback limit.</summary>
internal sealed class OutputBuffer
{
    private enum PromptState { None, Partial, Terminated }

    private readonly List<Line> lines = [];
    private PromptState prompt = PromptState.None;
    private int maxLines;

    private readonly Color foreground = AppState.Foreground;
    private readonly Color noticeColor = Blend(AppState.Foreground, AppState.Background, 0.45);
    private readonly Color echoColor = Color.FromArgb(255, 0xe5, 0xc0, 0x7b);

    public OutputBuffer(int maxLines)
    {
        this.maxLines = Math.Max(maxLines, 100);
    }

    public IReadOnlyList<Line> Lines => lines;

    public int MaxLines
    {
        get => maxLines;
        set => maxLines = Math.Max(value, 100);
    }

    /// <summary>Something changed; a view should redraw.</summary>
    public event Action? Changed;
    /// <summary>This many of the oldest lines were dropped.</summary>
    public event Action<int>? Trimmed;
    /// <summary>A whole line of text arrived (not a prompt), for reading aloud to a screen reader.</summary>
    public event Action<string>? LineAdded;

    private static Color Blend(Color a, Color b, double fraction) => Color.FromArgb(
        255,
        (byte)(a.R + (b.R - a.R) * fraction),
        (byte)(a.G + (b.G - a.G) * fraction),
        (byte)(a.B + (b.B - a.B) * fraction));

    /// <summary>A line from the engine (json::line in the bridge).</summary>
    public Line Styled(JsonNode? line)
    {
        var text = line.Str("text");
        var spans = new List<Span>();
        foreach (var span in line.Arr("spans"))
        {
            int start = span.Int("start"), length = span.Int("len");
            if (length <= 0 || start < 0 || start + length > text.Length) continue;
            spans.Add(new Span(
                start,
                length,
                Json.Hex(span.OptStr("fg")) ?? foreground,
                Json.Hex(span.OptStr("bg")),
                span.Bool("bold"),
                span.Bool("italic"),
                span.Bool("underline"),
                span.Bool("strike"),
                span.OptStr("link")));
        }
        return new Line(text, spans);
    }

    private Line Plain(string text, Color color) => new(text, [new Span(0, text.Length, color)]);

    private void ClearPartial()
    {
        if (prompt == PromptState.Partial && lines.Count > 0) lines.RemoveAt(lines.Count - 1);
        prompt = PromptState.None;
    }

    private void Append(Line line)
    {
        ClearPartial();
        lines.Add(line);
        Trim();
        LineAdded?.Invoke(line.Text);
        Changed?.Invoke();
    }

    public void AppendLine(JsonNode? line) => Append(Styled(line));

    /// <summary>A client message, shown dimmed.</summary>
    public void Notice(string text)
    {
        foreach (var row in text.Split('\n')) Append(Plain(row, noticeColor));
    }

    /// <summary>A command the user sent. It goes on the prompt's line, if there is a prompt.</summary>
    public void Echo(string text)
    {
        if (prompt == PromptState.None || lines.Count == 0)
        {
            Append(Plain(text, echoColor));
            return;
        }
        prompt = PromptState.None;
        lines[^1].Append(text, echoColor);
        Changed?.Invoke();
    }

    public void SetPrompt(JsonNode? line, bool terminated)
    {
        ClearPartial();
        lines.Add(Styled(line));
        prompt = terminated ? PromptState.Terminated : PromptState.Partial;
        Changed?.Invoke();
    }

    public void Clear()
    {
        var count = lines.Count;
        lines.Clear();
        prompt = PromptState.None;
        if (count > 0) Trimmed?.Invoke(count);
        Changed?.Invoke();
    }

    /// <summary>Drops the oldest lines once there are too many, a few hundred at a time.</summary>
    private void Trim()
    {
        if (lines.Count <= maxLines + 200) return;
        var drop = lines.Count - maxLines;
        lines.RemoveRange(0, drop);
        Trimmed?.Invoke(drop);
    }

    public string Text => string.Join("\r\n", lines.Select(l => l.Text));
}
