//! Exercises the hand-written UI Automation provider in `stipple::platform::uia`
//! (no `windows`/`uiautomation` crate) — the Windows accessibility bridge.
//!
//! The Windows visual CI job runs this and greps the output to confirm the
//! `IRawElementProviderSimple` COM object answers UIA property queries through
//! its vtable, in-process. The same job also walks a running window from
//! another process with the UI Automation client (`.github/scripts/uia-walk.ps1`).
//!
//! With `--patterns` it exercises the control patterns instead (Value,
//! Toggle, Selection, SelectionItem, ExpandCollapse, Scroll, ScrollItem and
//! Text with its ranges), each through the interface `GetPatternProvider`
//! hands out.

fn main() {
    #[cfg(target_os = "windows")]
    {
        if std::env::args().any(|a| a == "--patterns") {
            if let Err(e) = stipple::platform::uia::selftest_patterns() {
                eprintln!("UIA pattern selftest failed: {e}");
                std::process::exit(1);
            }
            return;
        }
        if let Err(e) = stipple::platform::uia::selftest("Stipple") {
            eprintln!("UIA selftest failed: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        println!("uiademo: UI Automation is Windows-only");
    }
}
