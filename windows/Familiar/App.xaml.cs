using Microsoft.UI.Xaml;

namespace Familiar;

public partial class App : Application
{
    private MainWindow? window;

    internal static MainWindow? Window { get; private set; }
    internal static Microsoft.UI.Dispatching.DispatcherQueue? Queue { get; private set; }

    public App()
    {
        InitializeComponent();
#if DEBUG
        // A debug build says why it died, since there's no console to say it on.
        UnhandledException += (_, e) => File.AppendAllText(Path.Combine(Path.GetTempPath(), "familiar-crash.txt"), $"{DateTime.Now}: {e.Exception}\n\n");
#endif
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var problem = AppState.Launch();
        Queue = Microsoft.UI.Dispatching.DispatcherQueue.GetForCurrentThread();
        Window = window = new MainWindow();
        window.Activate();
        window.WhenReady(async () =>
        {
            if (problem is { } p) await window.Alert(p.Title, p.Detail);
#if DEBUG
            if (RunDebugScript()) return;
#endif
            await window.OpenNamed(Environment.GetCommandLineArgs().Skip(1).ToArray());
        });
    }

#if DEBUG
    /// <summary>For checking the app without a person at it (debug builds only): connects to
    /// FAMILIAR_TEST_CONNECT ("host:port"), types FAMILIAR_TEST_INPUT (one command per line),
    /// then writes the output's text into the folder FAMILIAR_TEST_SNAPSHOT.</summary>
    private bool RunDebugScript()
    {
        var folder = Environment.GetEnvironmentVariable("FAMILIAR_TEST_SNAPSHOT");
        if (folder is null || window is null) return false;
        if (Environment.GetEnvironmentVariable("FAMILIAR_TEST_CONNECT") is { } address && Core.CallAnywhere("address.parse", Json.Args(("text", address))) is { } world)
        {
            // After the Connect tab is on screen, as when a person chooses a world there.
            var timer = window.DispatcherQueue.CreateTimer();
            timer.Interval = TimeSpan.FromMilliseconds(900);
            timer.IsRepeating = false;
            timer.Tick += (_, _) => window.OpenInPlace(world, null);
            timer.Start();
        }
        if (Environment.GetEnvironmentVariable("FAMILIAR_TEST_SETTINGS") is not null) window.ShowSettings();
        var rules = Environment.GetEnvironmentVariable("FAMILIAR_TEST_RULES") is not null ? window.ShowRules() : null;
        // FAMILIAR_TEST_EDIT opens an editor: "trigger" (with FAMILIAR_TEST_RULES) or "world".
        var edit = Environment.GetEnvironmentVariable("FAMILIAR_TEST_EDIT");
        window.DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, async () =>
        {
            await Task.Delay(3500);
            if (edit == "trigger") rules?.Edit(0);
            if (edit == "examples") rules?.AddExamples();
            if (edit == "code" && window.CurrentView is { } shown)
            {
                var code = "\"Shows what is here.\";\nif (this.dark && !player.wizard)\n  player:tell(\"It is too dark to see.\");\n  return E_PERM;\nendif\nfor thing in (this.contents)\n  /* skip the unseen */\n  if (thing != player && is_player(thing))\n    player:tell($string_utils:pronoun_sub(\"%N is here.\", thing), \" \", #123);\n  endif\nendfor\n";
                new CodeEditorWindow(shown.Session, Json.Args(("reference", "test"), ("name", "#62:look_self"), ("kind", "local"), ("content", code), ("language", "moo"))).Activate();
            }
            if (edit == "world" && AppState.Worlds.FirstOrDefault() is { } first) await WorldDialog.Edit(window.Content.XamlRoot, first, false);
        });
        var commands = (Environment.GetEnvironmentVariable("FAMILIAR_TEST_INPUT") ?? "").Split('\n', StringSplitOptions.RemoveEmptyEntries);
        var queue = window.DispatcherQueue;
        _ = Task.Run(async () =>
        {
            await Task.Delay(2500);
            foreach (var command in commands)
            {
                queue.TryEnqueue(() => window.CurrentView?.Session.Submit(command));
                await Task.Delay(400);
            }
            // FAMILIAR_TEST_FOCUS_OUTPUT: focus goes to the output, and whoever runs the test types.
            if (Environment.GetEnvironmentVariable("FAMILIAR_TEST_FOCUS_OUTPUT") is not null)
            {
                // "0" only waits, to check that the typing arrives at all.
                if (Environment.GetEnvironmentVariable("FAMILIAR_TEST_FOCUS_OUTPUT") != "0") queue.TryEnqueue(() => window.CurrentView?.Output.Focus(FocusState.Programmatic));
                await Task.Delay(3000);
            }
            await Task.Delay(1000);
            queue.TryEnqueue(() =>
            {
                Directory.CreateDirectory(folder);
                var session = window.CurrentView?.Session;
                File.WriteAllText(Path.Combine(folder, "output.txt"), $"title: {session?.Title}\nstate: {session?.State}\nview: {window.CurrentView?.Output.Debug}\nmap: {window.CurrentView?.MapDebug}\ninput: [{window.CurrentView?.Input.Text}] focused: {Microsoft.UI.Xaml.Input.FocusManager.GetFocusedElement(window.Content.XamlRoot)?.GetType().Name} inputHasFocus: {window.CurrentView?.Input.HasFocus} placeholder: {(Microsoft.UI.Xaml.Input.FocusManager.GetFocusedElement(window.Content.XamlRoot) as Microsoft.UI.Xaml.Controls.TextBox)?.PlaceholderText}\n---\n{session?.Output.Text}\n");
            });
        });
        return true;
    }
#endif
}
