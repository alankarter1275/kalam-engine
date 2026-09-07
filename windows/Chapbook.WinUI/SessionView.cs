using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media.Imaging;
using System.Runtime.InteropServices;
using Windows.System;
using WinRT;

namespace Chapbook.WinUI;

/// <summary>
/// A page of a book, drawn and driven.
/// </summary>
/// <remarks>
/// <para>
/// The host owns the <see cref="Chapbook.Session"/> and its lifetime; this
/// view borrows it. That is deliberate — a session is not shareable, and a
/// control that owned one would make its lifetime a property of the visual
/// tree, which is the wrong place for a file handle and a database
/// connection to live.
/// </para>
/// <para>
/// What it does own is the loop: metrics on every resize and scale change,
/// a repaint when something moved, and the translation of presses and keys
/// into the engine's own <see cref="ReaderAction"/> vocabulary. What it
/// deliberately does not own is chrome — <see cref="ReaderAction.ToggleMenu"/>
/// comes back as <see cref="MenuRequested"/> for the app to answer, because
/// a reader's menu is the app's and the engine has none.
/// </para>
/// </remarks>
public sealed class SessionView : UserControl
{
    private readonly Image _page = new()
    {
        Stretch = Microsoft.UI.Xaml.Media.Stretch.Fill,
    };

    private WriteableBitmap? _bitmap;
    private Session? _session;

    public SessionView()
    {
        Content = _page;
        IsTabStop = true;
        UseSystemFocusVisuals = true;

        SizeChanged += (_, _) => Invalidate();
        KeyDown += OnKeyDown;
        PointerPressed += OnPointerPressed;
        Loaded += (_, _) =>
        {
            if (XamlRoot is { } root)
            {
                root.Changed += (_, _) => Invalidate();
            }
            Invalidate();
        };
    }

    /// <summary>
    /// The book being read. Setting it redraws; setting it to <c>null</c>
    /// blanks the view.
    /// </summary>
    /// <remarks>
    /// The view neither disposes the old session nor takes ownership of the
    /// new one.
    /// </remarks>
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
    /// <remarks>
    /// The engine answers that action with
    /// <see cref="ActionOutcome.Unhandled"/> always, because a reader's
    /// chrome belongs to the app. This is where it arrives instead.
    /// </remarks>
    public event EventHandler? MenuRequested;

    /// <summary>The page was redrawn — a turn, a relayout, a theme change.</summary>
    /// <remarks>
    /// A host showing "page 3 of 12" updates from here, and so does anything
    /// that needs to know the accessibility tree has moved.
    /// </remarks>
    public event EventHandler? PageChanged;

    /// <summary>
    /// Device pixels per logical pixel, as this view is being shown.
    /// </summary>
    /// <remarks>
    /// Named for what it is rather than `Scale`, which `UIElement` already
    /// has and means something entirely different by — a render transform,
    /// not a rasterization ratio.
    /// </remarks>
    private double RasterScale => XamlRoot?.RasterizationScale ?? 1.0;

    /// <summary>How much white space to leave around the text, in logical pixels.</summary>
    public double Margin_ { get; set; } = 40;

    /// <summary>Redraw, laying the page out again if the box changed.</summary>
    public void Invalidate()
    {
        if (_session is null || ActualWidth <= 0 || ActualHeight <= 0)
        {
            _page.Source = null;
            return;
        }

        double scale = RasterScale;
        _session.SetMetrics(new PageMetrics(
            (float)ActualWidth, (float)ActualHeight, (float)Margin_, (float)scale));

        (uint width, uint height) = _session.RenderSize();
        if (width == 0 || height == 0)
        {
            return;
        }

        if (_bitmap is null || _bitmap.PixelWidth != width || _bitmap.PixelHeight != height)
        {
            _bitmap = new WriteableBitmap((int)width, (int)height);
            _page.Source = _bitmap;
        }

        // Straight into the bitmap's own buffer: the engine fills it, the
        // swizzle runs over it in place, and nothing is copied. Going via
        // a managed array and a stream would cost two copies of a page per
        // draw for no benefit — the buffer is exactly the right size and
        // exactly the right lifetime.
        unsafe
        {
            _bitmap.PixelBuffer.As<IBufferByteAccess>().Buffer(out byte* buffer);
            var pixels = new Span<byte>(buffer, (int)width * (int)height * 4);
            _session.RenderInto(pixels, width, height, (long)width * 4);

            // Premultiplied RGBA out of the engine; a `WriteableBitmap`'s
            // back buffer is premultiplied *BGRA*. Swapping the two outer
            // bytes is the whole conversion — and getting it wrong is not
            // an error, it is a page that reads cold blue where the sepia
            // theme should be warm paper.
            Swizzle(pixels);
        }
        _bitmap.Invalidate();

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
    /// Whether the event was consumed. The two answers are not the same
    /// bit: the last page of a book changes nothing and is still taken.
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

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (_session is null)
        {
            return;
        }
        // The engine's table, not this control's. A key it does not bind
        // falls through to whatever the app put behind the view — which is
        // why `Handled` is set from the outcome rather than unconditionally.
        Key? key = Translate(e.Key);
        if (key is null)
        {
            return;
        }
        e.Handled = Apply(Engine.DefaultAction(key.Value));
    }

    /// <summary>
    /// A WinUI virtual key in the engine's vocabulary, or <c>null</c> for
    /// one no reader binds.
    /// </summary>
    /// <remarks>
    /// Characters are not here. They come from
    /// <see cref="CharacterReceived"/> in a host that wants them, because a
    /// character is what the keyboard layout produced and a virtual key is
    /// only which key was struck — mapping one back to the other by hand is
    /// how a reader ends up with bindings that work on one layout.
    /// </remarks>
    private static Key? Translate(VirtualKey key) => key switch
    {
        VirtualKey.Left => Chapbook.Key.ArrowLeft,
        VirtualKey.Right => Chapbook.Key.ArrowRight,
        VirtualKey.Up => Chapbook.Key.ArrowUp,
        VirtualKey.Down => Chapbook.Key.ArrowDown,
        VirtualKey.PageUp => Chapbook.Key.PageUp,
        VirtualKey.PageDown => Chapbook.Key.PageDown,
        VirtualKey.Space => Chapbook.Key.Space,
        VirtualKey.Back => Chapbook.Key.Backspace,
        // The browser-back key, and where a five-button mouse's thumb
        // button arrives when the shell has not claimed it first.
        VirtualKey.GoBack => Chapbook.Key.Backspace,
        _ => null,
    };

    /// <summary>
    /// Feed a typed character to the engine's table — <c>t</c> for the
    /// theme, <c>n</c> and <c>p</c> for chapters, <c>+</c> and <c>-</c> for
    /// the size.
    /// </summary>
    /// <remarks>
    /// A host calls this from its own <c>CharacterReceived</c>, if it wants
    /// those bindings. It is not wired up here because a reading app
    /// usually has buttons for all of it and would rather keep the letters.
    /// </remarks>
    public bool ApplyCharacter(char typed) =>
        Apply(Engine.DefaultAction(char.ToLowerInvariant(typed)));

    private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        if (_session is null)
        {
            return;
        }
        Focus(FocusState.Pointer);

        // Logical units, which is what the metrics were given. Passing raw
        // device pixels is the silent failure here: on a 200% display every
        // press lands in the far band and always means "next page".
        Windows.Foundation.Point at = e.GetCurrentPoint(this).Position;
        if (_session.TapAction((float)at.X, (float)at.Y) is { } action)
        {
            e.Handled = Apply(action);
        }
    }

    /// <summary>
    /// Hand the page's text to Narrator.
    /// </summary>
    /// <remarks>
    /// A rasterized page is a picture, and a picture of text is unusable
    /// with a screen reader.
    /// </remarks>
    protected override AutomationPeer OnCreateAutomationPeer() => new PagePeer(this);

    internal Session? Reading => _session;
}

/// <summary>
/// Direct access to a WinRT buffer's bytes.
/// </summary>
/// <remarks>
/// The documented way to reach a <see cref="WriteableBitmap"/>'s back
/// buffer without copying. `IBuffer` has no managed indexer worth using
/// and CsWinRT offers no `AsStream` for it, so this is the one COM
/// declaration this assembly needs — everything else it touches, WinUI
/// projects for it.
/// </remarks>
[ComImport]
[Guid("905a0fef-bc53-11df-8c49-001e4fc686da")]
[InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IBufferByteAccess
{
    unsafe void Buffer(out byte* value);
}
