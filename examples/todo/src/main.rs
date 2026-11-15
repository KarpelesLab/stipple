//! The app that `docs/getting-started.md` builds: a todo list with a text
//! field, checkboxes, delete buttons and a dark-mode switch.
//!
//! ```sh
//! cargo run -p todo                         # the window
//! cargo run -p todo -- --selftest [OUT.png] # headless: add, toggle, delete, save a PNG
//! ```
//!
//! The tutorial quotes this file: its tests fail when a quoted block no
//! longer matches it.

use std::path::Path;
use stipple::core::Role;
use stipple::prelude::*;

struct Todo {
    text: String,
    done: bool,
}

#[derive(Default)]
struct Todos {
    draft: TextInput,
    items: Vec<Todo>,
    dark: bool,
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
    let left = state.items.iter().filter(|todo| !todo.done).count();
    column(vec![
        row(vec![
            heading(cx, "Todos"),
            spacer(),
            label(cx, "Dark"),
            switch(cx, state.dark, |s| s.dark = !s.dark),
        ])
        .gap(8.0),
        row(vec![
            text_field(cx, &state.draft, |s| &mut s.draft)
                .placeholder("What needs doing?")
                .on_submit(cx, Todos::add)
                .grow(1.0),
            button(cx, "Add").on_tap(cx, Todos::add),
        ])
        .gap(8.0),
        scroll(cx, 200.0).grow(1.0).children(
            state
                .items
                .iter()
                .enumerate()
                .map(|(i, todo)| todo_row(cx, i, todo))
                .collect(),
        ),
        label(cx, format!("{left} left")),
    ])
    .gap(12.0)
    .padding(Insets::uniform(16.0))
}

fn todo_row(cx: &mut Cx<Todos>, i: usize, todo: &Todo) -> Element {
    let mut text = label(cx, &todo.text);
    if todo.done {
        text = text.strikethrough();
    }
    row(vec![
        checkbox(cx, todo.done, move |s| s.items[i].done = !s.items[i].done).a11y_label(&todo.text),
        text,
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        let out = args.iter().skip(1).find(|a| !a.starts_with("--"));
        match selftest(Path::new(out.map_or("todo.png", String::as_str))) {
            Ok(()) => println!("todo: selftest ok"),
            Err(e) => {
                println!("todo: selftest FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    app().run();
}

/// Where the `n`th element with `role` (and `name`, unless empty) was laid
/// out in the last frame.
fn find(app: &App<Todos>, role: Role, name: &str, n: usize) -> Result<Point, String> {
    let tree = app.accessibility_tree().ok_or("no frame yet")?;
    tree.descendants()
        .into_iter()
        .filter(|node| node.role == role && (name.is_empty() || node.name == name))
        .nth(n)
        .map(|node| node.bounds.center())
        .ok_or(format!("no {role:?} {name:?} #{n}"))
}

fn selftest(png: &Path) -> Result<(), String> {
    // Changes snap instead of animating, so each frame shows the result.
    stipple::anim::set_reduced_motion(Some(true));
    let mut app = app();
    app.render_once();

    // Click the field, then type three todos, each followed by Enter.
    app.click_at(find(&app, Role::TextField, "", 0)?);
    for text in ["Buy milk", "Water the plants", "Write a tutorial"] {
        app.type_text(text);
        app.press_key(KeyInput::Enter);
    }
    app.render_once();
    let texts: Vec<&str> = app.state().items.iter().map(|t| t.text.as_str()).collect();
    if texts != ["Buy milk", "Water the plants", "Write a tutorial"] {
        return Err(format!("added {texts:?}"));
    }
    if !app.state().draft.is_empty() {
        return Err("Enter left the text in the field".into());
    }

    // Tick the first one, delete the second.
    app.click_at(find(&app, Role::CheckBox, "Buy milk", 0)?);
    app.render_once();
    app.click_at(find(&app, Role::Button, "Delete", 1)?);
    app.render_once();
    let done: Vec<bool> = app.state().items.iter().map(|t| t.done).collect();
    if done != [true, false] {
        return Err(format!("done flags after a tick and a delete: {done:?}"));
    }

    // Dark mode, and a picture of the result.
    app.click_at(find(&app, Role::Switch, "", 0)?);
    if !app.state().dark {
        return Err("the switch didn't turn dark mode on".into());
    }
    let frame = app.render_once();
    frame
        .save_png(png)
        .map_err(|e| format!("{}: {e}", png.display()))?;
    println!("todo: wrote {}", png.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The tutorial's code blocks that rustdoc doesn't compile (`rust ignore`)
    /// are excerpts of this file, word for word.
    #[test]
    fn the_tutorial_quotes_this_file() {
        let tutorial = include_str!("../../../docs/getting-started.md");
        let source = include_str!("main.rs");
        let mut quoted = 0;
        let mut lines = tutorial.lines();
        while let Some(line) = lines.next() {
            let Some(info) = line.strip_prefix("```") else {
                continue;
            };
            let block: Vec<&str> = lines.by_ref().take_while(|l| *l != "```").collect();
            let words: Vec<&str> = info.split([' ', ',']).collect();
            if words[0] == "rust" && words.contains(&"ignore") {
                let block = block.join("\n");
                assert!(
                    source.contains(&block),
                    "docs/getting-started.md quotes code that examples/todo/src/main.rs \
                     doesn't have:\n{block}"
                );
                quoted += 1;
            }
        }
        assert!(quoted > 0, "no excerpts found");
    }

    #[test]
    fn adds_ticks_deletes_and_goes_dark() {
        let png = std::env::temp_dir().join("stipple-todo-test.png");
        super::selftest(&png).unwrap();
    }
}
