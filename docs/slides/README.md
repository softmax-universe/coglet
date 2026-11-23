# Coglet architecture slides

- `index.html`: HTML deck (24 slides). Open directly or via GitHub Pages. Keys: ← → / space, `f` fullscreen, `#N` deep-links a slide.
- `coglet-architecture.pdf` / `coglet-architecture.pptx`: offline exports (each diagram at its final step; PPTX slides are images with clickable spec links).
- Diagrams and spec links are shared with the architecture page in `../site/assets/` (`diagrams.js`, `specs.js`).

Spec link status lives in `docs/site/assets/specs.js`; when a spec lands, flip its `status` to `live` there.
Regenerate the exports with headless Chrome by loading `index.html?export` (static, all slides stacked) and printing at 1920×1080.
