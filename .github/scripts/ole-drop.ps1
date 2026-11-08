# Drag files or text onto a window through real OLE drag and drop.
#
#   powershell -File ole-drop.ps1 -Title "Stipple drop demo" -Files C:\a.txt
#   powershell -File ole-drop.ps1 -Title "Stipple drop demo" -Text "hello"
#
# Runs DoDragDrop with our own IDropSource: it puts the cursor over the
# window's client area at (-X, -Y), nudges it a few times so the target sees
# DragEnter and DragOver, then answers DRAGDROP_S_DROP. The target's
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

    int calls;
    POINT origin;
    public uint LastFeedback;

    // Called on every pass of DoDragDrop's loop (on mouse and key
    // messages): move along, then drop.
    public int QueryContinueDrag(int esc, uint keys) {
        calls++;
        if (calls < 12) {
            SetCursorPos(origin.X + calls, origin.Y);
            System.Threading.Thread.Sleep(30);
            return 0; // S_OK: keep dragging
        }
        return 0x00040100; // DRAGDROP_S_DROP
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
        // DoDragDrop only consults the source when a mouse or key message
        // arrives, and nothing moves the mouse on a CI desktop: nudge the
        // cursor from another thread so its loop keeps turning.
        var jiggle = new System.Threading.Thread(() => {
            for (int i = 0; i < 400; i++) {
                System.Threading.Thread.Sleep(50);
                SetCursorPos(src.origin.X + src.calls + (i & 1), src.origin.Y);
            }
        });
        jiggle.IsBackground = true;
        jiggle.Start();
        DataObject data = files != null && files.Length > 0
            ? new DataObject(DataFormats.FileDrop, files)
            : new DataObject(DataFormats.UnicodeText, text);
        uint effect;
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
