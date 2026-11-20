# Drag files or text onto a window through real OLE drag and drop.
#
#   powershell -File ole-drop.ps1 -Title "Stipple drop demo" -Files C:\a.txt
#   powershell -File ole-drop.ps1 -Title "Stipple drop demo" -Text "hello"
#
# Runs DoDragDrop with our own IDropSource: it presses the left button on a
# small window of its own beside the target, then moves the mouse with
# synthesized input onto the target's client point (-X, -Y), so the target
# sees DragEnter and DragOver, and lets go there: the drop. The target's
# IDropTarget receives the drop exactly as from Explorer. Prints the effect
# the target chose; exits 1 when it refused (effect none) or on error.
# Windows PowerShell (5.1) runs scripts in an STA, which OLE needs.
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [string[]]$Files,
    [string]$Text,
    [int]$X = 100,
    [int]$Y = 150
)

$ErrorActionPreference = 'Stop'

Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using ComIDataObject = System.Runtime.InteropServices.ComTypes.IDataObject;

[ComImport, Guid("00000121-0000-0000-C000-000000000046"),
 InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IOleDropSource {
    [PreserveSig] int QueryContinueDrag(int fEscapePressed, uint grfKeyState);
    [PreserveSig] int GiveFeedback(uint dwEffect);
}

public class OleDrop : IOleDropSource {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }

    [DllImport("ole32.dll")] static extern int OleInitialize(IntPtr reserved);
    [DllImport("ole32.dll")] static extern int DoDragDrop(ComIDataObject data,
        IOleDropSource source, uint okEffects, out uint effect);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr hwnd, ref POINT p);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr hwnd, IntPtr after,
        int x, int y, int cx, int cy, uint flags);
    const uint SWP_NOSIZE = 1, SWP_NOMOVE = 2, SWP_NOACTIVATE = 0x10;
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr GetPropW(IntPtr hwnd, string name);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int GetClassNameW(IntPtr hwnd, System.Text.StringBuilder name, int max);
    [DllImport("user32.dll")] static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
    const uint MOUSEEVENTF_MOVE = 1, MOUSEEVENTF_LEFTDOWN = 2, MOUSEEVENTF_LEFTUP = 4,
        MOUSEEVENTF_ABSOLUTE = 0x8000;

    int calls;
    public uint LastFeedback;

    // Called on every pass of DoDragDrop's loop (on mouse and key
    // messages): keep dragging until the button is up, then drop.
    public int QueryContinueDrag(int esc, uint keys) {
        calls++;
        if (esc != 0) return 0x00040101; // DRAGDROP_S_CANCEL
        if ((keys & 1) == 0) return 0x00040100; // DRAGDROP_S_DROP (MK_LBUTTON clear)
        return 0; // S_OK: keep dragging
    }

    public int GiveFeedback(uint effect) {
        LastFeedback = effect;
        return 0x00040102; // DRAGDROP_S_USEDEFAULTCURSORS
    }

    // Move the pointer to a screen point with real (absolute) input, which
    // posts the mouse messages DoDragDrop's loop runs on.
    static void MoveTo(int x, int y) {
        int w = GetSystemMetrics(0), h = GetSystemMetrics(1); // SM_CXSCREEN, SM_CYSCREEN
        mouse_event(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
            (x * 65535 + (w - 1) / 2) / (w - 1), (y * 65535 + (h - 1) / 2) / (h - 1), 0, UIntPtr.Zero);
    }

    // What OLE finds at `p`: the window, whether it is (in) the target, and
    // whether that registered an IDropTarget.
    static string Under(POINT p, IntPtr target) {
        IntPtr under = WindowFromPoint(p);
        IntPtr top = GetAncestor(under, 2); // GA_ROOT
        var cls = new System.Text.StringBuilder(128);
        GetClassNameW(top, cls, cls.Capacity);
        return "under the drop point: 0x" + top.ToString("x") + " class " + cls
            + (top == target ? " (the target)" : " (NOT the target 0x" + target.ToString("x") + ")")
            + ", OleDropTargetInterface " + (GetPropW(top, "OleDropTargetInterface") != IntPtr.Zero ? "set" : "unset");
    }

    public static int Run(string title, string[] files, string text, int x, int y) {
        SetProcessDPIAware();
        int hr = OleInitialize(IntPtr.Zero);
        Console.WriteLine("ole-drop: OleInitialize 0x" + hr.ToString("x"));
        IntPtr hwnd = FindWindow(null, title);
        if (hwnd == IntPtr.Zero) { Console.WriteLine("ole-drop: window not found"); return 2; }
        var p = new POINT { X = x, Y = y };
        ClientToScreen(hwnd, ref p);
        // Nothing may cover the drop point (on the runner a console window
        // can sit over the target): keep the target on top for the drag.
        SetWindowPos(hwnd, new IntPtr(-1), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); // HWND_TOPMOST
        SetForegroundWindow(hwnd);
        System.Threading.Thread.Sleep(200);
        Console.WriteLine("ole-drop: " + Under(p, hwnd));
        // The drag starts on a window of our own, as a user's drag starts in
        // the source app: the press goes to our thread, and DoDragDrop's
        // capture then takes the mouse. A press over the target instead goes
        // to the target, which captures the mouse for itself (as any app does
        // on a press), and DoDragDrop's loop never sees a mouse message.
        var form = new Form {
            FormBorderStyle = FormBorderStyle.None, ShowInTaskbar = false, TopMost = true,
            StartPosition = FormStartPosition.Manual, BackColor = System.Drawing.Color.SteelBlue,
            Bounds = new System.Drawing.Rectangle(Math.Max(0, p.X - 160), Math.Max(0, p.Y - 30), 60, 60),
        };
        form.Show();
        Application.DoEvents();
        var start = new POINT { X = form.Left + 30, Y = form.Top + 30 };
        SetForegroundWindow(form.Handle);
        SetCursorPos(start.X, start.Y);
        MoveTo(start.X, start.Y);
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, UIntPtr.Zero);
        for (int i = 0; i < 10; i++) { System.Threading.Thread.Sleep(20); Application.DoEvents(); }
        var src = new OleDrop();
        // From another thread, once DoDragDrop runs: move onto the target a
        // step at a time, wiggle there (DragOver), then let go (the drop).
        var mover = new System.Threading.Thread(() => {
            System.Threading.Thread.Sleep(300);
            const int steps = 16;
            for (int i = 1; i <= steps; i++) {
                MoveTo(start.X + (p.X - start.X) * i / steps, start.Y + (p.Y - start.Y) * i / steps);
                System.Threading.Thread.Sleep(50);
            }
            for (int i = 0; i < 8; i++) {
                MoveTo(p.X + (i & 1), p.Y);
                System.Threading.Thread.Sleep(50);
            }
            Console.WriteLine("ole-drop: at the drop: " + Under(p, hwnd) + ", feedback so far " + src.LastFeedback);
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, UIntPtr.Zero);
        });
        mover.IsBackground = true;
        mover.Start();
        // Report progress while DoDragDrop runs, to tell a loop that never
        // turns (no QueryContinueDrag calls) from a target that never answers.
        var watch = new System.Threading.Thread(() => {
            for (int i = 1; ; i++) {
                System.Threading.Thread.Sleep(1000);
                POINT c;
                GetCursorPos(out c);
                Console.WriteLine("ole-drop: " + i + " s: " + src.calls + " QueryContinueDrag calls, feedback "
                    + src.LastFeedback + ", cursor " + c.X + "," + c.Y);
            }
        });
        watch.IsBackground = true;
        watch.Start();
        DataObject data = files != null && files.Length > 0
            ? new DataObject(DataFormats.FileDrop, files)
            : new DataObject(DataFormats.UnicodeText, text);
        uint effect;
        Console.WriteLine("ole-drop: dragging from " + start.X + "," + start.Y + " to " + p.X + "," + p.Y);
        hr = DoDragDrop(data, src, 1 | 2 | 4, out effect);
        Console.WriteLine("ole-drop: DoDragDrop 0x" + hr.ToString("x") + " effect " + effect
            + " (last feedback " + src.LastFeedback + ", " + src.calls + " passes)");
        form.Close();
        SetWindowPos(hwnd, new IntPtr(-2), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); // HWND_NOTOPMOST
        // DRAGDROP_S_DROP with an effect other than none: the target took it.
        return hr == 0x00040100 && effect != 0 ? 0 : 1;
    }
}
'@

$code = [OleDrop]::Run($Title, $Files, $Text, $X, $Y)
exit $code
