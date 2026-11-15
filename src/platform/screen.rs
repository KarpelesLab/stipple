//! The screen a phone's app fills: which way it is turned
//! ([`Orientation`]), which ways the app lets it turn ([`Orientations`]),
//! and how each platform spells those.

/// Which way a window is turned, from its size: wider than tall is
/// [`Landscape`](Self::Landscape), anything else [`Portrait`](Self::Portrait).
/// Read it with [`WindowInfo::orientation`](crate::platform::WindowInfo::orientation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Orientation {
    /// Taller than wide (or square).
    #[default]
    Portrait,
    /// Wider than tall.
    Landscape,
}

/// A set of the ways a phone or tablet may turn the app: what
/// [`App::orientations`](crate::App::orientations) and
/// [`AppHandle::set_orientations`](crate::core::AppHandle::set_orientations)
/// allow. Combine them with `|`:
/// `Orientations::PORTRAIT | Orientations::LANDSCAPE_LEFT`.
///
/// The landscape ones are named for the side the top of the device goes to
/// (as `UIDeviceOrientation` names them): turned a quarter counterclockwise,
/// the top is on the left.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Orientations(u8);

impl Orientations {
    /// Upright.
    pub const PORTRAIT: Self = Self(1);
    /// Upside down (phones without a home button mostly refuse it).
    pub const PORTRAIT_UPSIDE_DOWN: Self = Self(2);
    /// Turned a quarter counterclockwise: the top of the device on the left.
    pub const LANDSCAPE_LEFT: Self = Self(4);
    /// Turned a quarter clockwise: the top of the device on the right.
    pub const LANDSCAPE_RIGHT: Self = Self(8);
    /// Either landscape.
    pub const LANDSCAPE: Self = Self(4 | 8);
    /// Upright or upside down.
    pub const ANY_PORTRAIT: Self = Self(1 | 2);
    /// Every way.
    pub const ALL: Self = Self(15);

    /// Whether every orientation of `other` is in this set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The set as a `UIInterfaceOrientationMask`. UIKit names interface
    /// orientations for where the home button would be, so a device turned
    /// with its top on the left is `LandscapeRight`.
    #[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
    pub(crate) fn ios_mask(self) -> u64 {
        let s = self;
        let mut mask = 0;
        if s.contains(Self::PORTRAIT) {
            mask |= 1 << 1;
        }
        if s.contains(Self::PORTRAIT_UPSIDE_DOWN) {
            mask |= 1 << 2;
        }
        if s.contains(Self::LANDSCAPE_LEFT) {
            mask |= 1 << 3; // UIInterfaceOrientationLandscapeRight
        }
        if s.contains(Self::LANDSCAPE_RIGHT) {
            mask |= 1 << 4; // UIInterfaceOrientationLandscapeLeft
        }
        mask
    }

    /// The `ActivityInfo.SCREEN_ORIENTATION_*` value for
    /// `Activity.setRequestedOrientation`. A set Android has no constant for
    /// (portrait and one landscape) follows the sensor and the user's
    /// rotation lock.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn android_request(self) -> i32 {
        const PORTRAIT: i32 = 1;
        const LANDSCAPE: i32 = 0;
        const USER: i32 = 2;
        const REVERSE_LANDSCAPE: i32 = 8;
        const REVERSE_PORTRAIT: i32 = 9;
        const USER_LANDSCAPE: i32 = 11;
        const USER_PORTRAIT: i32 = 12;
        const FULL_USER: i32 = 13;
        match self.0 {
            0 | 15 => FULL_USER,
            1 => PORTRAIT,
            2 => REVERSE_PORTRAIT,
            3 => USER_PORTRAIT,
            4 => LANDSCAPE,
            8 => REVERSE_LANDSCAPE,
            12 => USER_LANDSCAPE,
            // Portrait upright and either landscape: what phones do anyway.
            13 => USER,
            _ => FULL_USER,
        }
    }
}

impl Default for Orientations {
    /// [`ALL`](Self::ALL).
    fn default() -> Self {
        Self::ALL
    }
}

impl std::ops::BitOr for Orientations {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Orientations {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::fmt::Debug for Orientations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const NAMES: [&str; 4] = [
            "PORTRAIT",
            "PORTRAIT_UPSIDE_DOWN",
            "LANDSCAPE_LEFT",
            "LANDSCAPE_RIGHT",
        ];
        let names: Vec<&str> = NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, n)| *n)
            .collect();
        write!(f, "Orientations({})", names.join(" | "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_combine_and_map_to_each_platform() {
        let set = Orientations::PORTRAIT | Orientations::LANDSCAPE_LEFT;
        assert!(set.contains(Orientations::PORTRAIT));
        assert!(!set.contains(Orientations::LANDSCAPE));
        assert_eq!(
            format!("{set:?}"),
            "Orientations(PORTRAIT | LANDSCAPE_LEFT)"
        );
        assert_eq!(Orientations::default(), Orientations::ALL);
        // UIKit: portrait 2, upside down 4, landscape right 8, left 16.
        assert_eq!(Orientations::PORTRAIT.ios_mask(), 2);
        assert_eq!(Orientations::LANDSCAPE_LEFT.ios_mask(), 8);
        assert_eq!(Orientations::LANDSCAPE.ios_mask(), 24);
        assert_eq!(Orientations::ALL.ios_mask(), 30);
        // Android's requested orientations.
        assert_eq!(Orientations::PORTRAIT.android_request(), 1);
        assert_eq!(Orientations::LANDSCAPE.android_request(), 11);
        assert_eq!(Orientations::LANDSCAPE_LEFT.android_request(), 0);
        assert_eq!(Orientations::LANDSCAPE_RIGHT.android_request(), 8);
        assert_eq!(Orientations::ANY_PORTRAIT.android_request(), 12);
        assert_eq!(
            (Orientations::PORTRAIT | Orientations::LANDSCAPE).android_request(),
            2
        );
        assert_eq!(Orientations::ALL.android_request(), 13);
    }
}
