# Getting started

This guide builds a small todo list with Stipple: a text field to add todos,
checkboxes to tick them off, buttons to delete them, and a switch for dark
mode. Then it tests the app without a window. It assumes you know Rust; it
doesn't assume you know Stipple.

<!-- live: todo -->

![The finished app](screenshots/getting-started-todo.png)

The finished app is [`examples/todo`](../examples/todo/main.rs) in this
repository (`cargo run --example todo`). The code below is compiled by
`cargo test`, so it stays in step with Stipple.

## 1. A new project

```sh
cargo new todos
cd todos
cargo add stipple
```

Stipple needs Rust 1.88 or newer, and no system libraries: it talks to
Wayland, X11, Win32 and Cocoa itself.

## 2. A window

Replace `src/main.rs` with:

```rust no_run
use stipple::prelude::*;

fn view(_state: &(), cx: &mut Cx<()>) -> Element {
    label(cx, "Hello, Stipple!")
}

fn main() {
    App::new((), view).title("Todos").run();
}
```

`cargo run` opens a window saying hello.

An `App` owns your state (here `()`, nothing yet) and a view: a function
from the state to a tree of `Element`s. Stipple calls it again after anything
that may change the state, and redraws what changed. Widgets are plain
functions that take `cx` first: it carries the theme they draw with, and
it's where they register what happens on a click or a key.

## 3. State and a view of it

A todo list needs the todos, and the text being typed:

```rust no_run
use stipple::prelude::*;

struct Todo {
    text: String,
    done: bool,
}

#[derive(Default)]
struct Todos {
    draft: TextInput,
    items: Vec<Todo>,
}

impl Todos {
    /// Add the draft as a new todo, and empty the field.
    fn add(&mut self) {
        let text = self.draft.take();
        if !text.trim().is_empty() {
            self.items.push(Todo {
                text: text.trim().to_string(),
                done: false,
            });
        }
    }
}

fn view(state: &Todos, cx: &mut Cx<Todos>) -> Element {
    column(vec![
        heading(cx, "Todos"),
        row(vec![
            text_field(cx, &state.draft, |s| &mut s.draft)
                .placeholder("What needs doing?")
                .on_submit(cx, Todos::add)
                .grow(1.0),
            button(cx, "Add").on_tap(cx, Todos::add),
        ])
        .gap(8.0),
        column(state.items.iter().map(|todo| label(cx, &todo.text)).collect()).gap(4.0),
    ])
    .gap(12.0)
    .padding(Insets::uniform(16.0))
}

fn main() {
    App::new(Todos::default(), view).title("Todos").run();
}
```

Type something and press Enter (or click Add): it joins the list.

- **Layout.** `column` stacks its children top to bottom and `row` left to
  right; both take a `Vec` of them. `.gap(..)` spaces them and `.padding(..)`
  keeps them off the edges. `.grow(1.0)` lets the field take the room the
  button leaves. A row centers its children vertically and a column stretches
  them to its width. [layout.md](layout.md) has the rest.
- **Handlers** run against the state: `on_tap(cx, Todos::add)` calls
  `add(&mut Todos)` on a click, and `on_submit` on Enter in the field. A
  closure works too: `.on_tap(cx, |s| s.items.clear())`. Since `cx` comes
  first, Rust knows `s` is a `&mut Todos`: no annotations.
- **State.** `#[derive(Default)]` starts it empty: no todos, an empty
  field.
- **A text field** edits a `TextInput` in your state, which the field reaches
  through an accessor, `|s| &mut s.draft`. `take()` returns its text and
  empties it. The placeholder shows while it's empty.

## 4. Ticking and deleting

Each todo gets a checkbox and a Delete button. Give the row a function of its
own:

```rust ignore
fn todo_row(cx: &mut Cx<Todos>, i: usize, todo: &Todo) -> Element {
    let mut check = checkbox(cx, &todo.text, todo.done, move |s| {
        s.items[i].done = !s.items[i].done;
    });
    if todo.done {
        check = check.strikethrough();
    }
    row(vec![
        check,
        spacer(),
        Button::new("Delete")
            .variant(Variant::Ghost)
            .view(cx)
            .on_tap(cx, move |s| {
                s.items.remove(i);
            }),
    ])
    .gap(8.0)
}
```

The handlers capture the todo's index: the view is rebuilt after each
change, so it always matches the list. The checkbox's label is the todo's
text: a click on the text ticks it too, and screen readers (and the test in
step 6) know the checkbox by it. Struck through, the checkbox strikes its
label. `button(cx, "Add")` is the common, filled button; `Button` makes the
other variants (a ghost button is just its text). `spacer()` pushes what
follows to the row's end.

In the view, the list becomes a scrolling one, built from these rows:

```rust ignore
        scroll(cx).children(
            state
                .items
                .iter()
                .enumerate()
                .map(|(i, todo)| todo_row(cx, i, todo))
                .collect(),
        ),
```

`scroll(cx)` is a column that scrolls what doesn't fit. It takes the height
the window's column leaves free (`.height(..)` would fix one instead), so the
list fills the window and the count stays at the bottom. Under it, the
finished view counts what's left to do:
`label(cx, format!("{left} left"))`, with `left` counted from
`state.items` before the `column`.

This shows the one rule for containers. `row` and `column` take their
children directly. A container that needs `cx` itself (`scroll`, `panel`,
`toolbar`) is built first, and its children are added after it, with
`.children(vec![…])` or `.child(el)`: `panel(cx).children(vec![label(cx,
"Hi"), button(cx, "OK").on_tap(cx, …)])`. That way `scroll(cx)` is done
with `cx` before the rows use it to register their handlers.

## 5. Dark mode

Add a `dark: bool` to the state:

```rust ignore
#[derive(Default)]
struct Todos {
    draft: TextInput,
    items: Vec<Todo>,
    dark: bool,
}
```

a switch to the view, in a row with the heading:

```rust ignore
        row(vec![
            heading(cx, "Todos"),
            spacer(),
            switch(cx, "Dark", state.dark, |s| s.dark = !s.dark),
        ])
        .gap(8.0),
```

and pick the theme from the state:

```rust ignore
fn app() -> App<Todos> {
    App::new(Todos::default(), view)
        .title("Todos")
        .logical_size(Size::new(420.0, 400.0))
        .theme_with(|state, _system| {
            if state.dark {
                Theme::dark()
            } else {
                Theme::light()
            }
        })
}
```

`main` is now `app().run();`. `theme_with` runs before each build; when the
theme it returns changes, the window cross-fades to it. Its second argument
is the theme set with `.theme(..)`: return it to follow the system's light
or dark mode (`Theme::native()` tracks it). Every widget reads its colors
from the theme, so nothing else changes.
`Theme::light().accent(color).radius(10.0)` makes a theme of your own.

That's the app: [`examples/todo/main.rs`](../examples/todo/main.rs)
has it whole.

## 6. Testing it without a window

An `App` runs headless too: `render_once` builds and draws a frame off
screen, and `type_text` and `press_key` deliver input as the window would.
To click, name what to click as a screen reader would know it: its role and
its name. `app.click(Role::Button, "Add")` clicks the middle of the Add
button, `app.find(Role::CheckBox, "Buy milk")` says where that checkbox is
and whether it's checked, and `app.find_text("1 left")` finds a label (or
anything else) showing that text. This is the example's `--selftest`, which
CI runs:

```rust ignore
fn selftest(png: &Path) -> Result<(), String> {
    // Changes snap instead of animating, so each frame shows the result.
    stipple::anim::set_reduced_motion(Some(true));
    let mut app = app();

    // Click the field, then type three todos, each followed by Enter.
    if !app.click(Role::TextField, "What needs doing?") {
        return Err("no text field to click".into());
    }
    for text in ["Buy milk", "Water the plants", "Write a tutorial"] {
        app.type_text(text);
        app.press_key(KeyInput::Enter);
    }
    let texts: Vec<&str> = app.state().items.iter().map(|t| t.text.as_str()).collect();
    if texts != ["Buy milk", "Water the plants", "Write a tutorial"] {
        return Err(format!("added {texts:?}"));
    }
    if !app.state().draft.is_empty() {
        return Err("Enter left the text in the field".into());
    }

    // Tick the first one, delete the second.
    app.click(Role::CheckBox, "Buy milk");
    let deletes = app.find_all(Role::Button, "Delete");
    app.click_at(
        deletes
            .get(1)
            .ok_or("no second Delete button")?
            .bounds
            .center(),
    );
    let done: Vec<bool> = app.state().items.iter().map(|t| t.done).collect();
    if done != [true, false] {
        return Err(format!("done flags after a tick and a delete: {done:?}"));
    }
    if app.find_text("1 left").is_none() {
        return Err("the count doesn't say 1 left".into());
    }

    // Dark mode, and a picture of the result.
    if !app.click(Role::Switch, "Dark") || !app.state().dark {
        return Err("the switch didn't turn dark mode on".into());
    }
    let frame = app.render_once();
    frame
        .save_png(png)
        .map_err(|e| format!("{}: {e}", png.display()))?;
    println!("todo: wrote {}", png.display());
    Ok(())
}
```

(`Path` is `std::path::Path`.) Call it from a `#[test]` and `cargo test`
checks the app, as the example does; the PNG shows what it drew.

`find` and `click` see the window as the state now is: no need to render
between a change and a lookup. A text field is found by its placeholder,
too, and `find_all` lists every match: `deletes[1]` above is the second
Delete button, which `click_at` clicks at a point. `render_once` draws a
frame as the window would at that moment, so a change that animates (the
switch's knob, the theme's cross-fade) shows its first step: reduced motion
makes them snap, as it does for users who ask for it.

## Next steps

- [Layout](layout.md): stacks, wrapping, grids, layers and size limits.
- [Widgets](widgets.md): every widget, and the ones that keep state (select,
  number input, split pane, toolbar, toasts, tabs…); then
  [lists, tables and trees](lists-and-tables.md) for long lists.
- [Timers and background work](async.md): `cx.handle()` returns an
  `AppHandle` that changes the state from timers, other threads or async
  tasks.
- [Animation](animation.md), [menus](menus.md),
  [drag and drop](drag-and-drop.md), [accessibility](accessibility.md) and
  the other topics in the [docs index](README.md).
- Platforms: [Linux](platforms/linux.md), [Windows](platforms/windows.md),
  [macOS](platforms/macos.md), [iOS](platforms/ios.md),
  [Android](platforms/android.md) and [the web](platforms/web.md): how to
  build, package and run on each.
- The [examples](../examples): `cargo run --example widgetsdemo` shows most
  widgets, and the README lists the rest.
