using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Chapbook;
using Chapbook.Wpf;

namespace Chapbook.Demo;

/// <summary>
/// A window, a book, and a status line.
/// </summary>
/// <remarks>
/// It exists to put the two things a compiler cannot judge in front of a
/// person: whether the page comes out the right colour, and whether a
/// screen reader can read it. Everything below the view is covered by the
/// tests next door; nothing there can open a window.
/// </remarks>
internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Length < 1)
        {
            Console.Error.WriteLine(
                "usage: ChapbookDemo <book> [library-dir] [--shot <png> [--theme <name>]]");
            return 2;
        }
        // `--shot` draws one page, writes it, and exits. It exists because
        // the two things this demo is *for* are the two a compiler cannot
        // judge, and one of them — whether the page comes out the right
        // colour — should not need a person at a screen every time. WPF
        // composites through DWM, which a screen grab of another process
        // does not reliably see, so the only honest capture is WPF
        // rendering its own visual tree.
        string? shot = Argument(args, "--shot");
        string? theme = Argument(args, "--theme");

        // The engine installs no log backend and is silent until a host
        // gives it somewhere to speak. Install it first: the reasons an
        // open failed are the ones most worth having.
        Engine.SetLogCallback(
            record => Console.Error.WriteLine($"DEMO [{record.Level}] {record.Target}: {record.Message}"),
            LogLevel.Warn);

        var configuration = new SessionConfiguration(FontSource.Host());
        if (args.Length > 1)
        {
            configuration.WithLibraryDirectory(args[1]);
        }

        Session session;
        try
        {
            session = Session.OpenPath(args[0], configuration);
        }
        catch (ChapbookException e)
        {
            Console.Error.WriteLine($"DEMO: {e.Message}");
            return 1;
        }

        if (theme is not null && Enum.TryParse(theme, ignoreCase: true, out Theme chosen))
        {
            session.SetSettings(session.Settings with { Theme = chosen }, SettingsScope.ThisBook);
        }

        var app = new Application();
        var view = new SessionView { Session = session };
        var status = new TextBlock
        {
            Margin = new Thickness(8, 4, 8, 4),
            FontSize = 12,
        };

        var layout = new DockPanel();
        DockPanel.SetDock(status, Dock.Bottom);
        layout.Children.Add(status);
        layout.Children.Add(view);

        var window = new Window
        {
            Title = session.Title ?? "chapbook",
            Width = 700,
            Height = 900,
            Content = layout,
        };

        void Refresh()
        {
            Position at = session.Position;
            string kind = session.Kind == BookKind.Epub ? "ch" : "pg";
            status.Text =
                $"{kind} {at.Spine + 1}/{session.SpineLength}  ·  " +
                $"p {at.Page + 1}/{Math.Max(1, session.PageCount)}  ·  " +
                $"{session.Settings.Theme}  ·  {session.FontFaceCount} faces";
        }

        view.PageChanged += (_, _) => Refresh();

        // The engine has no menu, so the middle band comes back here. A
        // real app opens its chrome; this one says the theme out loud,
        // which is the cheapest thing that proves the event arrives.
        view.MenuRequested += (_, _) => view.Apply(ReaderAction.CycleTheme);

        window.Loaded += (_, _) =>
        {
            view.Focus();
            view.Invalidate();
            if (shot is not null)
            {
                // After a layout pass, so the view has a size to draw into.
                window.Dispatcher.BeginInvoke(
                    System.Windows.Threading.DispatcherPriority.Loaded,
                    () =>
                    {
                        Capture(view, shot);
                        window.Close();
                    });
            }
        };

        // Leave a bookmark. Across this ABI `Suspend` is the only call that
        // persists a position — there is no `save_position` — and disposing
        // does not save.
        window.Closing += (_, _) =>
        {
            session.Suspend();
            session.Dispose();
        };

        return app.Run(window);
    }

    private static string? Argument(string[] args, string name)
    {
        int at = Array.IndexOf(args, name);
        return at >= 0 && at + 1 < args.Length ? args[at + 1] : null;
    }

    /// <summary>Ask WPF what it drew, and write it out.</summary>
    private static void Capture(Visual view, string path)
    {
        var element = (FrameworkElement)view;
        var target = new RenderTargetBitmap(
            (int)element.ActualWidth, (int)element.ActualHeight, 96, 96, PixelFormats.Pbgra32);
        target.Render(view);

        var png = new PngBitmapEncoder();
        png.Frames.Add(BitmapFrame.Create(target));
        using FileStream file = File.Create(path);
        png.Save(file);
        Console.Error.WriteLine($"DEMO: wrote {path}");
    }
}
