//! Message boxes: a question or a notice with buttons, answered without
//! blocking the event loop.
//!
//! The usual way in is [`AppHandle::message`](crate::core::AppHandle::message),
//! which shows the platform's own alert where there is one and draws one in
//! the window in the theme where there isn't, and applies the answer to the
//! app state. [`message`] is the platform half on its own: it shows the native
//! alert, or says there isn't one.
//!
//! | | Native alert |
//! |---|---|
//! | Windows | `TaskDialogIndirect` (common controls 6, activated for the call), else `MessageBoxW` when the buttons are a standard set, run on a thread of its own and owned by the active window |
//! | macOS | `NSAlert`, as a sheet on the key window (`runModal` when there's none) |
//! | iOS | `UIAlertController`, presented from the key window's root view controller |
//! | Web | `window.alert` (one button) or `window.confirm` (two) |
//! | Linux, Android | none: drawn in the window |
//!
//! A message box with a text [`input`](MessageBox::input) is drawn on every
//! platform but macOS, where it's the alert's accessory view.

/// What a message box is about, which sets its icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MessageKind {
    /// Something to know.
    #[default]
    Info,
    /// Something that may go wrong.
    Warning,
    /// Something that went wrong.
    Error,
    /// A question to answer.
    Question,
}

/// A text field in a message box (a name to enter, a passphrase).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageInput {
    /// The text the field starts with.
    pub value: String,
    /// Hide what is typed, as a password field does.
    pub secure: bool,
}

/// A message box: a message, an optional detail line, an icon for its
/// [`MessageKind`], buttons, and optionally a text field.
///
/// ```
/// use stipple::platform::dialog::{MessageBox, MessageKind};
/// let ask = MessageBox::new("Close this tab?")
///     .detail("A process is still running in it.")
///     .kind(MessageKind::Question)
///     .buttons(["Close", "Cancel"])
///     .cancel(1);
/// assert_eq!(ask.button_labels(), ["Close", "Cancel"]);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageBox {
    /// The window title (where the platform shows one); the app's name is a
    /// good choice.
    pub title: String,
    /// The message: one short sentence.
    pub text: String,
    /// More about it, in smaller text below.
    pub detail: String,
    pub kind: MessageKind,
    /// The buttons, in reading order; empty for a single "OK".
    pub buttons: Vec<String>,
    /// The button Enter presses.
    pub default: usize,
    /// The button Escape (and the window's close box) presses; `None` when
    /// the box can't be dismissed without choosing.
    pub cancel: Option<usize>,
    /// A text field under the message.
    pub input: Option<MessageInput>,
    /// Draw it in the window, in the theme, even where the platform has an
    /// alert.
    pub drawn: bool,
}

impl MessageBox {
    /// A box showing `text` with an "OK" button.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn kind(mut self, kind: MessageKind) -> Self {
        self.kind = kind;
        self
    }

    /// The buttons, in reading order. The first is the default unless
    /// [`default`](Self::default_button) says otherwise.
    pub fn buttons<I, T>(mut self, buttons: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.buttons = buttons.into_iter().map(Into::into).collect();
        self
    }

    /// The button Enter presses.
    pub fn default_button(mut self, index: usize) -> Self {
        self.default = index;
        self
    }

    /// The button Escape presses.
    pub fn cancel(mut self, index: usize) -> Self {
        self.cancel = Some(index);
        self
    }

    /// Add a text field starting with `value`, hidden when `secure`.
    pub fn input(mut self, value: impl Into<String>, secure: bool) -> Self {
        self.input = Some(MessageInput {
            value: value.into(),
            secure,
        });
        self
    }

    /// Draw it in the window even where there's a native alert.
    pub fn drawn(mut self, drawn: bool) -> Self {
        self.drawn = drawn;
        self
    }

    /// The buttons' labels: the ones given, or "OK".
    pub fn button_labels(&self) -> Vec<String> {
        if self.buttons.is_empty() {
            vec!["OK".to_string()]
        } else {
            self.buttons.clone()
        }
    }

    /// The default button's index, within the buttons.
    pub(crate) fn default_index(&self) -> usize {
        self.default.min(self.button_labels().len() - 1)
    }

    /// The cancel button's index, within the buttons; a single button
    /// cancels too.
    pub(crate) fn cancel_index(&self) -> Option<usize> {
        let n = self.button_labels().len();
        match self.cancel {
            Some(i) if i < n => Some(i),
            _ if n == 1 => Some(0),
            _ => None,
        }
    }
}

/// How a message box was answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageResponse {
    /// The button pressed (Escape counts as the cancel button); `None` if the
    /// box went away without one (a box with no cancel button closed by the
    /// platform).
    pub button: Option<usize>,
    /// The text field's text, for a box with one.
    pub input: Option<String>,
}

/// What a native alert calls with its answer. It may run on another thread
/// (Windows runs the dialog on one of its own).
pub type MessageDone = Box<dyn FnOnce(MessageResponse) + Send>;

/// Show `mb` as the platform's own alert, and call `done` with the answer
/// once the user gives it. Returns `Err(done)`, without calling it, where
/// there's no native alert for `mb` (Linux, Android, a box with a text field
/// outside macOS, or one marked [`drawn`](MessageBox::drawn)); draw one then,
/// as [`AppHandle::message`](crate::core::AppHandle::message) does.
///
/// Call it on the UI thread. It doesn't block on the desktops and on iOS:
/// `done` runs later (on Windows, from the dialog's own thread). The web's
/// `alert` and `confirm` block the page, and `done` runs before this returns.
pub fn message(mb: &MessageBox, done: MessageDone) -> Result<(), MessageDone> {
    if mb.drawn || std::env::var_os("STIPPLE_DRAWN_DIALOGS").is_some() {
        return Err(done);
    }
    native::show(mb, done)
}

/// A diagnostic line on stderr when `STIPPLE_DEBUG_DIALOG` is set (CI turns
/// it on to show what the native alert did).
#[allow(dead_code)] // only the native alerts log
fn debug(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("STIPPLE_DEBUG_DIALOG").is_some() {
        eprintln!("{args}");
    }
}

/// Whether an app's event loop runs, so native alerts can show (in unit
/// tests nothing would answer them).
static LOOP_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Note that the app's event loop started (or stopped).
pub(crate) fn set_event_loop_running(running: bool) {
    LOOP_RUNNING.store(running, std::sync::atomic::Ordering::Release);
}

/// Whether [`AppHandle::message`](crate::core::AppHandle::message) may use
/// a native alert: only while an event loop runs.
pub(crate) fn native_allowed() -> bool {
    LOOP_RUNNING.load(std::sync::atomic::Ordering::Acquire)
}

/// Press button `index` of the native alert showing now, as a click would.
/// For self-tests (CI dismisses alerts with it); returns whether there was
/// one to press.
#[doc(hidden)]
pub fn press_native_button(index: usize) -> bool {
    native::press(index)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[path = "message_windows.rs"]
mod native;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "message_macos.rs"]
mod native;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[path = "message_ios.rs"]
mod native;

#[cfg(target_arch = "wasm32")]
mod native {
    use super::{MessageBox, MessageDone, MessageResponse};

    pub(super) fn show(mb: &MessageBox, done: MessageDone) -> Result<(), MessageDone> {
        let n = mb.button_labels().len();
        if mb.input.is_some() || n > 2 {
            return Err(done);
        }
        let mut text = mb.text.clone();
        if !mb.detail.is_empty() {
            text.push_str("\n\n");
            text.push_str(&mb.detail);
        }
        // `confirm`'s OK is the default button, and its Cancel the other.
        let ok = crate::platform::backend::web::message_box(n == 2, &text);
        let button = match (n, ok) {
            (1, _) => 0,
            (_, true) => mb.default_index(),
            (_, false) => 1 - mb.default_index(),
        };
        done(MessageResponse {
            button: Some(button),
            input: None,
        });
        Ok(())
    }

    pub(super) fn press(_index: usize) -> bool {
        false
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_arch = "wasm32"
)))]
mod native {
    use super::{MessageBox, MessageDone};

    // Linux has no alert API outside a toolkit (and the desktop portal has
    // none); Android's `AlertDialog` would need a Java listener class.
    pub(super) fn show(_mb: &MessageBox, done: MessageDone) -> Result<(), MessageDone> {
        Err(done)
    }

    pub(super) fn press(_index: usize) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_cancel() {
        let one = MessageBox::new("Saved");
        assert_eq!(one.button_labels(), ["OK"]);
        assert_eq!(one.default_index(), 0);
        assert_eq!(one.cancel_index(), Some(0));
        let three = MessageBox::new("?")
            .buttons(["Save", "Don't Save", "Cancel"])
            .default_button(7);
        assert_eq!(three.default_index(), 2);
        assert_eq!(three.cancel_index(), None);
        assert_eq!(three.clone().cancel(2).cancel_index(), Some(2));
        assert_eq!(three.cancel(9).cancel_index(), None);
    }

    #[test]
    fn drawn_boxes_have_no_native_alert() {
        let r = message(&MessageBox::new("x").drawn(true), Box::new(|_| {}));
        assert!(r.is_err());
    }
}
