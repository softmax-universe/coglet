#!/usr/bin/env python3
"""Generate the SVG diagrams used by the pages in this folder.

No dependencies:  python3 docs/cogent-design/diagrams.py
Each diagram is a standalone SVG with its own light and dark palette, so it
reads on GitHub and when render.py inlines it into the HTML pages.
"""
from __future__ import annotations

from html import escape
from pathlib import Path

OUT = Path(__file__).resolve().parent / "diagrams"

STYLE = """
.dg-bg{fill:#fffdf8;stroke:#e2dccd}
.dg-group{fill:#f3efe4}
.dg-t{font:12px 'Avenir Next','Segoe UI',system-ui,sans-serif;fill:#1d1b16}
.dg-b{font:600 13px 'Avenir Next','Segoe UI',system-ui,sans-serif;fill:#1d1b16}
.dg-m{font:11px 'Avenir Next','Segoe UI',system-ui,sans-serif;fill:#6b6558}
.dg-cap{font:600 10px 'Avenir Next','Segoe UI',system-ui,sans-serif;fill:#6b6558;letter-spacing:.08em}
.dg-code{font:11px 'JetBrains Mono','SF Mono',ui-monospace,Menlo,monospace;fill:#1d1b16}
.dg-halo{paint-order:stroke;stroke:#fffdf8;stroke-width:5px;stroke-linejoin:round}
.dg-coglet{fill:#d9ece9;stroke:#0b6e69}
.dg-adapter{fill:#f8e6d6;stroke:#a4531c}
.dg-runtime{fill:#fffdf8;stroke:#8a8474}
.dg-hub{fill:#dfe9f5;stroke:#1f4e8c}
.dg-world{fill:none;stroke:#8a8474;stroke-dasharray:4 4}
.dg-life{stroke:#c9c2b0;stroke-dasharray:3 4}
.dg-ink{stroke:#3a372f;fill:none}.dg-ink-h{fill:#3a372f}
.dg-teal{stroke:#0b6e69;fill:none}.dg-teal-h{fill:#0b6e69}
.dg-warm{stroke:#a4531c;fill:none}.dg-warm-h{fill:#a4531c}
.dg-blue{stroke:#1f4e8c;fill:none}.dg-blue-h{fill:#1f4e8c}
.dg-dash{stroke-dasharray:6 4}
.dg-present{fill:none;stroke:#b8860b;stroke-width:2.4}
.dg-num{fill:#1d1b16}.dg-numt{font:600 10px 'Avenir Next',system-ui,sans-serif;fill:#fffdf8}
.dg-e-basic{fill:#f6e7c1;stroke:#a87b12}.dg-e-wasm{fill:#e6dff5;stroke:#6a4bb0}.dg-e-js{fill:#e3efc8;stroke:#5d7a1c}
.dg-e-java{fill:#f6d9d6;stroke:#b0463c}.dg-e-llm{fill:#d3ecf0;stroke:#19788a}
@media (prefers-color-scheme: dark){
.dg-bg{fill:#1b1e24;stroke:#2c3038}
.dg-group{fill:#22262d}
.dg-t,.dg-b,.dg-code{fill:#e9e6df}
.dg-m,.dg-cap{fill:#9a968c}
.dg-halo{stroke:#1b1e24}
.dg-coglet{fill:#17302f;stroke:#5cc8bf}
.dg-adapter{fill:#3a2616;stroke:#e8a06a}
.dg-runtime{fill:#1b1e24;stroke:#7d796f}
.dg-hub{fill:#1c2b40;stroke:#8fb8f0}
.dg-world{stroke:#7d796f}
.dg-life{stroke:#3a3f48}
.dg-ink{stroke:#c9c5bb}.dg-ink-h{fill:#c9c5bb}
.dg-teal{stroke:#5cc8bf}.dg-teal-h{fill:#5cc8bf}
.dg-warm{stroke:#e8a06a}.dg-warm-h{fill:#e8a06a}
.dg-blue{stroke:#8fb8f0}.dg-blue-h{fill:#8fb8f0}
.dg-num{fill:#e9e6df}.dg-numt{fill:#1b1e24}
.dg-present{stroke:#e5ff00}
.dg-e-basic{fill:#3a2f14;stroke:#ecc56b}.dg-e-wasm{fill:#2b2440;stroke:#c3a2e8}.dg-e-js{fill:#26331a;stroke:#b9c97a}
.dg-e-java{fill:#3d1f1d;stroke:#f0958a}.dg-e-llm{fill:#14323a;stroke:#6fd0e0}
}
"""

COLORS = ("ink", "teal", "warm", "blue")


class Svg:
    def __init__(self, name: str, width: int, height: int, label: str):
        self.name, self.w, self.h, self.label = name, width, height, label
        self.parts: list[str] = []

    def add(self, text: str) -> None:
        self.parts.append(text)

    def rect(self, x, y, w, h, cls, r=8) -> None:
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" class="{cls}"/>')

    def text(self, x, y, s, cls="dg-t", anchor="middle", halo=False) -> None:
        extra = " dg-halo" if halo else ""
        self.add(f'<text x="{x}" y="{y}" text-anchor="{anchor}" class="{cls}{extra}">{escape(s)}</text>')

    def box(self, x, y, w, h, title, cls, sub=None, code=False) -> None:
        self.rect(x, y, w, h, cls)
        cx = x + w / 2
        title_cls = "dg-code" if code else "dg-b"
        if sub:
            self.text(cx, y + h / 2 - 3, title, title_cls)
            self.text(cx, y + h / 2 + 13, sub, "dg-m")
        else:
            self.text(cx, y + h / 2 + 4, title, title_cls)

    def arrow(self, x1, y1, x2, y2, color="ink", dashed=False, both=False) -> None:
        cls = f"dg-{color}" + (" dg-dash" if dashed else "")
        start = f' marker-start="url(#{self.name}-{color}-s)"' if both else ""
        self.add(
            f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" class="{cls}" stroke-width="1.6"'
            f' marker-end="url(#{self.name}-{color})"{start}/>'
        )

    def path(self, d, color="ink", dashed=False, both=False) -> None:
        cls = f"dg-{color}" + (" dg-dash" if dashed else "")
        start = f' marker-start="url(#{self.name}-{color}-s)"' if both else ""
        self.add(f'<path d="{d}" class="{cls}" stroke-width="1.6" marker-end="url(#{self.name}-{color})"{start}/>')

    def number(self, x, y, n) -> None:
        self.add(f'<circle cx="{x}" cy="{y}" r="8" class="dg-num"/>')
        self.text(x, y + 3.5, str(n), "dg-numt")

    def render(self) -> str:
        markers = []
        for color in COLORS:
            markers.append(
                f'<marker id="{self.name}-{color}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7"'
                f' markerHeight="7" orient="auto"><path d="M0 0L10 5L0 10z" class="dg-{color}-h"/></marker>'
                f'<marker id="{self.name}-{color}-s" viewBox="0 0 10 10" refX="1" refY="5" markerWidth="7"'
                f' markerHeight="7" orient="auto"><path d="M10 0L0 5L10 10z" class="dg-{color}-h"/></marker>'
            )
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.w}" height="{self.h}" viewBox="0 0 {self.w} {self.h}" role="img"'
            f' aria-label="{escape(self.label)}">\n<style>{STYLE}</style>\n<defs>{"".join(markers)}</defs>\n'
            f'<rect x="0.5" y="0.5" width="{self.w - 1}" height="{self.h - 1}" rx="12" class="dg-bg"/>\n'
            + "\n".join(self.parts)
            + "\n</svg>\n"
        )


def legend(svg: Svg, x: int, y: int, items: list[tuple[str, bool, str]]) -> None:
    for color, dashed, label in items:
        svg.arrow(x, y, x + 34, y, color, dashed)
        svg.text(x + 42, y + 4, label, "dg-m", "start")
        x += 60 + len(label) * 6


def stacks(name: str, label: str, columns, hub_title: str = "Hub kernel") -> Svg:
    """columns: (x, width, caption, [(world, adapter, sub)], [coglet names])."""
    svg = Svg(name, 960, 500, label)
    for x, width, caption, adapters, coglets in columns:
        cx = x + width / 2
        svg.rect(x - 12, 92, width + 24, 262, "dg-group", 12)
        svg.text(x, 344, caption, "dg-cap", "start")
        gap = 14
        aw = (width - gap * (len(adapters) - 1)) / len(adapters)
        for i, (world, adapter, sub) in enumerate(adapters):
            ax = x + i * (aw + gap)
            ac = ax + aw / 2
            svg.box(ax + 8, 24, aw - 16, 40, world, "dg-world")
            svg.box(ax, 108, aw, 50, adapter, "dg-adapter", sub)
            svg.arrow(ac, 66, ac, 106, "ink", both=True)
            svg.arrow(ac, 160, ac, 202, "warm", both=True)
            if i == 0:
                svg.text(ac + 10, 90, "native I/O", "dg-m", "start")
                svg.text(ac + 10, 186, "events, actions", "dg-m", "start")
        svg.rect(x, 204, width, 118, "dg-runtime")
        svg.text(x + 12, 224, "CogletRuntime", "dg-b", "start")
        if width >= 280:
            svg.text(x + width - 12, 224, "runs coglets", "dg-m", "end")
        chip = (width - 24 - 8 * (len(coglets) - 1)) / len(coglets)
        for i, coglet in enumerate(coglets):
            svg.box(x + 12 + i * (chip + 8), 240, chip, 66, coglet, "dg-coglet")
        svg.arrow(cx, 324, cx, 408, "blue", both=True)
        svg.text(cx + 10, 384, "messages, patches", "dg-m", "start")
    svg.rect(40, 410, 880, 62, "dg-hub")
    svg.text(480, 437, hub_title, "dg-b")
    svg.text(480, 456, "finds runtimes · carries messages between them · carries patches and logs", "dg-m")
    return svg



def deployment() -> Svg:
    return stacks(
        "deployment",
        "The Softmax app on the user's machine runs Face and the coglets for its connected apps; a Softmax-hosted runtime runs the fast loop, factory, optimizer and memory; the router joins them.",
        [
            (
                40,
                440,
                "USER'S MACHINE: THE SOFTMAX APP",
                [("Orb, mic, speaker", "Softmax app", None), ("Blender", "Blender adapter", None), ("Chrome", "Chrome adapter", None)],
                ["FaceCoglet", "BlenderCoglet", "BrowserCoglet"],
            ),
            (540, 380, "SOFTMAX-HOSTED RUNTIME", [("Workers", "Factory adapter", "runs the jobs")], ["FastLoop", "Factory", "Optimizer", "Memory"]),
        ],
        hub_title="Router",
    )


def cogent() -> Svg:
    svg = Svg("cogent", 1040, 730, "A cogent has no centre: its coglets run on many runtimes, on a laptop, a robot, a phone, in the Softmax Universe and on a Softmax-hosted runtime, and one router carries messages and patches between all of them. The cogent is present on Face and Blender on the laptop; the rest run on their own.")
    stroke = 'class="dg-ink" stroke-width="1.5"'
    svg.add('<g transform="translate(0,190)">')

    def icon(kind, x, y):
        if kind == "laptop":
            svg.add(f'<rect x="{x + 3}" y="{y}" width="16" height="11" rx="1.5" {stroke}/><path d="M{x} {y + 14}H{x + 22}" {stroke}/>')
        elif kind == "phone":
            svg.add(f'<rect x="{x + 6}" y="{y - 2}" width="10" height="17" rx="2.5" {stroke}/><path d="M{x + 9.5} {y + 12}H{x + 12.5}" {stroke}/>')
        elif kind == "cogent":  # a robot head
            svg.add(
                f'<rect x="{x + 2}" y="{y + 1}" width="18" height="13" rx="3" {stroke}/>'
                f'<path d="M{x + 11} {y + 1}V{y - 4}" {stroke}/><circle cx="{x + 11}" cy="{y - 5.5}" r="1.6" {stroke}/>'
                f'<circle cx="{x + 7}" cy="{y + 7}" r="1.4" class="dg-ink-h"/><circle cx="{x + 15}" cy="{y + 7}" r="1.4" class="dg-ink-h"/>'
                f'<path d="M{x - 1} {y + 5}V{y + 10}M{x + 23} {y + 5}V{y + 10}" {stroke}/>'
            )
        else:  # a whole small robot
            t = y - 6
            svg.add(
                f'<path d="M{x + 11} {t}V{t + 3}" {stroke}/><circle cx="{x + 11}" cy="{t - 1}" r="1.3" class="dg-ink-h"/>'
                f'<rect x="{x + 4.5}" y="{t + 3}" width="13" height="9" rx="3" {stroke}/>'
                f'<circle cx="{x + 8.5}" cy="{t + 7.5}" r="1.2" class="dg-ink-h"/><circle cx="{x + 13.5}" cy="{t + 7.5}" r="1.2" class="dg-ink-h"/>'
                f'<rect x="{x + 6}" y="{t + 14}" width="10" height="8" rx="2.5" {stroke}/>'
                f'<path d="M{x + 3} {t + 15}V{t + 20}M{x + 19} {t + 15}V{t + 20}" {stroke}/>'
            )

    # the router, linking every runtime; no coglet is special
    svg.box(385, 135, 270, 62, "Router", "dg-hub", "a coglet: messages, patches, presence")
    icon("cogent", 398, 147)

    def side_device(x, kind, caption, apps, coglets, coglet_on_right, shared=None):
        """Rows of app and coglet, with the coglets on the side facing the cogent."""
        y, w = 30, 270
        svg.rect(x, y, w, 280, "dg-bg", 12)
        icon(kind, x + 14, y + 13)
        svg.text(x + 44, y + 25, caption, "dg-cap", "start")
        app_x = x + 14 if coglet_on_right else x + 146
        cog_x = x + 158 if coglet_on_right else x + 14
        for i, app in enumerate(apps):
            ry = y + 42 + i * 46
            svg.rect(app_x, ry, 110, 38, "dg-world")
            strip_x = app_x + 98 if coglet_on_right else app_x
            svg.rect(strip_x, ry, 12, 38, "dg-adapter", 5)
            svg.text(app_x + (49 if coglet_on_right else 61), ry + 23, app, "dg-t")
            ay = ry + 9 if i == shared else (ry + 24 if shared is not None and i == shared + 1 else ry + 19)
            if coglet_on_right:
                svg.arrow(x + 126, ay, x + 156, ay, "warm", both=True)
            else:
                svg.arrow(x + 114, ay, x + 144, ay, "warm", both=True)
            if i == shared:  # the coglet in the next row uses this app too
                svg.path(f"M{x + 114} {ry + 53}H{x + 129}V{ry + 29}H{x + 144}", "warm", both=True)
        row = 0
        for item in coglets:
            name, span = item if isinstance(item, tuple) else (item, 1)
            svg.box(cog_x, y + 42 + row * 46, 98, span * 38 + (span - 1) * 8, name, "dg-coglet")
            row += span

    side_device(20, "laptop", "LAPTOP", ["Softmax app", "Blender", "Chrome", "Minecraft", "Mindustry"], ["Face", "Blender", "Browser", "Minecraft", "Mindustry"], True)
    side_device(750, "robot", "ROBOT", ["Microphone", "Display", "Camera", "Wheels", "Hand"], [("Face", 3), ("Controller", 2)], False, shared=2)

    svg.arrow(383, 166, 292, 166, "blue", both=True)
    svg.arrow(657, 166, 748, 166, "blue", both=True)
    # presence: the cogent is paying attention to Face and Blender on the laptop
    for row in (0, 1):
        svg.rect(174, 68 + row * 46, 106, 46, "dg-present", 9)
    svg.add("</g>")

    def top_box(bx, bw, caption, pairs):
        """Apps on top, coglets underneath on the side facing the cogent."""
        by = 20
        svg.rect(bx, by, bw, 168, "dg-bg", 12)
        svg.text(bx + 46, by + 25, caption, "dg-cap", "start")
        cw = (bw - 28 - 8 * (len(pairs) - 1)) / len(pairs)
        for i, (app, name) in enumerate(pairs):
            gx = bx + 14 + i * (cw + 8)
            svg.rect(gx, by + 40, cw, 42, "dg-world")
            svg.rect(gx, by + 70, cw, 12, "dg-adapter", 5)
            svg.text(gx + cw / 2, by + 60, app, "dg-t")
            svg.arrow(gx + cw / 2, by + 84, gx + cw / 2, by + 112, "warm", both=True)
            svg.box(gx, by + 114, cw, 42, name, "dg-coglet")

    # games hosted in the Softmax Universe, above left
    top_box(213, 344, "SOFTMAX UNIVERSE", [("Paintbot", "Paintbot"), ("GOTA", "GOTA"), ("PudgeWars", "PudgeWars")])
    svg.add(
        f'<circle cx="238" cy="40" r="6" {stroke}/>'
        f'<ellipse cx="238" cy="40" rx="11" ry="3.6" transform="rotate(-20 238 40)" {stroke}/>'
    )
    # the phone, above right
    top_box(587, 240, "PHONE", [("Softmax app", "Face"), ("A phone app", "App coglet")])
    icon("phone", 601, 33)
    svg.arrow(450, 190, 450, 323, "blue", both=True)
    svg.arrow(600, 190, 600, 323, "blue", both=True)
    svg.text(462, 262, "installs, patches, messages", "dg-m", "start")

    # a Softmax-hosted runtime: just another runtime, its coglets run on their own
    hx, hy, hw = 250, 520, 540
    svg.rect(hx, hy, hw, 136, "dg-bg", 12)
    svg.text(hx + 14, hy + 25, "SOFTMAX-HOSTED RUNTIME", "dg-cap", "start")
    hosted = [("FastLoop", "attention"), ("Factory", "builds patches"), ("Optimizer", "improves"), ("Memory", "what it knows"), ("Database", "structured data")]
    cw = (hw - 28 - 8 * 4) / 5
    for i, (name, what) in enumerate(hosted):
        svg.box(hx + 14 + i * (cw + 8), hy + 40, cw, 60, name, "dg-coglet", what)
    svg.text(hx + hw / 2, hy + 122, "running on their own; the cogent checks in", "dg-m")
    svg.arrow(520, 389, 520, 518, "blue", both=True)

    legend(svg, 40, 695, [("blue", False, "the router carries it"), ("warm", False, "a coglet uses an app through its adapter")])
    svg.rect(500, 685, 22, 20, "dg-present", 5)
    svg.text(530, 699, "present", "dg-m", "start")
    lx = 610
    for cls, label in [("dg-coglet", "coglet"), ("dg-adapter", "adapter"), ("dg-world", "app or hardware")]:
        svg.rect(lx, 685, 20, 20, cls, 4)
        svg.text(lx + 28, 699, label, "dg-m", "start")
        lx += 60 + len(label) * 6
    return svg


def coglet() -> Svg:
    svg = Svg("coglet", 960, 430, "A coglet in the middle, with the apps it uses through adapters on the left, the capabilities its runtime provides below, and its manager on the right, sending patches and receiving logs.")
    svg.text(280, 40, "FRONT END: EVENTS AND CAPABILITIES", "dg-cap")
    svg.text(685, 40, "MANAGE PORT: PATCHES AND LOGS", "dg-cap")

    # the coglet, in the centre
    svg.rect(370, 64, 220, 214, "dg-coglet")
    svg.text(480, 90, "Coglet", "dg-b")
    rows = [("policy", "code, prompts, skills"), ("memory", "what it remembers"), ("event loop", "handles events"), ("capabilities", "what it may use")]
    for i, (path, what) in enumerate(rows):
        y = 104 + i * 42
        svg.rect(384, y, 192, 34, "dg-runtime", 6)
        svg.text(396, y + 21, path, "dg-code", "start")
        svg.text(564, y + 21, what, "dg-m", "end")

    # apps and hardware, through adapters, on the left
    svg.rect(30, 86, 162, 170, "dg-group", 10)
    svg.text(44, 110, "ADAPTERS", "dg-cap", "start")
    for i, name in enumerate(["An app", "A camera", "A display"]):
        svg.rect(44, 122 + i * 40, 134, 30, "dg-adapter", 6)
        svg.text(111, 141 + i * 40, name, "dg-t")
    svg.arrow(194, 142, 368, 142, "teal")
    svg.text(281, 134, "handle(event)", "dg-code")
    svg.arrow(368, 206, 194, 206, "teal")
    svg.text(281, 198, "actions", "dg-code")

    # what the runtime provides, below
    svg.rect(232, 334, 496, 76, "dg-group", 10)
    svg.text(246, 356, "THE RUNTIME", "dg-cap", "start")
    for i, name in enumerate(["LLM", "Database", "Random numbers", "Timer"]):
        svg.rect(246 + i * 119, 366, 111, 30, "dg-runtime", 6)
        svg.text(301.5 + i * 119, 385, name, "dg-t")
    svg.arrow(450, 280, 450, 332, "teal")
    svg.text(440, 310, "requests", "dg-code", "end")
    svg.arrow(510, 332, 510, 280, "teal")
    svg.text(520, 310, "replies, timer wakes", "dg-code", "start")

    # its manager, on the right
    svg.box(780, 96, 150, 150, "Its manager", "dg-hub", "a person or a coglet")
    svg.arrow(778, 124, 592, 124, "blue")
    svg.text(685, 116, "patch", "dg-code")
    svg.arrow(592, 174, 778, 174, "blue", dashed=True)
    svg.text(685, 166, "tail_log", "dg-code")
    svg.arrow(592, 224, 778, 224, "blue", dashed=True)
    svg.text(685, 216, "read_state · status", "dg-code")
    svg.text(685, 244, "each needs its own grant", "dg-m")
    return svg


def sequence(name, label, lifelines, messages, spacing=170, legend_items=None) -> Svg:
    """lifelines: [(title, box class)]; messages: (from, to, text, color, dashed) or ("note", text)."""
    left, top, row, head_w, head_h = 100, 24, 46, 136, 40
    xs = [left + i * spacing for i in range(len(lifelines))]
    height = top + head_h + 30 + row * len(messages) + (56 if legend_items else 24)
    svg = Svg(name, xs[-1] + left + 70, height, label)
    bottom = top + head_h + 14 + row * len(messages)
    for x, (title, cls) in zip(xs, lifelines):
        svg.add(f'<line x1="{x}" y1="{top + head_h}" x2="{x}" y2="{bottom}" class="dg-life" stroke-width="1.4"/>')
        svg.box(x - head_w / 2, top, head_w, head_h, title, cls)
    y = top + head_h + 44
    n = 0
    for message in messages:
        if message[0] == "note":
            svg.rect(xs[0] - 40, y - 24, xs[-1] - xs[0] + 80, 30, "dg-group", 6)
            svg.text((xs[0] + xs[-1]) / 2, y - 5, message[1], "dg-m")
            y += row
            continue
        src, dst, text, color, dashed = message
        n += 1
        x1, x2 = xs[src], xs[dst]
        if src == dst:
            svg.path(f"M{x1} {y - 12}h34v18h-27", color, dashed)
            svg.number(x1 - 14, y - 3, n)
            svg.text(x1 + 44, y + 1, text, "dg-code", "start", halo=True)
        else:
            direction = 1 if x2 > x1 else -1
            svg.arrow(x1 + 4 * direction, y, x2 - 4 * direction, y, color, dashed)
            svg.number(x1 + 18 * direction, y, n)
            svg.text((x1 + x2) / 2 + 9 * direction, y - 8, text, "dg-code", halo=True)
        y += row
    if legend_items:
        legend(svg, xs[0] - 60, height - 26, legend_items)
    return svg


def service_call() -> Svg:
    lifelines = [
        ("Sender coglet", "dg-coglet"),
        ("Sender's runtime", "dg-runtime"),
        ("Router", "dg-hub"),
        ("Receiver's runtime", "dg-runtime"),
        ("Receiver coglet", "dg-coglet"),
    ]
    messages = [
        (0, 1, "emit(Action)", "teal", False),
        (1, 1, "run succeeds", "ink", False),
        (1, 2, "send(message)", "blue", False),
        (2, 2, "check grant, pick receiver", "blue", False),
        (2, 1, "Accept", "blue", True),
        (1, 0, "action.outcome succeeded", "teal", True),
        (2, 3, "deliver(MessageDelivery)", "blue", False),
        (3, 4, "wake: new inbox message", "teal", False),
        ("note", "An answer is a new message from the receiver back to the sender, with the same correlation_id."),
    ]
    return sequence(
        "service-call",
        "A message between runtimes goes from the sender's runtime to the router, which accepts it and delivers it to the receiver's inbox.",
        lifelines,
        messages,
        spacing=205,
        legend_items=[("teal", False, "coglet and its runtime"), ("blue", False, "runtime and router"), ("ink", True, "dashed = answer")],
    )


def turn() -> Svg:
    lifelines = [
        ("App UI adapter", "dg-adapter"),
        ("FaceCoglet", "dg-coglet"),
        ("BlenderCoglet", "dg-coglet"),
        ("FastLoop", "dg-coglet"),
        ("Factory", "dg-coglet"),
    ]
    messages = [
        (0, 1, "speech.end", "warm", True),
        (1, 2, "intent.request", "teal", False),
        (1, 0, 'speak "On it."', "warm", False),
        (2, 2, "no skill for this", "ink", False),
        (2, 1, "intent.report needs_help", "teal", False),
        (1, 0, "ask the user to explain", "warm", False),
        (0, 1, "speech.end", "warm", True),
        (1, 3, "feature.request", "teal", False),
        (3, 4, "ticket.submit", "teal", False),
        (4, 4, "build, test", "ink", False),
        (4, 3, "ticket.result", "teal", False),
        (3, 2, "patch", "blue", False),
        (3, 1, "feature.update shipped", "teal", False),
        (1, 0, "tell the user", "warm", False),
    ]
    return sequence(
        "turn",
        "A spoken request goes from Face to the Blender coglet, which asks for help; Face files a feature request, the factory builds a patch, and the cogent applies it.",
        lifelines,
        messages,
        spacing=205,
    )


def calls() -> Svg:
    svg = Svg("calls", 770, 344, "Face exchanges intents with app coglets on the device and feature requests with the fast loop; the fast loop and the optimizer use memory and file tickets with the factory, wherever those coglets run.")
    # on a device, stacked
    svg.rect(20, 30, 190, 294, "dg-group", 14)
    svg.text(38, 54, "ON A DEVICE", "dg-cap", "start")
    svg.box(40, 70, 150, 56, "FaceCoglet", "dg-coglet", "talks with the user")
    svg.box(40, 250, 150, 56, "App coglets", "dg-coglet", "drive the apps")
    # the learning coglets, two by two, wherever they run
    svg.rect(310, 30, 440, 294, "dg-group", 14)
    svg.text(328, 54, "LEARNING AND MEMORY", "dg-cap", "start")
    svg.box(330, 70, 150, 56, "FastLoop", "dg-coglet", "responds, or forwards")
    svg.box(580, 70, 150, 56, "Memory", "dg-coglet", "what the cogent knows")
    svg.box(330, 250, 150, 56, "Factory", "dg-coglet", "tickets in, patches out")
    svg.box(580, 250, 150, 56, "Optimizer", "dg-coglet", "watches, improves")

    def group(x, y, name, verbs, anchor="start"):
        """A message family and the calls in it, as a small list."""
        svg.text(x, y, name, "dg-b", anchor)
        for i, verb in enumerate(verbs):
            svg.text(x, y + 15 + i * 13, verb, "dg-m", anchor)

    svg.arrow(110, 128, 110, 248, "teal", both=True)
    group(122, 172, "intent", ["request", "report"])
    svg.arrow(192, 98, 328, 98, "teal", both=True)
    svg.text(260, 74, "feature", "dg-b")
    svg.text(260, 89, "request · update", "dg-m")
    svg.arrow(482, 98, 578, 98, "teal", both=True)
    group(530, 88, "memory", [], "middle")
    group(530, 100, "", ["write", "query"], "middle")
    svg.arrow(405, 128, 405, 248, "teal", both=True)
    group(417, 152, "ticket", ["submit", "answer", "amend", "cancel", "replay"])
    svg.arrow(578, 278, 482, 278, "teal", both=True)
    group(530, 268, "ticket", [], "middle")
    group(530, 280, "", ["submit"], "middle")
    svg.arrow(655, 248, 655, 128, "teal", both=True)
    group(667, 178, "memory", ["query"])
    # the manage port, drawn separately from ordinary messages
    svg.path("M328 116H270V278H194", "blue")
    svg.text(282, 204, "patch", "dg-b", "start")
    svg.path("M470 128V190H620V248", "blue", dashed=True)
    svg.text(545, 182, "logs", "dg-b")
    return svg


def step() -> Svg:
    svg = Svg("step", 960, 330, "A coglet step stages effects, commits them atomically, and only then are they sent; results return later as new events.")
    svg.box(30, 96, 130, 64, "Event queue", "dg-runtime", "per coglet, ordered")
    svg.box(220, 96, 170, 64, "handle(events)", "dg-coglet", "reads files, stages effects", code=True)
    svg.box(450, 96, 170, 64, "Commit", "dg-runtime", "all of it, or none")
    svg.arrow(162, 128, 218, 128)
    svg.arrow(392, 128, 448, 128)
    targets = [
        (30, "Adapter", "dg-adapter", "action", "warm"),
        (106, "LLM", "dg-hub", "model call", "blue"),
        (182, "Router", "dg-hub", "message", "blue"),
    ]
    for y, title, cls, label, color in targets:
        svg.box(770, y, 160, 52, title, cls)
        svg.path(f"M622 128H690V{y + 26}H768", color)
        svg.text(726, y + 18, label, "dg-code", halo=True)
    svg.path("M850 236V288H95V162", "ink", dashed=True)
    svg.text(472, 280, "results come back later as new events: outcomes, model replies, messages", "dg-m", halo=True)
    svg.text(305, 190, "nonzero return: nothing is kept", "dg-m")
    svg.text(535, 190, "nothing is sent before this", "dg-m")
    svg.text(30, 44, "ONE STEP OF ONE COGLET", "dg-cap", "start")
    return svg


def adapters() -> Svg:
    svg = Svg("adapters", 960, 330, "Minecraft offers three adapters. Each coglet uses more than one of them, and each adapter can serve more than one coglet.")
    svg.text(30, 30, "COGLETS", "dg-cap", "start")
    svg.text(470, 30, "ADAPTERS", "dg-cap", "start")
    svg.rect(600, 40, 330, 262, "dg-world")
    svg.text(765, 64, "Minecraft", "dg-b")
    ys = [82, 156, 230]
    coglets = [("PlayerCoglet", "plays alongside you"), ("FaceCoglet", "talks with you"), ("BuilderCoglet", "builds what you ask for")]
    adapter_rows = [("UI adapter", "the user interface"), ("Avatar adapter", "one player character"), ("Builder adapter", "the whole world")]
    for y, (name, what) in zip(ys, coglets):
        svg.box(30, y, 180, 56, name, "dg-coglet", what)
    for y, (name, scope) in zip(ys, adapter_rows):
        svg.box(470, y, 130, 56, name, "dg-adapter")
        svg.rect(616, y, 298, 56, "dg-group", 8)
        svg.text(765, y + 32, scope, "dg-t")
    # (coglet row, y offset out, adapter row, y offset in, colour)
    # (coglet row, y offset out, adapter row, y offset in, colour, x of the vertical run)
    links = [
        (0, 14, 0, 14, "teal", None), (0, 32, 1, 20, "teal", 340),
        (1, 10, 0, 46, "warm", 290), (1, 34, 1, 34, "warm", None),
        (2, 12, 1, 48, "blue", 400), (2, 38, 2, 38, "blue", None),
    ]
    for c, dy_out, a, dy_in, color, via in links:
        y1, y2 = ys[c] + dy_out, ys[a] + dy_in
        if via is None:
            svg.arrow(212, y1, 468, y2, color, both=True)
        else:  # across, up or down, across: no diagonals
            svg.path(f"M212 {y1}H{via}V{y2}H468", color, both=True)
    return svg


def runtime() -> Svg:
    svg = Svg("runtime", 1180, 392, "Two runtimes joined by a network link. Each has pluggable execution engines and capabilities it provides, such as a database, a key-value store and random numbers; it runs coglets and connects to adapters outside it. Messages pass between coglets inside a runtime and between runtimes over the network.")
    engine_cls = {"BASIC": "dg-e-basic", "WASM": "dg-e-wasm", "JavaScript": "dg-e-js", "Java": "dg-e-java", "Python": "dg-e-llm"}

    def one(x, engines_on_left, adapters, coglets, engines, caps, uses):
        w = 520
        svg.rect(x, 104, w, 266, "dg-group", 14)
        ex = x + 12 if engines_on_left else x + w - 112
        ax = x + 124 if engines_on_left else x + 12   # left edge of the remaining area, 384 wide
        # execution engines, a vertical column on the outer side, each with its own colour
        svg.rect(ex, 116, 100, 242, "dg-bg", 10)
        svg.text(ex + 50, 138, "ENGINES", "dg-cap")
        for i, name in enumerate(engines):
            svg.rect(ex + 8, 150 + i * 40, 84, 32, engine_cls[name], 6)
            svg.text(ex + 50, 170 + i * 40, name, "dg-t")
        svg.text(ax + 192, 128, "RUNTIME", "dg-cap")
        xs = [ax + i * 140 for i in range(3)]
        for i, engine in enumerate(coglets):  # a coglet takes the colour of the engine it runs on
            svg.box(xs[i], 146, 104, 56, "Coglet", engine_cls[engine], engine)
        for i, name in adapters:
            svg.box(xs[i] - 8, 28, 120, 40, name, "dg-adapter")
            svg.arrow(xs[i] + 52, 70, xs[i] + 52, 144, "warm", both=True)
        svg.arrow(xs[0] + 106, 174, xs[1] - 2, 174, "teal", both=True)
        svg.text(xs[0] + 122, 164, "local", "dg-m")
        # capabilities the runtime provides, along the bottom, with the label underneath
        svg.rect(ax, 276, 384, 82, "dg-bg", 10)
        cw = (384 - 28 - 6 * (len(caps) - 1)) / len(caps)
        spans = []
        for i, name in enumerate(caps):
            cx = ax + 14 + i * (cw + 6)
            svg.rect(cx, 288, cw, 34, "dg-runtime", 6)
            svg.text(cx + cw / 2, 309, name, "dg-t")
            spans.append((cx, cx + cw))
        svg.text(ax + 192, 345, "CAPABILITIES IT PROVIDES", "dg-cap")
        for c, k, dx in uses:  # coglet index, capability index, offset along the coglet's bottom edge
            sx, (lo, hi) = xs[c] + 52 + dx, spans[k]
            if lo + 8 <= sx <= hi - 8:  # straight down
                svg.arrow(sx, 204, sx, 286, "ink")
            else:  # down, across, down: no diagonals
                land = lo + 20 if sx < lo else hi - 20
                svg.path(f"M{sx} 204V246H{land}V286", "ink")
        return xs

    left = one(30, True, [(0, "App adapter"), (2, "Camera adapter")], ["BASIC", "Python", "WASM"], ["BASIC", "WASM", "JavaScript", "Java", "Python"], ["Database", "KV store", "LLM", "Random"], [(0, 0, -12), (0, 1, 12), (1, 1, -12), (1, 2, 12), (2, 3, 0)])
    right = one(630, False, [(0, "Game adapter")], ["Java", "JavaScript", "Python"], ["JavaScript", "Java", "Python"], ["KV store", "Random", "LLM", "GPU"], [(0, 0, 0), (1, 1, 20), (2, 2, -14), (2, 3, 10)])
    svg.arrow(left[2] + 106, 174, right[0] - 2, 174, "blue", both=True)
    svg.text((left[2] + 104 + right[0]) / 2, 164, "network", "dg-m", halo=True)
    return svg


def softmax_app() -> Svg:
    """A fixed-colour picture of the desktop, like a screenshot: it does not follow the page theme."""
    svg = Svg("softmax-app", 960, 440, "The Softmax app on a Mac: its icon in the menu bar, its menu open, a small indicator inside the Blender window showing the cogent is there too, and the floating orb at the edge of the screen.")
    font = "font-family=\"-apple-system,'SF Pro Text','Helvetica Neue',system-ui,sans-serif\""

    def t(x, y, text, size=13, weight=400, fill="#1D1D1F", anchor="start"):
        svg.add(f'<text x="{x}" y="{y}" {font} font-size="{size}" font-weight="{weight}" fill="{fill}" text-anchor="{anchor}">{escape(text)}</text>')

    def face(x, y, size, pose=""):
        k = size / 600
        svg.add(
            f'<g transform="translate({x} {y}) scale({k})"><g transform="translate(-212 -212)">'
            '<rect x="212" y="212" width="600" height="600" rx="260" fill="#87EFBC"/>'
            '<rect x="360" y="390" width="58" height="80" rx="29" fill="#101917"/><rect x="606" y="390" width="58" height="80" rx="29" fill="#101917"/>'
            f'<rect x="428" y="570" width="168" height="16" rx="8" fill="#101917"/>{pose}</g></g>'
        )

    def panel(x, y, w, h):
        svg.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" fill="#F6F6F6" stroke="#000" stroke-opacity="0.22" stroke-width="0.6"/>')

    def check(x, y, color="#1D1D1F"):
        svg.add(f'<path d="M{x} {y}l2.7 2.8 5.3-6.4" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>')

    svg.add('<rect x="0.5" y="0.5" width="959" height="439" rx="12" fill="#2F4038"/>')
    # menu bar
    svg.add('<path d="M12.5 0.5H947.5A12 12 0 0 1 959.5 12.5V26H0.5V12.5A12 12 0 0 1 12.5 0.5Z" fill="#E9EEEB"/>')
    t(18, 18, "Blender", weight=700)
    for i, item in enumerate(["File", "Edit", "Render", "Window", "Help"]):
        t(86 + i * 56 - (i > 0) * 6 * 0, 18, item)
    t(944, 18, "Mon Nov 23  9:41 AM", anchor="end")
    svg.add('<rect x="754" y="3" width="30" height="20" rx="4" fill="#000" fill-opacity="0.14"/>')
    svg.add('<g transform="translate(760 4)"><rect x="3" y="4" width="12" height="10" rx="5" fill="none" stroke="#1D1D1F" stroke-width="1.5"/><rect x="6" y="7" width="1.5" height="3" rx=".75" fill="#1D1D1F"/><rect x="10.5" y="7" width="1.5" height="3" rx=".75" fill="#1D1D1F"/></g>')
    # another app's window
    svg.add('<rect x="30" y="52" width="520" height="350" rx="9" fill="#3D3D3D" stroke="#000" stroke-opacity="0.5" stroke-width="0.6"/>')
    svg.add('<path d="M39 52H541A9 9 0 0 1 550 61V78H30V61A9 9 0 0 1 39 52Z" fill="#2B2B2B"/>')
    for i, color in enumerate(["#FF5F57", "#FEBC2E", "#28C840"]):
        svg.add(f'<circle cx="{46 + i * 18}" cy="65" r="5.5" fill="{color}"/>')
    t(290, 70, "torus.blend", weight=600, fill="#D6D6D6", anchor="middle")
    svg.add(
        '<g transform="translate(70 96) scale(0.7)">'
        '<path d="M20 330H620M60 290H580M100 258H540M134 232H506" stroke="#4E4E4E" stroke-width="1.4"/>'
        '<path d="M320 220L40 400M320 220L180 400M320 220V400M320 220L460 400M320 220L600 400" stroke="#4E4E4E" stroke-width="1.4"/>'
        '<ellipse cx="320" cy="262" rx="176" ry="84" fill="#7C8681"/><ellipse cx="320" cy="250" rx="176" ry="84" fill="#A9B2AD"/>'
        '<ellipse cx="320" cy="238" rx="74" ry="26" fill="#5C6661"/><ellipse cx="320" cy="244" rx="70" ry="22" fill="#3D3D3D"/>'
        '<ellipse cx="320" cy="250" rx="176" ry="84" fill="none" stroke="#F5A623" stroke-width="2.6"/></g>'
    )
    # the Softmax menu
    mx, my, mw = 696, 30, 250
    panel(mx, my, mw, 220)
    svg.add(f'<rect x="{mx + 12}" y="{my + 10}" width="30" height="30" rx="7" fill="#101917"/>')
    face(mx + 17, my + 15, 20)
    t(mx + 52, my + 23, "Softmax", weight=600)
    t(mx + 52, my + 38, "Ready · hold Right Option to talk", size=11, fill="#5F5F64")
    rows = [
        ("sep",), ("Open Conversation", "\u2325Space", False, True), ("Show Orb", None, True, False), ("Mute Microphone", None, False, False),
        ("sep",), ("Pause Softmax", None, False, False),
        ("sep",), ("Settings\u2026", "\u2318,", False, False), ("Quit Softmax", "\u2318Q", False, False),
    ]
    y = my + 49
    apps_y = 0
    for row in rows:
        if row[0] == "sep":
            svg.add(f'<path d="M{mx + 15} {y + 5.5}H{mx + mw - 15}" stroke="#000" stroke-opacity="0.12"/>')
            y += 11
            continue
        label, hint, ticked, hot = row
        ink = "#FFFFFF" if hot else "#1D1D1F"
        if hot:
            svg.add(f'<rect x="{mx + 5}" y="{y}" width="{mw - 10}" height="22" rx="5" fill="#0A6CFF"/>')
            apps_y = y
        if ticked:
            check(mx + 13, y + 12)
        t(mx + 29, y + 15, label, fill=ink)
        if hint == ">":
            svg.add(f'<path d="M{mx + mw - 20} {y + 6.5}l4.4 4.5-4.4 4.5" fill="none" stroke="{ink}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>')
        elif hint:
            t(mx + mw - 15, y + 15, hint, fill="#FFFFFF" if hot else "#5F5F64", anchor="end")
        y += 22
    # the cogent's indicator inside the other app's window, top right
    svg.add('<rect x="408" y="88" width="132" height="26" rx="13" fill="#101917" fill-opacity="0.9" stroke="#FFFFFF" stroke-opacity="0.18" stroke-width="0.6"/>')
    face(414, 92, 18)
    t(438, 105, "Softmax is here", size=11, weight=600, fill="#ECF5EF")
    svg.add('<circle cx="529" cy="101" r="3.2" fill="#87EFBC"/>')
    # the floating orb
    svg.add('<circle cx="908" cy="362" r="32" fill="#101917" stroke="#FFFFFF" stroke-opacity="0.16"/>')
    face(886, 340, 44)
    return svg


def speech() -> Svg:
    svg = Svg("speech", 960, 500, "Two ways to do speech. With a realtime provider, audio goes straight from the Softmax app to the provider and the Face coglet gets transcripts and tool calls. With local models, audio never leaves the Mac.")

    def lane(y, title, note):
        svg.text(30, y, title, "dg-cap", "start")
        svg.text(930, y, note, "dg-m", "end")

    # realtime provider
    lane(34, "REALTIME PROVIDER (FIRST)", "lowest latency; audio goes to the provider")
    svg.rect(20, 48, 420, 186, "dg-group", 12)
    svg.text(34, 222, "THIS MAC", "dg-cap", "start")
    svg.box(34, 66, 130, 56, "Mic, speaker", "dg-world")
    svg.box(224, 66, 200, 56, "Softmax app", "dg-adapter", "audio, push-to-talk, barge-in")
    svg.box(224, 152, 200, 56, "Face coglet", "dg-coglet", "decides what gets done")
    svg.box(720, 66, 210, 56, "Realtime speech model", "dg-hub", "hears, speaks, calls tools")
    svg.box(720, 152, 210, 56, "Softmax key service", "dg-runtime", "holds the real API key")
    svg.arrow(166, 94, 222, 94, "ink", both=True)
    svg.arrow(426, 84, 718, 84, "blue", both=True)
    svg.text(572, 76, "audio, direct", "dg-code")
    svg.path("M718 180H572V108H428", "ink", dashed=True)
    svg.text(646, 172, "short-lived key", "dg-code")
    svg.arrow(294, 124, 294, 150, "teal")
    svg.arrow(354, 150, 354, 124, "teal")
    svg.text(214, 142, "transcripts, tool calls", "dg-m", "end")
    svg.text(434, 142, "instructions, results", "dg-m", "start")

    # on-machine models
    lane(288, "ON THIS MAC (LATER)", "private and offline; audio never leaves the machine")
    svg.rect(20, 302, 720, 186, "dg-group", 12)
    svg.text(34, 476, "THIS MAC", "dg-cap", "start")
    svg.box(34, 320, 130, 56, "Mic, speaker", "dg-world")
    svg.box(224, 320, 200, 56, "Softmax app", "dg-adapter", "audio, push-to-talk, barge-in")
    svg.box(224, 406, 200, 56, "Face coglet", "dg-coglet", "decides what to say and do")
    svg.box(510, 320, 210, 56, "Local speech models", "dg-runtime", "speech-to-text, text-to-speech")
    svg.box(510, 406, 210, 56, "LLM capability", "dg-runtime", "local or remote")
    svg.arrow(166, 348, 222, 348, "ink", both=True)
    svg.arrow(426, 348, 508, 348, "ink", both=True)
    svg.arrow(294, 378, 294, 404, "teal")
    svg.arrow(354, 404, 354, 378, "teal")
    svg.text(214, 396, "transcript", "dg-m", "end")
    svg.text(434, 396, "text to say", "dg-m", "start")
    svg.arrow(426, 434, 508, 434, "teal", both=True)
    return svg


def unforged_stack() -> Svg:
    svg = Svg("unforged-stack", 960, 470, "Every runtime keeps its coglets as git refs. Devices run steps in process; the router is an unforged daemon holding the cogent's refs; a Softmax-hosted runtime runs steps the same way, and can hand any step to unforged workers on CAOS.")
    # device
    svg.rect(24, 24, 300, 300, "dg-group", 12)
    svg.text(40, 48, "DEVICE", "dg-cap", "start")
    for i, name in enumerate(["Mic, speaker", "Blender"]):
        svg.box(40 + i * 140, 62, 128, 36, name, "dg-adapter")
        svg.arrow(104 + i * 140, 100, 104 + i * 140, 128, "warm", both=True)
    svg.rect(40, 130, 268, 178, "dg-runtime")
    svg.text(54, 151, "cogletd", "dg-code", "start")
    svg.text(294, 151, "hot path", "dg-m", "end")
    svg.box(54, 162, 116, 40, "Face", "dg-coglet")
    svg.box(178, 162, 116, 40, "Blender", "dg-coglet")
    svg.box(54, 212, 240, 34, "WAMR engine", "dg-e-wasm")
    svg.box(54, 254, 240, 42, "object store, in process", "dg-runtime", "git trees and commits, no git binary")
    # router
    svg.rect(352, 24, 256, 300, "dg-group", 12)
    svg.text(368, 48, "ROUTER", "dg-cap", "start")
    svg.rect(368, 62, 224, 246, "dg-hub")
    svg.text(480, 86, "unforged", "dg-code")
    svg.text(480, 102, "the cogent's repository", "dg-m")
    rows = [("refs/coglets/*", "every coglet's state"), ("refs/published/*", "promoted policies"), ("allowed_signers", "who may do what"), ("iroh ticket", "reach it from anywhere")]
    for i, (code, what) in enumerate(rows):
        y = 112 + i * 40
        svg.rect(380, y, 200, 36, "dg-runtime", 6)
        svg.text(392, y + 15, code, "dg-code", "start")
        svg.text(392, y + 29, what, "dg-m", "start")
    svg.box(380, 274, 200, 30, "Router coglet", "dg-coglet")
    # a Softmax-hosted runtime
    svg.rect(636, 24, 300, 300, "dg-group", 12)
    svg.text(652, 48, "SOFTMAX-HOSTED RUNTIME", "dg-cap", "start")
    svg.rect(652, 62, 268, 142, "dg-runtime")
    svg.text(666, 83, "cogletd", "dg-code", "start")
    for i, name in enumerate(["FastLoop", "Factory", "Optimizer", "Memory"]):
        svg.box(666 + (i % 2) * 124, 94 + (i // 2) * 52, 116, 42, name, "dg-coglet")
    svg.box(652, 236, 268, 72, "unforged workers on CAOS", "dg-hub", "river exec: replays, forks, tests")
    svg.arrow(786, 206, 786, 234, "teal", both=True)
    svg.text(796, 224, "a step as a job", "dg-m", "start")
    # links
    svg.arrow(310, 200, 366, 200, "blue", both=True)
    svg.arrow(594, 140, 650, 140, "blue", both=True)
    svg.arrow(310, 250, 366, 250, "teal", dashed=True, both=True)
    svg.arrow(594, 270, 650, 270, "teal", dashed=True, both=True)
    svg.text(480, 372, "Every box speaks the same objects: a coglet is a ref, a step is a commit, a patch is a merge.", "dg-t")
    svg.text(480, 394, "Only the transport differs: messages ride QUIC streams, state and patches ride git fetch and push over iroh.", "dg-m")
    legend(svg, 190, 440, [("blue", False, "messages, health"), ("teal", True, "git fetch / push of refs"), ("warm", False, "adapter events, actions")])
    return svg


def unforged_tree() -> Svg:
    svg = Svg("unforged-tree", 960, 400, "A coglet is a ref. It points at the commit of its latest step; that commit's tree holds the manifest, code, data, logs and the effects the step staged.")
    svg.text(30, 40, "REFS, PER COGLET", "dg-cap", "start")
    refs = [("…/state", "latest step"), ("…/policy", "candidate code"), ("…/published", "promoted code"), ("…/inputs", "recorded inputs")]
    for i, (name, what) in enumerate(refs):
        y = 56 + i * 44
        svg.rect(30, y, 210, 36, "dg-hub", 6)
        svg.text(42, y + 22, name, "dg-code", "start")
        svg.text(230, y + 22, what, "dg-m", "end")
    svg.text(30, 250, "refs/coglets/<id>/…", "dg-code", "start")
    svg.text(30, 268, "one namespace per runtime:", "dg-m", "start")
    svg.text(30, 284, "refs/namespaces/<node>/…", "dg-code", "start")
    # commit chain
    svg.text(300, 40, "STEP COMMITS", "dg-cap", "start")
    for i, (n, cls) in enumerate([("step 39", "dg-runtime"), ("step 40", "dg-runtime"), ("step 41", "dg-coglet")]):
        y = 56 + i * 70
        svg.box(300, y, 190, 52, n, cls, "parents: step n−1, input n")
        if i:
            svg.arrow(395, y, 395, y - 16, "ink")
    svg.arrow(242, 74, 298, 196, "blue")
    svg.text(300, 290, "trailers on every step commit:", "dg-m", "start")
    for i, line in enumerate(["Coglet-Step: 41", "Engine: <resolver oid>", "Status: committed"]):
        svg.text(300, 308 + i * 17, line, "dg-code", "start")
    # tree
    svg.arrow(492, 222, 548, 150, "ink")
    svg.rect(550, 30, 380, 340, "dg-runtime")
    svg.text(566, 54, "TREE OF STEP 41", "dg-cap", "start")
    rows = [
        ("manifest.cbor", "what it subscribes to, needs, owns"),
        ("code/bin/step.wasm", "the policy's code"),
        ("code/prompts/  code/skills/", "the policy's words"),
        ("data/…", "its memory, written by the guest"),
        ("data/inbox.jsonl", "append-only, written by the runtime"),
        ("logs/events.jsonl", "append-only"),
        ("out/actions/<id>", "effects staged, model calls too"),
        ("out/timers/<id>", "sent only after the ref moves"),
    ]
    for i, (path, what) in enumerate(rows):
        y = 68 + i * 36
        svg.rect(566, y, 348, 30, "dg-coglet" if path.startswith(("code", "manifest")) else ("dg-adapter" if path.startswith("out") else "dg-bg"), 6)
        svg.text(578, y + 19, path, "dg-code", "start")
        svg.text(904, y + 19, what, "dg-m", "end")
    return svg


def unforged_step() -> Svg:
    svg = Svg("unforged-step", 960, 380, "A step is a merge: the coglet's state (ours) and one input (theirs) go through a resolver that pins the engine, and the result is the next state commit. The same three ids key a cache.")
    svg.text(30, 40, "ONE STEP, AS A MERGE", "dg-cap", "start")
    svg.box(30, 64, 220, 64, "state @ step n", "dg-coglet", "ours: code, data, logs")
    svg.box(30, 176, 220, 76, "input n", "dg-runtime", "theirs: events, time, seed,")
    svg.text(140, 240, "grants, boundary, model replies", "dg-m")
    svg.box(330, 112, 220, 84, "resolver", "dg-hub", "pins the engine build by id")
    svg.text(440, 186, "coglet-engine-wamr @ <oid>", "dg-code")
    svg.path("M252 96H290V140H328", "teal")
    svg.path("M252 214H290V170H328", "ink")
    svg.box(630, 112, 300, 84, "state @ step n+1", "dg-coglet", "parents: state n, input n")
    svg.arrow(552, 154, 628, 154, "teal")
    svg.text(590, 146, "handle()", "dg-code")
    # memo
    svg.rect(330, 236, 220, 64, "dg-group", 8)
    svg.text(440, 260, "cache key", "dg-b")
    svg.text(440, 280, "(resolver, state tree, input tree)", "dg-code")
    svg.path("M440 234V200", "teal", dashed=True)
    # effects
    svg.box(700, 250, 160, 44, "Adapter, LLM, router", "dg-adapter")
    svg.arrow(780, 198, 780, 248, "warm")
    svg.text(790, 226, "out/ is sent after the ref CAS", "dg-m", "start")
    svg.text(30, 340, "A nonzero return keeps nothing: the new commit carries the old tree and an empty out/.", "dg-m", "start")
    svg.text(30, 358, "The cache is only used when the resolver says the engine is qualified as deterministic.", "dg-m", "start")
    return svg


def unforged_patch() -> Svg:
    lifelines = [
        ("Factory", "dg-coglet"),
        ("Router (unforged)", "dg-hub"),
        ("Runtime", "dg-runtime"),
        ("BlenderCoglet", "dg-coglet"),
    ]
    messages = [
        (0, 0, "test patch: river exec over recorded inputs", "ink", False),
        (0, 1, "push …/policy, signed", "blue", False),
        (1, 1, "check signer, CAS …/policy", "ink", False),
        (1, 2, "fetch …/policy over iroh", "teal", True),
        ("note", "between two steps, never during one"),
        (2, 2, "merge policy into state", "ink", False),
        (2, 3, "next handle() runs new code", "teal", False),
        (2, 1, "push …/state, receipt", "teal", True),
        (1, 0, "ticket.result applied", "blue", False),
    ]
    return sequence(
        "unforged-patch",
        "The factory tests a patch against recorded inputs, then pushes it as a signed ref. The router checks the signer and moves the ref; the runtime fetches it and merges it into the coglet's state between steps.",
        lifelines,
        messages,
        spacing=230,
        legend_items=[("blue", False, "signed ref update"), ("teal", True, "git over iroh"), ("ink", False, "local work")],
    )


def unforged_replay() -> Svg:
    svg = Svg("unforged-replay", 960, 380, "One journal, two runners. A device produces step commits in process; a Softmax-hosted runtime re-runs any of them as unforged jobs to replay, fork or test a candidate policy.")
    svg.text(30, 40, "ONE JOURNAL, TWO RUNNERS", "dg-cap", "start")
    svg.box(30, 70, 230, 70, "cogletd on a device", "dg-runtime", "in process, no fork, no git binary")
    svg.text(145, 160, "writes step commits", "dg-m")
    # journal
    for i in range(5):
        x = 300 + i * 74
        svg.box(x, 82, 60, 44, f"s{37 + i}", "dg-coglet")
        if i:
            svg.arrow(x - 2, 104, x - 12, 104, "ink")
    svg.arrow(262, 104, 298, 104, "teal")
    svg.text(475, 60, "refs/coglets/<id>/state, pushed to the router", "dg-m")
    svg.box(700, 70, 230, 70, "unforged on CAOS", "dg-hub", "a job per step, cached")
    svg.arrow(672, 104, 698, 104, "teal", dashed=True)
    uses = [
        ("Replay", "same inputs, cache off: tree ids must match"),
        ("Fork", "same state, new input: a new branch"),
        ("Gate", "candidate policy over recorded inputs"),
    ]
    for i, (name, what) in enumerate(uses):
        y = 190 + i * 52
        svg.rect(300, y, 630, 42, "dg-group", 8)
        svg.text(316, y + 26, name, "dg-b", "start")
        svg.text(380, y + 26, what, "dg-m", "start")
    svg.path("M815 142V172H615V188", "blue")
    svg.text(30, 220, "Each job leaves a receipt:", "dg-m", "start")
    for i, line in enumerate(["runner, worker image,", "executed or cached,", "which cache served it"]):
        svg.text(30, 240 + i * 17, line, "dg-code", "start")
    return svg


def secrets_lease() -> Svg:
    svg = Svg("secrets-lease", 960, 450, "The vault is two things: a policy repository in unforged that says which adapters may use which secret, and a key store that holds the bytes. An adapter leases a value for minutes; a coglet only ever names the secret's id.")
    # policy repo
    svg.rect(24, 24, 300, 250, "dg-group", 12)
    svg.text(40, 48, "POLICY, IN UNFORGED", "dg-cap", "start")
    svg.rect(40, 62, 268, 196, "dg-hub")
    svg.text(54, 84, "refs/vault/policy", "dg-code", "start")
    for i, (code, what) in enumerate([("github-main", "owner, kind, store path"), ("may lease", "mcp.github, ci"), ("state", "active | rotating | revoked")]):
        y = 96 + i * 42
        svg.rect(54, y, 240, 36, "dg-runtime", 6)
        svg.text(66, y + 15, code, "dg-code", "start")
        svg.text(66, y + 29, what, "dg-m", "start")
    svg.text(174, 246, "signed commits, no secret bytes", "dg-m")
    # key store
    svg.rect(24, 296, 300, 100, "dg-group", 12)
    svg.text(40, 320, "BYTES, IN A KEY STORE", "dg-cap", "start")
    svg.box(40, 332, 268, 50, "cloud KMS, Vault, OS keychain", "dg-e-wasm")
    # lease service
    svg.box(380, 150, 200, 80, "lease service", "dg-runtime", "checks policy tip, issues lease")
    svg.arrow(310, 160, 378, 176, "ink")
    svg.text(344, 154, "reads", "dg-m")
    svg.arrow(310, 356, 378, 216, "ink")
    svg.text(330, 300, "fetches", "dg-m", "start")
    # adapter
    svg.box(640, 150, 140, 80, "MCP adapter", "dg-adapter", "value in memory")
    svg.arrow(582, 190, 638, 190, "warm")
    svg.text(610, 182, "lease", "dg-m")
    svg.box(820, 150, 120, 80, "GitHub", "dg-bg", "the real API")
    svg.arrow(782, 190, 818, 190, "warm", both=True)
    # coglet
    svg.box(640, 300, 140, 64, "Coglet", "dg-coglet", "names github-main")
    svg.arrow(710, 298, 710, 234, "teal", dashed=True)
    svg.text(720, 270, "action with a secret id", "dg-m", "start")
    # audit
    svg.text(480, 270, "each lease appends to", "dg-m")
    svg.text(480, 288, "refs/vault/audit", "dg-code")
    svg.text(480, 306, "(who, which secret, until when)", "dg-m")
    svg.text(480, 428, "Rotate: put a new value in the store; new leases get it. Revoke: commit state revoked; the next lease fails.", "dg-m")
    return svg


def adapter_call() -> Svg:
    lifelines = [
        ("Coglet", "dg-coglet"),
        ("Runtime", "dg-runtime"),
        ("Dispatcher", "dg-runtime"),
        ("MCP adapter", "dg-adapter"),
    ]
    messages = [
        (0, 1, "write /net/github/create_issue/call", "ink", False),
        (1, 1, "stage out/actions/42, commit step", "ink", False),
        (1, 2, "state ref moved", "teal", True),
        (2, 2, "check grant, note 42 as sent", "ink", False),
        (2, 3, "mcp.github.create_issue", "warm", False),
        ("note", "the adapter uses its leased token; the coglet never sees it"),
        (3, 2, "outcome: issue #7 (scrubbed)", "warm", False),
        (2, 1, "append to next input commit", "teal", True),
        (1, 0, "next handle() reads the outcome", "blue", False),
    ]
    return sequence(
        "adapter-call",
        "Calling an adapter from a coglet. The step only stages a file; after the step commits, the dispatcher sends it, and the outcome comes back through the next input commit, so replays read it instead of calling again.",
        lifelines,
        messages,
        spacing=230,
        legend_items=[("ink", False, "local work"), ("teal", True, "through git"), ("warm", False, "live call"), ("blue", False, "event")],
    )


def llm_loop() -> Svg:
    lifelines = [
        ("Coglet", "dg-coglet"),
        ("Dispatcher", "dg-runtime"),
        ("LLM adapter", "dg-adapter"),
        ("Tool adapter", "dg-adapter"),
    ]
    messages = [
        (0, 0, "step 1: read data/chat, stage llm call", "ink", False),
        (0, 1, "out/actions/a1", "teal", True),
        (1, 2, "llm.claude.complete", "warm", False),
        (2, 0, "partial tokens, then final reply", "blue", False),
        (0, 0, "step 2: reply asks for a tool, stage it", "ink", False),
        (0, 1, "out/actions/a2", "teal", True),
        (1, 3, "mcp.github.search", "warm", False),
        (3, 0, "outcome", "blue", False),
        (0, 0, "step 3: append result, stage next llm call", "ink", False),
    ]
    return sequence(
        "llm-loop",
        "An agent loop unrolled into steps. Each step reads the conversation from its own files, stages one call and stops; each reply wakes the next step. Every turn is a commit, so a run can be replayed or forked at any turn.",
        lifelines,
        messages,
        spacing=230,
        legend_items=[("ink", False, "inside a step"), ("teal", True, "staged, sent after commit"), ("warm", False, "live call"), ("blue", False, "event into next input")],
    )


def runtime_split() -> Svg:
    svg = Svg("runtime-split", 960, 430, "What stays in cogletd and what moves to unforged. The runtime keeps what must be fast, local or metered; everything durable moves into git.")
    svg.rect(24, 24, 440, 380, "dg-group", 12)
    svg.text(40, 48, "STAYS IN COGLETD", "dg-cap", "start")
    stays = [
        ("Wasm engine", "WAMR, compute and deadline metering"),
        ("Step loop", "event batching, budgets, all-or-nothing"),
        ("Grant checks", "on every staged action"),
        ("Dispatcher", "live adapter connections, sends out/"),
        ("Timers", "firing needs a clock"),
        ("Speech and barge-in", "anything with a latency budget"),
    ]
    for i, (name, what) in enumerate(stays):
        y = 62 + i * 54
        svg.rect(40, y, 408, 44, "dg-runtime", 8)
        svg.text(56, y + 19, name, "dg-b", "start")
        svg.text(56, y + 36, what, "dg-m", "start")
    svg.rect(496, 24, 440, 380, "dg-group", 12)
    svg.text(512, 48, "MOVES TO UNFORGED", "dg-cap", "start")
    moves = [
        ("Snapshot, journal, WAL", "commits on the state ref"),
        ("Crash recovery", "read the ref: it is the last committed step"),
        ("Code updates, install, deploy", "merges and ref moves"),
        ("Inbox, logs, effect ledger", "input commits, files, a git note"),
        ("Manage reads and writes", "git reads, boundary commits"),
        ("Replays, forks, gates", "river exec jobs, cached"),
    ]
    for i, (name, what) in enumerate(moves):
        y = 62 + i * 54
        svg.rect(512, y, 408, 44, "dg-hub", 8)
        svg.text(528, y + 19, name, "dg-b", "start")
        svg.text(528, y + 36, what, "dg-m", "start")
    return svg


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for stale in OUT.glob("*.svg"):
        stale.unlink()
    for svg in (adapters(), coglet(), runtime(), softmax_app(), speech(), step(), service_call(), cogent(), deployment(), calls(), turn(),
                unforged_stack(), unforged_tree(), unforged_step(), unforged_patch(), unforged_replay(),
                runtime_split(), secrets_lease(), adapter_call(), llm_loop()):
        path = OUT / f"{svg.name}.svg"
        path.write_text(svg.render(), encoding="utf-8")
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
