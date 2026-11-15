#!/usr/bin/env python3
"""Build Stipple's site: the guides in docs/ as pages, the README as the
landing page, a gallery of the examples, and each web example running in the
browser. Standard library only (Python 3.11+, for tomllib).

    python3 tools/site/build.py wasm           # build every web example (one cargo call)
    python3 tools/site/build.py site --out target/site --font DejaVuSans.ttf
    python3 tools/site/build.py check-manifest # every example classified?
    python3 tools/site/check.py target/site    # links, examples load, thumbnails

Which examples run on the web, and how they're built, is in examples.toml.

A guide shows a running example where it says, on a line of its own,

    <!-- live: todo -->

(an HTML comment, so GitHub shows nothing). Options follow the name:
`height=360` sets the height, and `caption="…"` the caption. An image right
after the marker (a screenshot of the same example) is left out on the site,
where the example itself runs.
"""

import argparse
import html
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import md  # noqa: E402

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
REPO = "https://github.com/KarpelesLab/stipple"
BLOB = f"{REPO}/blob/master/"
TREE = f"{REPO}/tree/master/"
DOCS_RS = "https://docs.rs/stipple"
SITE = "https://karpeleslab.github.io/stipple"
LIVE = re.compile(r"^<!--\s*live:\s*([\w-]+)((?:\s+\w+=(?:\"[^\"]*\"|\S+))*)\s*-->$")


def esc(s):
    return html.escape(str(s), quote=True)


# --- The manifest ----------------------------------------------------------


class Example:
    def __init__(self, name, spec, web):
        self.name = name
        self.web = web
        self.spec = spec
        cargo = ROOT / "examples" / name / "Cargo.toml"
        meta = tomllib.loads(cargo.read_text()) if cargo.exists() else {}
        desc = meta.get("package", {}).get("description", "")
        desc = re.sub(r"^Stipple (example|spike):\s*", "", desc).strip()
        self.description = desc[:1].upper() + desc[1:] if desc else ""
        self.description = spec.get("description", self.description)
        self.title = spec.get("title", name)
        self.group = spec.get("group", "Other")
        self.size = spec.get("size", [640, 420])
        self.source = spec.get("source", "src/main.rs" if (ROOT / "examples" / name / "src/main.rs").exists() else "src/lib.rs")
        self.crate = spec.get("crate", name)
        self.artifact = spec.get("artifact", f"{name}_web")
        self.needs = spec.get("needs", "")

    @property
    def source_url(self):
        return f"{BLOB}examples/{self.name}/{self.source}"

    @property
    def dir_url(self):
        return f"{TREE}examples/{self.name}"


def load_manifest():
    data = tomllib.loads((HERE / "examples.toml").read_text())
    web = [Example(n, s, True) for n, s in data.get("web", {}).items()]
    native = [Example(n, s, False) for n, s in data.get("native", {}).items()]
    return data["build"], web, native


def check_manifest():
    _, web, native = load_manifest()
    known = {e.name for e in web + native}
    dirs = {p.name for p in (ROOT / "examples").iterdir() if (p / "Cargo.toml").exists()}
    ok = True
    for name in sorted(dirs - known):
        print(f"examples/{name} is in neither [web] nor [native] of tools/site/examples.toml")
        ok = False
    for name in sorted(known - dirs):
        print(f"tools/site/examples.toml lists {name}, which isn't in examples/")
        ok = False
    return ok


def wasm_command(build, web):
    args = " ".join(build["arg"].format(crate=e.crate, name=e.name, artifact=e.artifact) for e in web)
    return build["command"].format(args=args)


# --- Pages ------------------------------------------------------------------


class Site:
    def __init__(self, out, font, wasm_dir=None, strict=False):
        self.out = Path(out)
        self.font = font
        self.strict = strict
        self.build, self.web, self.native = load_manifest()
        self.by_name = {e.name: e for e in self.web}
        self.wasm_dir = wasm_dir
        self.problems = []
        # Guides: repository path -> site path.
        self.pages = {"README.md": "index.html", "docs/README.md": "docs/index.html"}
        for p in sorted((ROOT / "docs").rglob("*.md")):
            rel = p.relative_to(ROOT).as_posix()
            if rel != "docs/README.md":
                self.pages[rel] = rel[:-3] + ".html"
        self.nav = self.read_nav()
        self.assets = set()

    # Navigation from docs/README.md: the getting-started link, the
    # platforms list and the topics list.
    def read_nav(self):
        text = (ROOT / "docs/README.md").read_text()
        sections = [("Start here", [("Overview", "docs/README.md"), ("Getting started", "docs/getting-started.md")])]
        current = None
        for line in text.splitlines():
            h = re.match(r"^##\s+(.*)", line)
            if h:
                current = (h.group(1).strip(), [])
                sections.append(current)
                continue
            m = re.match(r"^- \[([^\]]+)\]\(([^)#]+\.md)\)", line)
            if m and current is not None:
                current[1].append((m.group(1), "docs/" + m.group(2)))
        return [(title, items) for title, items in sections if items]

    def rel(self, frm, to):
        """A relative URL from site page `frm` to site path `to`."""
        base = os.path.dirname(frm)
        r = os.path.relpath(to, base or ".")
        return r.replace(os.sep, "/")

    def resolve(self, src_repo_path, page, url, image):
        """Rewrite a link in the Markdown at `src_repo_path` (rendered to
        site page `page`)."""
        if url.startswith(SITE):
            # This site: a link within it. Its front page, from the guides,
            # meant the examples (it was the web demo).
            rest = url[len(SITE) :].lstrip("/")
            return self.rel(page, rest or "examples/index.html")
        if re.match(r"^[a-z][a-z0-9+.-]*:", url, re.I) or url.startswith("#") or not url:
            return url
        path, _, frag = url.partition("#")
        frag = "#" + frag if frag else ""
        target = os.path.normpath(os.path.join(os.path.dirname(src_repo_path), path)).replace(os.sep, "/")
        if target.startswith("../"):
            self.problems.append(f"{src_repo_path}: {url} leaves the repository")
            return url
        if target in self.pages:
            return self.rel(page, self.pages[target]) + frag
        full = ROOT / target
        if image and full.is_file() and target.startswith("docs/"):
            self.assets.add(target)
            return self.rel(page, target)
        if full.is_dir():
            return TREE + target + frag
        if full.exists():
            return BLOB + target + frag
        self.problems.append(f"{src_repo_path}: {url} doesn't exist")
        return BLOB + target + frag

    def live(self, page, line):
        m = LIVE.match(line)
        if not m:
            return None
        name = m.group(1)
        opts = dict(
            (k, v.strip('"')) for k, v in re.findall(r"(\w+)=(\"[^\"]*\"|\S+)", m.group(2) or "")
        )
        ex = self.by_name.get(name)
        if ex is None:
            self.problems.append(f"{page}: live example {name!r} isn't a web example in examples.toml")
            return ""
        return self.embed(page, ex, opts.get("caption"), int(opts.get("height", ex.size[1])))

    def embed(self, page, ex, caption=None, height=None, cls=""):
        height = height or ex.size[1]
        full = self.rel(page, f"examples/{ex.name}.html")
        cap = esc(caption) if caption else esc(ex.description)
        return f"""<figure class="live {cls}" style="--w:{ex.size[0]}px;--h:{height}px">
<div class="live-frame"><iframe src="{full}?embed" title="{esc(ex.title)}: a live Stipple example" loading="lazy" allow="gamepad; clipboard-read; clipboard-write; fullscreen"></iframe></div>
<figcaption><span class="live-badge">Live</span> <strong>{esc(ex.title)}</strong> <span class="live-desc">{cap}</span>
<span class="live-links"><a href="{esc(ex.source_url)}" rel="noopener">View source</a> · <a href="{full}">Open full page</a></span></figcaption>
</figure>"""

    def render_md(self, src_repo_path, page, text):
        # A live example stands in for the screenshot right after it.
        text = re.sub(r"^(<!--\s*live:[^\n]*-->)\n\n!\[[^\]]*\]\([^)\s]*\)\n", r"\1\n", text, flags=re.M)
        r = md.Renderer(
            link=lambda url, image: self.resolve(src_repo_path, page, url, image),
            block_hook=lambda line: self.live(page, line),
        )
        body = r.render(text)
        return body, r.headings

    # The page shell.
    def shell(self, page, title, body, *, sidebar=True, current=None, toc=None, cls="", description=""):
        r = lambda to: self.rel(page, to)  # noqa: E731
        nav = ""
        if sidebar:
            groups = []
            for sec, items in self.nav:
                lis = []
                for label, src in items:
                    href = self.pages.get(src)
                    if not href:
                        continue
                    here = ' aria-current="page"' if src == current else ""
                    lis.append(f'<li><a href="{r(href)}"{here}>{esc(label)}</a></li>')
                groups.append(f"<details open><summary>{esc(sec)}</summary><ul>{''.join(lis)}</ul></details>")
            nav = f'<nav class="sidebar" id="sidebar" aria-label="Guides">{"".join(groups)}</nav>'
        toc_html = ""
        if toc:
            items = "".join(
                f'<li class="l{lv}"><a href="#{hid}">{esc(html.unescape(t))}</a></li>' for lv, hid, t in toc if lv in (2, 3)
            )
            if items:
                toc_html = f'<aside class="toc" aria-label="On this page"><p>On this page</p><ul>{items}</ul></aside>'
        edit = ""
        if current and current in self.pages and current.endswith(".md"):
            edit = f'<p class="edit"><a href="{BLOB}{current}" rel="noopener">View this page on GitHub</a></p>'
        top = lambda label, to, key: (  # noqa: E731
            f'<a href="{r(to)}"' + (' aria-current="page"' if key == cls else "") + f">{label}</a>"
        )
        desc = esc(description or "Stipple: a cross-platform, self-drawn UI toolkit in Rust.")
        return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta name="description" content="{desc}">
<title>{esc(title)}</title>
<link rel="icon" href="{r('assets/logo.svg')}" type="image/svg+xml">
<link rel="stylesheet" href="{r('assets/site.css')}">
<script src="{r('assets/site.js')}" defer></script>
</head>
<body class="{cls}">
<a class="skip" href="#main">Skip to content</a>
<header class="topbar">
<button class="menu-toggle" aria-controls="sidebar" aria-expanded="false" aria-label="Guides" {'' if sidebar else 'hidden'}><span></span></button>
<a class="brand" href="{r('index.html')}"><img src="{r('assets/logo.svg')}" alt="" width="26" height="26"><span>Stipple</span></a>
<nav class="toplinks" aria-label="Site">
{top('Guides', 'docs/getting-started.html', 'guide')}
{top('Examples', 'examples/index.html', 'gallery')}
<a href="{DOCS_RS}" rel="noopener">API</a>
<a href="{REPO}" rel="noopener" class="gh">GitHub</a>
</nav>
</header>
<div class="layout{' with-sidebar' if sidebar else ''}{' with-toc' if toc_html else ''}">
{nav}
<main id="main">
{body}
{edit}
</main>
{toc_html}
</div>
<footer class="footer"><p>Stipple is MIT licensed. <a href="{REPO}" rel="noopener">Source</a> · <a href="{BLOB}CHANGELOG.md" rel="noopener">Changelog</a> · <a href="{BLOB}ROADMAP.md" rel="noopener">Roadmap</a> · <a href="https://crates.io/crates/stipple" rel="noopener">crates.io</a></p></footer>
</body>
</html>
"""

    def write(self, page, text):
        dest = self.out / page
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)

    # --- The pages ---

    def guide(self, src):
        page = self.pages[src]
        text = (ROOT / src).read_text()
        body, heads = self.render_md(src, page, text)
        title = heads[0][2] if heads else src
        if src == "docs/README.md":
            title = "Guides"
        return self.write(
            page,
            self.shell(page, f"{title} · Stipple", f'<article class="prose">{body}</article>', current=src, toc=heads, cls="guide"),
        )

    def landing(self):
        page = "index.html"
        text = (ROOT / "README.md").read_text()
        # The hero says what the title and badges say.
        text = re.sub(r"\A# Stipple\n+(\[!\[.*\n)+\n*", "", text)
        body, heads = self.render_md("README.md", page, text)
        hero_ex = self.by_name.get("widgetsdemo") or self.web[0]
        r = lambda to: self.rel(page, to)  # noqa: E731
        cards = "".join(self.card(page, self.by_name[n]) for n in ("todo", "gamedemo", "tabledemo", "motiondemo") if n in self.by_name)
        hero = f"""<section class="hero">
<div class="hero-text">
<h1>Stipple</h1>
<p class="tagline">A cross-platform UI toolkit in Rust that draws every pixel itself: the same app on Linux, Windows, macOS, iOS, Android and the web.</p>
<pre class="install" data-lang="sh"><code><span class="tok-k">cargo</span> add stipple</code></pre>
<p class="cta"><a class="button primary" href="{r('docs/getting-started.html')}">Get started</a>
<a class="button" href="{r('examples/index.html')}">Examples</a>
<a class="button" href="{DOCS_RS}" rel="noopener">API docs</a></p>
<p class="hero-note">Everything on this site that moves is Stipple, compiled to WebAssembly and drawn into a canvas. Click, type, scroll.</p>
</div>
<div class="hero-demo">{self.embed(page, hero_ex, height=540, cls="hero-live")}</div>
</section>
<section class="strip"><h2>Try the examples</h2><div class="cards">{cards}</div>
<p class="more"><a href="{r('examples/index.html')}">All {len(self.web)} examples in the browser →</a></p></section>
"""
        out = f'{hero}<article class="prose readme">{body}</article>'
        self.write(page, self.shell(page, "Stipple: a cross-platform UI toolkit in Rust", out, sidebar=False, cls="home"))

    def card(self, page, ex):
        run = self.rel(page, f"examples/{ex.name}.html")
        thumb = self.rel(page, f"thumbs/{ex.name}.png")
        return f"""<article class="card" id="{esc(ex.name)}">
<a class="thumb" href="{run}" tabindex="-1" aria-hidden="true"><img src="{thumb}" alt="" loading="lazy" width="{ex.size[0]}" height="{ex.size[1]}" onerror="this.parentNode.classList.add('none');this.remove()"><span class="play" aria-hidden="true"></span></a>
<div class="card-body"><h3><a href="{run}">{esc(ex.title)}</a></h3>
<p>{esc(ex.description)}</p>
<p class="card-links"><a href="{run}">Run</a> <a href="{esc(ex.dir_url)}" rel="noopener">Source</a> <code>{esc(ex.name)}</code></p></div>
</article>"""

    def gallery(self):
        page = "examples/index.html"
        groups = {}
        for ex in self.web:
            groups.setdefault(ex.group, []).append(ex)
        parts = [
            """<header class="page-head"><h1>Examples</h1>
<p>Each of these is one of the example programs in the repository's <code>examples/</code>, compiled to WebAssembly unchanged and running in your browser. The same code opens a native window with <code>cargo run -p NAME</code>.</p></header>"""
        ]
        for g, exs in groups.items():
            gid = md.slugify(g)
            parts.append(f'<section><h2 id="{gid}">{esc(g)}</h2><div class="cards">{"".join(self.card(page, e) for e in exs)}</div></section>')
        rows = "".join(
            f'<tr><td><a href="{esc(e.dir_url)}" rel="noopener"><code>{esc(e.name)}</code></a></td><td>{esc(e.description)}</td><td>{md.Renderer().inline(e.needs)}</td></tr>'
            for e in self.native
        )
        parts.append(
            f"""<section><h2 id="native-only">Native only</h2>
<p>These need something a browser doesn't have. Run them with <code>cargo run -p NAME</code>.</p>
<div class="table-wrap"><table class="native"><thead><tr><th>Example</th><th>What it shows</th><th>Needs</th></tr></thead><tbody>{rows}</tbody></table></div></section>"""
        )
        self.write(page, self.shell(page, "Examples · Stipple", "\n".join(parts), sidebar=False, cls="gallery"))

    def runner(self, ex):
        page = f"examples/{ex.name}.html"
        r = lambda to: self.rel(page, to)  # noqa: E731
        text = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta name="description" content="{esc(ex.description)}">
<title>{esc(ex.title)} · Stipple example</title>
<link rel="icon" href="{r('assets/logo.svg')}" type="image/svg+xml">
<link rel="stylesheet" href="{r('assets/site.css')}">
</head>
<body class="runner">
<header class="runbar">
<a class="brand" href="{r('index.html')}" aria-label="Stipple"><img src="{r('assets/logo.svg')}" alt="" width="22" height="22"></a>
<a href="{r('examples/index.html')}" class="back">Examples</a>
<h1>{esc(ex.title)}</h1>
<p class="desc">{esc(ex.description)}</p>
<a class="src" href="{esc(ex.source_url)}" rel="noopener">View source</a>
</header>
<main class="stage">
<canvas id="app" aria-label="{esc(ex.title)}"></canvas>
<p class="status" role="status">Loading…</p>
</main>
<script type="module" src="{r('assets/run.js')}" data-name="{esc(ex.name)}" data-wasm="{r(f'wasm/{ex.name}.wasm')}" data-font="{r('font.ttf')}"></script>
</body>
</html>
"""
        self.write(page, text)

    # --- Everything ---

    def copy(self, src, dest):
        dest = self.out / dest
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src, dest)

    def generate(self):
        if self.out.exists():
            shutil.rmtree(self.out)
        self.out.mkdir(parents=True)
        for src in self.pages:
            if src == "README.md":
                self.landing()
            else:
                self.guide(src)
        self.gallery()
        for ex in self.web:
            self.runner(ex)
        # Assets: the site's own, images the guides show, the page side of
        # Stipple and the font.
        for f in (HERE / "assets").iterdir():
            self.copy(f, f"assets/{f.name}")
        for a in sorted(self.assets):
            self.copy(ROOT / a, a)
        self.copy(ROOT / "web/stipple.js", "stipple.js")
        if self.font:
            self.copy(self.font, "font.ttf")
        else:
            self.problems.append("no --font: text in the examples won't render")
        # The modules.
        missing = []
        if self.wasm_dir is not None:
            for ex in self.web:
                src = Path(self.build["out"].format(artifact=ex.artifact, crate=ex.crate, name=ex.name))
                if not src.is_absolute():
                    src = ROOT / src
                if src.exists():
                    self.copy(src, f"wasm/{ex.name}.wasm")
                else:
                    missing.append(ex.name)
        if missing:
            self.problems.append("not built (run `build.py wasm`): " + ", ".join(missing))
        # The pages that were here before the site: the web demo and its
        # selftest, two apps on a page, the WebGPU probe, and the game.
        wd = ROOT / "examples/webdemo"
        self.copy(wd / "index.html", "webdemo.html")
        for f in ("multi.html", "webgpu.html"):
            self.copy(wd / f, f)
        self.copy(ROOT / "examples/gamedemo/game.html", "game.html")
        for name, file in (("webdemo", "webdemo.wasm"), ("gamedemo", "lampwick.wasm")):
            if (self.out / f"wasm/{name}.wasm").exists():
                self.copy(self.out / f"wasm/{name}.wasm", file)
        (self.out / ".nojekyll").write_text("")
        (self.out / "site.json").write_text(
            json.dumps(
                {"web": [{"name": e.name, "size": e.size, "title": e.title} for e in self.web]},
                indent=1,
            )
        )
        for p in self.problems:
            print("warning:", p, file=sys.stderr)
        return not (self.strict and self.problems)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    w = sub.add_parser("wasm", help="build every web example's wasm module")
    w.add_argument("--print", action="store_true", help="print the command instead")
    s = sub.add_parser("site", help="generate the site")
    s.add_argument("--out", default=str(ROOT / "target/site"))
    s.add_argument("--font", help="the TrueType font the examples draw text with")
    s.add_argument("--no-wasm", action="store_true", help="don't copy the wasm modules")
    s.add_argument("--strict", action="store_true", help="fail on warnings (broken links, missing modules)")
    sub.add_parser("check-manifest", help="check every example is in examples.toml")
    a = ap.parse_args()
    if a.cmd == "check-manifest":
        sys.exit(0 if check_manifest() else 1)
    if a.cmd == "wasm":
        build, web, _ = load_manifest()
        cmd = wasm_command(build, web)
        print(cmd, flush=True)
        if not a.print:
            sys.exit(subprocess.call(shlex.split(cmd), cwd=ROOT))
        return
    font = a.font
    site = Site(a.out, font, wasm_dir=None if a.no_wasm else True, strict=a.strict)
    ok = site.generate()
    print(f"site: {site.out}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
