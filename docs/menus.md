# Menus

An app declares a menu bar and context menus as plain data: a `MenuBar` of
`Menu`s of `MenuItem`s. Each item has a label, an action that runs against the
app state (like an `on_tap` handler), and optionally a shortcut, an enabled
state and a check mark. The `menudemo` example shows all of it:

```sh
cargo run -p menudemo            # or: dark
```

## The menu bar

Pass `App::menu` a function of the state. It's called again as the state
changes, so labels and enabled and checked states always match it.

```rust
struct Doc { text: String, saved: bool, wrap: bool }

App::new(doc, view).menu(|doc: &Doc| {
    MenuBar::new()
        .menu(
            Menu::new("&File")
                .item(MenuItem::new("&Save", |d: &mut Doc| d.saved = true)
                    .shortcut(Shortcut::cmd('s'))
                    .enabled(!doc.saved))
                .separator()
                .submenu(Menu::new("Open &Recent")
                    .item(MenuItem::new("notes.txt", |d: &mut Doc| { /* … */ })))
                .item(MenuItem::new("&Quit", |_: &mut Doc| { /* handle.exit() */ })),
        )
        .menu(Menu::new("&View").item(
            MenuItem::new("&Wrap Lines", |d: &mut Doc| d.wrap = !d.wrap).checked(doc.wrap),
        ))
})
```

- `MenuItem::new(label, action)` is a command. `.shortcut(..)`, `.enabled(..)`
  and `.checked(..)` set its shortcut, whether it can be picked, and makes it
  checkable with a check mark. A disabled item is greyed out and its shortcut
  does nothing.
- `Menu::separator()` adds a divider, `Menu::submenu(menu)` a nested menu.
- A `&` in a label marks its mnemonic: `"&File"` is opened with Alt+F, and in
  an open menu typing `s` picks `"&Save"`. Write `&&` for a literal `&`.
  Without a mnemonic, the first letter works.

To quit from a menu, keep a clone of `App::handle()` in the closure and call
`exit()` on it (see `menudemo`).

## How each platform shows it

| | Menu bar | Shortcuts | Context menus |
|---|---|---|---|
| macOS | the system menu bar (`NSMenu`), behind the standard application menu | ✅ | drawn by Stipple |
| Windows | the window's native menu bar (`HMENU`) | ✅ | drawn by Stipple |
| Linux (X11, Wayland) | drawn by Stipple across the top of the window | ✅ | drawn by Stipple |
| iOS, Android, web | drawn by Stipple | where keys arrive | drawn by Stipple |

**macOS.** The menu bar belongs to the application. Stipple puts the
standard application menu first, named after the window title: About, Hide
(⌘H), Hide Others (⌥⌘H), Show All and Quit (⌘Q), then the app's menus. Items
show their shortcuts as key equivalents, and AppKit asks Stipple whether each
is enabled (`validateMenuItem:`) before showing it. `&` mnemonics are dropped,
as macOS has none. Quit ends the event loop the way closing the window does.

**Windows.** Each top-level menu is a popup on the window's menu bar. The
mnemonics work as usual (Alt, Alt+letter and F10 are handled by Windows),
shortcuts are shown right-aligned, and disabled and checked items look like
every other Windows menu. Adding the bar grows the window so the content keeps
its size.

**Linux and elsewhere.** There's no system menu bar to use, so Stipple draws
one above the primary window's content, in the theme's colors. Clicking a title
opens its menu below it; while one is open, moving over another title switches
to it and moving over a submenu's item opens the submenu beside it. A press
outside closes the menus. From the keyboard:

| Key | Does |
|---|---|
| Alt (pressed and released on its own), F10 | put the keyboard on the menu bar |
| Alt+letter | open the menu with that mnemonic |
| Left, Right | move between menus (or close a submenu / open one) |
| Up, Down, Home, End | move in the open menu, skipping separators and disabled items |
| Enter, Space | pick the item, or open its submenu |
| a letter | pick the item with that mnemonic |
| Escape | close the submenu, then the menu, then leave the bar |

To have Stipple draw the menu bar on macOS and Windows too (to match a custom
theme, say), call `.menu_bar_in_window(true)`.

### Shortcuts

`Shortcut::cmd('s')` is Ctrl+S on Windows and Linux and ⌘S on macOS.
`.shift()`, `.alt()` and `.ctrl()` add modifiers (`.ctrl()` is the Control key
itself, distinct from ⌘ on macOS); `Shortcut::key(KeyCode::F5)` is a key on its
own and `Shortcut::cmd_key(KeyCode::Delete)` a named key with the command
modifier. Labels follow the platform: `Ctrl+Shift+S`, or `⇧⌘S`.

Stipple matches shortcuts itself, from the key events, on every platform and
whether the menu bar is native or drawn, so they behave the same everywhere. A
shortcut runs only when its item is enabled. One exception: when a text field
has focus, the editing keys it understands (Ctrl+C, Ctrl+V, Ctrl+Z, Ctrl+A…) go
to the field instead of the menu.

## Context menus

Give any element a context menu with `.context_menu(cx, menu)`:

```rust
label(&theme, "Right-click me").context_menu(
    cx,
    Menu::new("Canvas")
        .item(MenuItem::new("&Add Item", |s: &mut State| s.add()))
        .item(MenuItem::new("C&lear", |s: &mut State| s.clear()).enabled(!s.items.is_empty()))
        .separator()
        .submenu(color_menu(s)),
)
```

It opens at the pointer on a right-click over the element, or on the Menu key
or Shift+F10 while the pointer is over it (or, failing that, while the element
has focus). It moves left or up if it would cross the window's edge, and the
same keys and mouse moves work in it as in the menu bar's menus. Stipple draws
context menus on every platform, as overlays like a select's list.

## Accessibility

Menus report the `MenuBar`, `Menu` and `MenuItem` roles (and `Separator` for
dividers), which UI Automation, NSAccessibility and AT-SPI map to their own menu
roles. Native menu bars on macOS and Windows are exposed by the OS itself.

## What's not there yet

- The drawn menus don't underline mnemonics; the keys work without it.
- Radio groups are checkable items you keep exclusive yourself.
- Context menus aren't native popups on macOS and Windows.
- On macOS and Windows the native menu bar is verified to build, not run, in
  CI; the drawn bar and context menus are exercised end to end on X11.
