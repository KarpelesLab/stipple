//! Relations through UI Automation, from `platform::access::relations`:
//! `ControllerFor` (the elements a combo box or tab controls: its open
//! list, its panel) and a tree item's `Level` (UIA works a tree item's
//! level out from where it sits, but a flat tree's items are siblings),
//! and an item's `PositionInSet` / `SizeOfSet` (Windows 10 1703), from
//! `access::position`: UIA would count the siblings of the same control
//! type itself, but a virtualized list's built rows aren't all of them, nor
//! a flat tree's siblings one level's items.
//!
//! And acting on a node scrolled out of view: the click of an Invoke,
//! Toggle, SelectionItem or ExpandCollapse comes after the scroll that
//! brings it into view (`access::reveal`).

use super::*;
use crate::platform::event::Event;

const UIA_ControllerForPropertyId: i32 = 30104;
const UIA_PositionInSetPropertyId: i32 = 30152;
const UIA_SizeOfSetPropertyId: i32 = 30153;
const UIA_LevelPropertyId: i32 = 30154;
const VT_ARRAY: u16 = 0x2000;

/// The provider at `path` under `p`'s root, or null.
unsafe fn provider_at_path(p: *const Provider, path: &[usize]) -> *mut Provider {
    unsafe {
        let mut at = (*p).root;
        for &i in path {
            if at.is_null() {
                break;
            }
            let children: &[*mut Provider] = &(*at).children;
            at = children.get(i).copied().unwrap_or(core::ptr::null_mut());
        }
        at
    }
}

/// The relation properties of `p` into `v`; whether `id` is one of them.
pub(super) unsafe fn property(p: *const Provider, id: i32, v: &mut Variant) -> bool {
    unsafe {
        let Some(root) = (*(*p).root).source.as_deref() else {
            return false;
        };
        let path = &(*p).path;
        match id {
            UIA_ControllerForPropertyId => {
                let targets: Vec<*mut Provider> = root
                    .relations(path)
                    .controller_for
                    .iter()
                    .map(|t| provider_at_path(p, t))
                    .filter(|t| !t.is_null())
                    .collect();
                if targets.is_empty() {
                    return false;
                }
                let sa = patterns::provider_array(&targets);
                if sa.is_null() {
                    return false;
                }
                v.vt = VT_ARRAY | VT_UNKNOWN;
                v.val = sa as u64;
                true
            }
            UIA_PositionInSetPropertyId | UIA_SizeOfSetPropertyId => {
                match root.set_position(path) {
                    Some(at) => {
                        let n = if id == UIA_PositionInSetPropertyId {
                            at.position
                        } else {
                            at.size
                        };
                        v.vt = VT_I4;
                        v.val = n as u32 as u64;
                        true
                    }
                    None => false,
                }
            }
            UIA_LevelPropertyId => match root.tree_level(path) {
                Some(level) => {
                    v.vt = VT_I4;
                    v.val = level as u32 as u64;
                    true
                }
                None => false,
            },
            _ => false,
        }
    }
}

/// The events of a click on `p`'s node, brought into view first in the
/// scroll containers around it.
pub(super) unsafe fn activate_events(p: *const Provider) -> Option<Vec<Event>> {
    unsafe {
        let root = (*(*p).root).source.as_deref()?;
        root.activate_events(&(*p).path)
    }
}

/// Whether `p`'s node needs scrolling into view before a click lands on it.
pub(super) unsafe fn needs_reveal(p: *const Provider) -> bool {
    unsafe {
        (*(*p).root)
            .source
            .as_deref()
            .and_then(|root| root.scroll_into_view_events(&(*p).path))
            .is_some()
    }
}

/// Check the relation properties and the scrolled activation through the
/// providers' COM interfaces (`uiademo --patterns` runs it after the
/// patterns' self-test).
pub fn selftest_relations() -> Result<(), String> {
    selftest(false)
}

/// The relations' self-test, then items' `PositionInSet` / `SizeOfSet`
/// (`uiademo --positions`).
pub fn selftest_positions() -> Result<(), String> {
    selftest(true)
}

fn selftest(positions: bool) -> Result<(), String> {
    use crate::platform::access::A11yScroll;
    use crate::platform::event::ButtonState;
    fn node(role: A11yRole, name: &str, bounds: (f64, f64, f64, f64)) -> A11yNode {
        A11yNode::new(role, name, bounds)
    }
    let item = |name: &str, level, y| A11yNode {
        level: Some(level),
        ..node(A11yRole::TreeItem, name, (0.0, y, 200.0, 20.0))
    };
    let row = |i: usize| {
        node(
            A11yRole::Button,
            &format!("Row {i}"),
            (0.0, 140.0 + 40.0 * i as f64, 200.0, 40.0),
        )
    };
    let root = A11yNode {
        children: vec![
            A11yNode {
                controls: Some("fruits".into()),
                ..node(A11yRole::ComboBox, "Apple", (0.0, 0.0, 100.0, 20.0))
            },
            A11yNode {
                id: Some("fruits".into()),
                children: vec![
                    node(A11yRole::ListItem, "Pear", (0.0, 20.0, 100.0, 20.0)),
                    node(A11yRole::ListItem, "Plum", (0.0, 40.0, 100.0, 20.0)),
                ],
                ..node(A11yRole::List, "", (0.0, 20.0, 100.0, 40.0))
            },
            A11yNode {
                children: vec![item("root", 1, 60.0), item("child", 2, 80.0)],
                ..node(A11yRole::Tree, "", (0.0, 60.0, 200.0, 40.0))
            },
            A11yNode {
                scroll: Some(A11yScroll {
                    offset: 0.0,
                    max: 600.0,
                    viewport: 100.0,
                    horizontal: false,
                    rtl: false,
                }),
                children: (0..10).map(row).collect(),
                ..node(A11yRole::Group, "", (0.0, 140.0, 200.0, 100.0))
            },
        ],
        ..node(A11yRole::Window, "Relations", (0.0, 0.0, 400.0, 300.0))
    };
    let tree = UiaTree::build(&root);
    let check = |ok: bool, what: &str| -> Result<(), String> {
        if ok {
            println!("UIA relations: ok: {what}");
            Ok(())
        } else {
            Err(format!("UIA relations: {what}"))
        }
    };
    patterns::take_recorded();
    unsafe {
        let frag = query_through(tree.root_provider(), &IID_IRawElementProviderFragment);
        let find = |ty: i32, name: &str| {
            let f = patterns::find_through(frag, ty, name);
            if f.is_null() {
                Err(format!("no element {ty} {name:?}"))
            } else {
                Ok(f)
            }
        };
        let prop = |f: *mut c_void, id: i32| {
            let simple = query_through(f, &IID_IRawElementProviderSimple);
            let vt = &**(simple as *mut *const ProviderVtbl);
            let mut v = Variant::empty();
            (vt.get_property_value)(simple, id, &mut v);
            release_through(simple);
            v
        };

        // The combo box is ControllerFor its list.
        let combo = find(UIA_ComboBoxControlTypeId, "Apple")?;
        let v = prop(combo, UIA_ControllerForPropertyId);
        check(
            v.vt == VT_ARRAY | VT_UNKNOWN,
            "ControllerFor is an array of elements",
        )?;
        let sa = v.val as *mut c_void;
        let mut upper = -1;
        SafeArrayGetUBound(sa, 1, &mut upper);
        check(upper == 0, "...of one element")?;
        let mut target: *mut c_void = core::ptr::null_mut();
        SafeArrayGetElement(sa, &0, &mut target as *mut *mut c_void as *mut c_void);
        SafeArrayDestroy(sa);
        check(
            !target.is_null()
                && int_through(target, UIA_ControlTypePropertyId) == UIA_ListControlTypeId,
            "...the list",
        )?;
        if !target.is_null() {
            release_through(target);
        }
        let list = find(UIA_ListControlTypeId, "")?;
        check(
            prop(list, UIA_ControllerForPropertyId).vt == VT_EMPTY,
            "the list controls nothing",
        )?;

        // A flat tree's items say their level.
        let child = find(UIA_TreeItemControlTypeId, "child")?;
        let v = prop(child, UIA_LevelPropertyId);
        check(
            v.vt == VT_I4 && v.val as i32 == 2,
            "the child item's Level is 2",
        )?;

        // Items say their place in their set.
        if positions {
            let plum = find(UIA_ListItemControlTypeId, "Plum")?;
            let (at, of) = (
                prop(plum, UIA_PositionInSetPropertyId),
                prop(plum, UIA_SizeOfSetPropertyId),
            );
            check(
                at.vt == VT_I4 && at.val as i32 == 2 && of.vt == VT_I4 && of.val as i32 == 2,
                "'Plum' is item 2 of 2 (PositionInSet, SizeOfSet)",
            )?;
            let v = prop(child, UIA_SizeOfSetPropertyId);
            check(
                v.vt == VT_I4 && v.val as i32 == 1,
                "the child item is alone at its level (SizeOfSet 1)",
            )?;
            check(
                prop(combo, UIA_PositionInSetPropertyId).vt == VT_EMPTY,
                "a combo box isn't in a set",
            )?;
            release_through(plum);
        }

        // Invoking a row scrolled out of view scrolls first, then clicks
        // where it then is.
        let far = find(UIA_ButtonControlTypeId, "Row 5")?;
        let p = fragment_owner(far);
        check(needs_reveal(p), "'Row 5' is out of view")?;
        let events = activate_events(p).unwrap_or_default();
        let wheel = events.iter().any(|e| matches!(e, Event::Scroll { .. }));
        let click = events.iter().rev().find_map(|e| match e {
            Event::PointerButton {
                state: ButtonState::Released,
                position,
                ..
            } => Some(*position),
            _ => None,
        });
        check(
            wheel && click.is_some_and(|c| (140.0..240.0).contains(&c.y)),
            &format!("...a wheel, then a click inside the viewport ({click:?})"),
        )?;
        for f in [combo, list, child, far, frag] {
            release_through(f);
        }
    }
    Ok(())
}
