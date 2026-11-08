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
  Without a mnemonic, the first letter works. Windows' native menus get the
  `&` as is; macOS has no mnemonics and drops it.

To quit from a menu, keep a clone of `App::handle()` in the closure and call
`exit()` on it (see `menudemo`).

## How each platform shows it

| | Menu bar | Shortcuts | Context menus |
|---|---|---|---|
| macOS | the system menu bar (`NSMenu`), behind the standard application menu | ✅ | native (`NSMenu`) |
| Windows | the window's native menu bar (`HMENU`) | ✅ | native (`TrackPopupMenuEx`) |
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

**Mnemonic underlines.** The drawn menus follow Windows' convention: the
mnemonics are underlined while the keyboard is driving the menus, and not in
a menu opened with the mouse. They show while Alt is held, stay when Alt is
released on its own (the keyboard is then on the bar), after F10, Alt+letter,
the Menu key or Shift+F10, and once a key is used in an open menu; they go when
the menus close, or when Alt was used for something else (Alt+X, Alt+Tab).

To have Stipple draw the menus on macOS and Windows too (to match a custom
theme, say), call `.menu_bar_in_window(true)`: the menu bar and the context
menus are then drawn, in the theme's colors, as on Linux.

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

Each shortcut runs once. Windows gets no accelerator table, so only Stipple
matches the keys. On macOS the menu items carry key equivalents, so AppKit
matches the key too, after Stipple has seen it; the backend drops AppKit's pick
when Stipple's own matching would have taken the key for that item, whether
it ran the item or gave the key to a focused text field (so ⌘C in a field
copies the field's selection and doesn't also run Edit > Copy). A key AppKit
matches but Stipple can't (on a keyboard layout whose letters aren't Latin)
still runs the item, once.

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
has focus).

**macOS and Windows** show it as the system's own popup menu: an `NSMenu`
(`popUpMenuPositioningItem:atLocation:inView:`) or a `TrackPopupMenuEx` menu
built from the same model, with shortcuts, check marks, disabled items,
submenus and (on Windows) mnemonics. These run their own modal loop: the window
keeps painting, and what happens meanwhile (timers, input) is handled once the
menu closes. The pick runs the item's action as a drawn menu's would. The
popup always looks like the system's, whatever the theme, the way the native
menu bar does; for menus that match a custom theme, use
`.menu_bar_in_window(true)`, which draws the context menus too.

**Elsewhere** (Linux, and where the platform has no popup menus) Stipple draws
it as an overlay, like a select's list. It moves left or up if it would cross
the window's edge, and the same keys and mouse moves work in it as in the menu
bar's menus.

## Accessibility

Menus report the `MenuBar`, `Menu` and `MenuItem` roles (and `Separator` for
dividers), which UI Automation, NSAccessibility and AT-SPI map to their own menu
roles. Native menu bars on macOS and Windows are exposed by the OS itself.

## How it's tested

- **X11** (Xvfb, `xdotool`): CI holds Alt and checks the bar's mnemonics are
  underlined (and nothing else changed), opens File with Alt+F and with a
  click, uses Ctrl+D, opens the canvas's context menu with a right-click and
  picks from it with the keyboard, and checks from `menudemo`'s log that each
  command ran once.
- **Windows**: `.github/scripts/menu-windows.ps1` reads the `HMENU` back
  (`GetMenu`, `GetMenuStringW`, `GetMenuState`: titles with their `&`, shortcut
  text, grayed and checked items), sends `WM_COMMAND` with Edit > Add Item's id,
  opens Edit with Alt+E and picks with its mnemonic, presses Ctrl+D, right-clicks
  the canvas and waits for the native popup (window class `#32768`), picks from
  it with its mnemonic, and checks each ran exactly once.
- **macOS**: System Events (`osascript … click menu item`) needs an
  accessibility permission CI may not have, so CI tries it but doesn't rely on
  it. `STIPPLE_COCOA_MENU_SELFTEST` runs an in-process self-test through real
  Objective-C dispatch and the real event loop: it prints the `NSMenu` bar as
  AppKit holds it (titles, key equivalents, validated enabled states), picks
  Edit > Add Item with `performActionForItemAtIndex:`, posts ⌘D, right-clicks
  the canvas (the native context menu reports its items and picks the first),
  then posts ⌘C with no field focused and again with the note field focused.
  CI checks that each ran once and ⌘C in the field didn't run the menu item.
  Steps are given in the variable, e.g.
  `perform=Edit/Add Item;key=cmd+d;right=320,300;key=tab;key=cmd+c`.

## What's not there yet

- Radio groups are checkable items you keep exclusive yourself.
- The native popups' look doesn't follow a custom theme (Windows shows its
  light menus even under a dark theme); draw them with
  `.menu_bar_in_window(true)` for that.
- The drawn context menu's mnemonics are underlined only when it was opened
  from the keyboard (or a key was used in it), as on Windows; there is no
  setting for always showing them.
