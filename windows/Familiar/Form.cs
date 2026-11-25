using System.Text.Json.Nodes;
using Microsoft.Graphics.Canvas.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.Storage.Pickers;

namespace Familiar;

/// <summary>Controls that edit one value of a JSON object (a world, a character, a rule) in
/// place, for the dialogs.</summary>
internal static class Form
{
    public static Style Style(string key) => (Style)Application.Current.Resources[key];
    public static Brush Brush(string key) => (Brush)Application.Current.Resources[key];

    public static TextBlock Heading(string text) => new() { Text = text, Style = Style("BodyStrongTextBlockStyle"), Margin = new Thickness(0, 12, 0, 0) };

    public static TextBlock Caption(string text) => new() { Text = text, Style = Style("CaptionTextBlockStyle"), Foreground = Brush("TextFillColorSecondaryBrush"), TextWrapping = TextWrapping.Wrap };

    public static TextBox Text(JsonObject o, string key, string header, string? placeholder = null, Action? changed = null)
    {
        var box = new TextBox { Header = header, Text = o.Str(key), PlaceholderText = placeholder ?? "" };
        box.TextChanged += (_, _) =>
        {
            o[key] = box.Text;
            changed?.Invoke();
        };
        return box;
    }

    /// <summary>Fixed-width text in a box, while its heading stays in the window's font.</summary>
    public static TextBox Mono(TextBox box)
    {
        if (box.Header is string header) box.Header = new TextBlock { Text = header, FontFamily = new FontFamily("Segoe UI Variable") };
        box.FontFamily = new FontFamily(FontSpec.SystemMonospace);
        return box;
    }

    public static CheckBox Check(JsonObject o, string key, string label, string? tip = null, Action? changed = null)
    {
        var box = new CheckBox { Content = label, IsChecked = o.Bool(key) };
        if (tip is not null) ToolTipService.SetToolTip(box, tip);
        void Set()
        {
            o[key] = box.IsChecked == true;
            changed?.Invoke();
        }
        box.Checked += (_, _) => Set();
        box.Unchecked += (_, _) => Set();
        return box;
    }

    public static NumberBox Number(JsonObject o, string key, string header, double min, double max, Action? changed = null)
    {
        var box = new NumberBox { Header = header, Value = o.Num(key), Minimum = min, Maximum = max, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact, HorizontalAlignment = HorizontalAlignment.Left, MinWidth = 140 };
        box.ValueChanged += (_, _) =>
        {
            if (double.IsNaN(box.Value)) return;
            o[key] = (long)box.Value;
            changed?.Invoke();
        };
        return box;
    }

    /// <summary>A drop-down of labelled values.</summary>
    public static ComboBox Choice(string? header, IEnumerable<(string Label, string Value)> options, string current, Action<string> set)
    {
        var box = new ComboBox { Header = header, MinWidth = 180 };
        foreach (var (label, value) in options)
        {
            box.Items.Add(new ComboBoxItem { Content = label, Tag = value });
            if (value == current) box.SelectedIndex = box.Items.Count - 1;
        }
        box.SelectionChanged += (_, _) =>
        {
            if (box.SelectedItem is ComboBoxItem { Tag: string value }) set(value);
        };
        return box;
    }

    /// <summary>Asks for a file. Null if the user cancelled.</summary>
    public static async Task<string?> PickFile(params string[] extensions)
    {
        if (App.Window is null) return null;
        var picker = new FileOpenPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(App.Window));
        foreach (var extension in extensions) picker.FileTypeFilter.Add(extension);
        picker.FileTypeFilter.Add("*");
        return (await picker.PickSingleFileAsync())?.Path;
    }

    /// <summary>Asks where to save a file. Null if the user cancelled.</summary>
    public static async Task<string?> PickSaveFile(string suggestedName, string typeName, string extension)
    {
        if (App.Window is null) return null;
        var picker = new FileSavePicker { SuggestedFileName = suggestedName, SuggestedStartLocation = PickerLocationId.DocumentsLibrary };
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(App.Window));
        picker.FileTypeChoices.Add(typeName, [extension]);
        return (await picker.PickSaveFileAsync())?.Path;
    }

    /// <summary>Shows a dialog, unless another is already open (only one can be at a time).</summary>
    public static async Task<ContentDialogResult> Show(ContentDialog dialog)
    {
        try
        {
            return await dialog.ShowAsync();
        }
        catch (Exception)
        {
            return ContentDialogResult.None;
        }
    }

    public static ContentDialog Dialog(XamlRoot root, string title, object content, string? primary = null, string close = "Cancel") => new()
    {
        XamlRoot = root,
        Title = title,
        Content = content,
        PrimaryButtonText = primary ?? "",
        CloseButtonText = close,
        DefaultButton = primary is null ? ContentDialogButton.Close : ContentDialogButton.Primary,
        Style = Style("DefaultContentDialogStyle"),
    };

    public static async Task<bool> Confirm(XamlRoot root, string title, string detail, string action)
    {
        var dialog = Dialog(root, title, new TextBlock { Text = detail, TextWrapping = TextWrapping.Wrap }, action);
        return await Show(dialog) == ContentDialogResult.Primary;
    }
}

/// <summary>Chooses a fixed-width font and its size, as the description the config stores.</summary>
internal sealed class FontPicker : StackPanel
{
    private static readonly Lazy<List<string>> Monospaced = new(() =>
    {
        var families = new SortedSet<string>(StringComparer.OrdinalIgnoreCase);
        using var fonts = CanvasFontSet.GetSystemFontSet();
        foreach (var face in fonts.Fonts)
        {
            if (!face.IsMonospaced || face.IsSymbolFont) continue;
            if (face.FamilyNames.TryGetValue("en-us", out var name) || (name = face.FamilyNames.Values.FirstOrDefault()) is not null) families.Add(name);
        }
        return [.. families];
    });

    private readonly ComboBox family = new() { MinWidth = 200 };
    private readonly NumberBox size = new() { Minimum = 6, Maximum = 48, SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Compact, MinWidth = 110 };
    private bool setting;

    public event Action? Changed;

    public FontPicker(string description)
    {
        Orientation = Orientation.Horizontal;
        Spacing = 8;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(family, "Font");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(size, "Font size");
        foreach (var name in Monospaced.Value) family.Items.Add(name);
        Children.Add(family);
        Children.Add(size);
        Description = description;
        family.SelectionChanged += (_, _) => Raise();
        size.ValueChanged += (_, _) => Raise();
    }

    private void Raise()
    {
        if (!setting && !double.IsNaN(size.Value)) Changed?.Invoke();
    }

    public string Description
    {
        get => FontSpec.Describe(family.SelectedItem as string ?? "", size.Value);
        set
        {
            setting = true;
            var font = FontSpec.Parse(value);
            if (!family.Items.Contains(font.Family)) family.Items.Add(font.Family);
            family.SelectedItem = font.Family;
            size.Value = font.Points;
            setting = false;
        }
    }
}
