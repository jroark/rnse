#!/usr/bin/env python3
"""Build the rns-e.com wiki: wiki/pages/*.md -> a static site.

    tools/wiki.py [OUT]            (default: target/wiki/site)

Standard library only, like the rest of the project.  Every page is a
Markdown file with a few header lines:

    title: The 192's boards
    order: 30
    summary: one line for the home page and <meta name=description>
    ---
    Markdown ...

`index.md` becomes /index.html, any other NAME.md becomes
/NAME/index.html.  The Markdown understood is a small, strict subset:
ATX headings, paragraphs, '-' and '1.' lists (one level), fenced code,
'>' notes, pipe tables, and an image alone on a line, which becomes a
figure with its title as the caption (several such lines in a row
become a row of figures).  Inline: `code`, **bold**, *italic*, links,
and line breaks are not significant.

The build fails, rather than publish something broken or private, on:
a link to a page or file that is not in the site, a missing image, a
JPEG that still carries an EXIF/XMP segment (camera, time, place), and
any file in the output that does not look like part of a web site.
"""

import html
import os
import re
import shutil
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WIKI = os.path.join(ROOT, "wiki")
PAGES = os.path.join(WIKI, "pages")
SITE_NAME = "RNS-E"

# Screenshots of the emulator that already live in docs/ and are reused.
SHOTS = ["ui.png", "loader.png", "loader-193.png", "os-193.png", "nav-193.png"]

ALLOWED = {".html", ".css", ".js", ".wasm", ".png", ".jpg", ".svg", ".ico", ".txt"}

errors = []


def fail(msg):
    errors.append(msg)


# ---- images -----------------------------------------------------------------

def image_size(path):
    """(width, height) of a PNG or JPEG, or None."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        w, h = struct.unpack(">II", data[16:24])
        return w, h
    if data[:2] == b"\xff\xd8":
        i = 2
        while i + 4 <= len(data):
            if data[i] != 0xFF:
                return None
            marker = data[i + 1]
            if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
                i += 2
                continue
            length = struct.unpack(">H", data[i + 2:i + 4])[0]
            if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
                h, w = struct.unpack(">HH", data[i + 5:i + 9])
                return w, h
            i += 2 + length
    return None


def jpeg_metadata(path):
    """Names of metadata segments (EXIF, XMP, ...) left in a JPEG."""
    found = []
    with open(path, "rb") as f:
        data = f.read()
    i = 2
    while i + 4 <= len(data):
        if data[i] != 0xFF:
            break
        marker = data[i + 1]
        if marker == 0xDA:  # start of scan: no more headers
            break
        length = struct.unpack(">H", data[i + 2:i + 4])[0]
        body = data[i + 4:i + 2 + length]
        if marker == 0xE1:
            found.append("EXIF" if body.startswith(b"Exif") else "APP1/XMP")
        elif marker == 0xED:
            found.append("IPTC")
        i += 2 + length
    return found


# ---- inline Markdown ----------------------------------------------------------

INLINE = re.compile(
    r"(`+)(.+?)\1"                          # 1,2 code
    r"|!\[([^\]]*)\]\(([^)\s]+)\)"          # 3,4 inline image
    r"|\[([^\]]+)\]\(([^)\s]+)\)"           # 5,6 link
    r"|\*\*(.+?)\*\*"                       # 7 bold
    r"|(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])"  # 8 italic
)


class Page:
    def __init__(self, name, title, order, summary, body):
        self.name, self.title, self.order, self.summary, self.body = name, title, order, summary, body
        self.links = []   # (target, where)
        self.toc = []     # (id, text) of h2
        self.figures = 0

    @property
    def url(self):
        return "/" if self.name == "index" else "/%s/" % self.name


def inline(text, page):
    out, pos = [], 0
    for m in INLINE.finditer(text):
        out.append(html.escape(text[pos:m.start()], quote=False))
        if m.group(1):
            out.append("<code>%s</code>" % html.escape(m.group(2).strip(), quote=False))
        elif m.group(4):
            page.links.append(m.group(4))
            out.append('<img src="%s" alt="%s">' % (html.escape(m.group(4)), html.escape(m.group(3))))
        elif m.group(6):
            href = m.group(6)
            page.links.append(href)
            ext = ' rel="noopener"' if href.startswith("http") else ""
            out.append('<a href="%s"%s>%s</a>' % (html.escape(href), ext, inline(m.group(5), page)))
        elif m.group(7):
            out.append("<strong>%s</strong>" % inline(m.group(7), page))
        else:
            out.append("<em>%s</em>" % inline(m.group(8), page))
        pos = m.end()
    out.append(html.escape(text[pos:], quote=False))
    return "".join(out)


def slug(text):
    s = re.sub(r"<[^>]+>", "", text)
    s = re.sub(r"[^a-z0-9]+", "-", html.unescape(s).lower()).strip("-")
    return s or "section"


# ---- block Markdown -------------------------------------------------------------

FIGURE = re.compile(r'^!\[([^\]]*)\]\((\S+?)(?:\s+"([^"]*)")?\)\s*$')


def figure(m, page, sizes):
    alt, src, caption = m.group(1), m.group(2), m.group(3) or ""
    page.links.append(src)
    wh = sizes.get(src)
    dims = ' width="%d" height="%d"' % wh if wh else ""
    cap = "<figcaption>%s</figcaption>" % inline(caption, page) if caption else ""
    # The first picture of a page is likely on screen at once: no lazy loading.
    lazy = ' loading="lazy"' if page.figures else ""
    page.figures += 1
    return ('<figure><a href="%s"><img src="%s" alt="%s"%s%s decoding="async"></a>%s</figure>'
            % (html.escape(src), html.escape(src), html.escape(alt), dims, lazy, cap))


def render(page, sizes):
    lines = page.body.split("\n")
    out, i, used_ids = [], 0, set()

    def para(buf):
        if buf:
            out.append("<p>%s</p>" % inline(" ".join(s.strip() for s in buf), page))

    buf = []
    while i < len(lines):
        line = lines[i]
        s = line.strip()
        if not s:
            para(buf); buf = []
            i += 1
            continue
        if s.startswith("```"):
            para(buf); buf = []
            lang = s[3:].strip()
            i += 1
            code = []
            while i < len(lines) and not lines[i].strip().startswith("```"):
                code.append(lines[i])
                i += 1
            i += 1
            cls = ' class="lang-%s"' % html.escape(lang) if lang else ""
            out.append("<pre%s><code>%s</code></pre>" % (cls, html.escape("\n".join(code), quote=False)))
            continue
        m = re.match(r"^(#{1,3})\s+(.*)$", s)
        if m:
            para(buf); buf = []
            level = len(m.group(1))
            text = inline(m.group(2), page)
            if level == 1:
                fail("%s: use the title: line, not '# '" % page.name)
            hid = slug(m.group(2))
            n = 2
            while hid in used_ids:
                hid = "%s-%d" % (slug(m.group(2)), n); n += 1
            used_ids.add(hid)
            if level == 2:
                page.toc.append((hid, text))
            out.append('<h%d id="%s">%s<a class="anchor" href="#%s" aria-label="Link to this section">#</a></h%d>'
                       % (level, hid, text, hid, level))
            i += 1
            continue
        if FIGURE.match(s):
            para(buf); buf = []
            figs = []
            while i < len(lines) and FIGURE.match(lines[i].strip()):
                figs.append(figure(FIGURE.match(lines[i].strip()), page, sizes))
                i += 1
            if len(figs) == 1:
                out.append(figs[0])
            else:
                out.append('<div class="gallery g%d">%s</div>' % (min(len(figs), 3), "".join(figs)))
            continue
        if s.startswith(">"):
            para(buf); buf = []
            quote = []
            while i < len(lines) and lines[i].strip().startswith(">"):
                quote.append(lines[i].strip()[1:].strip())
                i += 1
            paras, cur = [], []
            for q in quote:
                if q:
                    cur.append(q)
                elif cur:
                    paras.append(cur); cur = []
            if cur:
                paras.append(cur)
            out.append('<aside class="note">%s</aside>'
                       % "".join("<p>%s</p>" % inline(" ".join(p), page) for p in paras))
            continue
        if s.startswith("|"):
            para(buf); buf = []
            rows = []
            while i < len(lines) and lines[i].strip().startswith("|"):
                rows.append([c.strip() for c in lines[i].strip().strip("|").split("|")])
                i += 1
            head = rows[0]
            body = [r for r in rows[2:]] if len(rows) > 1 and re.match(r"^[-: ]+$", "".join(rows[1])) else rows[1:]
            t = ["<div class=\"table\"><table><thead><tr>"]
            t += ["<th>%s</th>" % inline(c, page) for c in head]
            t.append("</tr></thead><tbody>")
            for r in body:
                t.append("<tr>%s</tr>" % "".join("<td>%s</td>" % inline(c, page) for c in r))
            t.append("</tbody></table></div>")
            out.append("".join(t))
            continue
        lm = re.match(r"^(-|\d+\.)\s+(.*)$", s)
        if lm and not buf:
            ordered = lm.group(1) != "-"
            items = []
            while i < len(lines):
                l = lines[i]
                m2 = re.match(r"^(-|\d+\.)\s+(.*)$", l.strip())
                if m2 and not l.startswith("  "):
                    items.append([m2.group(2)])
                elif l.strip() and l.startswith("  ") and items:
                    items[-1].append(l.strip())
                else:
                    break
                i += 1
            tag = "ol" if ordered else "ul"
            out.append("<%s>%s</%s>" % (tag, "".join("<li>%s</li>" % inline(" ".join(it), page) for it in items), tag))
            continue
        buf.append(line)
        i += 1
    para(buf)
    return "\n".join(out)


# ---- the site -------------------------------------------------------------------

def read_pages():
    pages = []
    for fn in sorted(os.listdir(PAGES)):
        if not fn.endswith(".md"):
            continue
        with open(os.path.join(PAGES, fn), encoding="utf-8") as f:
            text = f.read()
        head, sep, body = text.partition("\n---\n")
        if not sep:
            fail("%s: no header" % fn)
            continue
        meta = dict(l.split(":", 1) for l in head.splitlines() if ":" in l)
        meta = {k.strip(): v.strip() for k, v in meta.items()}
        pages.append(Page(fn[:-3], meta.get("title", fn[:-3]), int(meta.get("order", 999)),
                          meta.get("summary", ""), body))
    pages.sort(key=lambda p: (p.order, p.name))
    return pages


def nav(pages, current):
    items = []
    for p in pages:
        cur = ' aria-current="page"' if p is current else ""
        items.append('<li><a href="%s"%s>%s</a></li>' % (p.url, cur, html.escape(p.title)))
    return "\n".join(items)


def toc(page):
    if len(page.toc) < 3:
        return ""
    return ('<nav class="toc" aria-label="On this page"><h2>On this page</h2><ol>%s</ol></nav>'
            % "".join('<li><a href="#%s">%s</a></li>' % (i, re.sub(r"<a class=\"anchor\".*", "", t))
                      for i, t in page.toc))


def prevnext(pages, page):
    i = pages.index(page)
    parts = []
    if i > 0:
        parts.append('<a class="prev" href="%s"><span>Previous</span>%s</a>' % (pages[i - 1].url, html.escape(pages[i - 1].title)))
    if i + 1 < len(pages):
        parts.append('<a class="next" href="%s"><span>Next</span>%s</a>' % (pages[i + 1].url, html.escape(pages[i + 1].title)))
    return '<nav class="pager">%s</nav>' % "".join(parts) if parts else ""


def page_index(pages):
    """The list of pages with their summaries, for {{pages}} on the home page."""
    cards = []
    for p in pages:
        if p.name == "index":
            continue
        cards.append('<a class="card" href="%s"><strong>%s</strong><span>%s</span></a>'
                     % (p.url, html.escape(p.title), html.escape(p.summary)))
    return '<div class="cards">%s</div>' % "".join(cards)


def main():
    out = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target", "wiki", "site"))
    if os.path.isdir(out):
        shutil.rmtree(out)
    os.makedirs(out)

    # Media: the wiki's photographs and the emulator screenshots from docs/.
    sizes = {}
    media_out = os.path.join(out, "media")
    os.makedirs(media_out)
    for fn in sorted(os.listdir(os.path.join(WIKI, "media"))):
        src = os.path.join(WIKI, "media", fn)
        if fn.lower().endswith((".jpg", ".jpeg")):
            meta = jpeg_metadata(src)
            if meta:
                fail("wiki/media/%s still carries %s: strip it (magick ... -strip)" % (fn, ", ".join(meta)))
        shutil.copy(src, media_out)
        sizes["/media/" + fn] = image_size(src)
    shots_out = os.path.join(out, "shots")
    os.makedirs(shots_out)
    for fn in SHOTS:
        src = os.path.join(ROOT, "docs", fn)
        if os.path.exists(src):
            shutil.copy(src, shots_out)
            sizes["/shots/" + fn] = image_size(src)
    for fn in ("style.css", "favicon.svg"):
        shutil.copy(os.path.join(WIKI, fn), out)

    with open(os.path.join(WIKI, "template.html"), encoding="utf-8") as f:
        template = f.read()

    pages = read_pages()
    for p in pages:
        content = render(p, sizes)
        content = content.replace("<p>{{pages}}</p>", page_index(pages))
        doc = (template
               .replace("{{title}}", html.escape(p.title if p.name != "index" else "The unit, its emulator and how it was taken apart"))
               .replace("{{heading}}", html.escape(p.title))
               .replace("{{description}}", html.escape(p.summary))
               .replace("{{nav}}", nav(pages, p))
               .replace("{{toc}}", toc(p))
               .replace("{{pager}}", prevnext(pages, p))
               .replace("{{home}}", "home" if p.name == "index" else "page")
               .replace("{{content}}", content))
        d = out if p.name == "index" else os.path.join(out, p.name)
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "index.html"), "w", encoding="utf-8") as f:
            f.write(doc)

    # Every internal link has to land on something in the site.  The
    # emulator is built separately (wiki/build.sh) and copied in later.
    for p in pages:
        for href in p.links:
            if re.match(r"^[a-z]+:", href) or href.startswith("#"):
                continue
            if not href.startswith("/"):
                fail("%s: link %s is not absolute" % (p.name, href))
                continue
            path = href.split("#")[0]
            if path.startswith("/emulator/"):
                continue
            target = os.path.join(out, path.lstrip("/"))
            if path.endswith("/"):
                target = os.path.join(target, "index.html")
            if not os.path.exists(target):
                fail("%s: broken link %s" % (p.name, href))

    for dirpath, _, files in os.walk(out):
        for fn in files:
            if os.path.splitext(fn)[1].lower() not in ALLOWED:
                fail("unexpected file in the site: %s" % os.path.join(dirpath, fn))

    if errors:
        for e in errors:
            print("wiki: " + e, file=sys.stderr)
        sys.exit(1)
    print("wiki: %d pages into %s" % (len(pages), out))


if __name__ == "__main__":
    main()
