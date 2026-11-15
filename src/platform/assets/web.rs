//! The web: packaged files are on the server, under the page's assets URL
//! (`stipple.js`'s `assets` option), and every read is a `fetch`. The page
//! fetches the ones in its `preload` list before it starts the app; those
//! are kept here, for the blocking calls. Others come through
//! [`load`], which asks the page and waits for its answer.

use super::AssetError;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

type Done = Box<dyn FnOnce(Result<Vec<u8>, AssetError>)>;

thread_local! {
    /// What the page preloaded, by path.
    static PRELOADED: RefCell<BTreeMap<String, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
    static PENDING: RefCell<HashMap<u32, Done>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

pub(super) fn read(path: &str) -> Result<Vec<u8>, AssetError> {
    PRELOADED
        .with(|p| p.borrow().get(path).cloned())
        .ok_or_else(|| AssetError::NotPreloaded(path.into()))
}

pub(super) fn exists(path: &str) -> bool {
    PRELOADED.with(|p| p.borrow().contains_key(path))
}

/// The preloaded files directly in `dir`.
pub(super) fn list(dir: &str) -> Vec<String> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    PRELOADED.with(|p| {
        p.borrow()
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter(|rest| !rest.contains('/'))
            .map(str::to_owned)
            .collect()
    })
}

pub(super) fn load(path: String, done: Done) {
    if let Ok(bytes) = read(&path) {
        return done(Ok(bytes));
    }
    let id = NEXT.with(|n| {
        let id = n.get();
        // 0 is the page's preloads.
        n.set(id.checked_add(1).unwrap_or(1));
        id
    });
    PENDING.with(|p| p.borrow_mut().insert(id, done));
    crate::platform::backend::web::fetch_asset(id, &path);
}

/// The page's answer for request `id`; 0 is a preload.
pub(crate) fn fetched(id: u32, path: String, data: Vec<u8>, status: u32) {
    let result = match status {
        200 => Ok(data),
        404 | 410 => Err(AssetError::NotFound(path.clone())),
        0 => Err(AssetError::Io {
            path: path.clone(),
            message: "the fetch failed".into(),
        }),
        s => Err(AssetError::Io {
            path: path.clone(),
            message: format!("HTTP status {s}"),
        }),
    };
    if id == 0 {
        match result {
            Ok(bytes) => {
                PRELOADED.with(|p| p.borrow_mut().insert(path, bytes));
            }
            Err(e) => crate::platform::backend::web::log(&format!("stipple: {e}"), true),
        }
        return;
    }
    if let Some(done) = PENDING.with(|p| p.borrow_mut().remove(&id)) {
        done(result);
    }
}
