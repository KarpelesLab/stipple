//! External images on Direct3D 11: a DXGI shared handle opened as a
//! texture on the executor's device (`ID3D11Device1::OpenSharedResource1`
//! for an NT handle, `ID3D11Device::OpenSharedResource` for a legacy one)
//! and sampled through a shader resource view in the texture's own format.
//! No pixel is copied.
//!
//! Imports are cached by the image's identity, and the images the last
//! frame drew are held until the next one begins.

use super::*;
use crate::render::{ExternalFormat, ExternalImage};

/// `ID3D11Device::OpenSharedResource`.
const VT_DEVICE_OPEN_SHARED_RESOURCE: usize = 28;
/// `ID3D11Device1::OpenSharedResource1`.
const VT_DEVICE1_OPEN_SHARED_RESOURCE1: usize = 48;
/// `ID3D11Device1`.
const IID_ID3D11_DEVICE1: Guid = Guid {
    data1: 0xa04b_fb29,
    data2: 0x08ef,
    data3: 0x43d6,
    data4: [0xa4, 0x9c, 0xa9, 0xbd, 0xbd, 0xcb, 0xe6, 0x86],
};

struct Import {
    image: Weak<ExternalImage>,
    /// The texture and its view; `None` if it couldn't be opened.
    opened: Option<(Com, Com)>,
    used: u64,
}

/// The executor's imports.
#[derive(Default)]
pub(super) struct Imports {
    map: HashMap<usize, Import>,
    held: Vec<Arc<ExternalImage>>,
}

impl Imports {
    /// A frame begins: let go of the last frame's images, and free the
    /// imports of images gone or not drawn lately.
    pub(super) fn begin(&mut self, count: u64) {
        self.held.clear();
        self.map.retain(|_, i| {
            i.image.strong_count() > 0 && count.saturating_sub(i.used) < TEXTURE_KEEP_FRAMES
        });
        crate::platform::external::note_native(&[
            ExternalFormat::Rgba8,
            ExternalFormat::Bgra8,
            ExternalFormat::Rgbx8,
            ExternalFormat::Bgrx8,
        ]);
    }

    /// The shader resource view of `image`'s texture, opened on `device`
    /// unless it's cached; `None` if it isn't a shared handle or can't be
    /// opened here.
    pub(super) unsafe fn get(
        &mut self,
        device: *mut c_void,
        image: &Arc<ExternalImage>,
        count: u64,
    ) -> Option<*mut c_void> {
        let key = Arc::as_ptr(image) as usize;
        if self
            .map
            .get(&key)
            .is_some_and(|i| !i.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image)))
        {
            self.map.remove(&key);
        }
        let i = self.map.entry(key).or_insert_with(|| Import {
            image: Arc::downgrade(image),
            opened: unsafe { open(device, image) },
            used: count,
        });
        i.used = count;
        if !self.held.iter().any(|h| Arc::ptr_eq(h, image)) {
            self.held.push(image.clone());
        }
        i.opened.as_ref().map(|(_, srv)| srv.ptr())
    }
}

/// Open `image`'s shared handle on `device` and view it.
unsafe fn open(device: *mut c_void, image: &ExternalImage) -> Option<(Com, Com)> {
    use crate::platform::external::Handle;
    let Handle::Dxgi { handle, nt } = image.handle() else {
        return None;
    };
    unsafe {
        let mut tex = std::ptr::null_mut();
        let hr = if *nt {
            let mut dev1 = std::ptr::null_mut();
            let hr = com!(device, VT_QUERY_INTERFACE, (*const Guid, *mut *mut c_void) -> Hresult,
                &IID_ID3D11_DEVICE1, &mut dev1);
            if hr < 0 || dev1.is_null() {
                return None;
            }
            let dev1 = Com(dev1);
            com!(dev1.ptr(), VT_DEVICE1_OPEN_SHARED_RESOURCE1,
                (*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
                *handle, &IID_ID3D11_TEXTURE2D, &mut tex)
        } else {
            com!(device, VT_DEVICE_OPEN_SHARED_RESOURCE,
                (*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
                *handle, &IID_ID3D11_TEXTURE2D, &mut tex)
        };
        if hr < 0 || tex.is_null() {
            return None;
        }
        let tex = Com(tex);
        let srv = view(device, VT_DEVICE_CREATE_SRV, tex.ptr()).ok()?;
        Some((tex, srv))
    }
}
