//! A tiny writer for Dalvik executables (`.dex`, format version 035), so the
//! Android backend can define the few Java classes a `NativeActivity` lacks
//! (a `View` with an `InputConnection`, an `AccessibilityNodeProvider`, memory
//! callbacks) without a Java toolchain: the classes are assembled here at run
//! time and loaded with `dalvik.system.InMemoryDexClassLoader`.
//!
//! Every method is `native` (implemented in Rust and bound with JNI's
//! `RegisterNatives`), except each class's one constructor, which only passes
//! its arguments on to the superclass constructor with the same parameters:
//! `invoke-direct {p0, ..}, Super.<init>(..)V; return-void`. That keeps the
//! bytecode to two instructions and the file to the sections such classes
//! need: strings, types, prototypes, methods, class definitions, a few type
//! lists, the constructors' code and the class data, plus the map.
//!
//! The format is the one in AOSP's `dex-format` documentation; the sort
//! orders and alignments below are the ones ART's `DexFileVerifier` checks.

use std::collections::BTreeSet;

/// A `native` method of a [`Class`]: its name and JNI type descriptors.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Native {
    pub name: &'static str,
    /// The parameter types, e.g. `["I", "Ljava/lang/CharSequence;"]`.
    pub params: &'static [&'static str],
    /// The return type, e.g. `"Z"` or `"V"`.
    pub ret: &'static str,
}

impl Native {
    /// The method's JNI signature, `(params)ret`, as `RegisterNatives` wants it.
    pub fn signature(&self) -> String {
        format!("({}){}", self.params.concat(), self.ret)
    }
}

/// One public class: its superclass, the interfaces it implements, the
/// parameters of its constructor (the superclass must have a constructor with
/// the same ones) and its native methods.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Class {
    /// A type descriptor, `Lpackage/Name;`.
    pub name: &'static str,
    pub superclass: &'static str,
    pub interfaces: &'static [&'static str],
    pub ctor: &'static [&'static str],
    pub natives: &'static [Native],
}

const ACC_PUBLIC: u32 = 0x1;
const ACC_NATIVE: u32 = 0x100;
const ACC_CONSTRUCTOR: u32 = 0x1_0000;
const NO_INDEX: u32 = 0xffff_ffff;
const HEADER_SIZE: u32 = 0x70;
const ENDIAN_CONSTANT: u32 = 0x1234_5678;

// Map item types.
const TYPE_HEADER_ITEM: u16 = 0x0000;
const TYPE_STRING_ID_ITEM: u16 = 0x0001;
const TYPE_TYPE_ID_ITEM: u16 = 0x0002;
const TYPE_PROTO_ID_ITEM: u16 = 0x0003;
const TYPE_METHOD_ID_ITEM: u16 = 0x0005;
const TYPE_CLASS_DEF_ITEM: u16 = 0x0006;
const TYPE_MAP_LIST: u16 = 0x1000;
const TYPE_TYPE_LIST: u16 = 0x1001;
const TYPE_CLASS_DATA_ITEM: u16 = 0x2000;
const TYPE_CODE_ITEM: u16 = 0x2001;
const TYPE_STRING_DATA_ITEM: u16 = 0x2002;

/// A method's prototype: its return type and parameter types.
type Proto = (&'static str, Vec<&'static str>);

/// The shorty descriptor of a prototype: one character per type, return type
/// first, with every reference type (objects and arrays) as `L`.
fn shorty(proto: &Proto) -> String {
    let short = |t: &str| match t.as_bytes()[0] {
        b'L' | b'[' => 'L',
        c => c as char,
    };
    std::iter::once(short(proto.0))
        .chain(proto.1.iter().map(|t| short(t)))
        .collect()
}

fn uleb128(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn align4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn put_u32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Adler-32, the checksum ART checks in the header.
pub(crate) fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// SHA-1, the header's signature. ART doesn't check it, but tools do.
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// One method reference: its class, name and prototype.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct MethodRef {
    class: &'static str,
    name: &'static str,
    proto: Proto,
}

/// Assemble `classes` into a dex file.
///
/// Only ASCII names are supported: for them, the byte order the string table
/// is sorted in is the UTF-16 order the format asks for.
pub(crate) fn build(classes: &[Class]) -> Vec<u8> {
    // ---- Collect every string, type, prototype and method referenced. ----
    let mut protos: BTreeSet<Proto> = BTreeSet::new();
    let mut methods: BTreeSet<MethodRef> = BTreeSet::new();
    let mut types: BTreeSet<&'static str> = BTreeSet::new();
    for c in classes {
        types.insert(c.name);
        types.insert(c.superclass);
        types.extend(c.interfaces.iter().copied());
        let ctor: Proto = ("V", c.ctor.to_vec());
        for class in [c.name, c.superclass] {
            methods.insert(MethodRef {
                class,
                name: "<init>",
                proto: ctor.clone(),
            });
        }
        protos.insert(ctor);
        for n in c.natives {
            let proto: Proto = (n.ret, n.params.to_vec());
            methods.insert(MethodRef {
                class: c.name,
                name: n.name,
                proto: proto.clone(),
            });
            protos.insert(proto);
        }
    }
    for (ret, params) in &protos {
        types.insert(ret);
        types.extend(params.iter().copied());
    }
    let mut strings: BTreeSet<String> = types.iter().map(|t| t.to_string()).collect();
    strings.extend(protos.iter().map(shorty));
    strings.extend(methods.iter().map(|m| m.name.to_string()));
    debug_assert!(strings.iter().all(|s| s.is_ascii()));

    let strings: Vec<String> = strings.into_iter().collect();
    let string_idx = |s: &str| strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32;
    // Type ids sort by their descriptor's string index, which for sorted
    // strings is the descriptors' own order.
    let types: Vec<&str> = types.into_iter().collect();
    let type_idx = |t: &str| types.binary_search(&t).unwrap() as u32;
    // Prototypes sort by return type index, then by their parameters' indices.
    let mut protos: Vec<Proto> = protos.into_iter().collect();
    let proto_key = |p: &Proto| {
        (
            type_idx(p.0),
            p.1.iter().map(|t| type_idx(t)).collect::<Vec<_>>(),
        )
    };
    protos.sort_by_key(proto_key);
    let proto_idx = |p: &Proto| protos.iter().position(|x| x == p).unwrap() as u32;
    // Methods sort by class, then name, then prototype (all as indices).
    let mut methods: Vec<MethodRef> = methods.into_iter().collect();
    methods.sort_by_key(|m| (type_idx(m.class), string_idx(m.name), proto_idx(&m.proto)));
    let method_idx = |class: &str, name: &str, proto: &Proto| {
        methods
            .iter()
            .position(|m| m.class == class && m.name == name && &m.proto == proto)
            .unwrap() as u32
    };

    // ---- Lay out the file. ----
    let string_ids_off = HEADER_SIZE;
    let type_ids_off = string_ids_off + 4 * strings.len() as u32;
    let proto_ids_off = type_ids_off + 4 * types.len() as u32;
    let method_ids_off = proto_ids_off + 12 * protos.len() as u32;
    let class_defs_off = method_ids_off + 8 * methods.len() as u32;
    let data_off = class_defs_off + 32 * classes.len() as u32;

    // The data section, written into `data` with offsets relative to the file.
    let mut data: Vec<u8> = Vec::new();
    let file_off = |data: &Vec<u8>| data_off + data.len() as u32;
    let mut map: Vec<(u16, u32, u32)> = vec![
        (TYPE_HEADER_ITEM, 1, 0),
        (TYPE_STRING_ID_ITEM, strings.len() as u32, string_ids_off),
        (TYPE_TYPE_ID_ITEM, types.len() as u32, type_ids_off),
        (TYPE_PROTO_ID_ITEM, protos.len() as u32, proto_ids_off),
        (TYPE_METHOD_ID_ITEM, methods.len() as u32, method_ids_off),
        (TYPE_CLASS_DEF_ITEM, classes.len() as u32, class_defs_off),
    ];

    // Type lists: each prototype's parameters and each class's interfaces,
    // shared when equal.
    let mut lists: Vec<Vec<&str>> = protos
        .iter()
        .map(|p| p.1.clone())
        .chain(classes.iter().map(|c| c.interfaces.to_vec()))
        .filter(|l| !l.is_empty())
        .collect();
    lists.sort_by_key(|l| l.iter().map(|t| type_idx(t)).collect::<Vec<_>>());
    lists.dedup();
    let mut list_offs: Vec<(Vec<&str>, u32)> = Vec::new();
    if !lists.is_empty() {
        map.push((TYPE_TYPE_LIST, lists.len() as u32, file_off(&data)));
    }
    for list in lists {
        align4(&mut data);
        list_offs.push((list.clone(), file_off(&data)));
        u32le(&mut data, list.len() as u32);
        for t in &list {
            u16le(&mut data, type_idx(t) as u16);
        }
    }
    let list_off = |l: &[&str]| -> u32 {
        if l.is_empty() {
            return 0;
        }
        list_offs.iter().find(|(x, _)| x == l).unwrap().1
    };

    // The constructors' code: pass every argument to the superclass's
    // constructor. Each parameter here is a single register wide (no `J` or
    // `D`), so with `this` there are `1 + params` registers, all of them ins.
    align4(&mut data);
    map.push((TYPE_CODE_ITEM, classes.len() as u32, file_off(&data)));
    let mut code_offs = Vec::new();
    for c in classes {
        assert!(
            c.ctor.len() < 5 && c.ctor.iter().all(|t| *t != "J" && *t != "D"),
            "constructors take up to four single-register parameters"
        );
        align4(&mut data);
        code_offs.push(file_off(&data));
        let regs = 1 + c.ctor.len() as u16;
        u16le(&mut data, regs); // registers_size
        u16le(&mut data, regs); // ins_size
        u16le(&mut data, regs); // outs_size
        u16le(&mut data, 0); // tries_size
        u32le(&mut data, 0); // debug_info_off
        u32le(&mut data, 4); // insns_size, in code units
        // invoke-direct {v0 .. v(regs-1)}, Super.<init> (format 35c).
        let ctor: Proto = ("V", c.ctor.to_vec());
        u16le(&mut data, (regs << 12) | 0x70);
        u16le(&mut data, method_idx(c.superclass, "<init>", &ctor) as u16);
        let mut regs_list = 0u16;
        for r in 0..regs {
            regs_list |= r << (4 * r);
        }
        u16le(&mut data, regs_list);
        u16le(&mut data, 0x000e); // return-void
    }

    // Class data: the constructor as the one direct method, the natives as
    // virtual methods, each list in method index order.
    map.push((TYPE_CLASS_DATA_ITEM, classes.len() as u32, file_off(&data)));
    let mut class_data_offs = Vec::new();
    for (c, &code_off) in classes.iter().zip(&code_offs) {
        class_data_offs.push(file_off(&data));
        uleb128(&mut data, 0); // static fields
        uleb128(&mut data, 0); // instance fields
        uleb128(&mut data, 1); // direct methods
        uleb128(&mut data, c.natives.len() as u32); // virtual methods
        let ctor: Proto = ("V", c.ctor.to_vec());
        uleb128(&mut data, method_idx(c.name, "<init>", &ctor));
        uleb128(&mut data, ACC_PUBLIC | ACC_CONSTRUCTOR);
        uleb128(&mut data, code_off);
        let mut natives: Vec<u32> = c
            .natives
            .iter()
            .map(|n| method_idx(c.name, n.name, &(n.ret, n.params.to_vec())))
            .collect();
        natives.sort_unstable();
        let mut last = 0;
        for (i, idx) in natives.into_iter().enumerate() {
            uleb128(&mut data, if i == 0 { idx } else { idx - last });
            last = idx;
            uleb128(&mut data, ACC_PUBLIC | ACC_NATIVE);
            uleb128(&mut data, 0); // no code
        }
    }

    // String data: the UTF-16 length, then the (here plain ASCII) bytes.
    map.push((TYPE_STRING_DATA_ITEM, strings.len() as u32, file_off(&data)));
    let mut string_offs = Vec::new();
    for s in &strings {
        string_offs.push(file_off(&data));
        uleb128(&mut data, s.len() as u32);
        data.extend_from_slice(s.as_bytes());
        data.push(0);
    }

    // The map, last.
    align4(&mut data);
    let map_off = file_off(&data);
    map.push((TYPE_MAP_LIST, 1, map_off));
    u32le(&mut data, map.len() as u32);
    for &(kind, size, off) in &map {
        u16le(&mut data, kind);
        u16le(&mut data, 0);
        u32le(&mut data, size);
        u32le(&mut data, off);
    }

    // ---- The id sections. ----
    let mut out = vec![0u8; HEADER_SIZE as usize];
    for &off in &string_offs {
        u32le(&mut out, off);
    }
    for t in &types {
        u32le(&mut out, string_idx(t));
    }
    for p in &protos {
        u32le(&mut out, string_idx(&shorty(p)));
        u32le(&mut out, type_idx(p.0));
        u32le(&mut out, list_off(&p.1));
    }
    for m in &methods {
        u16le(&mut out, type_idx(m.class) as u16);
        u16le(&mut out, proto_idx(&m.proto) as u16);
        u32le(&mut out, string_idx(m.name));
    }
    for (c, &class_data_off) in classes.iter().zip(&class_data_offs) {
        u32le(&mut out, type_idx(c.name));
        u32le(&mut out, ACC_PUBLIC);
        u32le(&mut out, type_idx(c.superclass));
        u32le(&mut out, list_off(c.interfaces));
        u32le(&mut out, NO_INDEX); // source file
        u32le(&mut out, 0); // annotations
        u32le(&mut out, class_data_off);
        u32le(&mut out, 0); // static values
    }
    debug_assert_eq!(out.len() as u32, data_off);
    out.extend_from_slice(&data);

    // ---- The header. ----
    out[..8].copy_from_slice(b"dex\n035\0");
    let file_size = out.len() as u32;
    for (at, v) in [
        (32, file_size),
        (36, HEADER_SIZE),
        (40, ENDIAN_CONSTANT),
        (44, 0), // link_size
        (48, 0), // link_off
        (52, map_off),
        (56, strings.len() as u32),
        (60, string_ids_off),
        (64, types.len() as u32),
        (68, type_ids_off),
        (72, protos.len() as u32),
        (76, proto_ids_off),
        (80, 0), // field_ids_size
        (84, 0), // field_ids_off
        (88, methods.len() as u32),
        (92, method_ids_off),
        (96, classes.len() as u32),
        (100, class_defs_off),
        (104, file_size - data_off),
        (108, data_off),
    ] {
        put_u32(&mut out, at, v);
    }
    let signature = sha1(&out[32..]);
    out[12..32].copy_from_slice(&signature);
    let checksum = adler32(&out[12..]);
    put_u32(&mut out, 8, checksum);
    out
}

// ---- The classes the Android backend defines ---------------------------------

const fn native(name: &'static str, params: &'static [&'static str], ret: &'static str) -> Native {
    Native { name, params, ret }
}

const CHAR_SEQUENCE: &str = "Ljava/lang/CharSequence;";
const NODE_INFO: &str = "Landroid/view/accessibility/AccessibilityNodeInfo;";

/// The view the backend adds to its `NativeActivity`, which a
/// `NativeActivity` lacks: it gives the input method an `InputConnection`
/// and accessibility services a node per element.
pub(crate) const HOST_VIEW: Class = Class {
    name: "Lorg/stipple/android/HostView;",
    superclass: "Landroid/view/View;",
    interfaces: &[],
    ctor: &["Landroid/content/Context;"],
    natives: &[
        native(
            "onCreateInputConnection",
            &["Landroid/view/inputmethod/EditorInfo;"],
            "Landroid/view/inputmethod/InputConnection;",
        ),
        native("onCheckIsTextEditor", &[], "Z"),
        native(
            "getAccessibilityNodeProvider",
            &[],
            "Landroid/view/accessibility/AccessibilityNodeProvider;",
        ),
        native("dispatchHoverEvent", &["Landroid/view/MotionEvent;"], "Z"),
    ],
};

/// The input method's connection to the focused text field.
pub(crate) const INPUT: Class = Class {
    name: "Lorg/stipple/android/Input;",
    superclass: "Landroid/view/inputmethod/BaseInputConnection;",
    interfaces: &[],
    ctor: &["Landroid/view/View;", "Z"],
    natives: &[
        native("commitText", &[CHAR_SEQUENCE, "I"], "Z"),
        native("setComposingText", &[CHAR_SEQUENCE, "I"], "Z"),
        native("finishComposingText", &[], "Z"),
        native("deleteSurroundingText", &["I", "I"], "Z"),
        native("deleteSurroundingTextInCodePoints", &["I", "I"], "Z"),
        native("setComposingRegion", &["I", "I"], "Z"),
        native("setSelection", &["I", "I"], "Z"),
        native("getTextBeforeCursor", &["I", "I"], CHAR_SEQUENCE),
        native("getTextAfterCursor", &["I", "I"], CHAR_SEQUENCE),
        native("getSelectedText", &["I"], CHAR_SEQUENCE),
        native("getCursorCapsMode", &["I"], "I"),
        native("performEditorAction", &["I"], "Z"),
        native("performContextMenuAction", &["I"], "Z"),
    ],
};

/// The accessibility tree, a virtual view per node.
pub(crate) const NODES: Class = Class {
    name: "Lorg/stipple/android/Nodes;",
    superclass: "Landroid/view/accessibility/AccessibilityNodeProvider;",
    interfaces: &[],
    ctor: &[],
    natives: &[
        native("createAccessibilityNodeInfo", &["I"], NODE_INFO),
        native("performAction", &["I", "I", "Landroid/os/Bundle;"], "Z"),
        native("findFocus", &["I"], NODE_INFO),
    ],
};

/// Memory warnings (`onTrimMemory`), which `NativeActivity` doesn't forward.
pub(crate) const CALLBACKS: Class = Class {
    name: "Lorg/stipple/android/Callbacks;",
    superclass: "Ljava/lang/Object;",
    interfaces: &["Landroid/content/ComponentCallbacks2;"],
    ctor: &[],
    natives: &[
        native("onTrimMemory", &["I"], "V"),
        native("onLowMemory", &[], "V"),
        native(
            "onConfigurationChanged",
            &["Landroid/content/res/Configuration;"],
            "V",
        ),
    ],
};

/// Every class the Android backend defines, in the order it loads them.
pub(crate) const ANDROID_CLASSES: [Class; 4] = [HOST_VIEW, INPUT, NODES, CALLBACKS];

/// A class's binary name, as `ClassLoader.loadClass` wants it:
/// `Lorg/x/Y;` is `org.x.Y`.
pub(crate) fn binary_name(descriptor: &str) -> String {
    descriptor
        .trim_start_matches('L')
        .trim_end_matches(';')
        .replace('/', ".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        let hex: String = sha1(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
        let long = vec![b'a'; 1000];
        let hex: String = sha1(&long).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn uleb128_encodes_seven_bits_a_byte() {
        let mut out = Vec::new();
        for v in [0, 1, 127, 128, 300, 0xffff_ffff] {
            uleb128(&mut out, v);
        }
        assert_eq!(
            out,
            [
                0, 1, 0x7f, 0x80, 0x01, 0xac, 0x02, 0xff, 0xff, 0xff, 0xff, 0x0f
            ]
        );
    }

    #[test]
    fn shorty_folds_references() {
        assert_eq!(
            shorty(&("Z", vec!["Ljava/lang/CharSequence;", "I", "[B"])),
            "ZLIL"
        );
        assert_eq!(shorty(&("V", vec![])), "V");
    }

    const DEMO: &[Class] = &[
        Class {
            name: "Lorg/example/Thing;",
            superclass: "Ljava/lang/Object;",
            interfaces: &["Ljava/lang/Runnable;"],
            ctor: &[],
            natives: &[Native {
                name: "run",
                params: &[],
                ret: "V",
            }],
        },
        Class {
            name: "Lorg/example/Box;",
            superclass: "Landroid/view/View;",
            interfaces: &[],
            ctor: &["Landroid/content/Context;"],
            natives: &[
                Native {
                    name: "onCheckIsTextEditor",
                    params: &[],
                    ret: "Z",
                },
                Native {
                    name: "add",
                    params: &["I", "I"],
                    ret: "I",
                },
            ],
        },
    ];

    #[test]
    fn the_header_describes_the_file() {
        let dex = build(DEMO);
        assert_eq!(&dex[..8], b"dex\n035\0");
        assert_eq!(read_u32(&dex, 32) as usize, dex.len());
        assert_eq!(read_u32(&dex, 36), 0x70);
        assert_eq!(read_u32(&dex, 40), 0x1234_5678);
        assert_eq!(read_u32(&dex, 8), adler32(&dex[12..]));
        assert_eq!(&dex[12..32], &sha1(&dex[32..]));
        assert_eq!(dex.len() % 4, 0);
        // Two classes; methods: Object.<init>, Thing.<init>, Thing.run,
        // View.<init>, Box.<init>, Box.add, Box.onCheckIsTextEditor.
        assert_eq!(read_u32(&dex, 96), 2);
        assert_eq!(read_u32(&dex, 88), 7);
        // The data section runs to the end of the file, the map inside it.
        let (data_size, data_off) = (read_u32(&dex, 104), read_u32(&dex, 108));
        assert_eq!((data_off + data_size) as usize, dex.len());
        let map_off = read_u32(&dex, 52) as usize;
        assert!(map_off >= data_off as usize);
        // The map lists the sections in offset order, ending with itself.
        let n = read_u32(&dex, map_off) as usize;
        let entries: Vec<(u16, u32)> = (0..n)
            .map(|i| {
                let at = map_off + 4 + i * 12;
                (
                    u16::from_le_bytes([dex[at], dex[at + 1]]),
                    read_u32(&dex, at + 8),
                )
            })
            .collect();
        assert!(entries.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(entries.last(), Some(&(TYPE_MAP_LIST, map_off as u32)));
    }

    /// The strings, read back from the string table, must be sorted and
    /// unique, as ART's verifier requires.
    #[test]
    fn strings_are_sorted_and_unique() {
        let dex = build(DEMO);
        let (n, off) = (read_u32(&dex, 56) as usize, read_u32(&dex, 60) as usize);
        let strings: Vec<String> = (0..n)
            .map(|i| {
                let mut at = read_u32(&dex, off + 4 * i) as usize;
                let mut len = 0usize;
                let mut shift = 0;
                loop {
                    let b = dex[at];
                    at += 1;
                    len |= usize::from(b & 0x7f) << shift;
                    shift += 7;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                String::from_utf8(dex[at..at + len].to_vec()).unwrap()
            })
            .collect();
        assert!(strings.windows(2).all(|w| w[0] < w[1]), "{strings:?}");
        for s in ["<init>", "Landroid/view/View;", "ZII", "IIII", "V", "Z"] {
            let found = strings.iter().any(|x| x == s);
            // "ZII" and "IIII" aren't prototypes here; the others are.
            assert_eq!(found, !matches!(s, "ZII" | "IIII"), "{s}");
        }
        assert!(strings.iter().any(|x| x == "III"));
    }

    #[test]
    fn a_constructor_calls_the_superclass_constructor() {
        let dex = build(DEMO);
        // The code items follow the type lists; find each by its header:
        // registers = ins = outs, no tries, four code units.
        let needle_box = [2u8, 0, 2, 0, 2, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0];
        let at = dex
            .windows(needle_box.len())
            .position(|w| w == needle_box)
            .expect("Box's constructor");
        let insns = &dex[at + 16..at + 24];
        // invoke-direct with two registers, {v0, v1}, then return-void.
        assert_eq!(insns[0], 0x70);
        assert_eq!(insns[1], 0x20);
        assert_eq!(&insns[4..6], &[0x10, 0x00]);
        assert_eq!(&insns[6..8], &[0x0e, 0x00]);
        assert_eq!(at % 4, 0);
    }

    #[test]
    fn signatures_are_jni_style() {
        let n = Native {
            name: "commitText",
            params: &["Ljava/lang/CharSequence;", "I"],
            ret: "Z",
        };
        assert_eq!(n.signature(), "(Ljava/lang/CharSequence;I)Z");
    }

    #[test]
    fn the_android_classes_build() {
        let dex = build(&ANDROID_CLASSES);
        assert_eq!(read_u32(&dex, 96), 4);
        assert_eq!(read_u32(&dex, 8), adler32(&dex[12..]));
        assert_eq!(binary_name(HOST_VIEW.name), "org.stipple.android.HostView");
        // Each native is a virtual method of its class, so overriding works.
        for c in ANDROID_CLASSES {
            for n in c.natives {
                assert!(n.signature().starts_with('('), "{}", n.name);
            }
        }
    }

    /// With `STIPPLE_DEX_OUT` set, write the Android backend's classes there
    /// (and the demo next to it), so `dexdump` from the Android SDK's build
    /// tools can check them; CI does.
    #[test]
    fn writes_the_classes_for_dexdump() {
        if let Some(path) = std::env::var_os("STIPPLE_DEX_OUT") {
            let path = std::path::PathBuf::from(path);
            std::fs::write(&path, build(&ANDROID_CLASSES)).unwrap();
            std::fs::write(path.with_extension("demo.dex"), build(DEMO)).unwrap();
        }
    }
}
