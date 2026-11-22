//! A folder picked with the Storage Access Framework's
//! `ACTION_OPEN_DOCUMENT_TREE`, for
//! [`dialog::pick_folder_then`](crate::platform::dialog::pick_folder_then):
//! a document tree, with no path, whose documents are listed, read, written
//! and made through `DocumentsContract` and the `ContentResolver`
//! (`query` over the tree's child-documents URI, `openFileDescriptor`,
//! `createDocument`). The picker itself goes through the results fragment,
//! as the document pickers do (see [`pick`](super::pick)).
//!
//! A [`DocumentTree`] holds global references to the tree's URI and the
//! resolver, and the VM; each call attaches the calling thread if it must,
//! so a [`Folder`](crate::platform::dialog::Folder) works from any thread.

use std::ffi::c_void;
use std::io::{self, Read, Write};

use super::jni::{self, JObject, JValue, Jni};

const CONTRACT: &std::ffi::CStr = c"android/provider/DocumentsContract";
const URI_STRING: &std::ffi::CStr = c"(Landroid/net/Uri;Ljava/lang/String;)Landroid/net/Uri;";

/// `DocumentsContract.Document.MIME_TYPE_DIR`.
const MIME_DIR: &str = "vnd.android.document/directory";

/// A picked document tree: the tree's URI and the resolver, as global
/// references, and the VM to reach them from any thread.
pub(crate) struct DocumentTree {
    vm: usize,
    resolver: usize,
    tree: usize,
}

// Global references and the VM are usable from any thread.
unsafe impl Send for DocumentTree {}
unsafe impl Sync for DocumentTree {}

impl Drop for DocumentTree {
    fn drop(&mut self) {
        let (resolver, tree) = (self.resolver as JObject, self.tree as JObject);
        unsafe {
            jni::with_env(self.vm as *mut c_void, |jni| {
                jni.delete_global(resolver);
                jni.delete_global(tree);
                Some(())
            })
        };
    }
}

/// A document directly in a tree's folder.
pub(crate) struct Child {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) size: Option<u64>,
}

fn failed(what: &str) -> io::Error {
    io::Error::other(format!(
        "{what} failed (the document provider refused or threw)"
    ))
}

impl DocumentTree {
    /// Hold `tree` (the URI the picker returned) and `resolver` (both local
    /// references of `jni`'s thread). Returns the tree and its root
    /// document's id and display name.
    pub(super) fn new(
        jni: &Jni,
        vm: *mut c_void,
        resolver: JObject,
        tree: JObject,
    ) -> Option<(DocumentTree, String, String)> {
        let root = jni.call_static_object(
            CONTRACT,
            c"getTreeDocumentId",
            c"(Landroid/net/Uri;)Ljava/lang/String;",
            &[JValue { l: tree }],
        )?;
        let root = jni.to_rust16(root)?;
        let uri = document_uri(jni, tree, &root)?;
        let name = super::pick::display_name(jni, resolver, uri).unwrap_or_else(|| {
            // "primary:Documents/notes" names the folder last.
            root.rsplit(['/', ':']).next().unwrap_or(&root).to_string()
        });
        let held = DocumentTree {
            vm: vm as usize,
            resolver: jni.global(resolver) as usize,
            tree: jni.global(tree) as usize,
        };
        Some((held, root, name))
    }

    /// Run `f` with an env for this thread, the resolver and the tree.
    fn with<R>(
        &self,
        what: &str,
        f: impl FnOnce(&Jni, JObject, JObject) -> Option<R>,
    ) -> io::Result<R> {
        let (resolver, tree) = (self.resolver as JObject, self.tree as JObject);
        unsafe { jni::with_env(self.vm as *mut c_void, |jni| f(jni, resolver, tree)) }
            .ok_or_else(|| failed(what))
    }

    /// Document `id`'s URI, as a string.
    pub(crate) fn uri(&self, id: &str) -> Option<String> {
        self.with("the document's URI", |jni, _, tree| {
            let uri = document_uri(jni, tree, id)?;
            let s = jni.call_object(uri, c"toString", c"()Ljava/lang/String;", &[])?;
            jni.to_rust16(s)
        })
        .ok()
    }

    /// The documents directly in folder `id`.
    pub(crate) fn children(&self, id: &str) -> io::Result<Vec<Child>> {
        self.with("listing the folder", |jni, resolver, tree| {
            let parent = jni.string16(id)?;
            let uri = jni.call_static_object(
                CONTRACT,
                c"buildChildDocumentsUriUsingTree",
                URI_STRING,
                &[JValue { l: tree }, JValue { l: parent }],
            )?;
            let columns =
                jni.string_array(&["document_id", "_display_name", "mime_type", "_size"])?;
            let none = std::ptr::null_mut();
            let cursor = jni.call_object(
                resolver,
                c"query",
                c"(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
                &[
                    JValue { l: uri },
                    JValue { l: columns },
                    JValue { l: none },
                    JValue { l: none },
                    JValue { l: none },
                ],
            )?;
            let mut out = Vec::new();
            while jni.call_bool(cursor, c"moveToNext", c"()Z", &[]) == Some(true) {
                if !jni.push_frame() {
                    break;
                }
                let text = |i: i32| {
                    let s = jni.call_object(
                        cursor,
                        c"getString",
                        c"(I)Ljava/lang/String;",
                        &[JValue { i }],
                    )?;
                    jni.to_rust16(s)
                };
                let (id, name, mime) = (text(0), text(1), text(2));
                let size = (jni.call_bool(cursor, c"isNull", c"(I)Z", &[JValue { i: 3 }])
                    == Some(false))
                .then(|| jni.call_long(cursor, c"getLong", c"(I)J", &[JValue { i: 3 }]))
                .flatten()
                .and_then(|n| u64::try_from(n).ok());
                jni.pop_frame();
                if let (Some(id), Some(name)) = (id, name) {
                    let is_dir = mime.as_deref() == Some(MIME_DIR);
                    out.push(Child {
                        id,
                        name,
                        is_dir,
                        size: if is_dir { None } else { size },
                    });
                }
            }
            jni.call_void(cursor, c"close", c"()V", &[]);
            Some(out)
        })
    }

    /// The whole of document `id`.
    pub(crate) fn read(&self, id: &str) -> io::Result<Vec<u8>> {
        let mut file = self.with("opening the document", |jni, resolver, tree| {
            let uri = document_uri(jni, tree, id)?;
            super::pick::open_fd(jni, resolver, uri, "r")
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// Replace what document `id` holds with `bytes`.
    pub(crate) fn write(&self, id: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.with("opening the document", |jni, resolver, tree| {
            let uri = document_uri(jni, tree, id)?;
            // "wt" truncates; a provider without it gets "w".
            super::pick::open_fd(jni, resolver, uri, "wt")
                .or_else(|| super::pick::open_fd(jni, resolver, uri, "w"))
        })?;
        file.write_all(bytes)?;
        file.flush()
    }

    /// Make a document called `name` of type `mime` (a folder for
    /// [`MIME_DIR`]) in folder `parent`; its id.
    pub(crate) fn create(&self, parent: &str, name: &str, mime: &str) -> io::Result<String> {
        self.with("making the document", |jni, resolver, tree| {
            let parent = document_uri(jni, tree, parent)?;
            let mime = jni.string(mime)?;
            let name = jni.string16(name)?;
            let made = jni.call_static_object(
                CONTRACT,
                c"createDocument",
                c"(Landroid/content/ContentResolver;Landroid/net/Uri;Ljava/lang/String;Ljava/lang/String;)Landroid/net/Uri;",
                &[
                    JValue { l: resolver },
                    JValue { l: parent },
                    JValue { l: mime },
                    JValue { l: name },
                ],
            )?;
            let id = jni.call_static_object(
                CONTRACT,
                c"getDocumentId",
                c"(Landroid/net/Uri;)Ljava/lang/String;",
                &[JValue { l: made }],
            )?;
            jni.to_rust16(id)
        })
    }
}

/// `DocumentsContract.buildDocumentUriUsingTree(tree, id)`.
fn document_uri(jni: &Jni, tree: JObject, id: &str) -> Option<JObject> {
    let id = jni.string16(id)?;
    jni.call_static_object(
        CONTRACT,
        c"buildDocumentUriUsingTree",
        URI_STRING,
        &[JValue { l: tree }, JValue { l: id }],
    )
}
