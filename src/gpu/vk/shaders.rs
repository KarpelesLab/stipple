//! The Vulkan scene shaders: SPIR-V compiled from the GLSL next to it in
//! `src/gpu/shaders/` by `tools/shaders.sh` (glslang, pinned; CI recompiles
//! them and fails if the bytes differ), checked in so the crate needs no
//! shader compiler.
//!
//! The tests read the modules back and check that their push-constant
//! offsets and descriptor bindings are the ones `record.rs` and the executor
//! use.

pub(crate) const SCENE_VERT: &[u8] = include_bytes!("../shaders/scene.vert.spv");
pub(crate) const BOX_FRAG: &[u8] = include_bytes!("../shaders/box.frag.spv");
pub(crate) const TEX_FRAG: &[u8] = include_bytes!("../shaders/tex.frag.spv");
pub(crate) const FLAT_FRAG: &[u8] = include_bytes!("../shaders/flat.frag.spv");
pub(crate) const RESOLVE_FRAG: &[u8] = include_bytes!("../shaders/resolve.frag.spv");

/// A SPIR-V module as the 32-bit words `vkCreateShaderModule` takes
/// (`include_bytes!` only promises byte alignment).
pub(crate) fn words(spv: &[u8]) -> Vec<u32> {
    spv.as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::vk::record::*;
    use std::collections::HashMap;

    const MAGIC: u32 = 0x0723_0203;
    const OP_TYPE_POINTER: u32 = 32;
    const OP_VARIABLE: u32 = 59;
    const OP_DECORATE: u32 = 71;
    const OP_MEMBER_DECORATE: u32 = 72;
    const STORAGE_PUSH_CONSTANT: u32 = 9;
    const STORAGE_UNIFORM_CONSTANT: u32 = 0;
    const DECORATION_BINDING: u32 = 33;
    const DECORATION_DESCRIPTOR_SET: u32 = 34;
    const DECORATION_OFFSET: u32 = 35;

    /// What a module says about its push constants and descriptors.
    struct Reflect {
        /// The push-constant block's member offsets, in bytes.
        push: Vec<u32>,
        /// `(set, binding)` of each uniform-constant variable (samplers).
        bindings: Vec<(u32, u32)>,
    }

    fn reflect(spv: &[u8]) -> Reflect {
        let w = words(spv);
        assert_eq!(w[0], MAGIC, "a SPIR-V module");
        let (mut pointers, mut vars) = (HashMap::new(), Vec::new());
        let mut members: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        let mut decorations: HashMap<u32, HashMap<u32, u32>> = HashMap::new();
        let mut i = 5;
        while i < w.len() {
            let (len, op) = ((w[i] >> 16) as usize, w[i] & 0xffff);
            let a = &w[i + 1..i + len];
            match op {
                OP_TYPE_POINTER => {
                    pointers.insert(a[0], a[2]);
                }
                OP_VARIABLE => vars.push((a[0], a[2])),
                OP_MEMBER_DECORATE if a[2] == DECORATION_OFFSET => {
                    members.entry(a[0]).or_default().push((a[1], a[3]));
                }
                OP_DECORATE if a.len() > 2 => {
                    decorations.entry(a[0]).or_default().insert(a[1], a[2]);
                }
                _ => {}
            }
            i += len.max(1);
        }
        let mut push = Vec::new();
        let mut bindings = Vec::new();
        for &(ty, storage) in &vars {
            if storage == STORAGE_PUSH_CONSTANT {
                let mut m = members[&pointers[&ty]].clone();
                m.sort();
                push = m.into_iter().map(|(_, off)| off).collect();
            }
        }
        // Variables: (result type, result id, storage class) — find the
        // uniform-constant ones' decorations by their id.
        let mut i = 5;
        while i < w.len() {
            let (len, op) = ((w[i] >> 16) as usize, w[i] & 0xffff);
            let a = &w[i + 1..i + len];
            if op == OP_VARIABLE && a[2] == STORAGE_UNIFORM_CONSTANT {
                let d = &decorations[&a[1]];
                bindings.push((d[&DECORATION_DESCRIPTOR_SET], d[&DECORATION_BINDING]));
            }
            i += len.max(1);
        }
        Reflect { push, bindings }
    }

    fn bytes(floats: &[usize]) -> Vec<u32> {
        floats.iter().map(|&f| f as u32 * 4).collect()
    }

    #[test]
    fn the_vertex_shader_reads_xf_first() {
        let r = reflect(SCENE_VERT);
        assert_eq!(r.push, bytes(&[XF]));
        assert!(r.bindings.is_empty());
    }

    #[test]
    fn the_box_shader_reads_its_push_constants_where_they_are_written() {
        let r = reflect(BOX_FRAG);
        assert_eq!(
            r.push,
            bytes(&[BOX_RECT, BOX_COLOR, BOX_RADIUS, BOX_STROKE])
        );
        assert!(r.bindings.is_empty());
    }

    #[test]
    fn the_texture_shader_reads_its_push_constants_and_one_sampler() {
        let r = reflect(TEX_FRAG);
        assert_eq!(
            r.push,
            bytes(&[
                TEX_RECT,
                TEX_UV,
                TEX_CLAMP,
                TEX_TINT,
                TEX_PX,
                TEX_OPACITY,
                TEX_PREMUL,
                TEX_AA
            ])
        );
        assert_eq!(r.bindings, vec![(0, 0)]);
    }

    #[test]
    fn the_flat_and_resolve_shaders_match_too() {
        assert_eq!(reflect(FLAT_FRAG).push, bytes(&[FLAT_COLOR]));
        let r = reflect(RESOLVE_FRAG);
        assert_eq!(r.push, bytes(&[RESOLVE_COLOR, RESOLVE_INV, RESOLVE_TAP]));
        assert_eq!(r.bindings, vec![(0, 0)]);
    }

    #[test]
    fn every_push_constant_fits_the_range() {
        for spv in [SCENE_VERT, BOX_FRAG, TEX_FRAG, FLAT_FRAG, RESOLVE_FRAG] {
            let r = reflect(spv);
            // The last member is at most a vec4 (16 bytes).
            assert!(r.push.iter().all(|&o| o + 4 <= PUSH_BYTES));
        }
    }
}
