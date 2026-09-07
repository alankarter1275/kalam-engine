using System.Windows;
using System.Windows.Automation.Peers;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;

// `Key` is ambiguous here in a way the compiler resolves silently and
// wrongly: this namespace is nested inside `Chapbook`, so a bare `Key`
// binds to the engine's rather than to WPF's, and a switch over one
// against patterns from the other fails a dozen lines later with an error
// that names neither problem. Both get an alias so neither is bare.
using EngineKey = Chapbook.Key;
using WpfKey = System.Windows.Input.Key;

namespace Chapbook.Wpf;

/// <summary>
/// A page of a book, drawn and driven.
/// </summary>
/// <remarks>
/// <para>
/// The host owns the <see cref="Chapbook.Session"/> and its lifetime; this
/// view borrows it. A session's lifetime is a file handle and a database
/// connection, and neither belongs to a visual tree.
/// </para>
/// <para>
/// What it owns is the loop: metrics on every resize and DPI change, a
/// repaint when something moved, and presses and keys translated into the
/// engine's own <see cref="ReaderAction"/> vocabulary. What it does not own
/// is chrome — <see cref="ReaderAction.ToggleMenu"/> comes back as
/// <see cref="MenuRequested"/>, because the engine answers that action
/// <see cref="ActionOutcome.Unhandled"/> always and a reader's menu is the
/// app's.
/// </para>
/// </remarks>
public sealed class SessionView : FrameworkElement
{
    private WriteableBitmap? _bitmap;
    private Session? _session;

    public SessionView()
    {
        Focusable = true;
        FocusVisualStyle = null;
    }

    /// <summary>The book being read. Setting it redraws.</summary>
    public Session? Session
    {
        get => _session;
        set
        {
            _session = value;
            Invalidate();
        }
    }

    /// <summary>
    /// The middle band was pressed, or a key bound to
    /// <see cref="ReaderAction.ToggleMenu"/> was struck.
    /// </summary>
    public event EventHandler? MenuRequested;

    /// <summary>The page was redrawn — a turn, a relayout, a theme change.</summary>
    public event EventHandler? PageChanged;

    /// <summary>White space around the text, in device-independent pixels.</summary>
    public double PageMargin { get; set; } = 40;

    /// <summary>Device pixels per DIP, from the monitor this view is on.</summary>
    private double RasterScale => VisualTreeHelper.GetDpi(this).DpiScaleX;

    /// <summary>Redraw, laying the page out again if the box changed.</summary>
    public void Invalidate()
    {
        _bitmap = null;
        InvalidateVisual();
    }

    protected override void OnRenderSizeChanged(SizeChangedInfo info)
    {
        base.OnRenderSizeChanged(info);
        Invalidate();
    }

    protected override void OnDpiChanged(DpiScale before, DpiScale now)
    {
        base.OnDpiChanged(before, now);
        Invalidate();
    }

    protected override void OnRender(DrawingContext context)
    {
        base.OnRender(context);
        if (_session is null || ActualWidth <= 0 || ActualHeight <= 0)
        {
            return;
        }

        double scale = RasterScale;
        _session.SetMetrics(new PageMetrics(
            (float)ActualWidth, (float)ActualHeight, (float)PageMargin, (float)scale));

        (uint width, uint height) = _session.RenderSize();
        if (width == 0 || height == 0)
        {
            return;
        }

        if (_bitmap is null || _bitmap.PixelWidth != width || _bitmap.PixelHeight != height)
        {
            // Premultiplied BGRA at the monitor's own resolution, so the
            // bitmap is device pixels and WPF composites it 1:1 into a
            // DIP-sized box.
            _bitmap = new WriteableBitmap(
                (int)width, (int)height, 96 * scale, 96 * scale, PixelFormats.Pbgra32, null);
        }

        // Straight into the bitmap's back buffer: the engine fills it and
        // the swizzle runs over it in place, so a page turn copies nothing.
        // WPF hands the pointer over directly — no COM interop, which is
        // the one place this view is simpler than its WinUI sibling.
        _bitmap.Lock();
        try
        {
            unsafe
            {
                var pixels = new Span<byte>(
                    (void*)_bitmap.BackBuffer, _bitmap.BackBufferStride * (int)height);
                _session.RenderInto(pixels, width, height, _bitmap.BackBufferStride);

                // Premultiplied RGBA out of the engine, premultiplied BGRA
                // into the buffer. Swapping the two outer bytes is the whole
                // conversion, and getting it wrong is not an error — it is a
                // page that reads cold blue where the sepia theme should be
                // warm paper.
                Swizzle(pixels);
            }
            _bitmap.AddDirtyRect(new Int32Rect(0, 0, (int)width, (int)height));
        }
        finally
        {
            _bitmap.Unlock();
        }

        context.DrawImage(_bitmap, new Rect(0, 0, ActualWidth, ActualHeight));
        PageChanged?.Invoke(this, EventArgs.Empty);
    }

    private static void Swizzle(Span<byte> pixels)
    {
        for (int i = 0; i + 3 < pixels.Length; i += 4)
        {
            (pixels[i], pixels[i + 2]) = (pixels[i + 2], pixels[i]);
        }
    }

    /// <summary>
    /// Apply an action and repaint if it moved anything.
    /// </summary>
    /// <returns>
    /// Whether the event was consumed. Not the same bit as "repaint": the
    /// last page of a book changes nothing and is still taken.
    /// </returns>
    public bool Apply(ReaderAction action)
    {
        if (_session is null || action == ReaderAction.None)
        {
            return false;
        }
        ActionOutcome outcome = _session.Apply(action);
        if (outcome == ActionOutcome.Changed)
        {
            Invalidate();
        }
        if (outcome == ActionOutcome.Unhandled && action == ReaderAction.ToggleMenu)
        {
            MenuRequested?.Invoke(this, EventArgs.Empty);
            return true;
        }
        return outcome != ActionOutcome.Unhandled;
    }

    /// <summary>
    /// Feed a typed character to the engine's table — <c>t</c> for the
    /// theme, <c>n</c> and <c>p</c> for chapters, <c>+</c> and <c>-</c> for
    /// the size.
    /// </summary>
    public bool ApplyCharacter(char typed) =>
        Apply(Engine.DefaultAction(char.ToLowerInvariant(typed)));

    protected override void OnKeyDown(KeyEventArgs e)
    {
        base.OnKeyDown(e);
        if (_session is null || Translate(e.Key) is not { } key)
        {
            return;
        }
        // The engine's table, not this view's. A key it does not bind falls
        // through, which is why `Handled` comes from the outcome rather than
        // being set unconditionally.
        e.Handled = Apply(Engine.DefaultAction(key));
    }

    protected override void OnTextInput(TextCompositionEventArgs e)
    {
        base.OnTextInput(e);
        if (_session is null || e.Text.Length != 1)
        {
            return;
        }
        // Characters come from the layout, which is why they arrive here
        // and not through a virtual key mapped back to a letter by hand.
        e.Handled = ApplyCharacter(e.Text[0]);
    }

    /// <summary>
    /// A WPF key in the engine's vocabulary, or <c>null</c> for one no
    /// reader binds.
    /// </summary>
    private static EngineKey? Translate(WpfKey key) => key switch
    {
        WpfKey.Left => EngineKey.ArrowLeft,
        WpfKey.Right => EngineKey.ArrowRight,
        WpfKey.Up => EngineKey.ArrowUp,
        WpfKey.Down => EngineKey.ArrowDown,
        WpfKey.PageUp => EngineKey.PageUp,
        WpfKey.PageDown => EngineKey.PageDown,
        WpfKey.Space => EngineKey.Space,
        WpfKey.Back => EngineKey.Backspace,
        WpfKey.BrowserBack => EngineKey.Backspace,
        _ => null,
    };

    protected override void OnMouseDown(MouseButtonEventArgs e)
    {
        base.OnMouseDown(e);
        if (_session is null)
        {
            return;
        }
        Focus();

        // Device-independent pixels, which is the space the metrics were
        // given. WPF hands them over already; a framework that reported
        // device pixels would land every press in the far band on a 200%
        // display and always mean "next page".
        Point at = e.GetPosition(this);

        // The two thumb buttons, which is the closest a desktop comes to a
        // Kobo's bezel — and a binding the engine has carried since it was
        // written.
        EngineKey? bezel = e.ChangedButton switch
        {
            MouseButton.XButton1 => EngineKey.TurnPrev,
            MouseButton.XButton2 => EngineKey.TurnNext,
            _ => null,
        };
        if (bezel is { } button)
        {
            e.Handled = Apply(Engine.DefaultAction(button));
            return;
        }

        if (e.ChangedButton == MouseButton.Left
            && _session.TapAction((float)at.X, (float)at.Y) is { } action)
        {
            e.Handled = Apply(action);
        }
    }

    /// <summary>
    /// Hand the page's text to Narrator. A rasterized page is a picture,
    /// and a picture of text is unusable with a screen reader.
    /// </summary>
    protected override AutomationPeer OnCreateAutomationPeer() => new PagePeer(this);

    internal Session? Reading => _session;

    internal PageText? Page => _session is null ? null : PageText.Of(_session);
}
