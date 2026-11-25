using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.System;

namespace Familiar;

/// <summary>The input box under an output area: several lines, history, spell checking, and a
/// password box that takes over when the server turns echo off.</summary>
internal sealed class InputBox : UserControl
{
    private readonly TextBox box = new()
    {
        AcceptsReturn = true,
        TextWrapping = TextWrapping.Wrap,
        IsTextPredictionEnabled = false,
        PlaceholderText = "Type a command",
    };
    private readonly PasswordBox password = new() { PlaceholderText = "Password", Visibility = Visibility.Collapsed };
    private readonly History history;
    /// <summary>Composing an accented or CJK character: the keys belong to the input method.</summary>
    private bool composing;

    /// <summary>(text, secret). Secret text is a password: never echoed, logged or kept in history.</summary>
    public event Action<string, bool>? Submitted;
    /// <summary>Sees every key first, for keyboard macros. True means it was used.</summary>
    public Func<KeyRoutedEventArgs, bool>? Key;
    /// <summary>Page Up (true) or Page Down, for scrolling the output.</summary>
    public event Action<bool>? Page;
    /// <summary>Ctrl+C with nothing selected here: copy what's selected in the output. True if it did.</summary>
    public Func<bool>? CopyOutput;

    public InputBox(FontSpec font, int historySize, bool spellcheck)
    {
        history = new History(historySize);
        box.IsSpellCheckEnabled = spellcheck;
        ScrollViewer.SetVerticalScrollBarVisibility(box, ScrollBarVisibility.Auto);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(box, "Input");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(password, "Password");
        var root = new Grid();
        root.Children.Add(box);
        root.Children.Add(password);
        Content = root;
        Font = font;

        box.PreviewKeyDown += OnKeyDown;
        box.Loaded += (_, _) => FocusIfWanted();
        password.Loaded += (_, _) => FocusIfWanted();
        LayoutUpdated += (_, _) => FocusIfWanted();
        box.TextCompositionStarted += (_, _) => composing = true;
        box.TextCompositionEnded += (_, _) => composing = false;
        // Plain text only, with Unix line ends, whatever is pasted.
        box.Paste += async (_, e) =>
        {
            e.Handled = true;
            var content = Windows.ApplicationModel.DataTransfer.Clipboard.GetContent();
            if (!content.Contains(Windows.ApplicationModel.DataTransfer.StandardDataFormats.Text)) return;
            var text = (await content.GetTextAsync()).Replace("\r\n", "\r").Replace('\n', '\r').TrimEnd('\r');
            var at = box.SelectionStart;
            box.SelectedText = text;
            box.Select(at + text.Length, 0);
        };
        password.PreviewKeyDown += (_, e) =>
        {
            if (e.Key != VirtualKey.Enter) return;
            e.Handled = true;
            var secret = password.Password;
            password.Password = "";
            Submitted?.Invoke(secret, true);
        };
    }

    public FontSpec Font
    {
        set
        {
            var family = new FontFamily(value.Family);
            box.FontFamily = family;
            box.FontSize = value.Size;
            password.FontSize = value.Size;
            // As tall as its text, from one line up to eight.
            box.MaxHeight = value.Size * 1.35 * 8 + 16;
        }
    }

    public bool Spellcheck
    {
        set => box.IsSpellCheckEnabled = value;
    }

    public int HistorySize
    {
        set => history.Limit = Math.Max(value, 1);
    }

    /// <summary>What was typed before, oldest first, for searching the history.</summary>
    public IReadOnlyList<string> PastCommands => history.Entries;

    public bool IsPassword => password.Visibility == Visibility.Visible;

    public void SetPasswordMode(bool on)
    {
        if (on == IsPassword) return;
        var focused = HasFocus;
        password.Password = "";
        password.Visibility = on ? Visibility.Visible : Visibility.Collapsed;
        box.Visibility = on ? Visibility.Collapsed : Visibility.Visible;
        if (focused) FocusInput();
    }

    public bool HasFocus => box.FocusState != FocusState.Unfocused || password.FocusState != FocusState.Unfocused;

    /// <summary>Puts the keyboard here. Asked for before the box is on screen (a tab that's just
    /// been made), it happens as soon as it is.</summary>
    public void FocusInput()
    {
        Control target = IsPassword ? password : box;
        if (target.IsLoaded && target.Focus(FocusState.Programmatic)) return;
        wantsFocus = true;
    }

    private bool wantsFocus;

    private void FocusIfWanted()
    {
        if (!wantsFocus) return;
        Control target = IsPassword ? password : box;
        if (target.IsLoaded && target.Focus(FocusState.Programmatic)) wantsFocus = false;
    }

    /// <summary>The text, with lines separated by "\n" (the text box itself uses "\r").</summary>
    public string Text
    {
        get => box.Text.Replace("\r\n", "\n").Replace('\r', '\n');
        set
        {
            box.Text = value.Replace("\r\n", "\r").Replace('\n', '\r');
            box.Select(box.Text.Length, 0);
        }
    }

    /// <summary>Types text at the cursor (the user started typing with the output focused).</summary>
    public void Insert(string text)
    {
        FocusInput();
        if (IsPassword)
        {
            password.Password += text;
            return;
        }
        var at = box.SelectionStart;
        box.SelectedText = text;
        box.Select(at + text.Length, 0);
    }

    private void Submit()
    {
        var entered = Text;
        Text = "";
        history.Push(entered);
        Submitted?.Invoke(entered, false);
    }

    /// <summary>Which line the cursor is on, and how many there are.</summary>
    private (int Line, int Count) CursorLine
    {
        get
        {
            var text = box.Text;
            var before = text.AsSpan(0, Math.Min(box.SelectionStart, text.Length));
            return (before.Count('\r'), text.AsSpan().Count('\r') + 1);
        }
    }

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (composing) return;
        if (Keys.Ctrl && e.Key == VirtualKey.C && box.SelectionLength == 0 && CopyOutput?.Invoke() == true)
        {
            e.Handled = true;
            return;
        }
        if (Key?.Invoke(e) == true)
        {
            e.Handled = true;
            return;
        }
        var control = Keys.Ctrl;
        switch (e.Key)
        {
            // Shift+Enter and Alt+Enter start a new line.
            case VirtualKey.Enter when !Keys.Shift && !Keys.Alt && !control:
                Submit();
                e.Handled = true;
                break;
            case VirtualKey.Up when control || CursorLine.Line == 0:
                if (history.Older(Text) is { } older) Text = older;
                e.Handled = true;
                break;
            case VirtualKey.Down when control || CursorLine.Line == CursorLine.Count - 1:
                if (history.Newer() is { } newer) Text = newer;
                e.Handled = true;
                break;
            case VirtualKey.PageUp or VirtualKey.PageDown:
                Page?.Invoke(e.Key == VirtualKey.PageUp);
                e.Handled = true;
                break;
            case VirtualKey.Escape:
                history.Reset();
                Text = "";
                e.Handled = true;
                break;
        }
    }
}
