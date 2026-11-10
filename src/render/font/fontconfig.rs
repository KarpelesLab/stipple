//! Reading fontconfig's configuration by hand (no `libfontconfig`): the font
//! directories (`<dir>`) and family aliases (`<alias>`, e.g. the preferred
//! faces for `monospace`, or Liberation Sans standing in for Arial), following
//! `<include>`s. Match rules (`<match>`) and the binary caches are not read; the
//! directories are scanned instead (see `system.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the configuration says.
#[derive(Debug, Default)]
pub(crate) struct Config {
    /// Font directories, in the order they appear.
    pub dirs: Vec<PathBuf>,
    /// Lower-cased family → alias rules, merged in file order.
    pub aliases: HashMap<String, Alias>,
}

/// One family's substitutes: `prefer` goes before the family itself,
/// `accept` after it, `default` last.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Alias {
    pub prefer: Vec<String>,
    pub accept: Vec<String>,
    pub default: Vec<String>,
}

/// Where fontconfig keeps per-user files: `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME`
/// and `~`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Env {
    pub home: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub xdg_data: Option<PathBuf>,
}

impl Env {
    pub fn from_process() -> Self {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME");
        Self {
            xdg_config: var("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|h| h.join(".config"))),
            xdg_data: var("XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".local/share"))),
            home,
        }
    }

    fn expand(&self, text: &str, prefix: Option<&str>, base: &Path, data: bool) -> Option<PathBuf> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if prefix == Some("xdg") {
            let root = if data {
                &self.xdg_data
            } else {
                &self.xdg_config
            };
            return root.as_ref().map(|r| r.join(text));
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return self.home.as_ref().map(|h| h.join(rest));
        }
        if text == "~" {
            return self.home.clone();
        }
        let p = Path::new(text);
        Some(if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        })
    }
}

/// Load the system configuration: `$FONTCONFIG_FILE`, else
/// `/etc/fonts/fonts.conf`. Returns `None` when there is none.
pub(crate) fn load_system() -> Option<Config> {
    let env = Env::from_process();
    let file = std::env::var_os("FONTCONFIG_FILE")
        .map(PathBuf::from)
        .map(|f| {
            if f.is_absolute() {
                f
            } else {
                std::env::var_os("FONTCONFIG_PATH")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/etc/fonts"))
                    .join(f)
            }
        })
        .unwrap_or_else(|| PathBuf::from("/etc/fonts/fonts.conf"));
    let text = std::fs::read_to_string(&file).ok()?;
    let mut config = Config::default();
    let base = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    parse_into(
        &mut config,
        &text,
        &base,
        &env,
        &mut |p| std::fs::read_to_string(p).ok(),
        0,
    );
    Some(config)
}

/// One XML event.
#[derive(Debug, PartialEq)]
enum Event<'a> {
    Open(&'a str, Vec<(&'a str, String)>, bool),
    Close(&'a str),
    Text(String),
}

/// A forgiving XML tokenizer: elements, attributes and text, with comments,
/// processing instructions, `<!DOCTYPE>` and CDATA markers skipped.
fn events(src: &str) -> Vec<Event<'_>> {
    let mut out = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut out, rest);
            break;
        };
        push_text(&mut out, &rest[..lt]);
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            out.push(Event::Close(name.trim()));
            continue;
        }
        let (tag, empty) = match tag.strip_suffix('/') {
            Some(t) => (t, true),
            None => (tag, false),
        };
        let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let name = &tag[..name_end];
        let mut attrs = Vec::new();
        let mut a = &tag[name_end..];
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim();
            let v = a[eq + 1..].trim_start();
            let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(end) = v[1..].find(q) else { break };
            attrs.push((key, unescape(&v[1..1 + end])));
            a = &v[end + 2..];
        }
        out.push(Event::Open(name, attrs, empty));
    }
    out
}

fn push_text(out: &mut Vec<Event<'_>>, text: &str) {
    if !text.trim().is_empty() {
        out.push(Event::Text(unescape(text)));
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse one configuration file's text into `config`. `base` resolves
/// relative paths; `read` loads included files (a seam for tests).
pub(crate) fn parse_into(
    config: &mut Config,
    text: &str,
    base: &Path,
    env: &Env,
    read: &mut dyn FnMut(&Path) -> Option<String>,
    depth: u32,
) {
    if depth > 8 {
        return;
    }
    let ev = events(text);
    // Element names from the root down to the current one.
    let mut stack: Vec<(&str, Option<String>)> = Vec::new();
    let mut alias: Option<(Vec<String>, Alias)> = None;
    for e in ev {
        match e {
            Event::Open(name, attrs, empty) => {
                let prefix = attrs
                    .iter()
                    .find(|(k, _)| *k == "prefix")
                    .map(|(_, v)| v.clone());
                if name == "alias" && !empty {
                    alias = Some((Vec::new(), Alias::default()));
                }
                if !empty {
                    stack.push((name, prefix));
                }
            }
            Event::Close(name) => {
                if let Some(at) = stack.iter().rposition(|(n, _)| *n == name) {
                    stack.truncate(at);
                }
                if name == "alias"
                    && let Some((families, rule)) = alias.take()
                {
                    for f in families {
                        let entry = config.aliases.entry(f.to_lowercase()).or_default();
                        // As fontconfig applies them: each rule's `prefer`
                        // lands just before the family (after earlier
                        // rules' prefers), its `accept` just after it
                        // (before earlier accepts), its `default` last.
                        entry.prefer.extend(rule.prefer.iter().cloned());
                        let mut accept = rule.accept.clone();
                        accept.append(&mut entry.accept);
                        entry.accept = accept;
                        entry.default.extend(rule.default.iter().cloned());
                    }
                }
            }
            Event::Text(t) => {
                let Some((name, prefix)) = stack.last() else {
                    continue;
                };
                let parent = stack.len().checked_sub(2).map(|i| stack[i].0);
                match (*name, parent) {
                    ("dir", _) => {
                        if let Some(p) = env.expand(&t, prefix.as_deref(), base, true)
                            && !config.dirs.contains(&p)
                        {
                            config.dirs.push(p);
                        }
                    }
                    ("include", _) => {
                        if let Some(p) = env.expand(&t, prefix.as_deref(), base, false) {
                            include(config, &p, env, read, depth);
                        }
                    }
                    ("family", Some("alias")) => {
                        if let Some((families, _)) = &mut alias {
                            families.push(t.trim().to_string());
                        }
                    }
                    ("family", Some(kind @ ("prefer" | "accept" | "default"))) => {
                        if let Some((_, rule)) = &mut alias {
                            let list = match kind {
                                "prefer" => &mut rule.prefer,
                                "accept" => &mut rule.accept,
                                _ => &mut rule.default,
                            };
                            list.push(t.trim().to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Follow an `<include>`: a file, or a directory of `NN-name.conf` files read
/// in name order.
fn include(
    config: &mut Config,
    path: &Path,
    env: &Env,
    read: &mut dyn FnMut(&Path) -> Option<String>,
    depth: u32,
) {
    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.retain(|f| {
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with(".conf") && name.starts_with(|c: char| c.is_ascii_digit())
        });
        files.sort();
        for f in files {
            include(config, &f, env, read, depth + 1);
        }
        return;
    }
    if let Some(text) = read(path) {
        let base = path.parent().unwrap_or(Path::new("/")).to_path_buf();
        parse_into(config, &text, &base, env, read, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            home: Some("/home/u".into()),
            xdg_config: Some("/home/u/.config".into()),
            xdg_data: Some("/home/u/.local/share".into()),
        }
    }

    const MAIN: &str = r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <!-- <dir>/commented/out</dir> -->
  <dir>/usr/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>~/.fonts</dir>
  <include ignore_missing="yes">extra.conf</include>
  <alias>
    <family>monospace</family>
    <prefer><family>DejaVu Sans Mono</family><family>Liberation Mono</family></prefer>
  </alias>
  <alias binding="same">
    <family>Arial</family>
    <accept><family>Liberation Sans</family></accept>
    <default><family>sans-serif</family></default>
  </alias>
  <match target="pattern"><test name="family"><string>mono</string></test></match>
</fontconfig>"#;

    const EXTRA: &str = r#"<fontconfig>
  <dir>fonts-here</dir>
  <alias><family>monospace</family><prefer><family>JetBrains Mono</family></prefer></alias>
</fontconfig>"#;

    fn parse() -> Config {
        let mut config = Config::default();
        let mut read = |p: &Path| (p == Path::new("/etc/fonts/extra.conf")).then(|| EXTRA.into());
        parse_into(
            &mut config,
            MAIN,
            Path::new("/etc/fonts"),
            &env(),
            &mut read,
            0,
        );
        config
    }

    #[test]
    fn dirs_expand_home_xdg_and_relative_paths() {
        let c = parse();
        let dirs: Vec<&str> = c.dirs.iter().map(|d| d.to_str().unwrap()).collect();
        assert_eq!(
            dirs,
            [
                "/usr/share/fonts",
                "/home/u/.local/share/fonts",
                "/home/u/.fonts",
                "/etc/fonts/fonts-here",
            ]
        );
    }

    #[test]
    fn earlier_alias_rules_win() {
        let c = parse();
        let mono = &c.aliases["monospace"];
        // The include is read where it appears, before the main file's alias,
        // so its preference comes first (as a user's 50-user.conf beats the
        // distribution's 60-latin.conf).
        assert_eq!(
            mono.prefer,
            ["JetBrains Mono", "DejaVu Sans Mono", "Liberation Mono"]
        );
        let arial = &c.aliases["arial"];
        assert_eq!(arial.accept, ["Liberation Sans"]);
        assert_eq!(arial.default, ["sans-serif"]);
    }

    #[test]
    fn tokenizer_handles_attributes_entities_and_self_closing_tags() {
        let ev = events(r#"<a x="1" y='&amp;2'/><b>t &lt; u</b>"#);
        assert_eq!(
            ev,
            [
                Event::Open("a", vec![("x", "1".into()), ("y", "&2".into())], true),
                Event::Open("b", vec![], false),
                Event::Text("t < u".into()),
                Event::Close("b"),
            ]
        );
    }
}
