//! Building and reading the D-Bus bodies the AT-SPI interfaces exchange, on
//! top of the wire helpers in [`crate::platform::a11y`].

use crate::platform::a11y::{align, align_to, put_string, read_string, read_u32};

/// A message body being marshalled (little-endian, aligned from offset 0).
#[derive(Debug, Default)]
pub(super) struct Body(pub(super) Vec<u8>);

impl Body {
    pub(super) fn new() -> Self {
        Self(Vec::new())
    }

    pub(super) fn u32(&mut self, v: u32) -> &mut Self {
        align(&mut self.0, 4);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub(super) fn i32(&mut self, v: i32) -> &mut Self {
        self.u32(v as u32)
    }

    pub(super) fn bool(&mut self, v: bool) -> &mut Self {
        self.u32(u32::from(v))
    }

    pub(super) fn i16(&mut self, v: i16) -> &mut Self {
        align(&mut self.0, 2);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub(super) fn f64(&mut self, v: f64) -> &mut Self {
        align(&mut self.0, 8);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// A STRING or OBJECT_PATH.
    pub(super) fn str(&mut self, s: &str) -> &mut Self {
        put_string(&mut self.0, s);
        self
    }

    /// A SIGNATURE (a variant's type, too).
    pub(super) fn sig(&mut self, s: &str) -> &mut Self {
        self.0.push(s.len() as u8);
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
        self
    }

    /// An AT-SPI object reference `(so)`.
    pub(super) fn obj(&mut self, bus: &str, path: &str) -> &mut Self {
        align(&mut self.0, 8);
        self.str(bus).str(path)
    }

    /// An array whose elements align to `elem_align`, written by `f`.
    pub(super) fn array(&mut self, elem_align: usize, f: impl FnOnce(&mut Self)) -> &mut Self {
        align(&mut self.0, 4);
        let at = self.0.len();
        self.0.extend_from_slice(&0u32.to_le_bytes());
        // The padding to the first element is there even when it's empty,
        // and isn't counted in the length.
        align(&mut self.0, elem_align);
        let start = self.0.len();
        f(self);
        let len = (self.0.len() - start) as u32;
        self.0[at..at + 4].copy_from_slice(&len.to_le_bytes());
        self
    }

    /// An `as`.
    pub(super) fn strings(&mut self, items: &[&str]) -> &mut Self {
        self.array(4, |b| {
            for s in items {
                b.str(s);
            }
        })
    }

    /// An `a{ss}`.
    pub(super) fn str_dict(&mut self, items: &[(&str, &str)]) -> &mut Self {
        self.array(8, |b| {
            for (k, v) in items {
                align(&mut b.0, 8);
                b.str(k).str(v);
            }
        })
    }

    /// An empty `a{sv}`.
    pub(super) fn empty_dict(&mut self) -> &mut Self {
        self.array(8, |_| {})
    }

    /// A `v` holding `value`.
    pub(super) fn variant(&mut self, value: &Value) -> &mut Self {
        match value {
            Value::Str(s) => self.sig("s").str(s),
            Value::I32(v) => self.sig("i").i32(*v),
            Value::U32(v) => self.sig("u").u32(*v),
            Value::F64(v) => self.sig("d").f64(*v),
            Value::Obj(bus, path) => self.sig("(so)").obj(bus, path),
        }
    }
}

/// A value carried in a variant (a property, or an event's `any_data`).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    Str(String),
    I32(i32),
    U32(u32),
    F64(f64),
    /// An object reference: a bus name and a path.
    Obj(String, String),
}

/// Reads a method call's arguments in order.
pub(super) struct Args<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Args<'a> {
    pub(super) fn new(buf: &'a [u8]) -> Self {
        Self { buf, off: 0 }
    }

    pub(super) fn u32(&mut self) -> Option<u32> {
        read_u32(self.buf, &mut self.off).ok()
    }

    pub(super) fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }

    pub(super) fn str(&mut self) -> Option<String> {
        read_string(self.buf, &mut self.off).ok()
    }

    /// A variant holding an `i` (`Properties.Set` of the application's id).
    pub(super) fn variant_i32(&mut self) -> Option<i32> {
        let len = *self.buf.get(self.off)? as usize;
        let sig = self.buf.get(self.off + 1..self.off + 1 + len)?;
        if sig != b"i" {
            return None;
        }
        self.off += len + 2;
        align_to(&mut self.off, 4);
        self.i32()
    }
}
