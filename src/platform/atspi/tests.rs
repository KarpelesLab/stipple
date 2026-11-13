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
