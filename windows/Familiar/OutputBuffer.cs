using System.Text.Json.Nodes;
using Windows.UI;

namespace Familiar;

/// <summary>A run of one style within a line. Offsets count UTF-16 units, as .NET strings do.</summary>
internal sealed record Span(int Start, int Length, Color Foreground, Color? Background = null, bool Bold = false, bool Italic = false, bool Underline = false, bool Strike = false, string? Link = null);

/// <summary>A picture at the start of a line (BeipMU's avatars): who is speaking.</summary>
internal sealed record Avatar(string? Id, string? Url, string? Hover, string? Link)
{
    public static Avatar From(JsonNode? avatar) => new(avatar.OptStr("id"), avatar.OptStr("url"), avatar.OptStr("hover"), avatar.OptStr("click_url") ?? avatar.OptStr("url"));
}

internal sealed class Line(string text, List<Span> spans)
{
    /// <summary>Shown before the text, if the line has one.</summary>
    public Avatar? Avatar { get; private set; }
    /// <summary>A line that is a picture rather than text: its address, which clicking it opens.</summary>
    public string? Picture { get; init; }

    public void SetAvatar(Avatar avatar)
    {
        Avatar = avatar;
        Version++;
    }

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
    private Avatar? nextAvatar;
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
        if (nextAvatar is not null)
        {
            line.SetAvatar(nextAvatar);
            nextAvatar = null;
        }
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

    /// <summary>Puts an avatar at the start of the next line.</summary>
    public void SetNextAvatar(JsonNode? avatar) => nextAvatar = Avatar.From(avatar);

    /// <summary>The server changed an avatar (beip.ids): the ones already shown change too.</summary>
    public void UpdateAvatar(JsonNode? avatar)
    {
        var updated = Avatar.From(avatar);
        if (updated.Id is null) return;
        var changed = false;
        foreach (var line in lines)
        {
            if (line.Avatar?.Id != updated.Id) continue;
            line.SetAvatar(updated);
            changed = true;
        }
        if (changed) Changed?.Invoke();
    }

    /// <summary>A thumbnail on a line of its own that opens the picture when clicked.</summary>
    public void AppendImage(string url)
    {
        ClearPartial();
        lines.Add(new Line("", []) { Picture = url });
        Trim();
        Changed?.Invoke();
    }

    public void Clear()
    {
        var count = lines.Count;
        lines.Clear();
        prompt = PromptState.None;
        nextAvatar = null;
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
