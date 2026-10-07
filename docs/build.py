#!/usr/bin/env python3
"""Build the documentation site from wiki/, which stays the one copy of the
pages (the GitHub wiki is published from it too).

    python3 docs/build.py          # write docs/_build and build the site
    python3 docs/build.py serve    # the same, then serve it with live reload
    python3 docs/build.py pages    # only write the pages and config

The pages are written to docs/_build/pages as the site wants them: links
between pages (`[Running](Running#commands)`) point at their files, Home
becomes the front page, each page gets its name as a title, and GitHub's
`> [!WARNING]` notes become admonitions. The navigation comes from
wiki/_Sidebar.md. Links to a page or heading that doesn't exist, and pages
left out of the sidebar, stop the build.
"""

import re
import shutil
import subprocess
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WIKI = ROOT / "wiki"
DOCS = ROOT / "docs"
OUT = DOCS / "_build"
PAGES = OUT / "pages"

HOME = "Home"
SIDEBAR = "_Sidebar"

# `[text](Page)` or `[text](Page#heading)`, a page of the wiki.
WIKI_LINK = re.compile(r"\]\(([A-Za-z][A-Za-z0-9-]*)(#[^)\s]*)?\)")
# `[text](#heading)`, on the same page.
LOCAL_LINK = re.compile(r"\]\((#[^)\s]+)\)")
ALERT = re.compile(r"^> \[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*$")
ADMONITION = {"NOTE": "note", "TIP": "tip", "IMPORTANT": "info", "WARNING": "warning", "CAUTION": "danger"}


def slug(heading: str) -> str:
    """A heading's anchor, as Python-Markdown's table of contents makes it."""
    text = re.sub(r"<[^>]+>", "", heading)
    text = re.sub(r"[`*_]", "", text)
    text = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode("ascii")
    text = re.sub(r"[^\w\s-]", "", text).strip().lower()
    return re.sub(r"[-\s]+", "-", text)


def anchors(markdown: str) -> set:
    """The anchors of a page's headings (outside code blocks)."""
    found, fenced = set(), False
    for line in markdown.splitlines():
        if line.lstrip().startswith("```"):
            fenced = not fenced
        elif not fenced and (heading := re.match(r"^#{1,6}\s+(.*?)\s*#*\s*$", line)):
            found.add(slug(heading.group(1)))
    return found


def file_of(page: str) -> str:
    return "index.md" if page == HOME else f"{page}.md"


def sidebar() -> list:
    """The navigation: [(title, page)] at the top, [(section, [(title, page)])]."""
    nav, section = [], None
    for line in (WIKI / f"{SIDEBAR}.md").read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        link = re.search(r"\[([^\]]+)\]\(([^)#]+)\)", line)
        if link and line.startswith("-") and section is not None:
            section[1].append((link.group(1), link.group(2)))
        elif link:
            nav.append((link.group(1), link.group(2)))
            section = None
        elif heading := re.fullmatch(r"\*\*(.+)\*\*", line):
            section = (heading.group(1), [])
            nav.append(section)
        else:
            sys.exit(f"wiki/{SIDEBAR}.md: can't read the line {line!r}")
    return nav


def toml_string(text: str) -> str:
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def nav_toml(nav: list) -> str:
    def entry(title, target):
        if isinstance(target, list):
            items = ", ".join(entry(t, p) for t, p in target)
            return f"{{ {toml_string(title)} = [{items}] }}"
        return f"{{ {toml_string(title)} = {toml_string(file_of(target))} }}"

    lines = ",\n".join(f"  {entry(title, target)}" for title, target in nav)
    return f"nav = [\n{lines},\n]"


def convert(page: str, text: str, title: str, pages: dict, problems: list) -> str:
    """One wiki page as the site wants it."""

    def link(match):
        target, anchor = match.group(1), match.group(2) or ""
        if target not in pages:
            # Not a page of the wiki (a word in brackets, say): as it was.
            return match.group(0)
        if anchor and anchor[1:] not in anchors(pages[target]):
            problems.append(f"wiki/{page}.md links to {target}{anchor}, a heading it doesn't have")
        return f"]({file_of(target)}{anchor})"

    def local(match):
        if match.group(1)[1:] not in anchors(text):
            problems.append(f"wiki/{page}.md links to {match.group(1)}, a heading it doesn't have")
        return match.group(0)

    out, lines, i, fenced = [f"# {title}", ""], text.splitlines(), 0, False
    while i < len(lines):
        line = lines[i]
        if line.lstrip().startswith("```"):
            fenced = not fenced
        if not fenced and (alert := ALERT.match(line)):
            # `> [!WARNING]` and the quote's lines: an admonition.
            out.append(f"!!! {ADMONITION[alert.group(1)]}")
            out.append("")
            i += 1
            while i < len(lines) and lines[i].startswith(">"):
                body = lines[i][1:].removeprefix(" ")
                out.append(f"    {body}" if body else "")
                i += 1
            out.append("")
            continue
        if not fenced:
            line = LOCAL_LINK.sub(local, WIKI_LINK.sub(link, line))
        out.append(line)
        i += 1
    return "\n".join(out).rstrip() + "\n"


def write() -> None:
    pages = {path.stem: path.read_text() for path in WIKI.glob("*.md") if path.stem != SIDEBAR}
    nav = sidebar()
    titles = {}
    for name, target in nav:
        for title, page in target if isinstance(target, list) else [(name, target)]:
            titles[page] = title
    problems = [f"wiki/{SIDEBAR}.md lists {page}, which isn't in wiki/" for page in titles if page not in pages]
    problems += [f"wiki/{page}.md isn't in wiki/{SIDEBAR}.md" for page in pages if page not in titles]

    shutil.rmtree(OUT, ignore_errors=True)
    PAGES.mkdir(parents=True)
    for page, text in sorted(pages.items()):
        title = "rettui" if page == HOME else titles.get(page, page.replace("-", " "))
        (PAGES / file_of(page)).write_text(convert(page, text, title, pages, problems))
    if problems:
        sys.exit("The pages can't be built:\n  " + "\n  ".join(problems))

    config = (DOCS / "zensical.toml").read_text()
    if "#NAV#" not in config:
        sys.exit("docs/zensical.toml has no #NAV# line for the navigation")
    (OUT / "zensical.toml").write_text(config.replace("#NAV#", nav_toml(nav)))
    print(f"Wrote {len(pages)} pages to {PAGES.relative_to(ROOT)}")


def main() -> None:
    command = sys.argv[1] if len(sys.argv) > 1 else "build"
    if command not in ("build", "serve", "pages"):
        sys.exit(__doc__)
    write()
    config = str(OUT / "zensical.toml")
    if command == "build":
        subprocess.run(["zensical", "build", "--clean", "--strict", "-f", config], check=True)
        print(f"The site is in {(OUT / 'site').relative_to(ROOT)}")
    elif command == "serve":
        subprocess.run(["zensical", "serve", "-f", config], check=True)


if __name__ == "__main__":
    main()
