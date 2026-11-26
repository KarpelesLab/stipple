use super::marshal::{Args, Body, Value};
use super::*;
use crate::geometry::Point;
use crate::platform::a11y::{Message, read_string, read_u32};
use crate::platform::{ButtonState, Ime, PointerButton};

fn leaf(role: A11yRole, name: &str, bounds: (f64, f64, f64, f64)) -> A11yNode {
    A11yNode::new(role, name, bounds)
}

/// Window > [Text "Rows tapped: 0", TextField "hi" (focused), Group > [Button "Row 0"]].
fn tree() -> A11yNode {
    A11yNode {
        children: vec![
            leaf(A11yRole::Text, "Rows tapped: 0", (10.0, 10.0, 100.0, 20.0)),
            A11yNode {
                focused: true,
                ..leaf(A11yRole::TextField, "hi", (10.0, 40.0, 100.0, 20.0))
            },
            A11yNode {
                children: vec![leaf(A11yRole::Button, "Row 0", (20.0, 80.0, 60.0, 30.0))],
                ..leaf(A11yRole::Group, "", (10.0, 70.0, 200.0, 100.0))
            },
        ],
        ..leaf(A11yRole::Window, "", (0.0, 0.0, 300.0, 200.0))
    }
}

fn bridge() -> Bridge {
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    let w = b.win_mut(0x42);
    w.title = "Stipple Touch".into();
    w.origin = (100, 50);
    w.scale = 2.0;
    w.active = true;
    w.root = Some(Arc::new(tree()));
    b
}

fn call(b: &mut Bridge, path: &str, iface: &str, member: &str, body: Body) -> Reply {
    let m = Message {
        mtype: 1,
        serial: 9,
        path: path.into(),
        interface: iface.into(),
        member: member.into(),
        sender: ":1.2".into(),
        body: body.0,
        ..Default::default()
    };
    serve::answer(b, &m)
}

fn ok(r: Reply, sig: &str) -> Vec<u8> {
    match r {
        Reply::Return(s, body) => {
            assert_eq!(s, sig);
            body
        }
        Reply::Error(e) => panic!("error {e}"),
    }
}

fn prop(b: &mut Bridge, path: &str, iface: &str, name: &str) -> Vec<u8> {
    let mut body = Body::new();
    body.str(iface).str(name);
    ok(
        call(b, path, "org.freedesktop.DBus.Properties", "Get", body),
        "v",
    )
}

/// An `a(so)`'s paths.
fn paths(body: &[u8]) -> Vec<String> {
    let mut off = 0;
    let len = read_u32(body, &mut off).unwrap() as usize;
    crate::platform::a11y::align_to(&mut off, 8);
    let end = off + len;
    let mut out = Vec::new();
    while off < end {
        crate::platform::a11y::align_to(&mut off, 8);
        let _bus = read_string(body, &mut off).unwrap();
        out.push(read_string(body, &mut off).unwrap());
    }
    out
}

#[test]
fn paths_round_trip() {
    assert_eq!(node_path(0x42, &[]), "/org/a11y/atspi/accessible/w66");
    assert_eq!(node_path(7, &[2, 0]), "/org/a11y/atspi/accessible/w7_2_0");
    assert_eq!(
        parse_path("/org/a11y/atspi/accessible/w7_2_0"),
        Some((7, vec![2, 0]))
    );
    assert_eq!(
        parse_path("/org/a11y/atspi/accessible/w7"),
        Some((7, vec![]))
    );
    assert_eq!(parse_path(ROOT_PATH), None);
    assert_eq!(parse_path("/org/a11y/atspi/accessible/w7_x"), None);
}

#[test]
fn the_root_is_the_application_with_the_frames_as_children() {
    let mut b = bridge();
    let role = ok(
        call(
            &mut b,
            ROOT_PATH,
            "org.a11y.atspi.Accessible",
            "GetRole",
            Body::new(),
        ),
        "u",
    );
    assert_eq!(read_u32(&role, &mut 0).unwrap(), 75);
    let kids = ok(
        call(
            &mut b,
            ROOT_PATH,
            "org.a11y.atspi.Accessible",
            "GetChildren",
            Body::new(),
        ),
        "a(so)",
    );
    assert_eq!(paths(&kids), vec![node_path(0x42, &[])]);
    let toolkit = prop(
        &mut b,
        ROOT_PATH,
        "org.a11y.atspi.Application",
        "ToolkitName",
    );
    assert_eq!(read_string(&toolkit, &mut 3).unwrap(), "Stipple");
    // The registry sets the application's id.
    let mut body = Body::new();
    body.str("org.a11y.atspi.Application")
        .str("Id")
        .variant(&Value::I32(5));
    ok(
        call(
            &mut b,
            ROOT_PATH,
            "org.freedesktop.DBus.Properties",
            "Set",
            body,
        ),
        "",
    );
    assert_eq!(b.app_id, 5);
}

#[test]
fn nodes_answer_role_name_parent_state_and_children() {
    let mut b = bridge();
    let frame = node_path(0x42, &[]);
    // The frame takes the window's title, as its tree names none.
    let name = prop(&mut b, &frame, "org.a11y.atspi.Accessible", "Name");
    assert_eq!(read_string(&name, &mut 3).unwrap(), "Stipple Touch");
    let role = ok(
        call(
            &mut b,
            &frame,
            "org.a11y.atspi.Accessible",
            "GetRole",
            Body::new(),
        ),
        "u",
    );
    assert_eq!(read_u32(&role, &mut 0).unwrap(), 23);
    let kids = ok(
        call(
            &mut b,
            &frame,
            "org.a11y.atspi.Accessible",
            "GetChildren",
            Body::new(),
        ),
        "a(so)",
    );
    assert_eq!(paths(&kids).len(), 3);

    let button = node_path(0x42, &[2, 0]);
    let role = ok(
        call(
            &mut b,
            &button,
            "org.a11y.atspi.Accessible",
            "GetRole",
            Body::new(),
        ),
        "u",
    );
    assert_eq!(read_u32(&role, &mut 0).unwrap(), 43);
    let parent = prop(&mut b, &button, "org.a11y.atspi.Accessible", "Parent");
    // "(so)" variant: sig, then the struct at 8.
    let mut off = 8;
    assert_eq!(read_string(&parent, &mut off).unwrap(), ":1.7");
    assert_eq!(
        read_string(&parent, &mut off).unwrap(),
        node_path(0x42, &[2])
    );
    let index = ok(
        call(
            &mut b,
            &button,
            "org.a11y.atspi.Accessible",
            "GetIndexInParent",
            Body::new(),
        ),
        "i",
    );
    assert_eq!(read_u32(&index, &mut 0).unwrap(), 0);

    // The focused field's state has FOCUSABLE, FOCUSED, EDITABLE.
    let field = node_path(0x42, &[1]);
    let st = ok(
        call(
            &mut b,
            &field,
            "org.a11y.atspi.Accessible",
            "GetState",
            Body::new(),
        ),
        "au",
    );
    let mut off = 4;
    let lo = read_u32(&st, &mut off).unwrap();
    for bit in [7, 8, 11, 12, 26] {
        assert!(lo & (1 << bit) != 0, "state {bit}");
    }
    // The frame is active.
    let st = ok(
        call(
            &mut b,
            &frame,
            "org.a11y.atspi.Accessible",
            "GetState",
            Body::new(),
        ),
        "au",
    );
    assert!(read_u32(&st, &mut 4).unwrap() & (1 << 1) != 0);

    let ifaces = ok(
        call(
            &mut b,
            &field,
            "org.a11y.atspi.Accessible",
            "GetInterfaces",
            Body::new(),
        ),
        "as",
    );
    let s = String::from_utf8_lossy(&ifaces);
    assert!(s.contains("org.a11y.atspi.Text") && s.contains("org.a11y.atspi.EditableText"));
    assert!(!s.contains("org.a11y.atspi.Action"));

    // A path past the tree is no object.
    let r = call(
        &mut b,
        &node_path(0x42, &[9]),
        "org.a11y.atspi.Accessible",
        "GetRole",
        Body::new(),
    );
    assert_eq!(r, Reply::Error("org.freedesktop.DBus.Error.UnknownObject"));
}

#[test]
fn extents_and_hit_testing_follow_the_window() {
    let mut b = bridge();
    let button = node_path(0x42, &[2, 0]);
    let extents = |b: &mut Bridge, coord: u32| {
        let mut body = Body::new();
        body.u32(coord);
        let r = ok(
            call(b, &button, "org.a11y.atspi.Component", "GetExtents", body),
            "(iiii)",
        );
        let mut a = Args::new(&r);
        [
            a.i32().unwrap(),
            a.i32().unwrap(),
            a.i32().unwrap(),
            a.i32().unwrap(),
        ]
    };
    // Logical (20, 80, 60, 30) at scale 2, the window's content at (100, 50).
    assert_eq!(extents(&mut b, 0), [140, 210, 120, 60]);
    assert_eq!(extents(&mut b, 1), [40, 160, 120, 60]);
    // Relative to its group at (20, 140) in window pixels.
    assert_eq!(extents(&mut b, 2), [20, 20, 120, 60]);

    // The frame's hit test at the button's middle (screen) finds it.
    let mut body = Body::new();
    body.i32(200).i32(240).u32(0);
    let r = ok(
        call(
            &mut b,
            &node_path(0x42, &[]),
            "org.a11y.atspi.Component",
            "GetAccessibleAtPoint",
            body,
        ),
        "(so)",
    );
    let mut off = 0;
    read_string(&r, &mut off).unwrap();
    assert_eq!(read_string(&r, &mut off).unwrap(), button);
    let mut body = Body::new();
    body.i32(5).i32(5).u32(1);
    let r = ok(
        call(
            &mut b,
            &button,
            "org.a11y.atspi.Component",
            "Contains",
            body,
        ),
        "b",
    );
    assert_eq!(read_u32(&r, &mut 0).unwrap(), 0);
}

#[test]
fn an_action_clicks_the_middle_of_the_node_on_the_ui_thread() {
    let mut b = bridge();
    let button = node_path(0x42, &[2, 0]);
    let n = prop(&mut b, &button, "org.a11y.atspi.Action", "NActions");
    assert_eq!(read_u32(&n, &mut 0).map(|_| ()).ok(), Some(()));
    let name = {
        let mut body = Body::new();
        body.i32(0);
        ok(
            call(&mut b, &button, "org.a11y.atspi.Action", "GetName", body),
            "s",
        )
    };
    assert_eq!(read_string(&name, &mut 0).unwrap(), "click");
    let mut body = Body::new();
    body.i32(0);
    let r = ok(
        call(&mut b, &button, "org.a11y.atspi.Action", "DoAction", body),
        "b",
    );
    assert_eq!(read_u32(&r, &mut 0).unwrap(), 1);
    assert_eq!(b.actions.len(), 1);
    let (win, events) = &b.actions[0];
    assert_eq!(*win, 0x42);
    let at = Point::new(50.0, 95.0);
    assert_eq!(
        events,
        &vec![
            Event::PointerMoved { position: at },
            Event::PointerButton {
                button: PointerButton::Left,
                state: ButtonState::Pressed,
                position: at
            },
            Event::PointerButton {
                button: PointerButton::Left,
                state: ButtonState::Released,
                position: at
            },
        ]
    );
    // A label has no action.
    let mut body = Body::new();
    body.i32(0);
    let r = call(
        &mut b,
        &node_path(0x42, &[0]),
        "org.a11y.atspi.Action",
        "DoAction",
        body,
    );
    assert!(matches!(r, Reply::Error(_)));
}

#[test]
fn text_and_editable_text() {
    let mut b = bridge();
    let label = node_path(0x42, &[0]);
    let count = prop(&mut b, &label, "org.a11y.atspi.Text", "CharacterCount");
    assert_eq!(read_u32(&count, &mut 0).map(|_| ()), Ok(()));
    let mut off = 3;
    assert_eq!(read_u32(&count, &mut off).unwrap(), 14);
    let mut body = Body::new();
    body.i32(6).u32(1);
    let r = ok(
        call(
            &mut b,
            &label,
            "org.a11y.atspi.Text",
            "GetTextAtOffset",
            body,
        ),
        "sii",
    );
    let mut a = Args::new(&r);
    assert_eq!(a.str().as_deref(), Some("tapped: "));
    assert_eq!((a.i32(), a.i32()), (Some(5), Some(13)));
    let mut body = Body::new();
    body.i32(0).i32(-1);
    let r = ok(
        call(&mut b, &label, "org.a11y.atspi.Text", "GetText", body),
        "s",
    );
    assert_eq!(read_string(&r, &mut 0).unwrap(), "Rows tapped: 0");

    // The focused field's caret is at its end; inserting types into it.
    let field = node_path(0x42, &[1]);
    let caret = prop(&mut b, &field, "org.a11y.atspi.Text", "CaretOffset");
    assert_eq!(read_u32(&caret, &mut 3).unwrap(), 2);
    let mut body = Body::new();
    body.i32(1).str("ey").i32(2);
    let r = ok(
        call(
            &mut b,
            &field,
            "org.a11y.atspi.EditableText",
            "InsertText",
            body,
        ),
        "b",
    );
    assert_eq!(read_u32(&r, &mut 0).unwrap(), 1);
    assert_eq!(
        b.actions[0].1,
        vec![
            Event::Ime(Ime::DeleteSurrounding {
                before: 1 << 30,
                after: 1 << 30
            }),
            Event::Ime(Ime::Commit("heyi".into())),
        ]
    );
}

#[test]
fn a_swap_raises_name_children_and_focus_events() {
    let old = tree();
    let mut new = tree();
    new.children[0].name = "Rows tapped: 1".into();
    new.children[1].focused = false;
    new.children[2].children[0].focused = true;
    new.children[2]
        .children
        .push(leaf(A11yRole::Button, "Row 1", (0.0, 0.0, 1.0, 1.0)));
    let got = events::diff(1, &old, &new);
    let summary: Vec<String> = got
        .iter()
        .map(|s| {
            format!(
                "{}:{}:{} {} {}",
                s.member, s.kind, s.detail1, s.detail2, s.path
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            "PropertyChange:accessible-name:0 0 /org/a11y/atspi/accessible/w1_0",
            "TextChanged:delete:13 1 /org/a11y/atspi/accessible/w1_0",
            "TextChanged:insert:13 1 /org/a11y/atspi/accessible/w1_0",
            "ChildrenChanged:add:1 0 /org/a11y/atspi/accessible/w1_2",
            "StateChanged:focused:0 0 /org/a11y/atspi/accessible/w1_1",
            "StateChanged:focused:1 0 /org/a11y/atspi/accessible/w1_2_0",
            "Focus::0 0 /org/a11y/atspi/accessible/w1_2_0",
        ]
    );
    assert_eq!(
        got[3].any,
        Value::Obj(String::new(), "/org/a11y/atspi/accessible/w1_2_1".into())
    );
    // Nothing changed, nothing raised.
    assert!(events::diff(1, &old, &tree()).is_empty());
}

#[test]
fn actions_reach_the_window_on_a_wake() {
    // The handler wrapper delivers a queued action's events to its window
    // before the wake itself.
    #[derive(Debug)]
    struct W;
    impl Window for W {
        fn id(&self) -> WindowId {
            WindowId(0x5eed)
        }
        fn inner_size(&self) -> crate::geometry::PhysicalSize {
            crate::geometry::PhysicalSize::new(1, 1)
        }
        fn scale_factor(&self) -> crate::geometry::ScaleFactor {
            crate::geometry::ScaleFactor::IDENTITY
        }
        fn request_redraw(&self) {}
        fn set_title(&self, _: &str) {}
        fn create_surface(&self) -> Box<dyn crate::render::Surface> {
            unimplemented!()
        }
    }
    let window = W;
    let id = window.id().0;
    let click = Event::PointerMoved {
        position: Point::new(1.0, 2.0),
    };
    lock().actions.push((id, vec![click.clone()]));
    lock().actions.push((id ^ 1, vec![Event::PointerLeft]));
    let mut seen = Vec::new();
    let mut handler = wrap(|e: Event, _w: &dyn Window| {
        seen.push(e);
        ControlFlow::Wait
    });
    handler(Event::Wake, &window);
    drop(handler);
    assert_eq!(seen, vec![click, Event::Wake]);
    // The other window's action waits for it.
    assert_eq!(take_actions(id ^ 1), vec![Event::PointerLeft]);
}

/// Window > [Text "Name" (id "name"), TextField "héllo" (labelled by "name",
/// caret before "l", "él" selected), Button "OK"].
fn form() -> Bridge {
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    let w = b.win_mut(9);
    w.root = Some(Arc::new(A11yNode {
        children: vec![
            A11yNode {
                id: Some("name".into()),
                ..leaf(A11yRole::Text, "Name", (0.0, 0.0, 40.0, 20.0))
            },
            A11yNode {
                caret: Some(3),
                text_selection: Some((1, 3)),
                labelled_by: Some("name".into()),
                ..leaf(A11yRole::TextField, "héllo", (50.0, 0.0, 100.0, 20.0))
            },
            leaf(A11yRole::Button, "OK", (0.0, 30.0, 40.0, 20.0)),
        ],
        ..leaf(A11yRole::Window, "Form", (0.0, 0.0, 200.0, 60.0))
    }));
    b
}

fn text_call(b: &mut Bridge, path: &str, member: &str, body: Body) -> Reply {
    call(b, path, "org.a11y.atspi.Text", member, body)
}

fn boolean(r: Reply) -> bool {
    read_u32(&ok(r, "b"), &mut 0).unwrap() == 1
}

#[test]
fn caret_and_selection_read_and_set() {
    let mut b = form();
    let field = node_path(9, &[1]);
    // Character offsets, from the tree's byte indices.
    let caret = prop(&mut b, &field, "org.a11y.atspi.Text", "CaretOffset");
    assert_eq!(read_u32(&caret, &mut 3).unwrap(), 2);
    let n = ok(
        text_call(&mut b, &field, "GetNSelections", Body::new()),
        "i",
    );
    assert_eq!(read_u32(&n, &mut 0).unwrap(), 1);
    let mut body = Body::new();
    body.i32(0);
    let sel = ok(text_call(&mut b, &field, "GetSelection", body), "ii");
    let mut a = Args::new(&sel);
    assert_eq!((a.i32(), a.i32()), (Some(1), Some(2)));
    let mut body = Body::new();
    body.i32(1);
    let sel = ok(text_call(&mut b, &field, "GetSelection", body), "ii");
    let mut a = Args::new(&sel);
    assert_eq!((a.i32(), a.i32()), (Some(0), Some(0)));

    // Only one selection: adding another is refused.
    let mut body = Body::new();
    body.i32(0).i32(1);
    assert!(!boolean(text_call(&mut b, &field, "AddSelection", body)));
    assert!(b.actions.is_empty());

    // The field isn't focused: a click focuses it, then the field selects
    // (byte indices into its text).
    let mut body = Body::new();
    body.i32(0).i32(1).i32(4);
    assert!(boolean(text_call(&mut b, &field, "SetSelection", body)));
    let (win, events) = b.actions.pop().unwrap();
    assert_eq!(win, 9);
    assert_eq!(events.len(), 4);
    assert!(matches!(events[0], Event::PointerMoved { .. }));
    assert_eq!(
        events[3],
        Event::Ime(Ime::SetSelection {
            anchor: 1,
            caret: 5
        })
    );

    let mut body = Body::new();
    body.i32(5);
    assert!(boolean(text_call(&mut b, &field, "SetCaretOffset", body)));
    assert_eq!(
        b.actions.pop().unwrap().1.last(),
        Some(&Event::Ime(Ime::SetSelection {
            anchor: 6,
            caret: 6
        }))
    );
    // Removing the selection leaves the caret where it is.
    let mut body = Body::new();
    body.i32(0);
    assert!(boolean(text_call(&mut b, &field, "RemoveSelection", body)));
    assert_eq!(
        b.actions.pop().unwrap().1.last(),
        Some(&Event::Ime(Ime::SetSelection {
            anchor: 3,
            caret: 3
        }))
    );

    // A label has no caret to move.
    let label = node_path(9, &[0]);
    let mut body = Body::new();
    body.i32(1);
    assert!(!boolean(text_call(&mut b, &label, "SetCaretOffset", body)));
    let n = ok(
        text_call(&mut b, &label, "GetNSelections", Body::new()),
        "i",
    );
    assert_eq!(read_u32(&n, &mut 0).unwrap(), 0);
    assert!(b.actions.is_empty());
}

#[test]
fn a_focused_field_selects_without_a_click() {
    let mut b = form();
    let w = b.win_mut(9);
    let mut root = (**w.root.as_ref().unwrap()).clone();
    root.children[1].focused = true;
    root.children[1].text_selection = None;
    w.root = Some(Arc::new(root));
    let field = node_path(9, &[1]);
    let mut body = Body::new();
    body.i32(0).i32(5);
    assert!(boolean(text_call(&mut b, &field, "AddSelection", body)));
    assert_eq!(
        b.actions.pop().unwrap().1,
        vec![Event::Ime(Ime::SetSelection {
            anchor: 0,
            caret: 6
        })]
    );
    // Nothing selected: nothing to remove.
    let mut body = Body::new();
    body.i32(0);
    assert!(!boolean(text_call(&mut b, &field, "RemoveSelection", body)));
}

/// An `a(ua(so))`: each relation's type and target paths.
fn relation_set(body: &[u8]) -> Vec<(u32, Vec<String>)> {
    let mut off = 0;
    let len = read_u32(body, &mut off).unwrap() as usize;
    crate::platform::a11y::align_to(&mut off, 8);
    let end = off + len;
    let mut out = Vec::new();
    while off < end {
        crate::platform::a11y::align_to(&mut off, 8);
        let kind = read_u32(body, &mut off).unwrap();
        let inner = read_u32(body, &mut off).unwrap() as usize;
        crate::platform::a11y::align_to(&mut off, 8);
        let stop = off + inner;
        let mut targets = Vec::new();
        while off < stop {
            crate::platform::a11y::align_to(&mut off, 8);
            let _bus = read_string(body, &mut off).unwrap();
            targets.push(read_string(body, &mut off).unwrap());
        }
        out.push((kind, targets));
    }
    out
}

#[test]
fn relations_link_a_field_and_its_label() {
    let mut b = form();
    let mut relations = |path: &str| {
        relation_set(&ok(
            call(
                &mut b,
                path,
                "org.a11y.atspi.Accessible",
                "GetRelationSet",
                Body::new(),
            ),
            "a(ua(so))",
        ))
    };
    // LABELLED_BY (2) and LABEL_FOR (1).
    assert_eq!(
        relations(&node_path(9, &[1])),
        vec![(2, vec![node_path(9, &[0])])]
    );
    assert_eq!(
        relations(&node_path(9, &[0])),
        vec![(1, vec![node_path(9, &[1])])]
    );
    assert_eq!(relations(&node_path(9, &[2])), vec![]);
    assert_eq!(relations(&node_path(9, &[])), vec![]);
}

#[test]
fn a_swap_raises_caret_and_selection_events() {
    let b = form();
    let old = (**b.windows[0].root.as_ref().unwrap()).clone();
    let mut new = old.clone();
    new.children[1].caret = Some(6);
    new.children[1].text_selection = None;
    let got = events::diff(9, &old, &new);
    let summary: Vec<String> = got
        .iter()
        .map(|s| format!("{}:{} {}", s.member, s.detail1, s.path))
        .collect();
    assert_eq!(
        summary,
        vec![
            "TextCaretMoved:5 /org/a11y/atspi/accessible/w9_1",
            "TextSelectionChanged:0 /org/a11y/atspi/accessible/w9_1",
        ]
    );
    // Typing: the text changes, then the caret moves after it.
    let mut typed = new.clone();
    typed.children[1].name = "héllo!".into();
    typed.children[1].caret = Some(7);
    let got = events::diff(9, &new, &typed);
    let members: Vec<&str> = got.iter().map(|s| s.member).collect();
    assert_eq!(
        members,
        vec!["PropertyChange", "TextChanged", "TextCaretMoved"]
    );
    assert_eq!(got[2].detail1, 6);
}

/// Window > [Group (a scroll container, scrolled 40 of 300) > [Button "Row 0"
/// (above the viewport), Button "Row 5" (in view), Button "Row 9" (below)]].
fn scroll_tree() -> A11yNode {
    use crate::platform::access::A11yScroll;
    let row = |name: &str, y: f64| leaf(A11yRole::Button, name, (0.0, y, 100.0, 20.0));
    A11yNode {
        children: vec![A11yNode {
            scroll: Some(A11yScroll {
                offset: 40.0,
                max: 300.0,
                viewport: 100.0,
                horizontal: false,
                rtl: false,
            }),
            children: vec![row("Row 0", -30.0), row("Row 5", 40.0), row("Row 9", 150.0)],
            ..leaf(A11yRole::Group, "", (0.0, 0.0, 100.0, 100.0))
        }],
        ..leaf(A11yRole::Window, "Scroll", (0.0, 0.0, 100.0, 200.0))
    }
}

fn scroller() -> Bridge {
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    b.win_mut(3).root = Some(Arc::new(scroll_tree()));
    b
}

/// The wheel delta of the last action's events.
fn last_wheel(b: &mut Bridge) -> (f64, f64) {
    match b.actions.pop().and_then(|(_, e)| e.last().cloned()) {
        Some(Event::Scroll { delta }) => (delta.dx, delta.dy),
        other => panic!("no wheel: {other:?}"),
    }
}

fn f64_prop(b: &mut Bridge, path: &str, iface: &str, name: &str) -> f64 {
    let v = prop(b, path, iface, name);
    // A "d" variant: its signature (length, "d", NUL), padded to 8.
    f64::from_le_bytes(v[8..16].try_into().unwrap())
}

#[test]
fn a_scroll_container_is_a_scroll_pane_with_scroll_actions_and_a_value() {
    let mut b = scroller();
    let pane = node_path(3, &[0]);
    let accessible = "org.a11y.atspi.Accessible";
    let role = ok(call(&mut b, &pane, accessible, "GetRole", Body::new()), "u");
    assert_eq!(read_u32(&role, &mut 0).unwrap(), 49, "SCROLL_PANE");
    let ifaces = ok(
        call(&mut b, &pane, accessible, "GetInterfaces", Body::new()),
        "as",
    );
    let has = |name: &str| ifaces.windows(name.len()).any(|w| w == name.as_bytes());
    assert!(has("org.a11y.atspi.Action") && has("org.a11y.atspi.Value"));
    // The value is the offset, within 0..max.
    let value = "org.a11y.atspi.Value";
    assert_eq!(f64_prop(&mut b, &pane, value, "CurrentValue"), 40.0);
    assert_eq!(f64_prop(&mut b, &pane, value, "MaximumValue"), 300.0);
    assert_eq!(f64_prop(&mut b, &pane, value, "MinimumValue"), 0.0);
    // Two actions, a page forward and back, each a wheel at its middle.
    let n = prop(&mut b, &pane, "org.a11y.atspi.Action", "NActions");
    assert_eq!(read_u32(&n, &mut 4).unwrap(), 2);
    for (i, dy) in [(0, 80.0), (1, -40.0)] {
        let mut body = Body::new();
        body.i32(i);
        let r = call(&mut b, &pane, "org.a11y.atspi.Action", "DoAction", body);
        assert!(boolean(r));
        assert_eq!(last_wheel(&mut b), (0.0, dy));
    }
    // Setting the value scrolls there.
    let mut body = Body::new();
    body.str(value)
        .str("CurrentValue")
        .variant(&Value::F64(250.0));
    let props = "org.freedesktop.DBus.Properties";
    ok(call(&mut b, &pane, props, "Set", body), "");
    assert_eq!(last_wheel(&mut b), (0.0, 210.0));
}

#[test]
fn scroll_to_brings_a_node_into_view() {
    let mut b = scroller();
    let scroll_to = |b: &mut Bridge, chain: &[usize]| {
        let mut body = Body::new();
        body.u32(6); // ATSPI_SCROLL_ANYWHERE
        let path = node_path(3, chain);
        boolean(call(b, &path, "org.a11y.atspi.Component", "ScrollTo", body))
    };
    assert!(scroll_to(&mut b, &[0, 0]));
    assert_eq!(last_wheel(&mut b), (0.0, -30.0));
    assert!(scroll_to(&mut b, &[0, 1]), "in view: nothing to do");
    assert!(b.actions.is_empty());
    assert!(scroll_to(&mut b, &[0, 2]));
    assert_eq!(last_wheel(&mut b), (0.0, 70.0));
    // Not in a scroll container.
    assert!(!scroll_to(&mut b, &[0]));
}

#[test]
fn a_toast_appearing_is_announced_and_a_scroll_changes_the_value() {
    let old = tree();
    let mut new = tree();
    new.children.push(leaf(
        A11yRole::Alert,
        "Row 2 tapped",
        (0.0, 0.0, 10.0, 10.0),
    ));
    let got = events::diff(1, &old, &new);
    let announce = got
        .iter()
        .find(|s| s.member == "Announcement")
        .expect("announced");
    assert_eq!(announce.path, "/org/a11y/atspi/accessible/w1_3");
    assert_eq!(announce.detail1, 1, "polite");
    assert_eq!(announce.any, Value::Str("Row 2 tapped".into()));
    // Still there next time: not announced again.
    assert!(events::diff(1, &new, &new).is_empty());

    let before = scroll_tree();
    let mut after = scroll_tree();
    after.children[0].scroll.as_mut().unwrap().offset = 120.0;
    let got = events::diff(3, &before, &after);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, "accessible-value");
    assert_eq!(got[0].any, Value::F64(120.0));
}

#[test]
fn a_scroll_bar_is_a_scroll_bar_with_its_orientation_and_a_value() {
    use crate::platform::access::A11yScroll;
    let mut tree = scroll_tree();
    let bar = A11yNode {
        scroll_bar: Some(A11yScroll {
            offset: 40.0,
            max: 300.0,
            viewport: 100.0,
            horizontal: false,
            rtl: false,
        }),
        ..leaf(A11yRole::ScrollBar, "", (88.0, 0.0, 12.0, 100.0))
    };
    tree.children[0].children.push(bar);
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    b.win_mut(3).root = Some(Arc::new(tree));
    let path = node_path(3, &[0, 3]);
    let accessible = "org.a11y.atspi.Accessible";
    let role = ok(call(&mut b, &path, accessible, "GetRole", Body::new()), "u");
    assert_eq!(read_u32(&role, &mut 0).unwrap(), 48, "SCROLL_BAR");
    let st = ok(
        call(&mut b, &path, accessible, "GetState", Body::new()),
        "au",
    );
    let lo = read_u32(&st, &mut 4).unwrap();
    assert!(lo & (1 << 29) != 0, "VERTICAL");
    assert!(lo & (1 << 14) == 0, "not HORIZONTAL");
    let value = "org.a11y.atspi.Value";
    assert_eq!(f64_prop(&mut b, &path, value, "CurrentValue"), 40.0);
    assert_eq!(f64_prop(&mut b, &path, value, "MaximumValue"), 300.0);
    assert_eq!(f64_prop(&mut b, &path, value, "MinimumValue"), 0.0);
    // Setting it scrolls its container: a wheel at the bar.
    let mut body = Body::new();
    body.str(value)
        .str("CurrentValue")
        .variant(&Value::F64(100.0));
    ok(
        call(
            &mut b,
            &path,
            "org.freedesktop.DBus.Properties",
            "Set",
            body,
        ),
        "",
    );
    assert_eq!(last_wheel(&mut b), (0.0, 60.0));
}

#[test]
fn relations_name_controls_groups_and_tree_parents() {
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    let item = |name: &str, level| A11yNode {
        level: Some(level),
        ..leaf(A11yRole::TreeItem, name, (0.0, 0.0, 10.0, 10.0))
    };
    let radio = |name: &str| leaf(A11yRole::RadioButton, name, (0.0, 0.0, 10.0, 10.0));
    b.win_mut(9).root = Some(Arc::new(A11yNode {
        children: vec![
            A11yNode {
                controls: Some("list".into()),
                ..leaf(A11yRole::ComboBox, "Apple", (0.0, 0.0, 40.0, 20.0))
            },
            A11yNode {
                id: Some("list".into()),
                ..leaf(A11yRole::List, "", (0.0, 20.0, 40.0, 40.0))
            },
            A11yNode {
                children: vec![radio("A"), radio("B")],
                ..leaf(A11yRole::Group, "", (0.0, 60.0, 40.0, 20.0))
            },
            A11yNode {
                children: vec![item("root", 1), item("child", 2)],
                ..leaf(A11yRole::Tree, "", (0.0, 80.0, 40.0, 20.0))
            },
        ],
        ..leaf(A11yRole::Window, "Form", (0.0, 0.0, 200.0, 100.0))
    }));
    let mut relations = |path: &[usize]| {
        relation_set(&ok(
            call(
                &mut b,
                &node_path(9, path),
                "org.a11y.atspi.Accessible",
                "GetRelationSet",
                Body::new(),
            ),
            "a(ua(so))",
        ))
    };
    // CONTROLLER_FOR (3) / CONTROLLED_BY (4).
    assert_eq!(relations(&[0]), vec![(3, vec![node_path(9, &[1])])]);
    assert_eq!(relations(&[1]), vec![(4, vec![node_path(9, &[0])])]);
    // MEMBER_OF (5): the whole group.
    assert_eq!(
        relations(&[2, 1]),
        vec![(5, vec![node_path(9, &[2, 0]), node_path(9, &[2, 1])])]
    );
    // NODE_PARENT_OF (8) / NODE_CHILD_OF (7).
    assert_eq!(relations(&[3, 0]), vec![(8, vec![node_path(9, &[3, 1])])]);
    assert_eq!(relations(&[3, 1]), vec![(7, vec![node_path(9, &[3, 0])])]);
    // The child's depth is its `level` attribute.
    let attrs = ok(
        call(
            &mut b,
            &node_path(9, &[3, 1]),
            "org.a11y.atspi.Accessible",
            "GetAttributes",
            Body::new(),
        ),
        "a{ss}",
    );
    let text = String::from_utf8_lossy(&attrs);
    assert!(text.contains("level") && text.contains('2'), "{text:?}");
    // Its place among its level's items: 1 of 1.
    let pairs = str_dict(&attrs);
    assert!(
        pairs.contains(&("posinset".into(), "1".into()))
            && pairs.contains(&("setsize".into(), "1".into())),
        "{pairs:?}"
    );
    // A radio button's in its group: 2 of 2, and no level.
    let attrs = ok(
        call(
            &mut b,
            &node_path(9, &[2, 1]),
            "org.a11y.atspi.Accessible",
            "GetAttributes",
            Body::new(),
        ),
        "a{ss}",
    );
    let pairs = str_dict(&attrs);
    let has = |k: &str, v: &str| pairs.iter().any(|(a, b)| a == k && b == v);
    assert!(has("posinset", "2") && has("setsize", "2"), "{pairs:?}");
    assert!(!pairs.iter().any(|(k, _)| k == "level"), "{pairs:?}");
}

/// An `a{ss}`, as pairs.
fn str_dict(body: &[u8]) -> Vec<(String, String)> {
    let mut off = 0;
    let len = read_u32(body, &mut off).unwrap() as usize;
    crate::platform::a11y::align_to(&mut off, 8);
    let end = off + len;
    let mut out = Vec::new();
    while off < end {
        crate::platform::a11y::align_to(&mut off, 8);
        let k = read_string(body, &mut off).unwrap();
        out.push((k, read_string(body, &mut off).unwrap()));
    }
    out
}

/// Window > [Slider "Volume" (0.5 of 0..1), ProgressBar "Loading",
/// Table (11 rows, 2 columns) > [header Row > [Name, Value], Group > [row 5
/// > [a5, b5]]]].
fn range_tree() -> A11yNode {
    use crate::platform::access::{A11yCell, A11yRange, A11yTable};
    let range = |value| A11yRange {
        value,
        min: 0.0,
        max: 1.0,
        step: 0.05,
        rtl: false,
    };
    let at = |row, column| Some(A11yCell { row, column });
    let cell = |role, name: &str, row, column| A11yNode {
        cell: at(row, column),
        ..leaf(role, name, (0.0, 0.0, 50.0, 20.0))
    };
    A11yNode {
        children: vec![
            A11yNode {
                range: Some(range(0.5)),
                ..leaf(A11yRole::Slider, "Volume", (0.0, 0.0, 200.0, 20.0))
            },
            A11yNode {
                range: Some(range(0.3)),
                ..leaf(A11yRole::ProgressBar, "Loading", (0.0, 30.0, 200.0, 8.0))
            },
            A11yNode {
                table: Some(A11yTable {
                    rows: 11,
                    columns: 2,
                    frozen: 0,
                }),
                selected: false,
                children: vec![
                    A11yNode {
                        cell: at(0, 0),
                        children: vec![
                            cell(A11yRole::ColumnHeader, "Name", 0, 0),
                            cell(A11yRole::ColumnHeader, "Value", 0, 1),
                        ],
                        ..leaf(A11yRole::Row, "", (0.0, 40.0, 100.0, 20.0))
                    },
                    A11yNode {
                        children: vec![A11yNode {
                            cell: at(5, 0),
                            selected: true,
                            children: vec![
                                cell(A11yRole::Cell, "a5", 5, 0),
                                cell(A11yRole::Cell, "b5", 5, 1),
                            ],
                            ..leaf(A11yRole::Row, "", (0.0, 60.0, 100.0, 20.0))
                        }],
                        ..leaf(A11yRole::Group, "", (0.0, 60.0, 100.0, 40.0))
                    },
                ],
                ..leaf(A11yRole::Table, "Data", (0.0, 40.0, 100.0, 60.0))
            },
        ],
        ..leaf(A11yRole::Window, "Ranges", (0.0, 0.0, 300.0, 200.0))
    }
}

fn ranges() -> Bridge {
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    b.win_mut(4).root = Some(Arc::new(range_tree()));
    b
}

fn i32_prop(b: &mut Bridge, path: &str, iface: &str, name: &str) -> i32 {
    let v = prop(b, path, iface, name);
    // An "i" variant: its signature (length, "i", NUL), then the number.
    i32::from_le_bytes(v[4..8].try_into().unwrap())
}

#[test]
fn a_slider_is_a_settable_value_and_a_progress_bar_a_read_one() {
    let mut b = ranges();
    let (slider, bar) = (node_path(4, &[0]), node_path(4, &[1]));
    let value = "org.a11y.atspi.Value";
    assert_eq!(f64_prop(&mut b, &slider, value, "CurrentValue"), 0.5);
    assert_eq!(f64_prop(&mut b, &slider, value, "MaximumValue"), 1.0);
    assert_eq!(f64_prop(&mut b, &slider, value, "MinimumIncrement"), 0.05);
    let text = prop(&mut b, &slider, value, "Text");
    assert!(text.windows(3).any(|w| w == b"50%"));
    let role = ok(
        call(
            &mut b,
            &bar,
            "org.a11y.atspi.Accessible",
            "GetRole",
            Body::new(),
        ),
        "u",
    );
    assert_eq!(read_u32(&role, &mut 0).unwrap(), 42, "PROGRESS_BAR");
    assert_eq!(f64_prop(&mut b, &bar, value, "CurrentValue"), 0.3);
    // Setting the slider's value presses it there.
    let set = |b: &mut Bridge, path: &str, v: f64| {
        let mut body = Body::new();
        body.str(value).str("CurrentValue").variant(&Value::F64(v));
        call(b, path, "org.freedesktop.DBus.Properties", "Set", body)
    };
    ok(set(&mut b, &slider, 0.25), "");
    let (_, events) = b.actions.pop().expect("events for the window");
    assert_eq!(events.len(), 4);
    assert!(matches!(
        events[3],
        Event::PointerButton { position, state: ButtonState::Released, .. }
            if position == Point::new(50.0, 10.0)
    ));
    // The progress bar's can't be set.
    assert!(matches!(set(&mut b, &bar, 0.5), Reply::Error(_)));
    assert!(b.actions.is_empty());

    let before = range_tree();
    let mut after = range_tree();
    after.children[0].range.as_mut().unwrap().value = 0.25;
    let got = events::diff(4, &before, &after);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, "accessible-value");
    assert_eq!(got[0].any, Value::F64(0.25));
}

#[test]
fn a_table_gives_its_cells_by_row_and_column() {
    let mut b = ranges();
    let table = node_path(4, &[2]);
    let t = "org.a11y.atspi.Table";
    let ifaces = ok(
        call(
            &mut b,
            &table,
            "org.a11y.atspi.Accessible",
            "GetInterfaces",
            Body::new(),
        ),
        "as",
    );
    assert!(ifaces.windows(t.len()).any(|w| w == t.as_bytes()));
    assert_eq!(i32_prop(&mut b, &table, t, "NRows"), 11);
    assert_eq!(i32_prop(&mut b, &table, t, "NColumns"), 2);
    let at = |b: &mut Bridge, row: i32, column: i32| {
        let mut body = Body::new();
        body.i32(row).i32(column);
        let r = ok(call(b, &table, t, "GetAccessibleAt", body), "(so)");
        let mut off = 0;
        let _bus = read_string(&r, &mut off).unwrap();
        read_string(&r, &mut off).unwrap()
    };
    assert_eq!(at(&mut b, 5, 1), node_path(4, &[2, 1, 0, 1]));
    assert_eq!(at(&mut b, 0, 0), node_path(4, &[2, 0, 0]), "a header");
    assert_eq!(at(&mut b, 3, 0), NULL_PATH, "a row not built");
    let mut body = Body::new();
    body.i32(1);
    let header = ok(call(&mut b, &table, t, "GetColumnHeader", body), "(so)");
    let mut off = 0;
    let _ = read_string(&header, &mut off);
    assert_eq!(
        read_string(&header, &mut off).unwrap(),
        node_path(4, &[2, 0, 1])
    );
    let mut body = Body::new();
    body.i32(11);
    let index = ok(call(&mut b, &table, t, "GetRowAtIndex", body), "i");
    assert_eq!(read_u32(&index, &mut 0).unwrap(), 5);
    let rows = ok(
        call(&mut b, &table, t, "GetSelectedRows", Body::new()),
        "ai",
    );
    assert_eq!(read_u32(&rows, &mut 0).unwrap(), 4, "one row");
    assert_eq!(read_u32(&rows, &mut 4).unwrap(), 5, "row 5");

    // A cell: its position, its table, its column's header.
    let cell = node_path(4, &[2, 1, 0, 1]);
    let tc = "org.a11y.atspi.TableCell";
    let pos = prop(&mut b, &cell, tc, "Position");
    // "(ii)" variant: signature (length, "(ii)", NUL) padded to 8, then two.
    assert_eq!(read_u32(&pos, &mut 8).unwrap(), 5);
    assert_eq!(read_u32(&pos, &mut 12).unwrap(), 1);
    let parent = prop(&mut b, &cell, tc, "Table");
    assert!(parent.windows(table.len()).any(|w| w == table.as_bytes()));
    let span = ok(
        call(&mut b, &cell, tc, "GetRowColumnSpan", Body::new()),
        "iiii",
    );
    assert_eq!(read_u32(&span, &mut 0).unwrap(), 5);
    let heads = ok(
        call(&mut b, &cell, tc, "GetColumnHeaderCells", Body::new()),
        "a(so)",
    );
    assert_eq!(paths(&heads), [node_path(4, &[2, 0, 1])]);
}

/// An `a{ss}ii` reply: the attributes, then the run's start and end.
fn dict_ii(body: &[u8]) -> (Vec<(String, String)>, i32, i32) {
    let mut off = 0;
    let len = read_u32(body, &mut off).unwrap() as usize;
    while !off.is_multiple_of(8) {
        off += 1;
    }
    let end = off + len;
    let mut out = Vec::new();
    while off < end {
        while !off.is_multiple_of(8) {
            off += 1;
        }
        let k = read_string(body, &mut off).unwrap();
        let v = read_string(body, &mut off).unwrap();
        out.push((k, v));
    }
    let s = read_u32(body, &mut off).unwrap() as i32;
    let e = read_u32(body, &mut off).unwrap() as i32;
    (out, s, e)
}

#[test]
fn text_attributes_follow_the_runs() {
    use crate::platform::access::{A11yFont, A11yRuns, A11yTextRun};
    let font = |weight| A11yFont {
        family: Some("Inter".into()),
        size: 16.0,
        weight,
        italic: false,
    };
    let mut b = Bridge::new();
    b.bus = ":1.7".into();
    b.win_mut(9).root = Some(Arc::new(A11yNode {
        children: vec![A11yNode {
            font: Some(font(400)),
            runs: A11yRuns::fixed(vec![
                A11yTextRun::plain((0, 4), font(400)),
                A11yTextRun {
                    language: Some("fr".into()),
                    ..A11yTextRun::plain((4, 8), font(700))
                },
            ]),
            ..leaf(A11yRole::Text, "é: bold", (0.0, 0.0, 70.0, 20.0))
        }],
        ..leaf(A11yRole::Window, "Form", (0.0, 0.0, 200.0, 60.0))
    }));
    let text = node_path(9, &[0]);
    // "é: " is three characters (four bytes): the bold run is 3..7.
    let mut body = Body::new();
    body.i32(4).bool(false);
    let (attrs, s, e) = dict_ii(&ok(
        text_call(&mut b, &text, "GetAttributeRun", body),
        "a{ss}ii",
    ));
    assert_eq!((s, e), (3, 7));
    assert_eq!(
        attrs,
        vec![
            ("weight".to_string(), "700".to_string()),
            ("language".to_string(), "fr".to_string())
        ]
    );
    let mut body = Body::new();
    body.i32(4).bool(true);
    let (attrs, _, _) = dict_ii(&ok(
        text_call(&mut b, &text, "GetAttributeRun", body),
        "a{ss}ii",
    ));
    assert!(
        attrs.contains(&("family-name".into(), "Inter".into())),
        "{attrs:?}"
    );
    let mut body = Body::new();
    body.i32(1);
    let (attrs, s, e) = dict_ii(&ok(
        text_call(&mut b, &text, "GetAttributes", body),
        "a{ss}ii",
    ));
    assert_eq!((attrs.len(), s, e), (0, 0, 3));
    let mut body = Body::new();
    body.i32(5).str("weight");
    let v = ok(text_call(&mut b, &text, "GetAttributeValue", body), "s");
    assert_eq!(read_string(&v, &mut 0).unwrap(), "700");
    let d = ok(
        text_call(&mut b, &text, "GetDefaultAttributes", Body::new()),
        "a{ss}",
    );
    let d = String::from_utf8_lossy(&d);
    assert!(d.contains("family-name") && d.contains("400"), "{d:?}");
}
