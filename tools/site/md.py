"""A small Markdown renderer for Stipple's docs (the GitHub-flavored subset
they use), and a syntax highlighter for their code blocks. Standard library
only.

Blocks: ATX headings (with GitHub's anchor ids), paragraphs, fenced code,
block quotes, ordered and unordered lists (nested, tight or loose), tables,
thematic breaks, HTML blocks and comments. Inlines: code spans, links and
images (with a hook that rewrites their targets), autolinks, emphasis,
strikethrough, backslash escapes and inline HTML.
"""

import html
import re

__all__ = ["Renderer", "slugify", "highlight"]


def esc(text):
    return html.escape(text, quote=False)


def attr(text):
    return html.escape(text, quote=True)


def slugify(text, seen=None):
    """GitHub's heading anchors: lowercase, punctuation dropped, spaces to
    dashes; repeats get -1, -2…"""
    text = re.sub(r"<[^>]+>", "", text)
    text = html.unescape(text).strip().lower()
    slug = re.sub(r"[^\w\- ]", "", text).replace(" ", "-")
    if seen is not None:
        base, n = slug, 0
        while slug in seen:
            n += 1
            slug = f"{base}-{n}"
        seen.add(slug)
    return slug


# --- Highlighting ---------------------------------------------------------

RUST_KW = (
    "as async await break const continue crate dyn else enum extern false fn for if impl in let "
    "loop match mod move mut pub ref return self Self static struct super trait true type unsafe "
    "use where while"
).split()
JS_KW = (
    "async await break case catch class const continue default delete do else export extends "
    "false finally for from function if import in instanceof let new null of return static "
    "super switch this throw true try typeof undefined var void while yield"
).split()


def _lexer(rules):
    return re.compile("|".join(f"(?P<{name}>{rx})" for name, rx in rules), re.M | re.S)


LEXERS = {
    "rust": _lexer(
        [
            ("c", r"//[^\n]*|/\*.*?\*/"),
            ("s", r'b?r(?P<hashes>#*)".*?"(?P=hashes)|b?"(?:\\.|[^"\\])*"|b?\'(?:\\.|[^\'\\])\''),
            ("a", r"#!?\[[^\]\n]*\]"),
            ("l", r"'[a-z_]\w*\b"),
            ("m", r"\b[a-z_]\w*!"),
            ("n", r"\b\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?(?:[iuf](?:8|16|32|64|128|size))?\b"),
            ("k", r"\b(?:" + "|".join(RUST_KW) + r")\b"),
            ("t", r"\b[A-Z][A-Za-z0-9_]*\b"),
            ("f", r"\b[a-z_]\w*(?=\s*(?:::<[^>]*>)?\()"),
        ]
    ),
    "js": _lexer(
        [
            ("c", r"//[^\n]*|/\*.*?\*/"),
            ("s", r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|`(?:\\.|[^`\\])*`'),
            ("n", r"\b\d+(?:\.\d+)?\b"),
            ("k", r"\b(?:" + "|".join(JS_KW) + r")\b"),
            ("f", r"\b[A-Za-z_$][\w$]*(?=\()"),
        ]
    ),
    "toml": _lexer(
        [
            ("c", r"#[^\n]*"),
            ("s", r'"(?:\\.|[^"\\])*"|\'[^\']*\''),
            ("a", r"^\s*\[[^\]\n]*\]"),
            ("k", r"^\s*[\w.\-]+(?=\s*=)"),
            ("n", r"\b(?:\d+(?:\.\d+)?|true|false)\b"),
        ]
    ),
    "sh": _lexer(
        [
            ("c", r"(?:^|(?<=\s))#[^\n]*"),
            ("s", r'"(?:\\.|[^"\\])*"|\'[^\']*\''),
            ("v", r"\$\{?[\w@#?]+\}?"),
            ("k", r"(?:^|(?<=[\s;|&]))(?:cargo|rustup|python3|git|cd|export|sudo|apt-get|xcrun|adb|npm|mkdir|cp)\b"),
            ("o", r"(?<=\s)--?[\w\-]+"),
        ]
    ),
    "xml": _lexer(
        [
            ("c", r"<!--.*?-->"),
            ("s", r'"[^"]*"'),
            ("k", r"</?[\w:\-]+|/?>|<\?|\?>|<!\w+"),
            ("a", r"\b[\w:\-]+(?==)"),
        ]
    ),
}
ALIASES = {
    "rs": "rust",
    "javascript": "js",
    "mjs": "js",
    "bash": "sh",
    "shell": "sh",
    "console": "sh",
    "html": "xml",
    "plist": "xml",
}


def highlight(code, lang):
    """`code` as HTML, its tokens in <span class="tok-X"> for `lang`."""
    lang = ALIASES.get(lang, lang)
    lexer = LEXERS.get(lang)
    if lexer is None:
        return esc(code)
    out, pos = [], 0
    for m in lexer.finditer(code):
        kind = m.lastgroup
        if m.start() == m.end():
            continue
        out.append(esc(code[pos : m.start()]))
        out.append(f'<span class="tok-{kind}">{esc(m.group())}</span>')
        pos = m.end()
    out.append(esc(code[pos:]))
    return "".join(out)


# --- Blocks ---------------------------------------------------------------

FENCE = re.compile(r"^( {0,3})(`{3,}|~{3,})\s*([^`\s]*)[^`]*$")
HEADING = re.compile(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$")
HR = re.compile(r"^ {0,3}(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$")
BULLET = re.compile(r"^( {0,3})([-*+]|\d{1,9}[.)])( +|$)")
TABLE_SEP = re.compile(r"^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$")
HTML_BLOCK = re.compile(
    r"^ {0,3}(?:<!--|<\?|</?(?:p|div|details|summary|table|thead|tbody|tr|td|th|img|br|hr|h[1-6]"
    r"|ul|ol|li|picture|source|figure|figcaption|section|pre|blockquote|center|kbd|a|sub|sup)\b)",
    re.I,
)
REFDEF = re.compile(r"^ {0,3}\[([^\]]+)\]:\s*<?(\S+?)>?(?:\s+[\"'(](.*)[\"')])?\s*$")


def _starts_block(line):
    return bool(
        FENCE.match(line)
        or HEADING.match(line)
        or HR.match(line)
        or line.lstrip().startswith(">")
        or HTML_BLOCK.match(line)
        or BULLET.match(line)
    )


class Renderer:
    """Markdown to HTML.

    `link(url, is_image)` rewrites link and image targets; `block_hook(line)`
    may return HTML for a whole line (the live-example markers), or None.
    Headings are collected in `self.headings` as (level, id, text).
    """

    def __init__(self, link=None, block_hook=None):
        self.link = link or (lambda url, image: url)
        self.block_hook = block_hook or (lambda line: None)
        self.refs = {}
        self.headings = []
        self.seen = set()

    def render(self, text):
        lines = text.replace("\r\n", "\n").replace("\t", "    ").split("\n")
        # Link reference definitions, anywhere.
        kept = []
        in_fence = None
        for line in lines:
            f = FENCE.match(line)
            if f and (in_fence is None or f.group(2)[0] == in_fence):
                in_fence = None if in_fence else f.group(2)[0]
            m = REFDEF.match(line) if in_fence is None else None
            if m:
                self.refs[m.group(1).lower()] = m.group(2)
            else:
                kept.append(line)
        return self.blocks(kept)

    # A list of lines to HTML.
    def blocks(self, lines, tight=False):
        out = []
        i, n = 0, len(lines)
        while i < n:
            line = lines[i]
            if not line.strip():
                i += 1
                continue
            hooked = self.block_hook(line.strip())
            if hooked is not None:
                out.append(hooked)
                i += 1
                continue
            m = FENCE.match(line)
            if m:
                indent, fence, lang = len(m.group(1)), m.group(2), m.group(3)
                j = i + 1
                body = []
                while j < n and not re.match(rf"^ {{0,3}}{re.escape(fence[0])}{{{len(fence)},}}\s*$", lines[j]):
                    body.append(re.sub(rf"^ {{0,{indent}}}", "", lines[j]))
                    j += 1
                lang = lang.split(",")[0].lower()
                if lang in ("rust", "rs"):
                    # rustdoc's hidden lines: compiled, not shown.
                    body = [b for b in body if not re.match(r"^\s*#(?: |$)", b)]
                    body = [re.sub(r"^(\s*)##", r"\1#", b) for b in body]
                code = "\n".join(body)
                cls = f' class="language-{attr(lang)}"' if lang else ""
                label = f' data-lang="{attr(lang)}"' if lang else ""
                out.append(f"<pre{label}><code{cls}>{highlight(code, lang)}</code></pre>")
                i = j + 1
                continue
            m = HEADING.match(line)
            if m:
                level = len(m.group(1))
                inner = self.inline(m.group(2) or "")
                hid = slugify(inner, self.seen)
                self.headings.append((level, hid, re.sub(r"<[^>]+>", "", inner)))
                out.append(
                    f'<h{level} id="{hid}"><a class="anchor" href="#{hid}" aria-hidden="true">#</a>{inner}</h{level}>'
                )
                i += 1
                continue
            if HR.match(line):
                out.append("<hr>")
                i += 1
                continue
            if line.lstrip().startswith(">"):
                body = []
                while i < n and lines[i].strip() and (lines[i].lstrip().startswith(">") or body):
                    if not lines[i].lstrip().startswith(">") and _starts_block(lines[i]):
                        break
                    body.append(re.sub(r"^\s*> ?", "", lines[i]))
                    i += 1
                out.append(f"<blockquote>{self.blocks(body)}</blockquote>")
                continue
            if HTML_BLOCK.match(line):
                body = []
                if line.lstrip().startswith("<!--"):
                    while i < n:
                        body.append(lines[i])
                        i += 1
                        if "-->" in body[-1]:
                            break
                    continue  # Comments stay out of the page.
                while i < n and lines[i].strip():
                    body.append(lines[i])
                    i += 1
                out.append(self.inline_html_block("\n".join(body)))
                continue
            if BULLET.match(line):
                i = self.list_block(lines, i, out)
                continue
            if "|" in line and i + 1 < n and TABLE_SEP.match(lines[i + 1]) and "-" in lines[i + 1]:
                i = self.table(lines, i, out)
                continue
            # A paragraph.
            body = [line.strip()]
            i += 1
            while i < n and lines[i].strip() and not _starts_block(lines[i]):
                if "|" in lines[i] and i + 1 < n and TABLE_SEP.match(lines[i + 1]):
                    break
                body.append(lines[i].strip())
                i += 1
            text = self.inline("\n".join(body))
            out.append(text if tight else f"<p>{text}</p>")
        return "\n".join(out)

    def inline_html_block(self, text):
        # Markdown inside an HTML block line (GitHub renders `<p>` blocks
        # verbatim; keep them, rewriting the URLs they hold).
        def fix(m):
            return f'{m.group(1)}="{attr(self.link(html.unescape(m.group(2)), m.group(1) == "src"))}"'

        return re.sub(r'\b(href|src)="([^"]*)"', fix, text)

    def list_block(self, lines, i, out):
        first = BULLET.match(lines[i])
        ordered = first.group(2)[0].isdigit()
        items = []
        n = len(lines)
        loose = False
        while i < n:
            m = BULLET.match(lines[i])
            if not m or m.group(2)[0].isdigit() != ordered:
                break
            content_indent = len(m.group(1)) + len(m.group(2)) + max(1, min(len(m.group(3)), 4))
            if not m.group(3):
                content_indent = len(m.group(1)) + len(m.group(2)) + 1
            body = [lines[i][content_indent:] if len(lines[i]) > content_indent else lines[i][m.end() :]]
            i += 1
            blank_inside = False
            while i < n:
                line = lines[i]
                if not line.strip():
                    # A blank line: the item goes on if the next line is
                    # indented to its content.
                    j = i
                    while j < n and not lines[j].strip():
                        j += 1
                    if j < n and len(lines[j]) - len(lines[j].lstrip()) >= content_indent:
                        body.extend([""] * (j - i))
                        blank_inside = True
                        i = j
                        continue
                    if j < n and BULLET.match(lines[j]) and BULLET.match(lines[j]).group(2)[0].isdigit() == ordered and len(BULLET.match(lines[j]).group(1)) < content_indent:
                        loose = True
                    i = j if j < n and BULLET.match(lines[j]) and len(BULLET.match(lines[j]).group(1)) < content_indent else i
                    break
                indent = len(line) - len(line.lstrip())
                if indent >= content_indent:
                    body.append(line[content_indent:])
                elif BULLET.match(line) or (body and _starts_block(line) and not body[-1].strip() == ""):
                    if BULLET.match(line) or HEADING.match(line) or FENCE.match(line) or HR.match(line):
                        break
                    body.append(line.strip())
                else:
                    body.append(line.strip())  # Lazy continuation.
                i += 1
            if blank_inside:
                # Blank lines between an item's own blocks make it loose only
                # when they separate paragraphs at its top level.
                loose = loose or any(not b.strip() for b in body if True) and _has_top_level_gap(body)
            items.append(body)
            if i < n and not lines[i].strip():
                break
        tag = "ol" if ordered else "ul"
        start = ""
        if ordered:
            num = int(re.match(r"\d+", first.group(2)).group())
            if num != 1:
                start = f' start="{num}"'
        parts = [f"<{tag}{start}>"]
        for body in items:
            parts.append(f"<li>{self.blocks(body, tight=not loose)}</li>")
        parts.append(f"</{tag}>")
        out.append("".join(parts))
        return i

    def table(self, lines, i, out):
        def cells(line):
            line = line.strip()
            if line.startswith("|"):
                line = line[1:]
            if line.endswith("|") and not line.endswith("\\|"):
                line = line[:-1]
            parts, cur, code = [], "", False
            k = 0
            while k < len(line):
                c = line[k]
                if c == "\\" and k + 1 < len(line) and line[k + 1] == "|":
                    cur += "|"
                    k += 2
                    continue
                if c == "`":
                    code = not code
                if c == "|" and not code:
                    parts.append(cur.strip())
                    cur = ""
                else:
                    cur += c
                k += 1
            parts.append(cur.strip())
            return parts

        head = cells(lines[i])
        aligns = []
        for spec in cells(lines[i + 1]):
            left, right = spec.startswith(":"), spec.endswith(":")
            aligns.append("center" if left and right else "right" if right else "left" if left else "")
        i += 2
        rows = []
        while i < len(lines) and lines[i].strip() and "|" in lines[i]:
            rows.append(cells(lines[i]))
            i += 1

        def row(cs, tag):
            res = []
            for k in range(len(head)):
                c = cs[k] if k < len(cs) else ""
                al = aligns[k] if k < len(aligns) and aligns[k] else ""
                st = f' style="text-align:{al}"' if al else ""
                res.append(f"<{tag}{st}>{self.inline(c)}</{tag}>")
            return "<tr>" + "".join(res) + "</tr>"

        body = "".join(row(r, "td") for r in rows)
        out.append(
            f'<div class="table-wrap"><table><thead>{row(head, "th")}</thead><tbody>{body}</tbody></table></div>'
        )
        return i

    # --- Inlines ---

    def inline(self, text):
        out = []
        i, n = 0, len(text)
        buf = []

        def flush():
            if buf:
                out.append(self.emphasis("".join(buf)))
                buf.clear()

        while i < n:
            c = text[i]
            if c == "\\" and i + 1 < n and text[i + 1] in "\\`*_{}[]()#+-.!|<>~\"'":
                buf.append("\x00" + text[i + 1])
                i += 2
                continue
            if c == "`":
                run = re.match(r"`+", text[i:]).group()
                end = text.find(run, i + len(run))
                while end != -1 and end + len(run) < n and text[end + len(run)] == "`":
                    end = text.find(run, end + len(run) + 1)
                if end != -1:
                    flush()
                    code = text[i + len(run) : end].replace("\n", " ")
                    if code.startswith(" ") and code.endswith(" ") and code.strip():
                        code = code[1:-1]
                    out.append(f"<code>{esc(code)}</code>")
                    i = end + len(run)
                    continue
                buf.append(run)
                i += len(run)
                continue
            if c == "!" and text.startswith("![", i):
                m = self.match_link(text, i + 1)
                if m:
                    label, url, title, end = m
                    flush()
                    alt = re.sub(r"<[^>]+>", "", self.inline(label))
                    t = f' title="{attr(title)}"' if title else ""
                    out.append(f'<img src="{attr(self.link(url, True))}" alt="{attr(html.unescape(alt))}"{t} loading="lazy">')
                    i = end
                    continue
            if c == "[":
                m = self.match_link(text, i)
                if m:
                    label, url, title, end = m
                    flush()
                    t = f' title="{attr(title)}"' if title else ""
                    target = self.link(url, False)
                    ext = ' rel="noopener"' if re.match(r"https?:", target) else ""
                    out.append(f'<a href="{attr(target)}"{t}{ext}>{self.inline(label)}</a>')
                    i = end
                    continue
            if c == "<":
                m = re.match(r"<((?:https?|mailto):[^\s<>]+)>", text[i:])
                if m:
                    flush()
                    url = m.group(1)
                    out.append(f'<a href="{attr(url)}" rel="noopener">{esc(url)}</a>')
                    i += m.end()
                    continue
                m = re.match(r"</?[a-zA-Z][\w-]*(?:\s+[\w:-]+(?:=(?:\"[^\"]*\"|'[^']*'|[^\s>]+))?)*\s*/?>|<!--.*?-->", text[i:], re.S)
                if m and re.match(r"</?(?:kbd|br|sub|sup|b|i|em|strong|span|img|a|code|del|s|u|small|mark|abbr|details|summary)\b|<!--", m.group(), re.I):
                    flush()
                    tag = m.group()
                    if not tag.startswith("<!--"):
                        out.append(self.inline_html_block(tag))
                    i += m.end()
                    continue
            buf.append(c)
            i += 1
        flush()
        return "".join(out)

    def match_link(self, text, i):
        """A link at text[i] == '[': (label, url, title, end) or None."""
        depth, j = 0, i
        n = len(text)
        while j < n:
            ch = text[j]
            if ch == "\\":
                j += 2
                continue
            if ch == "`":
                run = re.match(r"`+", text[j:]).group()
                close = text.find(run, j + len(run))
                if close != -1:
                    j = close + len(run)
                    continue
            if ch == "[":
                depth += 1
            elif ch == "]":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        if j >= n:
            return None
        label = text[i + 1 : j]
        rest = text[j + 1 :]
        m = re.match(r"\(\s*<?([^\s()<>]*(?:\([^\s()]*\)[^\s()<>]*)*)>?(?:\s+\"([^\"]*)\")?\s*\)", rest)
        if m:
            return label, m.group(1), m.group(2), j + 1 + m.end()
        m = re.match(r"\[([^\]]*)\]", rest)
        if m:
            key = (m.group(1) or label).lower()
            if key in self.refs:
                return label, self.refs[key], None, j + 1 + m.end()
        if label.lower() in self.refs and not rest.startswith("("):
            return label, self.refs[label.lower()], None, j + 1
        return None

    def emphasis(self, text):
        text = esc(text)
        text = re.sub(r"\*\*(?=\S)(.+?)(?<=\S)\*\*", r"<strong>\1</strong>", text, flags=re.S)
        text = re.sub(r"(?<![\w])__(?=\S)(.+?)(?<=\S)__(?![\w])", r"<strong>\1</strong>", text, flags=re.S)
        text = re.sub(r"(?<![*\w])\*(?=[^\s*])(.+?)(?<=[^\s*])\*(?![*\w])", r"<em>\1</em>", text, flags=re.S)
        text = re.sub(r"(?<![\w])_(?=[^\s_])(.+?)(?<=[^\s_])_(?![\w])", r"<em>\1</em>", text, flags=re.S)
        text = re.sub(r"~~(?=\S)(.+?)(?<=\S)~~", r"<del>\1</del>", text, flags=re.S)
        text = text.replace("\n", "\n")
        return re.sub(r"\x00(.)", lambda m: m.group(1), text)


def _has_top_level_gap(body):
    """Whether blank lines in a list item separate two of its top-level
    paragraphs (not lines inside a code block or a nested list)."""
    in_fence = False
    prev_text = False
    gap = False
    for line in body:
        if FENCE.match(line):
            in_fence = not in_fence
            prev_text = False
            continue
        if in_fence:
            continue
        if not line.strip():
            gap = prev_text
            continue
        if gap and not line.startswith(" ") and not BULLET.match(line):
            return True
        gap = False
        prev_text = not line.startswith(" ") and not BULLET.match(line)
    return False
