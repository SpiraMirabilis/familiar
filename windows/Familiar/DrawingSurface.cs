using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Familiar;

/// <summary>Somewhere to draw with Win2D that survives being moved about the window.
///
/// A CanvasControl stops drawing for good if it's told it was unloaded after it was loaded
/// again, which is the order WinUI reports things in when a tab's content changes hands. So
/// the canvas is made when this is loaded and thrown away when it's really gone.</summary>
internal sealed class DrawingSurface : UserControl
{
    private readonly Grid root = new() { Background = new SolidColorBrush(Microsoft.UI.Colors.Transparent) };
    private Color clearColor = Microsoft.UI.Colors.Transparent;

    public CanvasControl? Canvas { get; private set; }

    public event Action<CanvasControl, CanvasDrawEventArgs>? Draw;
    /// <summary>There's a new canvas: anything made with the old one is no use now.</summary>
    public event Action? Ready;

    public DrawingSurface()
    {
        Content = root;
        IsTabStop = false;
        Loaded += (_, _) => Create();
        Unloaded += (_, _) =>
        {
            if (!IsLoaded) Destroy();
        };
    }

    public Color ClearColor
    {
        get => clearColor;
        set
        {
            clearColor = value;
            if (Canvas is not null) Canvas.ClearColor = value;
        }
    }

    public bool ReadyToDraw => Canvas?.ReadyToDraw == true;

    public void Invalidate() => Canvas?.Invalidate();

    private void Create()
    {
        if (Canvas is not null) return;
        Canvas = new CanvasControl { ClearColor = clearColor };
        Canvas.Draw += (sender, args) => Draw?.Invoke(sender, args);
        Canvas.CreateResources += (_, _) => Ready?.Invoke();
        root.Children.Add(Canvas);
    }

    private void Destroy()
    {
        if (Canvas is null) return;
        root.Children.Remove(Canvas);
        Canvas.RemoveFromVisualTree();
        Canvas = null;
        Ready?.Invoke();
    }
}
