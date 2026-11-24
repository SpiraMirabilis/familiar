using System.Text.Json.Nodes;
using Microsoft.UI.Dispatching;

namespace Familiar;

/// <summary>One session's sounds. The engine's bridge plays them (it has the decoders); this
/// keeps track of which are this session's.</summary>
internal sealed class SoundPlayer(long session)
{
    private readonly List<(string Name, string Key)> playing = [];
    /// <summary>Bumped by every stop, so a sound still downloading when it arrives doesn't start.</summary>
    private int stops;
    private int next;

    private bool IsPlaying(string name)
    {
        playing.RemoveAll(sound => Core.Send("sound.playing", Json.Args(("name", sound.Key)))?.GetValue<bool>() != true);
        return playing.Any(sound => sound.Name == name);
    }

    public void Play(string name, string path, double volume, int loops, Action<string> problem)
    {
        // Each sound gets a key of its own, so stopping one never stops another of the same name.
        var key = $"{session}:{name}#{next++}";
        try
        {
            Core.Call("sound.play", Json.Args(("name", key), ("path", path), ("volume", Math.Clamp(volume, 0, 1)), ("loops", loops)));
            playing.Add((name, key));
        }
        catch (CoreException e)
        {
            problem(e.Message);
        }
    }

    public void Stop(string? name)
    {
        stops++;
        foreach (var sound in playing.Where(s => name is null || s.Name == name).ToList())
        {
            Core.Send("sound.stop", Json.Args(("name", sound.Key)));
            playing.Remove(sound);
        }
    }

    /// <summary>Runs a Client.Media command from the engine. Downloads happen off the UI thread.</summary>
    public void Run(JsonNode? command, Action<string> problem)
    {
        var url = command.Str("url");
        switch (command.Str("op"))
        {
            case "stop":
                Stop(command.OptStr("name"));
                break;
            case "preload":
                Task.Run(() =>
                {
                    try
                    {
                        Core.CallAnywhere("media.fetch", Json.Args(("url", url)));
                    }
                    catch (CoreException)
                    {
                        // It will be reported when the sound is played.
                    }
                });
                break;
            case "play":
                var name = command.Str("name");
                if (IsPlaying(name))
                {
                    if (!command.Bool("restart")) return;
                    Stop(name);
                }
                var stopsBefore = stops;
                var volume = command.OptNum("volume") ?? 1;
                var loops = (int)(command.OptNum("loops") ?? 1);
                var ui = DispatcherQueue.GetForCurrentThread();
                Task.Run(() =>
                {
                    string? path = null, error = null;
                    try
                    {
                        path = Core.CallAnywhere("media.fetch", Json.Args(("url", url))).Str("path");
                    }
                    catch (CoreException e)
                    {
                        error = e.Message;
                    }
                    ui.TryEnqueue(() =>
                    {
                        if (stops != stopsBefore) return;
                        if (path is not null) Play(name, path, volume, loops, problem);
                        else problem($"Couldn’t get the sound: {error}");
                    });
                });
                break;
        }
    }
}
