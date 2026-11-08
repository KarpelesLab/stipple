# Drag files or text onto a window through real OLE drag and drop.
#
#   powershell -File ole-drop.ps1 -Title "Stipple drop demo" -Files C:\a.txt
#   powershell -File ole-drop.ps1 -Title "Stipple drop demo" -Text "hello"
#
# Runs DoDragDrop with our own IDropSource: it puts the cursor over the
# window's client area at (-X, -Y), holds the left button and moves the mouse
# with synthesized input so the target sees DragEnter and DragOver, then drops. The target's
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

Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition @'
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
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
    const uint MOUSEEVENTF_MOVE = 1, MOUSEEVENTF_LEFTDOWN = 2, MOUSEEVENTF_LEFTUP = 4;

    int calls;
    public POINT origin;
    public uint LastFeedback;

    // Called on every pass of DoDragDrop's loop (on mouse and key
    // messages): move along, then drop.
    public int QueryContinueDrag(int esc, uint keys) {
        calls++;
        if (esc != 0) return 0x00040101; // DRAGDROP_S_CANCEL
        // Drop once the button is up (MK_LBUTTON clear) or after enough moves.
        if ((keys & 1) == 0 || calls >= 12) return 0x00040100; // DRAGDROP_S_DROP
        return 0; // S_OK: keep dragging
    }

    public int GiveFeedback(uint effect) {
        LastFeedback = effect;
        return 0x00040102; // DRAGDROP_S_USEDEFAULTCURSORS
    }

    public static int Run(string title, string[] files, string text, int x, int y) {
        SetProcessDPIAware();
        int hr = OleInitialize(IntPtr.Zero);
        Console.WriteLine("ole-drop: OleInitialize 0x" + hr.ToString("x"));
        IntPtr hwnd = FindWindow(null, title);
        if (hwnd == IntPtr.Zero) { Console.WriteLine("ole-drop: window not found"); return 2; }
        SetForegroundWindow(hwnd);
        var p = new POINT { X = x, Y = y };
        ClientToScreen(hwnd, ref p);
        var src = new OleDrop { origin = p };
        SetCursorPos(p.X, p.Y);
        // DoDragDrop runs on real input: hold the left button (as a user who
        // started a drag would), and from another thread move the mouse a
        // little at a time, then let go. Each mouse message turns its loop,
        // which asks QueryContinueDrag below; the button's release ends the
        // drag with a drop even if the source hasn't said so yet.
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, UIntPtr.Zero);
        var mover = new System.Threading.Thread(() => {
            for (int i = 0; i < 20; i++) {
                System.Threading.Thread.Sleep(60);
                mouse_event(MOUSEEVENTF_MOVE, (i & 1) == 0 ? 2 : -1, 0, 0, UIntPtr.Zero);
            }
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, UIntPtr.Zero);
        });
        mover.IsBackground = true;
        mover.Start();
        DataObject data = files != null && files.Length > 0
            ? new DataObject(DataFormats.FileDrop, files)
            : new DataObject(DataFormats.UnicodeText, text);
        uint effect;
        Console.WriteLine("ole-drop: dragging from " + p.X + "," + p.Y);
        hr = DoDragDrop(data, src, 1 | 2 | 4, out effect);
        Console.WriteLine("ole-drop: DoDragDrop 0x" + hr.ToString("x") + " effect " + effect
            + " (last feedback " + src.LastFeedback + ", " + src.calls + " passes)");
        // DRAGDROP_S_DROP with an effect other than none: the target took it.
        return hr == 0x00040100 && effect != 0 ? 0 : 1;
    }
}
'@

$code = [OleDrop]::Run($Title, $Files, $Text, $X, $Y)
exit $code
