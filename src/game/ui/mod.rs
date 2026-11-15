//! A game's own UI, built from Stipple's widgets: a theme from the game's
//! font and colors ([`GameStyle`]), menus the arrows and a gamepad move
//! through ([`menu_button`], [`menu_list`], with
//! [`App::gamepad_navigation`](crate::App::gamepad_navigation)), screens
//! (title, pause, game over), a settings screen's rows (volume, controls,
//! display) and a HUD over the game (bars, counters, a minimap slot,
//! floating text, a dialogue box with typewriter text).
//!
//! None of this is a framework: screens are views, switched with
//! [`page_stack`](crate::widgets::page_stack) or a `match` in the view, and
//! the pause menu is a modal overlay. See the "Game UI" section of
//! `docs/games.md` and `examples/gameui`.

mod hud;
mod menu;
pub(crate) mod nav;
mod settings;
mod theme;

#[cfg(test)]
mod tests;

pub use hud::{
    Bar, FILL_MOTION, FLOAT_LIFE, FloatingTexts, GHOST_MOTION, TYPEWRITER_SPEED, Typewriter,
    counter, dialogue_box, floating_texts, health_bar, hud, minimap_slot, stat_bar,
};
pub use menu::{
    MENU_FOCUS_SCALE, controls_hint, game_over_screen, key_cap, menu_button, menu_list, pause_menu,
    title_screen,
};
pub use nav::{REPEAT_DELAY, REPEAT_EVERY, STICK_PRESS, STICK_RELEASE};
pub use settings::{
    BindingRowState, CONTROL_WIDTH, Capture, RebindCapture, binding_name, binding_row, choice_row,
    conflicts, fullscreen_row, game_slider, key_name, pad_button_name, setting_row, toggle_row,
    volume_row,
};
pub use theme::GameStyle;
