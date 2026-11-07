//! Platform-neutral accessibility tree.
//!
//! `stipple::platform` is dependency-free, so it can't name
//! `stipple::core::AccessNode`. The umbrella `App` maps that semantic tree into
//! this mirror and hands it to a backend through
//! [`Window::set_accessibility_tree`](crate::platform::Window::set_accessibility_tree).
//! Each backend then exposes it to its OS accessibility API — `NSAccessibility`
//! on macOS, UI Automation on Windows, AT-SPI on Linux — so a screen reader sees
//! the *whole* element hierarchy, not just the window root.
//!
//! This carries the same five fields `stipple::core::AccessNode` does; keep
//! [`A11yRole`] in sync with `stipple::core::a11y::Role`.

/// The semantic kind of an [`A11yNode`] — mirrors `stipple::core::a11y::Role`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum A11yRole {
    /// The top-level window (tree root).
    Window,
    /// A non-interactive container.
    Group,
    /// An activatable control.
    Button,
    /// An editable text field.
    TextField,
    /// A run of static text.
    Text,
}

/// A node in the platform-neutral accessibility tree.
#[derive(Clone, Debug, PartialEq)]
pub struct A11yNode {
    pub role: A11yRole,
    /// The accessible name (a control's label, or the text itself).
    pub name: String,
    /// Logical bounds `(x, y, width, height)` — used for frame reporting and
    /// hit-testing by backends whose AT exposes geometry.
    pub bounds: (f64, f64, f64, f64),
    /// Whether this node currently holds keyboard focus.
    pub focused: bool,
    pub children: Vec<A11yNode>,
}

impl A11yNode {
    /// Total node count (including this one), for tests/inspection.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(A11yNode::count).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode {
            role,
            name: name.to_string(),
            bounds: (0.0, 0.0, 0.0, 0.0),
            focused: false,
            children: Vec::new(),
        }
    }

    #[test]
    fn count_includes_all_descendants() {
        let root = A11yNode {
            role: A11yRole::Window,
            name: String::new(),
            bounds: (0.0, 0.0, 100.0, 100.0),
            focused: false,
            children: vec![
                leaf(A11yRole::Text, "Title"),
                A11yNode {
                    children: vec![leaf(A11yRole::Button, "OK")],
                    ..leaf(A11yRole::Group, "")
                },
            ],
        };
        // root + Title + Group + OK = 4
        assert_eq!(root.count(), 4);
    }
}
