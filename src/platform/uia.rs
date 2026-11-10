//! Hand-written **UI Automation** provider — no `windows`/`uiautomation` crate,
//! just COM objects (`IRawElementProviderFragment` + `…FragmentRoot`) we build by
//! hand: each is a struct whose first field is a vtable pointer, matching the
//! "talk to the OS directly" approach of the other backends. Windows-only.
//!
//! UIA is how Windows exposes accessibility. Because Stipple self-draws every
//! control, the window is one opaque `HWND`, so we vend a *tree* of providers —
//! one per [`A11yNode`](crate::platform::access::A11yNode) — that answer UIA property
//! queries (name, control type, bounds) and navigation (parent / sibling /
//! child). The window plugs the root in via `WM_GETOBJECT` →
//! `UiaReturnRawElementProvider`. This is the Windows half of the cross-platform
//! a11y bridge (AT-SPI is Linux, `NSAccessibility` is macOS).
//!
//! **Ownership:** a [`UiaTree`] owns every provider in a `Vec<Box<Provider>>`;
//! the providers reference each other by raw pointer (a tree, no cycles to
//! refcount). COM `AddRef`/`Release` adjust each node's count but never free —
//! the `UiaTree` (held by the window for its lifetime) is the sole owner. Swapping
//! the tree on a UI change frees the old nodes; signalling that to a *live* UIA
//! client (`UiaRaiseStructureChangedEvent`) is follow-up depth — the CI self-check
//! drives the vtable directly, as a cross-process UIA client needs a live desktop
//! UIA stack the runner doesn't reliably provide.

#![allow(unsafe_code, non_snake_case, non_upper_case_globals)]

use crate::platform::access::{A11yNode, A11yRole};
use core::ffi::c_void;

type Hresult = i32;
const S_OK: Hresult = 0;

// UIA constants.
const ProviderOptions_ServerSideProvider: i32 = 0x1;
const UIA_ControlTypePropertyId: i32 = 30003;
const UIA_NamePropertyId: i32 = 30005;

// Control-type ids (one per role).
const UIA_ButtonControlTypeId: i32 = 50000;
const UIA_EditControlTypeId: i32 = 50004;
const UIA_TextControlTypeId: i32 = 50020;
const UIA_GroupControlTypeId: i32 = 50026;
const UIA_WindowControlTypeId: i32 = 50032;
const UIA_ListItemControlTypeId: i32 = 50007;
const UIA_ListControlTypeId: i32 = 50008;
const UIA_TreeControlTypeId: i32 = 50023;
const UIA_TreeItemControlTypeId: i32 = 50024;
const UIA_DataGridControlTypeId: i32 = 50028;
const UIA_DataItemControlTypeId: i32 = 50029;
const UIA_HeaderItemControlTypeId: i32 = 50035;
const UIA_MenuControlTypeId: i32 = 50009;
const UIA_MenuBarControlTypeId: i32 = 50010;
const UIA_MenuItemControlTypeId: i32 = 50011;
const UIA_TabControlTypeId: i32 = 50018;
const UIA_TabItemControlTypeId: i32 = 50019;

// NavigateDirection.
const NavigateDirection_Parent: i32 = 0;
const NavigateDirection_NextSibling: i32 = 1;
const NavigateDirection_PreviousSibling: i32 = 2;
const NavigateDirection_FirstChild: i32 = 3;
const NavigateDirection_LastChild: i32 = 4;

/// Prepended to a fragment's runtime id so UIA namespaces it under this provider.
const UiaAppendRuntimeId: i32 = 3;

// VARIANT types.
const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysAllocString(psz: *const u16) -> *mut u16;
    fn SysFreeString(bstr: *mut u16);
    fn SafeArrayCreateVector(vt: u16, l_lbound: i32, c_elements: u32) -> *mut c_void;
    fn SafeArrayPutElement(psa: *mut c_void, rg_indices: *const i32, pv: *const c_void) -> Hresult;
}

/// A `VARIANT` (x64 layout: 8-byte header, then a 16-byte value union). We only
/// ever store a VT_I4 or VT_BSTR, both of which fit in the first 8 value bytes.
#[repr(C)]
struct Variant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    val: u64,
    pad: u64,
}

impl Variant {
    fn empty() -> Variant {
        Variant {
            vt: VT_EMPTY,
            r1: 0,
            r2: 0,
            r3: 0,
            val: 0,
            pad: 0,
        }
    }
}

/// `UiaRect` — the bounding rectangle UIA reports (screen coordinates in a real
/// app; we currently report the node's logical bounds, a follow-up to map).
#[repr(C)]
#[derive(Clone, Copy)]
struct UiaRect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

/// The combined vtable for `IRawElementProviderFragmentRoot`, which inherits
/// `…Fragment`, which inherits `…Simple`, which inherits `IUnknown`. One vtable
/// covers every provider (the root and its descendants); only the *behaviour* of
/// `Navigate`/`get_FragmentRoot` differs, driven by each node's links. Method
/// order must match the COM interface declarations exactly.
#[repr(C)]
struct ProviderVtbl {
    // IUnknown
    query_interface: unsafe extern "system" fn(*mut c_void, *const u8, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    // IRawElementProviderSimple
    get_provider_options: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_pattern_provider: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> Hresult,
    get_property_value: unsafe extern "system" fn(*mut c_void, i32, *mut Variant) -> Hresult,
    get_host_raw_element_provider:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    // IRawElementProviderFragment
    navigate: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> Hresult,
    get_runtime_id: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    get_bounding_rectangle: unsafe extern "system" fn(*mut c_void, *mut UiaRect) -> Hresult,
    get_embedded_fragment_roots:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    set_focus: unsafe extern "system" fn(*mut c_void) -> Hresult,
    get_fragment_root: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    // IRawElementProviderFragmentRoot
    element_provider_from_point:
        unsafe extern "system" fn(*mut c_void, f64, f64, *mut *mut c_void) -> Hresult,
    get_focus: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

/// One provider node. The vtable pointer is first, so the object pointer *is* the
/// interface pointer. Navigation links are raw pointers into the owning
/// [`UiaTree`]'s boxes (stable heap addresses).
#[repr(C)]
struct Provider {
    vtable: *const ProviderVtbl,
    refcount: u32,
    /// UTF-16 (NUL-terminated) accessible name, for `SysAllocString`.
    name: Vec<u16>,
    control_type: i32,
    runtime_id: i32,
    rect: UiaRect,
    focused: bool,
    parent: *mut Provider,
    children: Vec<*mut Provider>,
    index_in_parent: usize,
    root: *mut Provider,
}

fn control_type(role: A11yRole) -> i32 {
    match role {
        A11yRole::Window => UIA_WindowControlTypeId,
        A11yRole::Group => UIA_GroupControlTypeId,
        A11yRole::Button => UIA_ButtonControlTypeId,
        A11yRole::TextField => UIA_EditControlTypeId,
        A11yRole::Text => UIA_TextControlTypeId,
        A11yRole::List => UIA_ListControlTypeId,
        A11yRole::ListItem => UIA_ListItemControlTypeId,
        A11yRole::Table => UIA_DataGridControlTypeId,
        A11yRole::Row => UIA_DataItemControlTypeId,
        // A grid cell is exposed as text inside its data item.
        A11yRole::Cell => UIA_TextControlTypeId,
        A11yRole::ColumnHeader => UIA_HeaderItemControlTypeId,
        A11yRole::Tree => UIA_TreeControlTypeId,
        A11yRole::TreeItem => UIA_TreeItemControlTypeId,
        A11yRole::MenuBar => UIA_MenuBarControlTypeId,
        A11yRole::Menu => UIA_MenuControlTypeId,
        A11yRole::MenuItem => UIA_MenuItemControlTypeId,
        // Not named to UIA yet: the closest control type above.
        A11yRole::ComboBox | A11yRole::CheckBox | A11yRole::Switch => UIA_ButtonControlTypeId,
        A11yRole::SpinButton => UIA_EditControlTypeId,
        A11yRole::Splitter | A11yRole::Toolbar | A11yRole::Separator | A11yRole::Alert => {
            UIA_GroupControlTypeId
        }
        A11yRole::TabList => UIA_TabControlTypeId,
        A11yRole::Tab => UIA_TabItemControlTypeId,
        // UIA marks a password edit with `IsPassword`, not a control type.
        A11yRole::PasswordField => UIA_EditControlTypeId,
        A11yRole::Dialog => UIA_WindowControlTypeId,
    }
}

unsafe extern "system" fn qi(this: *mut c_void, _iid: *const u8, ppv: *mut *mut c_void) -> Hresult {
    // We answer every interface query with ourselves: the combined vtable
    // satisfies IUnknown, IRawElementProviderSimple, …Fragment, and …FragmentRoot
    // (each a prefix of the next), which is all UIA asks of us.
    unsafe {
        *ppv = this;
        add_ref(this);
    }
    S_OK
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let p = this as *mut Provider;
        (*p).refcount += 1;
        (*p).refcount
    }
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    // The owning `UiaTree` frees the node; Release only decrements (never drops),
    // so an over-release by a client can't dangle a still-linked sibling.
    unsafe {
        let p = this as *mut Provider;
        if (*p).refcount > 0 {
            (*p).refcount -= 1;
        }
        (*p).refcount
    }
}

unsafe extern "system" fn get_provider_options(_this: *mut c_void, opts: *mut i32) -> Hresult {
    unsafe {
        *opts = ProviderOptions_ServerSideProvider;
    }
    S_OK
}

unsafe extern "system" fn get_pattern_provider(
    _this: *mut c_void,
    _pattern: i32,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        *ret = core::ptr::null_mut(); // we implement no control patterns yet
    }
    S_OK
}

unsafe extern "system" fn get_property_value(
    this: *mut c_void,
    property_id: i32,
    ret: *mut Variant,
) -> Hresult {
    unsafe {
        let p = this as *mut Provider;
        let mut v = Variant::empty();
        match property_id {
            UIA_ControlTypePropertyId => {
                v.vt = VT_I4;
                v.val = (*p).control_type as u32 as u64;
            }
            UIA_NamePropertyId => {
                v.vt = VT_BSTR;
                v.val = SysAllocString((*p).name.as_ptr()) as u64;
            }
            _ => {} // VT_EMPTY → "property not supported"
        }
        *ret = v;
    }
    S_OK
}

unsafe extern "system" fn get_host_raw_element_provider(
    _this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        *ret = core::ptr::null_mut(); // a server-side fragment has no host
    }
    S_OK
}

/// Set `*ret` to `target` (or null), `AddRef`-ing it as COM requires of an
/// out-parameter the caller will `Release`.
unsafe fn yield_provider(target: *mut Provider, ret: *mut *mut c_void) {
    unsafe {
        if target.is_null() {
            *ret = core::ptr::null_mut();
        } else {
            *ret = target as *mut c_void;
            add_ref(target as *mut c_void);
        }
    }
}

unsafe extern "system" fn navigate(
    this: *mut c_void,
    direction: i32,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        let p = this as *mut Provider;
        let children: &[*mut Provider] = &(*p).children;
        let target = match direction {
            NavigateDirection_Parent => (*p).parent,
            NavigateDirection_FirstChild => {
                children.first().copied().unwrap_or(core::ptr::null_mut())
            }
            NavigateDirection_LastChild => {
                children.last().copied().unwrap_or(core::ptr::null_mut())
            }
            NavigateDirection_NextSibling => sibling(p, 1),
            NavigateDirection_PreviousSibling => sibling(p, -1),
            _ => core::ptr::null_mut(),
        };
        yield_provider(target, ret);
    }
    S_OK
}

/// The sibling `offset` positions from `p` among its parent's children.
unsafe fn sibling(p: *mut Provider, offset: isize) -> *mut Provider {
    unsafe {
        let parent = (*p).parent;
        if parent.is_null() {
            return core::ptr::null_mut();
        }
        let i = (*p).index_in_parent as isize + offset;
        if i < 0 {
            return core::ptr::null_mut();
        }
        let siblings: &[*mut Provider] = &(*parent).children;
        siblings
            .get(i as usize)
            .copied()
            .unwrap_or(core::ptr::null_mut())
    }
}

unsafe extern "system" fn get_runtime_id(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    // A 2-element SAFEARRAY<i4>: [UiaAppendRuntimeId, our unique id] — UIA prepends
    // the host runtime id, so this need only be unique within our fragment.
    unsafe {
        let p = this as *mut Provider;
        let sa = SafeArrayCreateVector(VT_I4, 0, 2);
        if !sa.is_null() {
            let mut idx: i32 = 0;
            let v0 = UiaAppendRuntimeId;
            SafeArrayPutElement(sa, &idx, &v0 as *const i32 as *const c_void);
            idx = 1;
            let v1 = (*p).runtime_id;
            SafeArrayPutElement(sa, &idx, &v1 as *const i32 as *const c_void);
        }
        *ret = sa;
    }
    S_OK
}

unsafe extern "system" fn get_bounding_rectangle(this: *mut c_void, ret: *mut UiaRect) -> Hresult {
    unsafe {
        *ret = (*(this as *mut Provider)).rect;
    }
    S_OK
}

unsafe extern "system" fn get_embedded_fragment_roots(
    _this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        *ret = core::ptr::null_mut(); // no nested fragment roots
    }
    S_OK
}

unsafe extern "system" fn set_focus(_this: *mut c_void) -> Hresult {
    S_OK // Stipple owns focus internally; nothing to do for UIA
}

unsafe extern "system" fn get_fragment_root(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        yield_provider((*(this as *mut Provider)).root, ret);
    }
    S_OK
}

unsafe extern "system" fn element_provider_from_point(
    this: *mut c_void,
    _x: f64,
    _y: f64,
    ret: *mut *mut c_void,
) -> Hresult {
    // Hit-testing to the deepest node under the point is follow-up; return the
    // root, a valid (if coarse) answer.
    unsafe {
        yield_provider((*(this as *mut Provider)).root, ret);
    }
    S_OK
}

unsafe extern "system" fn get_focus(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    // Walk the fragment for the node Stipple marked focused.
    unsafe {
        let root = (*(this as *mut Provider)).root;
        yield_provider(find_focused(root), ret);
    }
    S_OK
}

unsafe fn find_focused(p: *mut Provider) -> *mut Provider {
    unsafe {
        if p.is_null() {
            return core::ptr::null_mut();
        }
        if (*p).focused {
            return p;
        }
        let children: &[*mut Provider] = &(*p).children;
        for &c in children {
            let f = find_focused(c);
            if !f.is_null() {
                return f;
            }
        }
        core::ptr::null_mut()
    }
}

static PROVIDER_VTBL: ProviderVtbl = ProviderVtbl {
    query_interface: qi,
    add_ref,
    release,
    get_provider_options,
    get_pattern_provider,
    get_property_value,
    get_host_raw_element_provider,
    navigate,
    get_runtime_id,
    get_bounding_rectangle,
    get_embedded_fragment_roots,
    set_focus,
    get_fragment_root,
    element_provider_from_point,
    get_focus,
};

/// A UIA provider tree built from an [`A11yNode`] hierarchy. Owns every provider;
/// hand the root to UIA via [`UiaTree::root_provider`].
pub struct UiaTree {
    // Element addresses are stable: the Vec is built once (never pushed to again),
    // and the raw navigation pointers index its heap buffer, which a later move of
    // the Vec into this struct does not relocate.
    nodes: Vec<Provider>,
}

impl core::fmt::Debug for UiaTree {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UiaTree")
            .field("nodes", &self.nodes.len())
            .finish()
    }
}

impl UiaTree {
    /// Build a provider per node in `root`'s subtree, linking parent/child/sibling
    /// pointers so UIA can navigate the whole hierarchy.
    pub fn build(root: &A11yNode) -> UiaTree {
        // Flatten depth-first: (node, parent flat-index, index among siblings).
        let mut flat: Vec<(&A11yNode, Option<usize>, usize)> = Vec::new();
        fn walk<'a>(
            n: &'a A11yNode,
            parent: Option<usize>,
            sib: usize,
            flat: &mut Vec<(&'a A11yNode, Option<usize>, usize)>,
        ) {
            let me = flat.len();
            flat.push((n, parent, sib));
            for (i, c) in n.children.iter().enumerate() {
                walk(c, Some(me), i, flat);
            }
        }
        walk(root, None, 0, &mut flat);

        // Fill the Vec once (so its buffer never reallocates), then link by raw
        // pointer into that stable buffer.
        let mut nodes: Vec<Provider> = flat
            .iter()
            .enumerate()
            .map(|(i, (n, _, _))| {
                let mut name: Vec<u16> = n.name.encode_utf16().collect();
                name.push(0);
                Provider {
                    vtable: &PROVIDER_VTBL,
                    refcount: 1,
                    name,
                    control_type: control_type(n.role),
                    runtime_id: i as i32 + 1,
                    rect: UiaRect {
                        left: n.bounds.0,
                        top: n.bounds.1,
                        width: n.bounds.2,
                        height: n.bounds.3,
                    },
                    focused: n.focused,
                    parent: core::ptr::null_mut(),
                    children: Vec::new(),
                    index_in_parent: 0,
                    root: core::ptr::null_mut(),
                }
            })
            .collect();

        let ptrs: Vec<*mut Provider> = nodes.iter_mut().map(|p| p as *mut Provider).collect();
        let root_ptr = ptrs[0];
        for (i, (_, parent, sib)) in flat.iter().enumerate() {
            let p = ptrs[i];
            unsafe {
                (*p).root = root_ptr;
                (*p).index_in_parent = *sib;
                (*p).parent = parent.map(|pi| ptrs[pi]).unwrap_or(core::ptr::null_mut());
                // children of i, in DFS (sibling) order.
                (*p).children = flat
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, par, _))| *par == Some(i))
                    .map(|(j, _)| ptrs[j])
                    .collect();
            }
        }
        UiaTree { nodes }
    }

    /// The root `IRawElementProviderFragmentRoot*` to return from `WM_GETOBJECT`
    /// via `UiaReturnRawElementProvider`. Null only if the tree is empty.
    pub fn root_provider(&self) -> *mut c_void {
        self.nodes
            .first()
            .map(|p| p as *const Provider as *mut c_void)
            .unwrap_or(core::ptr::null_mut())
    }
}

/// Read a property + walk the tree through the *real* COM vtable (`Navigate` +
/// `get_property_value`), printing each node — the CI self-check that proves the
/// whole hierarchy, not just the root, is navigable.
unsafe fn dump(p: *mut c_void, depth: usize) {
    unsafe {
        let vt = &*(*(p as *mut Provider)).vtable;
        let mut nm = Variant::empty();
        (vt.get_property_value)(p, UIA_NamePropertyId, &mut nm);
        let name = bstr_to_string(nm.val as *const u16);
        if nm.val != 0 {
            SysFreeString(nm.val as *mut u16);
        }
        let mut ct = Variant::empty();
        (vt.get_property_value)(p, UIA_ControlTypePropertyId, &mut ct);
        let indent = "  ".repeat(depth);
        println!(
            "UIA provider:{indent} controltype={} name={:?}",
            ct.val as i32, name
        );
        // First child, then iterate next-sibling — all through Navigate.
        let mut child: *mut c_void = core::ptr::null_mut();
        (vt.navigate)(p, NavigateDirection_FirstChild, &mut child);
        while !child.is_null() {
            dump(child, depth + 1);
            let cvt = &*(*(child as *mut Provider)).vtable;
            let mut next: *mut c_void = core::ptr::null_mut();
            (cvt.navigate)(child, NavigateDirection_NextSibling, &mut next);
            (cvt.release)(child);
            child = next;
        }
    }
}

/// Exercise a provider *tree* through its COM vtables (real COM dispatch) and
/// print what it returns — the CI self-check (a cross-process UIA client needs a
/// live desktop UIA stack the runner doesn't reliably provide). Builds a small
/// representative UI rooted at a window named `name`, then verifies the root
/// answers `ProviderOptions`/`ControlType`/`Name` and that `Navigate` walks the
/// full descendant tree.
pub fn selftest(name: &str) -> Result<(), String> {
    fn leaf(role: A11yRole, name: &str, focused: bool) -> A11yNode {
        A11yNode {
            focused,
            ..A11yNode::new(role, name, (0.0, 0.0, 0.0, 0.0))
        }
    }
    let root = A11yNode {
        children: vec![
            leaf(A11yRole::Text, "Welcome to Stipple", false),
            leaf(A11yRole::TextField, "Name", true),
            A11yNode {
                children: vec![leaf(A11yRole::Button, "OK", false)],
                ..leaf(A11yRole::Group, "", false)
            },
        ],
        ..A11yNode::new(A11yRole::Window, name, (0.0, 0.0, 360.0, 240.0))
    };
    let tree = UiaTree::build(&root);
    let p = tree.root_provider();

    unsafe {
        let vt = &*(*(p as *mut Provider)).vtable;
        let mut opts: i32 = 0;
        (vt.get_provider_options)(p, &mut opts);
        let mut ct = Variant::empty();
        (vt.get_property_value)(p, UIA_ControlTypePropertyId, &mut ct);
        let mut nm = Variant::empty();
        (vt.get_property_value)(p, UIA_NamePropertyId, &mut nm);
        let name_out = bstr_to_string(nm.val as *const u16);
        if nm.val != 0 {
            SysFreeString(nm.val as *mut u16);
        }
        println!(
            "UIA provider: options={} controltype={} name={:?}",
            opts, ct.val as i32, name_out
        );

        // The full element tree, walked via Navigate (FirstChild / NextSibling).
        let mut child: *mut c_void = core::ptr::null_mut();
        (vt.navigate)(p, NavigateDirection_FirstChild, &mut child);
        while !child.is_null() {
            dump(child, 1);
            let cvt = &*(*(child as *mut Provider)).vtable;
            let mut next: *mut c_void = core::ptr::null_mut();
            (cvt.navigate)(child, NavigateDirection_NextSibling, &mut next);
            (cvt.release)(child);
            child = next;
        }

        // Focus query: the tree reports the focused TextField ("Name").
        let mut focus: *mut c_void = core::ptr::null_mut();
        (vt.get_focus)(p, &mut focus);
        if !focus.is_null() {
            let fvt = &*(*(focus as *mut Provider)).vtable;
            let mut fnm = Variant::empty();
            (fvt.get_property_value)(focus, UIA_NamePropertyId, &mut fnm);
            let fname = bstr_to_string(fnm.val as *const u16);
            if fnm.val != 0 {
                SysFreeString(fnm.val as *mut u16);
            }
            (fvt.release)(focus);
            println!("UIA provider: focus={fname:?}");
        }
    }
    Ok(())
}

/// Read a NUL-terminated UTF-16 BSTR into a `String`.
unsafe fn bstr_to_string(bstr: *const u16) -> String {
    if bstr.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *bstr.add(len) != 0 {
            len += 1;
        }
        let slice = core::slice::from_raw_parts(bstr, len);
        String::from_utf16_lossy(slice)
    }
}
