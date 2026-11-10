#!/usr/bin/env python3
"""Build site/guide.html from docs/USER_GUIDE.md.

Keeps the page around the guide (header, intro, footer) and rebuilds both tables of
contents and every section. Handles the Markdown the guide uses: headings, paragraphs,
nested lists, code fences, tables, quotes, inline code, bold, emphasis and links. Run it
after changing the guide: python3 site/build-guide.py
"""

import html
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUIDE = ROOT / "docs" / "USER_GUIDE.md"
PAGE = ROOT / "site" / "guide.html"
REPO = "https://github.com/neuralpunk/flux-backup/blob/main/"

# Section ids the site already used (links to them stay valid).
KNOWN_IDS = {"compatibility-mode-backups-that-dont-need-flux": "compatibility-mode"}
SHORT_TITLES = {"Compatibility mode: backups that don't need Flux": "Backups that don't need Flux"}


def slug(title: str) -> str:
    s = re.sub(r"[`*]", "", title).lower()
    s = re.sub(r"[^a-z0-9 _-]", "", s)
    s = s.strip().replace(" ", "-")
    return KNOWN_IDS.get(s, s)


def link_target(url: str) -> str:
    if url.startswith("#"):
        return "#" + slug(url[1:].replace("-", " ")) if url[1:] not in KNOWN_IDS else "#" + KNOWN_IDS[url[1:]]
    if re.match(r"^[a-z]+:", url):
        return url
    path, _, frag = url.partition("#")
    if path.endswith(".md"):
        full = (Path("docs") / path).as_posix()
        full = re.sub(r"docs/\.\./", "", full)
        return REPO + full + (("#" + frag) if frag else "")
    return url


def inline(text: str) -> str:
    codes: list[str] = []

    def keep(m: re.Match) -> str:
        codes.append("<code>" + html.escape(m.group(1), quote=False) + "</code>")
        return f"\x00{len(codes) - 1}\x00"

    text = re.sub(r"`([^`]+)`", keep, text)
    text = html.escape(text, quote=False)
    text = re.sub(
        r"\[([^\]]+)\]\(([^)\s]+)\)",
        lambda m: f'<a href="{html.escape(link_target(m.group(2)))}">{m.group(1)}</a>',
        text,
    )
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", text)
    return re.sub(r"\x00(\d+)\x00", lambda m: codes[int(m.group(1))], text)


LIST = re.compile(r"^(\s*)([-*]|\d+\.) (.*)$")


def blocks(lines: list[str]) -> str:
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if not line.strip():
            i += 1
            continue
        stripped = line.strip()
        if stripped.startswith("```"):
            lang = stripped[3:].strip()
            indent = len(line) - len(line.lstrip())
            body = []
            i += 1
            while i < len(lines) and not lines[i].strip().startswith("```"):
                body.append(lines[i][indent:] if lines[i][:indent].strip() == "" else lines[i])
                i += 1
            i += 1
            attr = f' data-lang="{lang}"' if lang else ""
            out.append(f'<div class="code"><pre><code{attr}>' + html.escape("\n".join(body), quote=False) + "</code></pre></div>")
            continue
        m = re.match(r"^(#{3,4}) (.*)$", line)
        if m:
            level = len(m.group(1))
            title = m.group(2)
            out.append(f'<h{level} id="{slug(title)}">{inline(title)}</h{level}>')
            i += 1
            continue
        if stripped.startswith("|") and i + 1 < len(lines) and re.match(r"^\s*\|[\s:|-]+\|\s*$", lines[i + 1]):
            rows = []
            while i < len(lines) and lines[i].strip().startswith("|"):
                rows.append([c.strip() for c in lines[i].strip().strip("|").split("|")])
                i += 1
            head, body = rows[0], rows[2:]
            t = ['<div class="table-wrap"><table>']
            t.append("<thead><tr>" + "".join(f'<th scope="col">{inline(c)}</th>' for c in head) + "</tr></thead>")
            t.append("<tbody>")
            for r in body:
                t.append("<tr>" + "".join(f"<td>{inline(c)}</td>" for c in r) + "</tr>")
            t.append("</tbody></table></div>")
            out.append("\n".join(t))
            continue
        if stripped.startswith(">"):
            quote = []
            while i < len(lines) and lines[i].strip().startswith(">"):
                quote.append(re.sub(r"^\s*> ?", "", lines[i]))
                i += 1
            out.append("<blockquote>" + blocks(quote) + "</blockquote>")
            continue
        m = LIST.match(line)
        if m and len(m.group(1)) == 0:
            ordered = m.group(2)[0].isdigit()
            items: list[list[str]] = []
            while i < len(lines):
                m = LIST.match(lines[i])
                if m and len(m.group(1)) == 0:
                    width = len(m.group(2)) + 1
                    items.append([m.group(3)])
                    i += 1
                    continue
                if lines[i].strip() == "":
                    # A blank line inside the list continues it only if indented text follows.
                    j = i + 1
                    if j < len(lines) and lines[j].startswith(" " * 2) and lines[j].strip():
                        items[-1].append("")
                        i += 1
                        continue
                    break
                if lines[i].startswith("  "):
                    items[-1].append(lines[i][width:] if lines[i][:width].strip() == "" else lines[i].lstrip())
                    i += 1
                    continue
                break
            tag = "ol" if ordered else "ul"
            parts = []
            for it in items:
                if len(it) > 1 and any(l.strip().startswith(("```", "- ", "* ")) or LIST.match(l) for l in it[1:]) or "" in it:
                    parts.append("<li>" + blocks(it) + "</li>")
                else:
                    parts.append("<li>" + inline(" ".join(l.strip() for l in it)) + "</li>")
            out.append(f"<{tag}>" + "".join(parts) + f"</{tag}>")
            continue
        para = []
        while i < len(lines) and lines[i].strip() and not lines[i].strip().startswith(("```", "|", ">", "#")) and not (LIST.match(lines[i]) and not para):
            if LIST.match(lines[i]) and len(LIST.match(lines[i]).group(1)) == 0:
                break
            para.append(lines[i].strip())
            i += 1
        out.append("<p>" + inline(" ".join(para)) + "</p>")
    return "\n".join(out)


def main() -> None:
    md = GUIDE.read_text().split("\n")
    sections: list[tuple[str, list[str]]] = []
    for line in md:
        if line.startswith("## "):
            sections.append((line[3:].strip(), []))
        elif sections:
            sections[-1][1].append(line)
    toc = "\n".join(
        f'            <li><a href="#{slug(t)}">{html.escape(SHORT_TITLES.get(t, t))}</a></li>' for t, _ in sections
    )
    body = []
    for title, lines in sections:
        sid = slug(title)
        body.append(
            f'        <section id="{sid}">\n'
            f'          <h2>{inline(SHORT_TITLES.get(title, title))} <a class="anchor" href="#{sid}" aria-label="Link to this section">#</a></h2>\n'
            + blocks(lines)
            + "\n        </section>\n"
        )
    page = PAGE.read_text()
    # Both tables of contents.
    page = re.sub(
        r'(<aside class="toc"[^>]*>\s*<p class="toc-title">Contents</p>\s*<ol>)(.*?)(</ol>)',
        lambda m: m.group(1) + "\n" + toc.replace("            <li>", "        <li>") + "\n      " + m.group(3),
        page,
        count=1,
        flags=re.S,
    )
    page = re.sub(
        r'(<details class="toc-mobile">\s*<summary>Contents</summary>\s*<ol>)(.*?)(</ol>)',
        lambda m: m.group(1) + "\n" + toc + "\n          " + m.group(3),
        page,
        count=1,
        flags=re.S,
    )
    # Every section, after the intro.
    intro = page.index('<header class="guide-intro">')
    marker = "<!-- Built from docs/USER_GUIDE.md"
    start = page.index(marker, intro) if marker in page[intro:] else page.index("<!-- ====", intro)
    end = page.index("      </div>\n    </main>")
    page = page[:start] + "<!-- Built from docs/USER_GUIDE.md by site/build-guide.py -->\n" + "".join(body) + page[end:]
    PAGE.write_text(page)
    print(f"{len(sections)} sections written to {PAGE.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
