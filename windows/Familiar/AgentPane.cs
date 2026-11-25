using System.Text.Json.Nodes;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Web.WebView2.Core;

namespace Familiar;

/// <summary>An LLM agent (/agent: Claude Code, Codex, ...) in the side dock. The engine runs its
/// program in a ConPTY and serves it the world's tools; this shows the terminal (xterm.js, in
/// Terminal\terminal.html) and passes keys and sizes back.</summary>
internal sealed class AgentPane : UserControl
{
    private const string Host = "familiar-terminal.local";
    private readonly WebView2 view = new();
    private readonly Session session;
    // What the program printed before the page was ready for it.
    private readonly List<string> waiting = [];
    private bool ready;
    // Focus asked for before the page was ready.
    private bool focusWanted;

    public string AgentName { get; }

    public AgentPane(Session session, string name)
    {
        this.session = session;
        AgentName = name;
        Content = view;
        Prepare();
    }

    private async void Prepare()
    {
        try
        {
            await view.EnsureCoreWebView2Async();
            var core = view.CoreWebView2;
            // Keys like Ctrl+R, Ctrl+F and F5 are the agent's, not the browser's.
            core.Settings.AreBrowserAcceleratorKeysEnabled = false;
            core.Settings.AreDefaultContextMenusEnabled = false;
            core.Settings.IsStatusBarEnabled = false;
            core.SetVirtualHostNameToFolderMapping(Host, Path.Combine(AppContext.BaseDirectory, "Terminal"), CoreWebView2HostResourceAccessKind.Deny);
            core.PermissionRequested += (_, e) =>
            {
                if (e.PermissionKind == CoreWebView2PermissionKind.ClipboardRead) e.State = CoreWebView2PermissionState.Allow;
            };
            core.WebMessageReceived += (_, e) =>
            {
                if (JsonNode.Parse(e.WebMessageAsJson) is { } message) OnMessage(message);
            };
            core.Navigate($"https://{Host}/terminal.html");
        }
        catch (Exception e)
        {
            session.Output.Notice($"Couldn’t show the agent’s terminal: {e.Message}");
        }
    }

    private void OnMessage(JsonNode message)
    {
        switch (message.Str("op"))
        {
            case "ready":
                var font = session.Font;
                Post(Json.Args(
                    ("op", "start"),
                    ("font", font.Family),
                    ("size", font.Size),
                    ("background", Hex(AppState.Background)),
                    ("foreground", Hex(AppState.Foreground))));
                ready = true;
                foreach (var data in waiting) Post(Json.Args(("op", "write"), ("data", data)));
                waiting.Clear();
                if (focusWanted) FocusTerminal();
                break;
            case "input": session.AgentWrite(message.Str("data")); break;
            case "resize": session.AgentResize(message.Int("cols"), message.Int("rows")); break;
        }
    }

    private static string Hex(Windows.UI.Color c) => $"#{c.R:x2}{c.G:x2}{c.B:x2}";

    private void Post(JsonNode message)
    {
        try
        {
            view.CoreWebView2?.PostWebMessageAsJson(message.ToJsonString());
        }
        catch (Exception)
        {
            // The page is gone.
        }
    }

    /// <summary>What the program printed, as base64.</summary>
    public void Write(string data)
    {
        if (ready) Post(Json.Args(("op", "write"), ("data", data)));
        else waiting.Add(data);
    }

    /// <summary>Puts the keyboard in the terminal (once it's on screen, if it isn't yet).</summary>
    public void FocusTerminal()
    {
        focusWanted = !ready;
        if (!ready) return;
        view.Focus(Microsoft.UI.Xaml.FocusState.Programmatic);
        Post(Json.Args(("op", "focus")));
    }

    public void Shut() => view.Close();
}
