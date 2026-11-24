using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace Familiar;

public static partial class Program
{
    [LibraryImport("shell32", SetLastError = true)]
    private static partial nint CommandLineToArgvW([MarshalAs(UnmanagedType.LPWStr)] string commandLine, out int count);

    [LibraryImport("kernel32")]
    private static partial nint LocalFree(nint memory);

    /// <summary>Starting Familiar again brings the window that's already open to the front (and
    /// opens the world named on the command line there), as the other clients do.
    /// FAMILIAR_SEPARATE_INSTANCE turns that off.</summary>
    [STAThread]
    private static int Main(string[] args)
    {
        WinRT.ComWrappersSupport.InitializeComWrappers();
        // Web views keep their data with ours, not beside the program (which may be read-only).
        Environment.SetEnvironmentVariable("WEBVIEW2_USER_DATA_FOLDER", Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "familiar", "WebView2"));
        var separate = Environment.GetEnvironmentVariable("FAMILIAR_SEPARATE_INSTANCE") is not null || Environment.GetEnvironmentVariable("FAMILIAR_TEST_SNAPSHOT") is not null;
        var instance = AppInstance.FindOrRegisterForKey(separate ? $"familiar-{Environment.ProcessId}" : "familiar");
        if (!instance.IsCurrent)
        {
            var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
            Task.Run(async () => await instance.RedirectActivationToAsync(activation)).Wait(TimeSpan.FromSeconds(10));
            return 0;
        }
        instance.Activated += (_, e) =>
        {
            var arguments = e.Data is Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs launch ? Split(launch.Arguments).Skip(1).ToArray() : [];
            App.Queue?.TryEnqueue(() => App.Window?.Reopened(arguments));
        };
        Application.Start(p =>
        {
            SynchronizationContext.SetSynchronizationContext(new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread()));
            var app = new App();
        });
        return 0;
    }

    /// <summary>A command line as its arguments, the program's name first.</summary>
    private static string[] Split(string commandLine)
    {
        if (string.IsNullOrWhiteSpace(commandLine)) return [];
        var list = CommandLineToArgvW(commandLine, out var count);
        if (list == 0) return [];
        try
        {
            return Enumerable.Range(0, count).Select(i => Marshal.PtrToStringUni(Marshal.ReadIntPtr(list, i * nint.Size)) ?? "").ToArray();
        }
        finally
        {
            LocalFree(list);
        }
    }
}
