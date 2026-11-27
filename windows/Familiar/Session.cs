using System.Runtime.InteropServices;
using System.Text.Json.Nodes;
using Microsoft.UI.Dispatching;

namespace Familiar;

internal enum ConnectionState { Connecting, Connected, Disconnected }

/// <summary>One connection: the engine's session and what it has shown. A
/// <see cref="SessionView"/> puts it on screen.</summary>
internal sealed partial class Session
{
    [LibraryImport("user32")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool MessageBeep(uint type);

    public long Id { get; }
    public JsonNode Info { get; private set; }
    public ConnectionState State { get; private set; } = ConnectionState.Disconnected;
    /// <summary>MCP status text, shown under the output.</summary>
    public string StatusText { get; private set; } = "";
    public bool Logging { get; private set; }
    /// <summary>The connection ended (rather than never having started), and why, if the engine knows.</summary>
    public bool Dropped { get; private set; }
    public string? DropReason { get; private set; }
    /// <summary>New lines since this session's tab was last looked at.</summary>
    public bool Unread { get; private set; }
    public bool Important { get; private set; }

    public OutputBuffer Output { get; }
    public Dictionary<string, OutputBuffer> Spawns { get; } = [];
    public Dictionary<string, OutputBuffer> Puppets { get; } = [];
    /// <summary>Set while there are web views open, which are told about every line.</summary>
    public bool HasPages { get; set; }

    // What the engine says that only a view can act on (panels, editors, web views). It's kept
    // until there is a view.
    private readonly List<JsonNode?> waiting = [];
    private Action<JsonNode?>? view;

    private readonly SoundPlayer sounds;
    private readonly DispatcherQueueTimer mapSaver;
    private bool closed;

    /// <summary>The title, the connection state, logging or the activity marks changed.</summary>
    public event Action? Changed;
    public event Action<bool>? PasswordMode;
    public event Action<string>? SetInput;
    /// <summary>Whether this session is the one the user is looking at right now.</summary>
    public Func<bool>? IsShowing;

    public string Title => Info.Str("title");
    public string WorldName => Info.Get("world").Str("name");
    public FontSpec Font => AppState.FontFor(WorldName);
    public bool IsConnected => State != ConnectionState.Disconnected;

    public Session(JsonNode world, string? character)
    {
        // LLM agents (/agent): the engine runs them in a ConPTY, and the dock shows the terminal.
        var opened = Core.Call("session.open", Json.Args(("world", world), ("character", character), ("agents", "pty")))!;
        Id = opened.Long("id");
        Info = opened;
        Output = new OutputBuffer(AppState.Settings.Int("scrollback_lines"));
        sounds = new SoundPlayer(Id);
        AppState.Sessions[Id] = this;
        Apply(opened.Arr("outputs"));
        // The map is also saved when the session closes.
        mapSaver = DispatcherQueue.GetForCurrentThread().CreateTimer();
        mapSaver.Interval = TimeSpan.FromSeconds(30);
        mapSaver.Tick += (_, _) => SaveMap();
        mapSaver.Start();
    }

    /// <summary>Gives the view the outputs that are its to show, starting with any that came before it.</summary>
    public void Attach(Action<JsonNode?> handler)
    {
        view = handler;
        foreach (var o in waiting) handler(o);
        waiting.Clear();
    }

    private void ToView(JsonNode? o)
    {
        if (view is null) waiting.Add(o);
        else view(o);
    }

    public bool IsClosed => closed;

    // MARK: Talking to the engine

    public JsonNode? Call(string method, JsonObject? args = null)
    {
        if (closed) return null;
        args ??= [];
        args["id"] = Id;
        var reply = Core.Send(method, args);
        Apply(reply.Arr("outputs"));
        return reply;
    }

    /// <summary>Collects what the engine has for us: network traffic and timers.</summary>
    public void Poll()
    {
        var reply = Call("session.poll");
        if (reply.Bool("more"))
        {
            // Let the window redraw between batches of a flood.
            DispatcherQueue.GetForCurrentThread().TryEnqueue(DispatcherQueuePriority.Low, Poll);
        }
    }

    public void Connect() => Call("session.connect");
    public void Disconnect() => Call("session.disconnect");
    public void ToggleLog() => Call("session.toggle_log");
    public void FollowLink(string uri) => Call("session.link", Json.Args(("uri", uri)));
    public void RestartScripts() => Call("session.restart_scripts");
    public void Submit(string text, bool secret = false) => Call("session.submit", Json.Args(("text", text), ("secret", secret)));
    public void Resize(int cols, int rows) => Call("session.resize", Json.Args(("cols", cols), ("rows", rows)));

    public void AgentWrite(string data) => Call("agent.write", Json.Args(("data", data)));
    public void AgentResize(int cols, int rows) => Call("agent.resize", Json.Args(("cols", cols), ("rows", rows)));
    public void AgentStop() => Call("agent.stop");

    public void PuppetSubmit(string name, string text) => Call("session.puppet_submit", Json.Args(("name", name), ("text", text)));
    public void SaveEdit(JsonNode request, string content)
    {
        // Remembered, so it can be reopened if the server finds errors in it.
        var sent = request.DeepClone();
        sent["content"] = content;
        LastEdit = (sent, null);
        Call("session.save_edit", Json.Args(("request", request), ("content", content)));
    }

    /// <summary>The last program sent back from an editor, and the first compile error the
    /// server reported in it since (line, message).</summary>
    public (JsonNode Request, (int Line, string Message)? Error)? LastEdit { get; private set; }

    /// <summary>The user closed a puppet's tab.</summary>
    public void PuppetClosed(string name)
    {
        Puppets.Remove(name);
        if (!closed) Core.Send("session.puppet_open", Json.Args(("id", Id), ("name", name), ("open", false)));
    }

    /// <summary>The map as the engine has it now: rooms, areas and the current room.</summary>
    public JsonNode? Map() => closed ? null : Core.Send("map.get", Json.Args(("id", Id)));

    /// <summary>True if the key was a macro's.</summary>
    public bool RunMacro(JsonObject chord) => Call("session.macro", chord).Bool("handled");

    public void SaveMap()
    {
        if (closed) return;
        try
        {
            Core.Call("map.save", Json.Args(("id", Id)));
        }
        catch (CoreException e)
        {
            Output.Notice($"Couldn’t save the map: {e.Message}");
        }
    }

    /// <summary>The config changed: fonts and settings may have.</summary>
    public void ConfigChanged()
    {
        if (closed) return;
        Info = Core.Send("session.info", Json.Args(("id", Id))) ?? Info;
        Output.MaxLines = AppState.Settings.Int("scrollback_lines");
        foreach (var spawn in Spawns.Values) spawn.MaxLines = Output.MaxLines;
        Changed?.Invoke();
    }

    /// <summary>Closes the connection and everything that belongs to it.</summary>
    public void Close()
    {
        if (closed) return;
        mapSaver.Stop();
        sounds.Stop(null);
        try
        {
            Core.Call("session.close", Json.Args(("id", Id)));
        }
        catch (CoreException e)
        {
            System.Diagnostics.Debug.WriteLine($"Familiar: closing a session: {e.Message}");
        }
        closed = true;
        AppState.Sessions.Remove(Id);
    }

    /// <summary>The user is looking at this session's tab.</summary>
    public void Seen()
    {
        if (!Unread && !Important) return;
        Unread = false;
        Important = false;
        Changed?.Invoke();
    }

    public void StopSounds()
    {
        sounds.Stop(null);
        Core.Send("speech.stop");
    }

    // MARK: Showing what the engine says

    private OutputBuffer Spawn(string name)
    {
        if (!Spawns.TryGetValue(name, out var buffer))
        {
            buffer = new OutputBuffer(AppState.Settings.Int("scrollback_lines"));
            Spawns[name] = buffer;
            ToView(Json.Args(("t", "spawn"), ("name", name)));
        }
        return buffer;
    }

    private void Apply(IList<JsonNode?> outputs)
    {
        foreach (var o in outputs)
        {
            switch (o.Str("t"))
            {
                case "line":
                    if (o.OptStr("target") is { } target)
                    {
                        Spawn(target).AppendLine(o.Get("line"));
                    }
                    else
                    {
                        Output.AppendLine(o.Get("line"));
                        if (o.Bool("bell")) MessageBeep(0);
                        if (HasPages) ToView(Json.Args(("t", "display"), ("text", o.Str("plain"))));
                    }
                    break;
                case "prompt": Output.SetPrompt(o.Get("line"), o.Bool("terminated")); break;
                case "notice": Output.Notice(o.Str("text")); break;
                case "echo": Output.Echo(o.Str("text")); break;
                case "bell": MessageBeep(0); break;
                case "password": PasswordMode?.Invoke(o.Bool("on")); break;
                case "status": SetStatus(o.Get("status")); break;
                case "activity": MarkActivity(o.Bool("important")); break;
                case "notify": Core.Send("notify", Json.Args(("title", o.Str("title")), ("body", o.Str("body")))); break;
                case "sound": PlaySound(o.Str("path")); break;
                case "open_uri": OpenUri(o.Str("uri")); break;
                case "set_input": SetInput?.Invoke(o.Str("text")); break;
                case "clear": Output.Clear(); break;
                case "logging":
                    Logging = !o.IsNull("path");
                    Changed?.Invoke();
                    break;
                case "speak":
                    if (Core.Send("speech.say", Json.Args(("text", o.Str("text")))).OptStr("problem") is { } problem) Output.Notice(problem);
                    break;
                case "speech_stop": Core.Send("speech.stop"); break;
                case "media": sounds.Run(o.Get("command"), Output.Notice); break;
                case "status_text":
                    StatusText = o.Str("text");
                    Changed?.Invoke();
                    break;
                case "config_changed": AppState.ReloadFromEngine(); break;
                case "open_puppet":
                    if (!Puppets.ContainsKey(o.Str("name"))) Puppets[o.Str("name")] = new OutputBuffer(AppState.Settings.Int("scrollback_lines"));
                    ToView(o);
                    break;
                case "puppet_line":
                    if (Puppets.TryGetValue(o.Str("name"), out var puppet)) puppet.AppendLine(o.Get("line"));
                    break;
                case "puppet_echo":
                    if (Puppets.TryGetValue(o.Str("name"), out var echoed)) echoed.Echo(o.Str("text"));
                    break;
                case "compile_error":
                    if (LastEdit is { Error: null } last) LastEdit = (last.Request, (o.Int("line"), o.Str("message")));
                    ToView(o);
                    break;
                case "tilemap" or "map_changed" or "show_map" or "stats" or "edit" or "webview"
                    or "agent_start" or "agent_output" or "agent_stop" or "agent_focus":
                    ToView(o);
                    break;
                case "gmcp" or "received" or "sent":
                    if (HasPages) ToView(o);
                    break;
                case "line_avatar": Output.SetNextAvatar(o.Get("avatar")); break;
                case "avatar_updated": Output.UpdateAvatar(o.Get("avatar")); break;
                case "inline_image": Output.AppendImage(o.Str("url")); break;
                default:
                    System.Diagnostics.Debug.WriteLine($"Familiar: an output this app doesn't know: {o?.ToJsonString()}");
                    break;
            }
        }
    }

    private void SetStatus(JsonNode? status)
    {
        switch (status.Str("state"))
        {
            case "connecting":
                State = ConnectionState.Connecting;
                Dropped = false;
                break;
            case "connected":
                State = ConnectionState.Connected;
                Dropped = false;
                break;
            default:
                State = ConnectionState.Disconnected;
                Dropped = true;
                DropReason = status.OptStr("reason");
                SaveMap();
                break;
        }
        Changed?.Invoke();
    }

    private void MarkActivity(bool important)
    {
        // The tab that's showing doesn't need marking.
        if (IsShowing?.Invoke() == true) return;
        Unread = true;
        if (important) Important = true;
        Changed?.Invoke();
    }

    public static async void OpenUri(string uri)
    {
        try
        {
            if (Uri.TryCreate(uri, UriKind.Absolute, out var parsed)
                && parsed.Scheme is "http" or "https" or "mailto")
            {
                await Windows.System.Launcher.LaunchUriAsync(parsed);
            }
        }
        catch (Exception e)
        {
            System.Diagnostics.Debug.WriteLine($"Familiar: opening {uri}: {e.Message}");
        }
    }

    private void PlaySound(string path)
    {
        if (!File.Exists(path))
        {
            Output.Notice($"Sound file not found: {path}");
            return;
        }
        sounds.Play(Path.GetFileName(path), path, 1, 1, Output.Notice);
    }
}
