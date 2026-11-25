#!/usr/bin/env python3
"""Render the seven Markdown pages in this folder to HTML.

Needs the `markdown` and `pygments` packages:
    python3 docs/cogent-design/render.py
or  uv run --with markdown --with pygments docs/cogent-design/render.py
"""
from __future__ import annotations

import html
import re
from pathlib import Path

import markdown

HERE = Path(__file__).resolve().parent
STATUSES = {
    "frozen": "frozen",
    "ADR-0014": "adr",
    "ADR-0013": "adr",
    "draft": "draft",
    "proposed": "proposed",
}

CSS = r"""
:root {
  --paper: #f6f3ec; --panel: #fffdf8; --ink: #1d1b16; --muted: #6b6558;
  --rule: #e2dccd; --accent: #0b6e69; --accent-soft: #d9ece9;
  --code-bg: #fbf9f3; --code-ink: #2a2721;
  --kw: #8a3b12; --cls: #0b6e69; --fn: #1f4e8c; --str: #5d6b1e; --doc: #6b6558;
  --num: #8c2f5a; --deco: #7a4fa3; --cmt: #9a9384;
  --frozen-bg: #dfe9f5; --frozen-ink: #1f4e8c;
  --adr-bg: #f3e6c8; --adr-ink: #7a5200;
  --draft-bg: #e9e4f3; --draft-ink: #5b3f8f;
  --proposed-bg: #d9ece9; --proposed-ink: #0b5a56;
  --sans: "Avenir Next", "Segoe UI", system-ui, -apple-system, sans-serif;
  --serif: "Iowan Old Style", "Palatino Linotype", Palatino, Georgia, serif;
  --mono: "JetBrains Mono", "SF Mono", ui-monospace, Menlo, Consolas, monospace;
}
@media (prefers-color-scheme: dark) {
  :root {
    --paper: #14161a; --panel: #1b1e24; --ink: #e9e6df; --muted: #9a968c;
    --rule: #2c3038; --accent: #5cc8bf; --accent-soft: #17302f;
    --code-bg: #171a1f; --code-ink: #dcd8cf;
    --kw: #e8a06a; --cls: #5cc8bf; --fn: #8fb8f0; --str: #b9c97a; --doc: #9a968c;
    --num: #e58fb6; --deco: #c3a2e8; --cmt: #6f6b62;
    --frozen-bg: #1c2b40; --frozen-ink: #9cc2f2;
    --adr-bg: #3a2f14; --adr-ink: #ecc56b;
    --draft-bg: #2b2440; --draft-ink: #c3a2e8;
    --proposed-bg: #17302f; --proposed-ink: #7fdcd3;
  }
}
* { box-sizing: border-box; }
html { scroll-behavior: smooth; scroll-padding-top: 1.5rem; }
body { margin: 0; background: var(--paper); color: var(--ink);
  font: 16px/1.6 var(--sans); -webkit-font-smoothing: antialiased; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
.layout { display: grid; grid-template-columns: 17.5rem minmax(0, 1fr); min-height: 100vh; }
nav { position: sticky; top: 0; align-self: start; height: 100vh; overflow-y: auto;
  padding: 1.5rem 1rem 3rem 1.5rem; border-right: 1px solid var(--rule); background: var(--panel); }
nav .brand { font: 600 1.15rem/1.2 var(--serif); letter-spacing: .01em; margin-bottom: .2rem; }
nav .sub { color: var(--muted); font-size: .8rem; margin-bottom: 1rem; }
nav input { width: 100%; padding: .45rem .6rem; margin-bottom: 1rem; font: inherit; font-size: .85rem;
  color: var(--ink); background: var(--paper); border: 1px solid var(--rule); border-radius: 6px; }
nav input:focus { outline: 2px solid var(--accent); outline-offset: 1px; }
nav ul { list-style: none; margin: 0; padding: 0; }
nav li.h2 > a { display: block; margin-top: .9rem; padding: .15rem .5rem; font-weight: 600;
  font-size: .78rem; letter-spacing: .08em; text-transform: uppercase; color: var(--ink); }
nav li.h3 > a { display: block; padding: .12rem .5rem .12rem .9rem; font-size: .84rem;
  color: var(--muted); border-left: 2px solid transparent; }
nav li.h3 > a code { font: inherit; font-family: var(--mono); font-size: .78rem; }
nav a:hover { text-decoration: none; color: var(--accent); }
nav a.active { color: var(--accent); border-left-color: var(--accent); background: var(--accent-soft); }
main { padding: 3rem clamp(1rem, 4vw, 4rem) 6rem; max-width: 62rem; }
h1 { font: 600 2.6rem/1.1 var(--serif); margin: 0 0 1rem; letter-spacing: -.01em; }
h2 { font: 600 1.75rem/1.2 var(--serif); margin: 4rem 0 .8rem; padding-top: 1.2rem;
  border-top: 2px solid var(--ink); }
h3 { font: 600 1.05rem/1.3 var(--sans); margin: 2.4rem 0 .6rem; display: flex;
  flex-wrap: wrap; align-items: center; gap: .5rem; }
h3 code { font-family: var(--mono); font-size: 1rem; background: none; padding: 0; color: var(--cls); }
p, li { max-width: 46rem; }
p code, li code, td code { font-family: var(--mono); font-size: .86em; background: var(--accent-soft);
  color: var(--ink); padding: .08em .35em; border-radius: 4px; white-space: nowrap; }
.badge { font: 600 .68rem/1 var(--sans); letter-spacing: .06em; text-transform: uppercase;
  padding: .3em .6em; border-radius: 99px; white-space: nowrap; }
.badge.frozen { background: var(--frozen-bg); color: var(--frozen-ink); }
.badge.adr { background: var(--adr-bg); color: var(--adr-ink); }
.badge.draft { background: var(--draft-bg); color: var(--draft-ink); }
.badge.proposed { background: var(--proposed-bg); color: var(--proposed-ink); }
.headerlink { opacity: 0; margin-left: .2rem; color: var(--muted); font-weight: 400; }
h2:hover .headerlink, h3:hover .headerlink { opacity: 1; }
.codehilite { background: var(--code-bg); border: 1px solid var(--rule); border-left: 3px solid var(--accent);
  border-radius: 8px; margin: .8rem 0 1.2rem; overflow-x: auto; }
.codehilite pre { margin: 0; padding: 1rem 1.2rem; font: 13.5px/1.6 var(--mono); color: var(--code-ink); }
.codehilite .k, .codehilite .kn, .codehilite .kc, .codehilite .kd, .codehilite .kt, .codehilite .ow { color: var(--kw); }
.codehilite .nc, .codehilite .nn { color: var(--cls); font-weight: 600; }
.codehilite .nf, .codehilite .fm { color: var(--fn); font-weight: 600; }
.codehilite .nd { color: var(--deco); }
.codehilite .s, .codehilite .s1, .codehilite .s2, .codehilite .sa, .codehilite .sr { color: var(--str); }
.codehilite .sd { color: var(--doc); font-family: var(--sans); font-size: 13.5px; }
.codehilite .mi, .codehilite .mf, .codehilite .mh { color: var(--num); }
.codehilite .c, .codehilite .c1, .codehilite .cm { color: var(--cmt); font-style: italic; }
.codehilite .nb, .codehilite .bp { color: var(--fn); }
.tablewrap { overflow-x: auto; margin: 1rem 0 1.4rem; }
table { border-collapse: collapse; width: 100%; font-size: .92rem; }
th { text-align: left; font-size: .72rem; letter-spacing: .08em; text-transform: uppercase;
  color: var(--muted); border-bottom: 2px solid var(--ink); padding: .5rem .8rem .5rem 0; white-space: nowrap; }
td { border-bottom: 1px solid var(--rule); padding: .55rem .8rem .55rem 0; vertical-align: top; }
blockquote { margin: 1rem 0; padding: .2rem 1rem; border-left: 3px solid var(--accent); color: var(--muted); }
ol, ul { padding-left: 1.3rem; }
li { margin: .25rem 0; }
.idl .nc { color: var(--cls); font-weight: 600; }
.idl .nf { color: var(--fn); font-weight: 600; }
.idl .na { color: var(--code-ink); }
.idl .ty { color: var(--kw); }
.idl .op { color: var(--str); }
.idl .kw, .idl .rt { color: var(--deco); }
.idl .sd { color: var(--doc); font-family: var(--sans); }
.idl .c { color: var(--cmt); font-style: italic; }
main p > img { display: block; max-width: min(100%, 34rem); height: auto; margin: 1rem 0; border-radius: 14px; }
figure.diagram { margin: 1.4rem 0 .4rem; }
figure.diagram svg { display: block; width: 100%; height: auto; }
figure.diagram + p { color: var(--muted); font-size: .88rem; margin-top: .3rem; }
nav .pages { display: flex; flex-direction: column; gap: .3rem; margin-bottom: 1rem; }
nav .pages a { padding: .35rem .6rem; font-size: .8rem; font-weight: 600;
  border: 1px solid var(--rule); border-radius: 6px; color: var(--muted); }
nav .pages a.here { background: var(--accent); border-color: var(--accent); color: var(--panel); }
@media (max-width: 860px) {
  .layout { grid-template-columns: 1fr; }
  nav { position: static; height: auto; border-right: 0; border-bottom: 1px solid var(--rule); max-height: 45vh; }
  h1 { font-size: 2rem; }
}
@media print { nav { display: none; } .layout { display: block; } }
"""

JS = r"""
const links = [...document.querySelectorAll('nav a[href^="#"]')];
const byId = new Map(links.map(a => [a.getAttribute('href').slice(1), a]));
const seen = new IntersectionObserver(entries => {
  for (const e of entries) if (e.isIntersecting) {
    links.forEach(a => a.classList.remove('active'));
    const a = byId.get(e.target.id);
    if (a) { a.classList.add('active'); a.scrollIntoView({block: 'nearest'}); }
  }
}, {rootMargin: '0px 0px -75% 0px'});
document.querySelectorAll('main h2[id], main h3[id]').forEach(h => seen.observe(h));
document.getElementById('filter').addEventListener('input', ev => {
  const q = ev.target.value.trim().toLowerCase();
  document.querySelectorAll('nav li').forEach(li => {
    li.hidden = q !== '' && !li.textContent.toLowerCase().includes(q);
  });
});
"""


def badge(match: re.Match[str]) -> str:
    name = match.group(1)
    return f'<span class="badge {STATUSES[name]}">{name}</span>'


PAGES = [
    ("README.md", "index.html", "Cogent"),
    ("coglets-and-runtime.md", "coglets-and-runtime.html", "Coglets and runtime"),
    ("app-integration.md", "app-integration.html", "App integration"),
    ("softmax-app.md", "softmax-app.html", "The Softmax app"),
    ("cognitive-architecture.md", "cognitive-architecture.html", "Cognitive architecture"),
    ("unforged.md", "unforged.html", "Coglets on unforged"),
    ("unforged-staging.md", "unforged-staging.html", "Staged integration"),
]
COGLET_ROW = re.compile(r"^  (subscribes|needs|owns|on|provides|calls)(\s{2,})(.*)$")
METHOD = re.compile(r"^  (\w+)\((.*?)\)(?: -> (.*?))?(\s+\[[^\]]+\])?$")
FIELD = re.compile(r"^  (\w+)(\s{2,})(\S.*?)(?:(\s{2,})(\S.*))?$")


def span(cls: str, text: str) -> str:
    return f'<span class="{cls}">{html.escape(text)}</span>' if text else ""


def idl_line(line: str) -> str:
    """Colour one line of the compact type notation."""
    if not line.strip():
        return ""
    if line.startswith("-- "):
        return span("c", line)
    if not line.startswith(" "):
        name, eq, rest = line.partition(" = ")
        return span("nc", name) + (" = " + span("ty", rest) if eq else "")
    indent = len(line) - len(line.lstrip())
    if line.lstrip().startswith("# "):
        return " " * indent + span("sd", line.strip()[2:])
    if indent >= 6:
        return " " * indent + span("sd", line.strip())
    if m := COGLET_ROW.match(line):
        kind, gap, rest = m.groups()
        if kind in ("subscribes", "needs", "owns"):
            return "  " + span("kw", kind) + gap + span("op", rest)
        cells = re.split(r"(\s{2,})", rest)
        classes = ["op", "ty", "rt"]
        out, i = "", 0
        for cell in cells:
            if cell.strip():
                out += span(classes[min(i, 2)], cell)
                i += 1
            else:
                out += cell
        return "  " + span("kw", kind) + gap + out
    if m := METHOD.match(line):
        name, args, ret, tag = m.groups()
        out = "  " + span("nf", name) + "(" + span("ty", args) + ")"
        if ret:
            out += " -&gt; " + span("ty", ret)
        return out + span("c", tag or "")
    if m := FIELD.match(line):
        name, gap, typ, gap2, desc = m.groups()
        return "  " + span("na", name) + gap + span("ty", typ) + (gap2 or "") + span("sd", desc or "")
    return html.escape(line)


def render(source_name: str, out_name: str) -> str:
    source = (HERE / source_name).read_text(encoding="utf-8")
    blocks: list[str] = []

    def stash(match: re.Match[str]) -> str:
        lines = "\n".join(idl_line(line) for line in match.group(1).rstrip("\n").split("\n"))
        blocks.append(f'<div class="codehilite idl"><pre>{lines}</pre></div>')
        return f"\n\nIDLBLOCK{len(blocks) - 1}END\n\n"

    text = re.sub(r"```idl\n(.*?)```", stash, source, flags=re.S)
    md = markdown.Markdown(
        extensions=["fenced_code", "codehilite", "tables", "toc", "sane_lists"],
        extension_configs={
            "codehilite": {"guess_lang": False, "css_class": "codehilite"},
            "toc": {"permalink": "#", "toc_depth": "2-3"},
        },
    )
    body = md.convert(text)
    body = re.sub(r"<p>IDLBLOCK(\d+)END</p>", lambda m: blocks[int(m.group(1))], body)

    def figure(match: re.Match[str]) -> str:
        svg = (HERE / match.group(1)).read_text(encoding="utf-8")
        return f'<figure class="diagram">{svg}</figure>'

    body = re.sub(r'<p><img alt="[^"]*" src="(diagrams/[\w-]+\.svg)" ?/?></p>', figure, body)
    names = "|".join(re.escape(name) for name in STATUSES)
    status = re.compile(rf"<code>({names})</code>")
    # Status words become badges in headings and in table cells that hold only a status.
    body = re.sub(
        r"<h3[^>]*>.*?</h3>", lambda m: status.sub(badge, m.group(0)), body, flags=re.S
    )
    body = re.sub(rf"<td>\s*<code>({names})</code>\s*</td>", lambda m: f"<td>{badge(m)}</td>", body)
    body = body.replace("<table>", '<div class="tablewrap"><table>').replace(
        "</table>", "</table></div>"
    )
    for md_name, html_name, _ in PAGES:
        body = body.replace(f'href="{md_name}', f'href="{html_name}')

    items = []
    for h2 in md.toc_tokens:
        items.append(f'<li class="h2"><a href="#{h2["id"]}">{html.escape(h2["name"])}</a></li>')
        for h3 in h2.get("children", []):
            label = status.sub("", h3["html"]).replace(" + ", "").strip()
            items.append(f'<li class="h3"><a href="#{h3["id"]}">{label}</a></li>')
    heading = re.search(r"^# (.+)$", source, flags=re.M)
    title = heading.group(1) if heading else "Coglet interfaces"
    pages = "".join(
        f'<a href="{html_name}"{" class=\"here\"" if html_name == out_name else ""}>{label}</a>'
        for _, html_name, label in PAGES
    )
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<style>{CSS}</style>
</head>
<body>
<div class="layout">
<nav>
  <div class="brand">{html.escape(title)}</div>
  <div class="sub">Generated from {source_name}. Do not edit.</div>
  <div class="pages">{pages}</div>
  <input id="filter" type="search" placeholder="Filter types and APIs" aria-label="Filter">
  <ul>
  {"".join(items)}
  </ul>
</nav>
<main>
{body}
</main>
</div>
<script>{JS}</script>
</body>
</html>
"""


if __name__ == "__main__":
    for source_name, out_name, _ in PAGES:
        out = HERE / out_name
        out.write_text(render(source_name, out_name), encoding="utf-8")
        print(f"wrote {out}")
