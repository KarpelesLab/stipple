//! System font discovery, by hand on every OS: the font directories are
//! scanned and each file's headers read (not the whole font) to learn its
//! family, weight and slant.
//!
//! - Linux and BSDs: the directories fontconfig's configuration lists
//!   (`/etc/fonts/fonts.conf` and its includes), and its family aliases.
//! - Windows: `%WINDIR%\Fonts` and the per-user
//!   `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
//! - macOS: `/System/Library/Fonts`, `/Library/Fonts`, `~/Library/Fonts`;
//!   iOS: `/System/Library/Fonts`.
//! - Android: `/system/fonts` and `/product/fonts`.
//! - The web: none (the app ships its fonts).
//!
//! Extra directories can be listed in `STIPPLE_FONT_DIRS` (separated like
//! `PATH`); they are searched first.

use super::fontconfig::{self, Alias};
use super::sfnt::{self, FaceInfo, FileSource};
use super::{FontFamilyInfo, FontStyle, FontWeight};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One installed face.
#[derive(Clone, Debug)]
pub(crate) struct SystemFace {
    pub path: PathBuf,
    pub index: u32,
    pub info: FaceInfo,
}

/// Every installed face, by family.
#[derive(Debug, Default)]
pub(crate) struct Index {
    faces: Vec<SystemFace>,
    /// Normalized family name → indices into `faces`.
    by_family: HashMap<String, Vec<usize>>,
    /// Normalized family name → fontconfig alias rules.
    aliases: HashMap<String, Alias>,
}

/// The process-wide index, built on first use.
pub(crate) fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let (dirs, aliases) = font_dirs();
        Index::new(scan(&dirs), aliases)
    })
}

/// Family names compare ignoring case and spaces, as fontconfig does.
pub(crate) fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

impl Index {
    pub(crate) fn new(faces: Vec<SystemFace>, aliases: HashMap<String, Alias>) -> Self {
        let mut by_family: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, f) in faces.iter().enumerate() {
            // CFF-outline faces can't be shaped by the face chain yet.
            if f.info.cff || f.info.family.is_empty() {
                continue;
            }
            by_family
                .entry(normalize(&f.info.family))
                .or_default()
                .push(i);
        }
        let aliases = aliases
            .into_iter()
            .map(|(k, v)| (normalize(&k), v))
            .collect();
        Self {
            faces,
            by_family,
            aliases,
        }
    }

    /// The family `name` resolves to and its faces: the family itself if
    /// installed, else through fontconfig's aliases and this platform's
    /// generic families (`monospace` and so on).
    pub(crate) fn resolve(&self, name: &str) -> Option<(String, Vec<&SystemFace>)> {
        self.resolve_at(name, 0)
    }

    fn resolve_at(&self, name: &str, depth: u32) -> Option<(String, Vec<&SystemFace>)> {
        if depth > 6 {
            return None;
        }
        let key = normalize(name);
        let key = match key.as_str() {
            "sans" | "sansserif" => "sans-serif".to_string(),
            "mono" => "monospace".to_string(),
            _ => key,
        };
        let alias = self.aliases.get(&key);
        let builtin = generic(&key);
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(a) = alias {
            candidates.extend(a.prefer.iter().map(String::as_str));
        }
        candidates.push(name);
        if let Some(a) = alias {
            candidates.extend(a.accept.iter().map(String::as_str));
        }
        candidates.extend(builtin.iter().copied());
        if let Some(a) = alias {
            candidates.extend(a.default.iter().map(String::as_str));
        }
        for c in candidates {
            let ck = normalize(c);
            if let Some(ids) = self.by_family.get(&ck) {
                let faces: Vec<&SystemFace> = ids.iter().map(|&i| &self.faces[i]).collect();
                return Some((faces[0].info.family.clone(), faces));
            }
            if ck != key
                && let Some(found) = self.resolve_at(c, depth + 1)
            {
                return Some(found);
            }
        }
        None
    }

    /// Every family, sorted by name.
    pub(crate) fn families(&self) -> Vec<FontFamilyInfo> {
        let mut out: Vec<FontFamilyInfo> = self
            .by_family
            .values()
            .map(|ids| {
                let faces: Vec<&SystemFace> = ids.iter().map(|&i| &self.faces[i]).collect();
                let mut styles: Vec<FontStyle> = faces
                    .iter()
                    .map(|f| FontStyle {
                        weight: FontWeight(f.info.weight),
                        italic: f.info.italic,
                    })
                    .collect();
                styles.sort_by_key(|s| (s.italic, s.weight));
                styles.dedup();
                FontFamilyInfo {
                    name: faces[0].info.family.clone(),
                    monospace: faces.iter().any(|f| f.info.monospace),
                    styles,
                }
            })
            .collect();
        out.sort_by_key(|f| f.name.to_lowercase());
        out
    }
}

/// Built-in candidates for a generic family on this platform, tried after
/// any fontconfig aliases.
fn generic(key: &str) -> &'static [&'static str] {
    let windows = cfg!(windows);
    let apple = cfg!(any(target_os = "macos", target_os = "ios"));
    let android = cfg!(target_os = "android");
    match key {
        "sans-serif" | "system-ui" => {
            if windows {
                &["Segoe UI", "Arial", "Tahoma"]
            } else if apple {
                &["Helvetica Neue", "Helvetica", "Arial", "Geneva"]
            } else if android {
                &["Roboto", "Noto Sans", "Droid Sans"]
            } else {
                &[
                    "DejaVu Sans",
                    "Liberation Sans",
                    "Noto Sans",
                    "Cantarell",
                    "Ubuntu",
                    "FreeSans",
                ]
            }
        }
        "serif" => {
            if windows {
                &["Times New Roman", "Cambria", "Georgia"]
            } else if apple {
                &["Times New Roman", "Times", "Georgia"]
            } else if android {
                &["Noto Serif", "Droid Serif"]
            } else {
                &[
                    "DejaVu Serif",
                    "Liberation Serif",
                    "Noto Serif",
                    "FreeSerif",
                ]
            }
        }
        "monospace" => {
            if windows {
                &["Cascadia Mono", "Consolas", "Lucida Console", "Courier New"]
            } else if apple {
                &["SF Mono", "Menlo", "Monaco", "Courier New", "Courier"]
            } else if android {
                &[
                    "Droid Sans Mono",
                    "Roboto Mono",
                    "Cutive Mono",
                    "Noto Sans Mono",
                ]
            } else {
                &[
                    "DejaVu Sans Mono",
                    "Liberation Mono",
                    "Noto Sans Mono",
                    "Ubuntu Mono",
                    "FreeMono",
                ]
            }
        }
        "emoji" => {
            if windows {
                &["Segoe UI Emoji"]
            } else if apple {
                &["Apple Color Emoji"]
            } else {
                &[
                    "Noto Color Emoji",
                    "Twemoji",
                    "Twitter Color Emoji",
                    "JoyPixels",
                    "Noto Emoji",
                    "Symbola",
                ]
            }
        }
        "symbol" => {
            if windows {
                &["Segoe UI Symbol"]
            } else if apple {
                &["Apple Symbols"]
            } else if android {
                &["Noto Sans Symbols", "Noto Sans Symbols 2"]
            } else {
                &[
                    "DejaVu Sans",
                    "Noto Sans Symbols",
                    "Noto Sans Symbols2",
                    "Symbola",
                    "FreeSerif",
                ]
            }
        }
        "cjk" => {
            if windows {
                &[
                    "Microsoft YaHei",
                    "Yu Gothic",
                    "Malgun Gothic",
                    "MS Gothic",
                    "SimSun",
                ]
            } else if apple {
                &[
                    "Hiragino Sans",
                    "PingFang SC",
                    "Apple SD Gothic Neo",
                    "STHeiti",
                    "Heiti SC",
                ]
            } else {
                &[
                    "Noto Sans CJK SC",
                    "Source Han Sans",
                    "Droid Sans Fallback",
                    "WenQuanYi Micro Hei",
                    "WenQuanYi Zen Hei",
                    "VL Gothic",
                    "IPAGothic",
                    "IPAPGothic",
                    "Takao Gothic",
                    "Baekmuk Gulim",
                    "UnDotum",
                    "NanumGothic",
                ]
            }
        }
        _ => &[],
    }
}

/// This platform's font directories, and fontconfig's aliases where it has
/// them.
fn font_dirs() -> (Vec<PathBuf>, HashMap<String, Alias>) {
    let mut dirs: Vec<PathBuf> = std::env::var_os("STIPPLE_FONT_DIRS")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    let mut aliases = HashMap::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_family = "wasm") {
        // No file system: only fonts the app loads itself.
    } else if cfg!(windows) {
        let windir = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        dirs.push(windir.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        dirs.push("/Network/Library/Fonts".into());
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
        }
    } else if cfg!(target_os = "android") {
        dirs.push("/system/fonts".into());
        dirs.push("/product/fonts".into());
    } else if let Some(config) = fontconfig::load_system() {
        dirs.extend(config.dirs);
        aliases = config.aliases;
    } else {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(h) = &home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
    }
    (dirs, aliases)
}

/// Read the headers of every font file under `dirs`, recursively.
pub(crate) fn scan(dirs: &[PathBuf]) -> Vec<SystemFace> {
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in dirs {
        collect(d, 0, &mut files, &mut seen);
    }
    let mut out = Vec::new();
    for path in files {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut src = FileSource(file);
        for (index, info) in sfnt::read_faces(&mut src) {
            out.push(SystemFace {
                path: path.clone(),
                index,
                info,
            });
        }
    }
    out
}

fn collect(
    dir: &Path,
    depth: u32,
    out: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    if depth > 8 {
        return;
    }
    // Directories reached twice (symlinks, nested config entries) are read once.
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, depth + 1, out, seen);
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "ttf" | "ttc" | "otf" | "otc"
            )
        }) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, weight: u16, italic: bool, mono: bool) -> SystemFace {
        SystemFace {
            path: format!("/fonts/{family}-{weight}-{italic}.ttf").into(),
            index: 0,
            info: FaceInfo {
                family: family.into(),
                weight,
                italic,
                monospace: mono,
                ..FaceInfo::default()
            },
        }
    }

    fn index() -> Index {
        let faces = vec![
            face("JetBrains Mono", 400, false, true),
            face("JetBrains Mono", 700, false, true),
            face("JetBrains Mono", 400, true, true),
            face("Liberation Sans", 400, false, false),
            face("DejaVu Sans Mono", 400, false, true),
        ];
        let mut aliases = HashMap::new();
        aliases.insert(
            "Arial".to_string(),
            Alias {
                accept: vec!["Liberation Sans".into()],
                ..Alias::default()
            },
        );
        aliases.insert(
            "monospace".to_string(),
            Alias {
                prefer: vec!["Missing Mono".into(), "JetBrains Mono".into()],
                ..Alias::default()
            },
        );
        Index::new(faces, aliases)
    }

    #[test]
    fn families_match_ignoring_case_and_spaces() {
        let idx = index();
        let (name, faces) = idx.resolve("jetbrainsmono").unwrap();
        assert_eq!(name, "JetBrains Mono");
        assert_eq!(faces.len(), 3);
        assert!(idx.resolve("Nonexistent Font").is_none());
    }

    #[test]
    fn aliases_and_generics_resolve_in_order() {
        let idx = index();
        // fontconfig's prefer list: the missing family is skipped.
        assert_eq!(idx.resolve("monospace").unwrap().0, "JetBrains Mono");
        assert_eq!(idx.resolve("mono").unwrap().0, "JetBrains Mono");
        // A metric alias stands in for a missing family.
        assert_eq!(idx.resolve("Arial").unwrap().0, "Liberation Sans");
        // Without aliases, the built-in generic list applies.
        let bare = Index::new(index().faces, HashMap::new());
        if cfg!(all(target_os = "linux", not(target_os = "android"))) {
            assert_eq!(bare.resolve("monospace").unwrap().0, "DejaVu Sans Mono");
            assert_eq!(bare.resolve("sans-serif").unwrap().0, "Liberation Sans");
        }
    }

    #[test]
    fn families_list_styles_and_spacing() {
        let list = index().families();
        let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["DejaVu Sans Mono", "JetBrains Mono", "Liberation Sans"]
        );
        let jb = &list[1];
        assert!(jb.monospace);
        assert_eq!(
            jb.styles,
            [FontStyle::NORMAL, FontStyle::BOLD, FontStyle::ITALIC]
        );
        assert!(!list[2].monospace);
    }

    #[test]
    fn scanning_reads_font_files_in_nested_directories() {
        let dir = std::env::temp_dir().join(format!("stipple-fontscan-{}", std::process::id()));
        let nested = dir.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        let bytes = super::super::sfnt::tests::fake_face("Scan Test", "Bold", 700, false, false);
        std::fs::write(nested.join("ScanTest-Bold.TTF"), &bytes).unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a font").unwrap();
        std::fs::write(dir.join("broken.ttf"), b"garbage").unwrap();
        let faces = scan(&[dir.clone(), dir.join("a")]);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(faces.len(), 1, "{faces:?}");
        assert_eq!(faces[0].info.family, "Scan Test");
        assert_eq!(faces[0].info.weight, 700);
    }

    /// On a host with fonts, discovery finds a usable monospace family, and
    /// its bold is a real face rather than a synthesized one when the family
    /// has one.
    #[test]
    fn real_system_monospace_has_a_bold_face() {
        let Ok(mono) = crate::render::Font::from_family("monospace", FontStyle::NORMAL) else {
            eprintln!("skipping: no system monospace font");
            return;
        };
        assert!(mono.is_monospace());
        // Asking again shares the loaded family.
        let again = crate::render::Font::from_family("monospace", FontStyle::NORMAL).unwrap();
        assert_eq!(again, mono);
        assert_eq!(again.bold(), mono.bold());
        let bold = mono.bold();
        assert_eq!(bold.style(), FontStyle::BOLD);
        let has_bold = mono
            .available_styles()
            .iter()
            .any(|s| s.weight.0 >= 600 && !s.italic);
        assert_eq!(bold.is_synthesized().0, !has_bold);
        assert!(!crate::render::Font::families().is_empty());
    }
}
