/* Coglet architecture diagrams.
 * Shared by docs/site/index.html and docs/slides/index.html.
 * Every diagram mounts into <figure class="dg" data-diagram="NAME">. Stepped diagrams get
 * prev / play / next controls, keyboard (← → space) and a caption per step.
 * Static mode (print / PDF export, or ?static) renders each diagram at its final step.
 * Content follows docs/design/coglet-runtime.md and docs/plan.md; spec names come from there.
 */
(function () {
  'use strict';
  var NS = 'http://www.w3.org/2000/svg';
  var REDUCED = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  var STATIC = /[?&]static\b/.test(location.search) || document.documentElement.classList.contains('static');

  // ---------- tiny SVG builder ----------
  function S(tag, attrs, kids) {
    var e = document.createElementNS(NS, tag);
    if (attrs) for (var k in attrs) if (attrs[k] !== undefined && attrs[k] !== null) e.setAttribute(k, attrs[k]);
    (kids || []).forEach(function (c) {
      if (c == null) return;
      e.appendChild(typeof c === 'string' ? document.createTextNode(c) : c);
    });
    return e;
  }
  function svgRoot(w, h, label) {
    return S('svg', { viewBox: '0 0 ' + w + ' ' + h, role: 'img', 'aria-label': label, class: 'dg-svg' });
  }
  function T(x, y, str, cls, extra) {
    var a = { x: x, y: y, class: cls || 't' };
    if (extra) for (var k in extra) a[k] = extra[k];
    return S('text', a, [str]);
  }
  // multi-line text
  function TL(x, y, lines, cls, lh, extra) {
    var g = S('g');
    lines.forEach(function (l, i) { g.appendChild(T(x, y + i * (lh || 16), l, cls, extra)); });
    return g;
  }
  function box(x, y, w, h, cls, rx) {
    return S('rect', { x: x, y: y, width: w, height: h, rx: rx == null ? 6 : rx, class: cls || 'bx' });
  }
  function line(x1, y1, x2, y2, cls, marker) {
    return S('line', { x1: x1, y1: y1, x2: x2, y2: y2, class: cls || 'ln', 'marker-end': marker ? 'url(#' + marker + ')' : null });
  }
  function path(d, cls, marker) {
    return S('path', { d: d, class: cls || 'ln', fill: 'none', 'marker-end': marker ? 'url(#' + marker + ')' : null });
  }
  var uid = 0;
  function defs(svg) {
    // unique marker ids per svg (a diagram can render twice: page + slides, or hidden copies)
    var id = 'm' + (++uid);
    var d = S('defs');
    ['ink', 'cog', 'srf', 'hub', 'bad', 'ok', 'dim'].forEach(function (c) {
      d.appendChild(S('marker', { id: id + c, viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: 'auto-start-reverse' },
        [S('path', { d: 'M0,0 L10,5 L0,10 z', class: 'mk mk-' + c })]));
    });
    svg.appendChild(d);
    return function (c) { return id + c; };
  }
  function g(attrs, kids) { return S('g', attrs, kids); }
  function step(el, s, e) { el.setAttribute('data-s', s); if (e != null) el.setAttribute('data-e', e); return el; }

  // ---------- stepper ----------
  function stepper(fig, svg, steps, opts) {
    opts = opts || {};
    var cur = -1, timer = null, playing = false, anims = [];
    var cap = fig.querySelector('.dg-cap');
    // a stepper can be rebuilt (variant toggles): drop the previous controls, key handler and observer
    if (fig._dg) fig._dg.stop();
    var old = fig.querySelector('.dg-ctl'); if (old) old.remove();
    if (fig._keyh) fig.removeEventListener('keydown', fig._keyh);
    if (fig._io) fig._io.disconnect();
    var ctr = null;
    if (!cap) { cap = document.createElement('figcaption'); cap.className = 'dg-cap'; fig.appendChild(cap); }
    {
      ctr = document.createElement('div'); ctr.className = 'dg-ctl';
      ctr.innerHTML =
        '<button type="button" data-a="prev" aria-label="Previous step" title="Previous (←)">‹</button>' +
        '<button type="button" data-a="play" aria-label="Play or pause" title="Play / pause (space)">▶</button>' +
        '<button type="button" data-a="next" aria-label="Next step" title="Next (→)">›</button>' +
        '<span class="dg-n"></span><span class="dg-dots"></span>';
      fig.insertBefore(ctr, cap);
    }
    var nEl = ctr.querySelector('.dg-n'), dots = ctr.querySelector('.dg-dots'), playBtn = ctr.querySelector('[data-a=play]');
    dots.innerHTML = '';
    steps.forEach(function (_, i) {
      var b = document.createElement('button'); b.type = 'button'; b.className = 'dot'; b.setAttribute('aria-label', 'Step ' + (i + 1));
      b.addEventListener('click', function () { stop(); go(i, true); });
      dots.appendChild(b);
    });
    function apply(n) {
      svg.querySelectorAll('[data-s]').forEach(function (el) {
        var s = +el.getAttribute('data-s'), e = el.hasAttribute('data-e') ? +el.getAttribute('data-e') : Infinity;
        el.classList.toggle('off', !(n >= s && n < e));
      });
      svg.querySelectorAll('[data-hl]').forEach(function (el) {
        var on = el.getAttribute('data-hl').split(',').map(Number).indexOf(n) >= 0;
        el.classList.toggle('hl', on);
      });
      dots.querySelectorAll('.dot').forEach(function (d, i) { d.classList.toggle('on', i === n); d.classList.toggle('past', i < n); });
      nEl.textContent = (n + 1) + ' / ' + steps.length;
      cap.innerHTML = steps[n].cap;
      if (window.COGLET && window.COGLET.decorateSpecs) window.COGLET.decorateSpecs(cap);
    }
    function clearAnims() { anims.forEach(function (a) { a.cancel = true; }); anims = []; svg.querySelectorAll('.pkt').forEach(function (p) { p.remove(); }); }
    function packet(pts, cls, dur) {
      var c = S('circle', { r: 6, class: 'pkt ' + (cls || ''), cx: pts[0][0], cy: pts[0][1] });
      svg.appendChild(c);
      var segs = [], total = 0;
      for (var i = 1; i < pts.length; i++) { var L = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]); segs.push(L); total += L; }
      var a = { cancel: false }, t0 = null; dur = dur || 900;
      anims.push(a);
      function f(ts) {
        if (a.cancel) return;
        if (t0 == null) t0 = ts;
        var p = Math.min(1, (ts - t0) / dur), e = p < .5 ? 2 * p * p : 1 - Math.pow(-2 * p + 2, 2) / 2, d = e * total, i = 0;
        while (i < segs.length - 1 && d > segs[i]) { d -= segs[i]; i++; }
        var r = segs[i] ? d / segs[i] : 1;
        c.setAttribute('cx', pts[i][0] + (pts[i + 1][0] - pts[i][0]) * r);
        c.setAttribute('cy', pts[i][1] + (pts[i + 1][1] - pts[i][1]) * r);
        if (p < 1) requestAnimationFrame(f); else setTimeout(function () { c.classList.add('fade'); setTimeout(function () { c.remove(); }, 400); }, 250);
      }
      requestAnimationFrame(f);
    }
    function go(n, animate) {
      n = Math.max(0, Math.min(steps.length - 1, n));
      var prev = cur; cur = n;
      clearAnims();
      apply(n);
      var st = steps[n];
      if (opts.onStep) opts.onStep(n, prev, animate && !REDUCED && !STATIC);
      if (animate && !REDUCED && !STATIC && st.pk) (Array.isArray(st.pk[0][0]) ? st.pk : [st.pk]).forEach(function (pk, i) {
        setTimeout(function () { packet(pk, st.pkc, st.pkd); }, i * 160);
      });
    }
    function tick() {
      if (cur >= steps.length - 1) { if (opts.loop === false) { stop(); return; } go(0, true); }
      else go(cur + 1, true);
      timer = setTimeout(tick, (steps[cur].dwell || opts.dwell || 2300));
    }
    function play() { if (playing) return; playing = true; playBtn.textContent = '❚❚'; fig.classList.add('playing'); timer = setTimeout(tick, cur < 0 ? 0 : 900); }
    function stop() { playing = false; clearTimeout(timer); playBtn.textContent = '▶'; fig.classList.remove('playing'); }
    ctr.addEventListener('click', function (ev) {
      var a = ev.target.closest('button'); if (!a || !a.dataset.a) return;
      if (a.dataset.a === 'prev') { stop(); go(cur - 1, true); }
      if (a.dataset.a === 'next') { stop(); go(cur + 1, true); }
      if (a.dataset.a === 'play') { playing ? stop() : play(); }
    });
    fig.tabIndex = 0;
    fig.addEventListener('keydown', fig._keyh = function (ev) {
      if (ev.target !== fig) return;
      if (ev.key === 'ArrowRight') { stop(); go(cur + 1, true); ev.preventDefault(); ev.stopPropagation(); }
      else if (ev.key === 'ArrowLeft') { stop(); go(cur - 1, true); ev.preventDefault(); ev.stopPropagation(); }
      else if (ev.key === ' ') { playing ? stop() : play(); ev.preventDefault(); ev.stopPropagation(); }
    });
    var api = { go: go, play: play, stop: stop, get cur() { return cur; }, steps: steps, fig: fig };
    fig._dg = api;
    if (STATIC) { go(steps.length - 1, false); ctr.style.display = 'none'; return api; }
    go(0, false);
    if (!REDUCED && opts.autoplay !== false && 'IntersectionObserver' in window) {
      var io = fig._io = new IntersectionObserver(function (es) {
        es.forEach(function (e) {
          if (fig.closest('.slide') && !fig.closest('.slide.active')) { stop(); return; }
          if (e.isIntersecting && e.intersectionRatio > .45) { if (!fig.dataset.userStopped) play(); } else stop();
        });
      }, { threshold: [0, .45, .8] });
      io.observe(fig);
      ctr.addEventListener('click', function () { fig.dataset.userStopped = playing ? '' : '1'; });
    }
    return api;
  }

  function variantBar(fig, name, options, onPick) {
    var bar = document.createElement('div'); bar.className = 'dg-var'; bar.setAttribute('role', 'group'); bar.setAttribute('aria-label', name);
    bar.innerHTML = '<span class="lbl">' + name + '</span>' + options.map(function (o, i) {
      return '<button type="button" data-v="' + o.v + '" aria-pressed="' + (i === 0) + '">' + o.label + '</button>';
    }).join('');
    bar.addEventListener('click', function (ev) {
      var b = ev.target.closest('button'); if (!b) return;
      bar.querySelectorAll('button').forEach(function (x) { x.setAttribute('aria-pressed', x === b); });
      onPick(b.dataset.v);
    });
    var holder = fig.querySelector('.dg-scroll') || fig.firstChild;
    fig.insertBefore(bar, holder);
    return bar;
  }

  function mount(fig, svg, minW) {
    var sc = document.createElement('div'); sc.className = 'dg-scroll';
    if (minW) svg.style.minWidth = minW + 'px';
    sc.appendChild(svg);
    fig.insertBefore(sc, fig.firstChild);
    return sc;
  }

  // =====================================================================
  // 1. ROLES — one brain, many bodies (ambient)
  // =====================================================================
  function roles(fig) {
    var W = 1000, H = 540, svg = svgRoot(W, H, 'Hub at the top connected over iroh to four surfaces, each running cogletd with coglets inside'), M = defs(svg);
    // hub
    svg.appendChild(box(300, 20, 400, 150, 'bx hub-b', 10));
    svg.appendChild(T(320, 50, 'HUB', 'lbl hub-t'));
    svg.appendChild(T(680, 50, 'runs anywhere', 'small dim', { 'text-anchor': 'end' }));
    var chips = [['kernel', 'k'], ['fast loop', 'c'], ['factory', 'c'], ['LLM gateway', 's'], ['gate', 's'], ['trace store', 's']];
    chips.forEach(function (c, i) {
      var x = 320 + (i % 3) * 124, y = 68 + Math.floor(i / 3) * 44;
      svg.appendChild(box(x, y, 114, 34, 'chip chip-' + c[1], 5));
      svg.appendChild(T(x + 57, y + 22, c[0], 'mono small', { 'text-anchor': 'middle' }));
    });
    var bodies = [
      ['Mac', 'surfaces/face-mac · Swift', 'face'],
      ['Mindustry', 'game adapter · Java (mindister)', 'avatar'],
      ['Baby Bear EO1', 'pod body (babybear)', 'display'],
      ['Toy robot', 'sim/worlds/gridbot', 'gridbot-pilot']
    ];
    bodies.forEach(function (b, i) {
      var x = 16 + i * 246, y = 300, w = 226;
      // link
      var hx = 360 + i * 93, d = 'M' + (x + w / 2) + ',' + y + ' C' + (x + w / 2) + ',' + (y - 70) + ' ' + hx + ',' + 240 + ' ' + hx + ',170';
      var p = path(d, 'ln link'); p.id = 'rl' + uid + '_' + i; svg.appendChild(p);
      if (!REDUCED && !STATIC) {
        var up = S('circle', { r: 4.5, class: 'pkt-a srf-f' }, [S('animateMotion', { dur: (2.6 + i * .37) + 's', repeatCount: 'indefinite', path: d, keyPoints: '0;1', keyTimes: '0;1', calcMode: 'linear' })]);
        var dn = S('circle', { r: 4.5, class: 'pkt-a cog-f' }, [S('animateMotion', { dur: (3.1 + i * .29) + 's', begin: (i * .5) + 's', repeatCount: 'indefinite', path: d, keyPoints: '1;0', keyTimes: '0;1', calcMode: 'linear' })]);
        svg.appendChild(up); svg.appendChild(dn);
      }
      svg.appendChild(box(x, y, w, 220, 'bx srf-b', 10));
      svg.appendChild(T(x + 14, y + 26, 'SURFACE · ' + b[0], 'lbl srf-t'));
      svg.appendChild(T(x + 14, y + 46, b[1], 'mono small dim'));
      svg.appendChild(box(x + 14, y + 62, w - 28, 142, 'bx rt-b', 8));
      svg.appendChild(T(x + 26, y + 84, 'cogletd', 'mono small'));
      svg.appendChild(T(x + w - 26, y + 84, 'runtime', 'small dim', { 'text-anchor': 'end' }));
      svg.appendChild(box(x + 28, y + 100, w - 56, 40, 'chip chip-c', 6));
      svg.appendChild(T(x + w / 2, y + 125, 'coglet: ' + b[2], 'mono small', { 'text-anchor': 'middle' }));
      svg.appendChild(box(x + 28, y + 150, w - 56, 40, 'chip chip-ghost', 6));
      svg.appendChild(T(x + w / 2, y + 175, '+ more coglets', 'small dim', { 'text-anchor': 'middle' }));
    });
    svg.appendChild(T(500, 262, 'iroh · one ALPN · coglets dial out (works behind NAT, inside the game)', 'small', { 'text-anchor': 'middle' }));
    // no direct coglet<->coglet
    svg.appendChild(path('M' + (16 + 226 - 30) + ',420 L' + (262 + 30) + ',420', 'ln bad-l dash'));
    svg.appendChild(T(252, 425, '✕', 'bad-t', { 'text-anchor': 'middle' }));
    svg.appendChild(T(500, 535, 'Coglets never talk to each other directly — all traffic goes through the hub.', 'small dim', { 'text-anchor': 'middle' }));
    mount(fig, svg, 640);
  }

  // =====================================================================
  // 2. EVENT FLOW — one wake, event → action (sequence diagram)
  // =====================================================================
  function eventflow(fig) {
    var W = 1000, H = 660, svg = svgRoot(W, H, 'Sequence diagram: an event travels from the surface through cogletd into a fresh Wasm instance and an action returns to the surface'), M = defs(svg);
    var L = { srf: 80, rt: 300, wasm: 520, fs: 720, hub: 910 };
    var heads = [['srf', 'Surface', 'face-mac', 'srf'], ['rt', 'cogletd', 'router · journal', 'rt'], ['wasm', 'bin/step.wasm', 'fresh instance', 'cog'], ['fs', 'files', '/code /data /logs', 'fs'], ['hub', 'Hub', 'LLM gateway', 'hub']];
    heads.forEach(function (h) {
      var x = L[h[0]];
      svg.appendChild(box(x - 78, 14, 156, 50, 'bx head-' + h[3], 8));
      svg.appendChild(T(x, 36, h[1], 'mono hd', { 'text-anchor': 'middle' }));
      svg.appendChild(T(x, 54, h[2], 'small dim', { 'text-anchor': 'middle' }));
      svg.appendChild(line(x, 64, x, H - 10, 'life'));
    });
    // activation bar for wasm instance (steps 4..9)
    svg.appendChild(step(box(L.wasm - 9, 226, 18, 236, 'act-bar', 2), 4));
    svg.appendChild(step(T(L.wasm + 18, 480, '✕ instance dropped', 'small cog-t'), 9));
    function msg(s, y, a, b, label, cls, dashed) {
      var x1 = L[a], x2 = L[b], dir = x2 > x1 ? 1 : -1;
      var gg = g({ 'data-hl': String(s) });
      gg.appendChild(line(x1 + dir * 10, y, x2 - dir * 12, y, 'msg ' + (cls || '') + (dashed ? ' dash' : ''), M(cls === 'srf-l' ? 'srf' : cls === 'hub-l' ? 'hub' : cls === 'cog-l' ? 'cog' : 'ink')));
      gg.appendChild(T((x1 + x2) / 2, y - 8, label, 'mono small', { 'text-anchor': 'middle' }));
      return step(gg, s);
    }
    function note(s, x, y, w, lines, cls) {
      var gg = g({ 'data-hl': String(s) });
      gg.appendChild(box(x, y, w, 12 + lines.length * 16, 'note ' + (cls || ''), 4));
      gg.appendChild(TL(x + 10, y + 18, lines, 'mono small'));
      return step(gg, s);
    }
    svg.appendChild(msg(1, 100, 'srf', 'rt', 'event  speech.end_of_utterance', 'srf-l'));
    svg.appendChild(note(2, L.rt + 14, 116, 300, ['router: match subscriptions; events that', 'arrive mid-step batch into the next (capped)']));
    svg.appendChild(note(3, L.rt - 150, 166, 136, ['journal ←', 'event batch #n']));
    svg.appendChild(msg(4, 236, 'rt', 'wasm', 'load @ pinned commit · fuel · deadline', 'cog-l'));
    svg.appendChild(msg(5, 278, 'rt', 'wasm', 'handle(events, step, logical_time, seed)', 'cog-l'));
    svg.appendChild(msg(6, 318, 'wasm', 'rt', 'host fn: read state', ''));
    svg.appendChild(msg(6, 340, 'rt', 'wasm', 'reply (pinned snapshot)', '', true));
    svg.appendChild(msg(7, 382, 'wasm', 'fs', 'write /data · append /logs', ''));
    svg.appendChild(note(7, L.fs - 60, 392, 200, ['/code read-only (pinned)', 'copy-on-write tree']));
    svg.appendChild(msg(8, 446, 'wasm', 'rt', 'emit action · set timer · LLM request', 'cog-l'));
    svg.appendChild(note(9, L.fs - 60, 466, 200, ['state = tree hash h(n)', 'snapshot is free']));
    svg.appendChild(note(10, L.rt - 150, 492, 136, ['journal ←', 'actions out']));
    svg.appendChild(msg(10, 520, 'rt', 'hub', 'LLM request → gateway (answer = later event)', 'hub-l'));
    svg.appendChild(msg(11, 568, 'rt', 'srf', 'act speech.speak_chunk', 'srf-l'));
    svg.appendChild(msg(11, 590, 'srf', 'rt', 'accepted (immediate)', '', true));
    svg.appendChild(msg(12, 636, 'srf', 'rt', 'outcome → event → next wake', 'srf-l', true));
    var ab = function (a, b, y) { return [[L[a], y], [L[b], y]]; };
    var steps = [
      { cap: 'One <b>wake</b> of a Face coglet, from event to action. A coglet runs only when there is something to handle; idle coglets cost nothing. Spec: <a data-spec="abi"></a>, <a data-spec="surface"></a>.' },
      { cap: 'The surface sends an <code>event</code> message over the <b>surface protocol</b> (local socket, or in-process when the runtime is embedded, e.g. iOS). <a data-spec="surface"></a>', pk: ab('srf', 'rt', 100), pkc: 'srf-f' },
      { cap: 'The <b>event router</b> fans the event out to every coglet subscribed to it. Events that arrive while a step runs are batched into the next one, with a cap.' },
      { cap: 'The batch is <b>journaled before the step runs</b>. Replay records event batches, not ticks. Journal format: <a data-spec="abi"></a>.' },
      { cap: 'The engine loads <code>bin/step.wasm</code> from the pinned commit into a <b>fresh instance</b>, sets the compute cap (fuel) and the deadline (interrupt), and wires the host functions. Proven on the WAMR <b>interpreter</b>; AOT not yet qualified (<a data-spec="adrCap"></a>).', pk: ab('rt', 'wasm', 236), pkc: 'cog-f' },
      { cap: 'It calls the one export: <code>handle(events, step, logical_time, seed)</code>. No wall clock and no randomness except these inputs, so the step is deterministic.', pk: ab('rt', 'wasm', 278), pkc: 'cog-f' },
      { cap: 'Reads go through host functions against <b>one pinned snapshot</b> of the surface state, granted per capability in the manifest (<a data-spec="manifest"></a>).', pk: [ab('wasm', 'rt', 318), ab('rt', 'wasm', 340)] },
      { cap: 'State lives in files, not in the instance: <code>/code</code> read-only at the pinned commit, <code>/data</code> private (incl. <code>/data/inbox.jsonl</code>), <code>/logs</code> append-only. All copy-on-write and content-addressed.', pk: ab('wasm', 'fs', 382) },
      { cap: 'Outputs are host calls: <b>emit action</b>, <b>set timer</b> (“wake me in 500 ms”), <b>LLM request</b>, <b>log</b>.', pk: ab('wasm', 'rt', 446), pkc: 'cog-f' },
      { cap: 'The instance is dropped. The coglet’s whole state is now a <b>tree hash</b>: cheap to snapshot, diffable by the hub, and maps directly onto git.' },
      { cap: 'Emitted actions are <b>journaled, then dispatched</b>. The LLM request goes to the hub’s LLM gateway; its answer arrives as an event in a later wake and is recorded for replay.', pk: ab('rt', 'hub', 520), pkc: 'hub-f' },
      { cap: '<code>act</code> gets an <b>immediate accept or reject</b> from the surface. (World acts in Mindustry instead relay through the hub’s motor queue: pacing, player-wins, fenced acts.)', pk: [ab('rt', 'srf', 568), ab('srf', 'rt', 590)], pkc: 'srf-f' },
      { cap: 'The <b>outcome arrives later as an event</b>, which wakes the coglet again. The loop is events in, actions out, files as state.', pk: ab('srf', 'rt', 636), pkc: 'srf-f' }
    ];
    mount(fig, svg, 760);
    stepper(fig, svg, steps, { dwell: 2600 });
  }

  // =====================================================================
  // 3. GIT REFS — hub / published/main / logs
  // =====================================================================
  function gitrefs(fig) {
    var W = 1000, H = 470, svg = svgRoot(W, H, 'Git refs hub, published/main and logs syncing between the hub and a device'), M = defs(svg);
    var lanes = [['refs/heads/hub', 130, 'hub'], ['refs/published/main', 235, 'srf'], ['refs/heads/logs', 340, 'dim']];
    var P = [{ x: 16, w: 440, t: 'HUB · origin', c: 'hub' }, { x: 544, w: 440, t: 'DEVICE · cogletd clone', c: 'srf' }];
    P.forEach(function (p) {
      svg.appendChild(box(p.x, 20, p.w, 400, 'bx ' + p.c + '-b', 10));
      svg.appendChild(T(p.x + 16, 48, p.t, 'lbl ' + p.c + '-t'));
      lanes.forEach(function (l) {
        svg.appendChild(line(p.x + 20, l[1], p.x + p.w - 20, l[1], 'lane'));
        svg.appendChild(T(p.x + 20, l[1] - 30, l[0], 'mono small dim'));
      });
    });
    svg.appendChild(T(500, 236, 'iroh', 'small dim', { 'text-anchor': 'middle' }));
    svg.appendChild(T(500, 252, 'git stream', 'small dim', { 'text-anchor': 'middle' }));
    function commit(x, y, label, cls, s, e, sub) {
      var gg = g({});
      gg.appendChild(S('circle', { cx: x, cy: y, r: 9, class: 'commit ' + (cls || '') }));
      gg.appendChild(T(x, y + 26, label, 'mono small', { 'text-anchor': 'middle' }));
      if (sub) gg.appendChild(T(x, y + 42, sub, 'small dim', { 'text-anchor': 'middle' }));
      if (s != null) step(gg, s, e);
      return gg;
    }
    // hub side
    svg.appendChild(line(60, 130, 140, 130, 'chain')); svg.appendChild(commit(60, 130, 'c1', 'hub-c')); svg.appendChild(commit(140, 130, 'c2', 'hub-c'));
    svg.appendChild(step(line(140, 130, 260, 130, 'chain'), 1)); svg.appendChild(commit(260, 130, 'c3', 'hub-c', 1, null, 'src/ + bin/step.wasm, signed'));
    svg.appendChild(commit(60, 235, 'p1', 'srf-c'));
    svg.appendChild(step(line(60, 235, 160, 235, 'chain'), 6)); svg.appendChild(commit(160, 235, 'p2', 'srf-c', 6));
    svg.appendChild(commit(60, 340, 'L1', 'dim-c')); svg.appendChild(line(60, 340, 140, 340, 'chain')); svg.appendChild(commit(140, 340, 'L2', 'dim-c'));
    svg.appendChild(step(line(140, 340, 220, 340, 'chain'), 5)); svg.appendChild(commit(220, 340, 'L3', 'dim-c', 5));
    // device side
    var dx = 528;
    svg.appendChild(line(dx + 60, 130, dx + 140, 130, 'chain')); svg.appendChild(commit(dx + 60, 130, 'c1', 'hub-c')); svg.appendChild(commit(dx + 140, 130, 'c2', 'hub-c'));
    svg.appendChild(step(line(dx + 140, 130, dx + 260, 130, 'chain'), 2)); svg.appendChild(commit(dx + 260, 130, 'c3', 'hub-c', 2, null, 'signature verified ✓'));
    svg.appendChild(commit(dx + 60, 235, 'p1', 'srf-c'));
    svg.appendChild(step(line(dx + 60, 235, dx + 160, 235, 'chain'), 6)); svg.appendChild(commit(dx + 160, 235, 'p2', 'srf-c', 6, null, 'coglet-signed'));
    svg.appendChild(commit(dx + 60, 340, 'L1', 'dim-c')); svg.appendChild(line(dx + 60, 340, dx + 140, 340, 'chain')); svg.appendChild(commit(dx + 140, 340, 'L2', 'dim-c'));
    svg.appendChild(step(line(dx + 140, 340, dx + 220, 340, 'chain'), 4)); svg.appendChild(commit(dx + 220, 340, 'L3', 'dim-c', 4, null, 'batch commit'));
    // /code pointer
    function ptr(x, s, e) {
      var gg = g({ class: 'ptr' });
      gg.appendChild(path('M' + x + ',96 L' + x + ',116', 'ln cog-l', M('cog')));
      gg.appendChild(box(x - 44, 70, 88, 24, 'chip chip-c', 4));
      gg.appendChild(T(x, 87, '/code →', 'mono small', { 'text-anchor': 'middle' }));
      return step(gg, s, e);
    }
    svg.appendChild(ptr(dx + 140, 0, 3));
    svg.appendChild(ptr(dx + 260, 3));
    svg.appendChild(step(g({ 'data-hl': '3' }, [box(dx + 300, 66, 140, 30, 'note', 4), T(dx + 310, 86, 'at step boundary', 'mono small')]), 3));
    // /logs jsonl on device
    svg.appendChild(step(g({ 'data-hl': '4' }, [box(dx + 262, 360, 160, 44, 'note', 4), TL(dx + 272, 378, ['/logs/*.jsonl', '~30 s or on rotation'], 'mono small')]), 4));
    // /data never synced
    svg.appendChild(step(g({ 'data-hl': '7' }, [box(dx + 20, 428, 436, 32, 'note bad-n', 4), T(dx + 32, 449, '/data stays on the device · never synced through git', 'mono small')]), 7));
    svg.appendChild(step(g({ 'data-hl': '1' }, [box(300, 84, 140, 30, 'note', 4), T(310, 104, 'factory builds', 'mono small')]), 1, 3));
    var steps = [
      { cap: 'Every coglet is a <b>git repo</b>: <code>src/</code>, <code>bin/step.wasm</code>, <code>prompts/</code>, <code>manifest</code>. Three refs, one writer each, so they never conflict. Spec: <a data-spec="hub"></a> (git control plane).' },
      { cap: 'The <b>hub builds</b> <code>src/</code> into <code>bin/step.wasm</code> and <b>commits the binary</b>, signed, to <code>refs/heads/hub</code>. The runtime never compiles; it only loads.' },
      { cap: 'The commit travels on the git stream of the one hub connection. The runtime <b>verifies the signature</b> before accepting it.', pk: [[260, 130], [500, 130], [dx + 260, 130]], pkc: 'hub-f' },
      { cap: 'Nothing changes mid-step. At the <b>next step boundary</b> the runtime fast-forwards and <code>/code</code> pins to c3. New code takes effect only here.' },
      { cap: 'Logs are JSONL appends under <code>/logs</code>, <b>batch committed</b> (~every 30 s or on rotation) to <code>refs/heads/logs</code>. Never one commit per step; real-time logs use the debug stream.' },
      { cap: 'Logs sync on their own ref, so a log commit never conflicts with a hub code push.', pk: [[dx + 220, 340], [500, 340], [220, 340]] },
      { cap: '<code>refs/published/main</code> is the coglet’s <b>signed published state</b>.', pk: [[dx + 160, 235], [500, 235], [160, 235]], pkc: 'srf-f' },
      { cap: 'Private state in <code>/data</code> is <b>never synced through git</b>. The hub can only read it through a granted debug stream, and nothing streams unless the hub asks.' }
    ];
    mount(fig, svg, 700);
    stepper(fig, svg, steps, { dwell: 2500 });
  }

  // =====================================================================
  // 4. PROMOTION LOOP — ticket → factory → propose → promote → probation → rollback
  // =====================================================================
  function promotion(fig) {
    var W = 1000, H = 470, svg = svgRoot(W, H, 'Promotion loop: fast loop files ticket, factory builds, kernel proposes, promotes, probation, then live or rollback'), M = defs(svg);
    var N = {
      fl: [30, 40, 'fast loop', 'coglet', ['files tickets', 'narrates progress', 'calls propose'], 'c'],
      fa: [30, 270, 'factory', 'coglet', ['ticket → code', 'builds Wasm', 'never promotes'], 'c'],
      ga: [290, 270, 'gate service', 'service', ['replay journals', 'headless game fork', 'human approval'], 's'],
      pr: [290, 40, 'propose', 'kernel', ['verify signature', 'gates vs policy'], 'k'],
      pm: [540, 40, 'promote', 'kernel', ['at next step boundary', 'keep last-known-good'], 'k'],
      pb: [790, 40, 'probation', 'kernel', ['health checks for', 'policy probation length'], 'k'],
      lv: [790, 270, 'live', 'kernel status', ['candidate is live', 'becomes last-known-good'], 'ok'],
      rb: [540, 270, 'rollback', 'kernel', ['revert to last-known-good', 'automatic on bad health'], 'bad']
    };
    var bw = 180, bh = 118;
    Object.keys(N).forEach(function (k) {
      var n = N[k], hl = { fl: '1,2,4,5', fa: '1,2,3,4', ga: '3', pr: '5', pm: '6', pb: '7', lv: '8', rb: '8' }[k];
      var gg = g({ 'data-hl': hl, class: 'node node-' + k });
      gg.appendChild(box(n[0], n[1], bw, bh, 'bx nd-' + n[5], 8));
      gg.appendChild(T(n[0] + 14, n[1] + 26, n[2], 'mono hd'));
      gg.appendChild(T(n[0] + bw - 12, n[1] + 26, n[3], 'small dim', { 'text-anchor': 'end' }));
      gg.appendChild(TL(n[0] + 14, n[1] + 52, n[4], 'small', 18));
      if (k === 'lv' || k === 'rb') gg.setAttribute('data-branch', k);
      svg.appendChild(gg);
    });
    function c(k, side) { var n = N[k]; return side === 'r' ? [n[0] + bw, n[1] + bh / 2] : side === 'l' ? [n[0], n[1] + bh / 2] : side === 't' ? [n[0] + bw / 2, n[1]] : [n[0] + bw / 2, n[1] + bh]; }
    function arrow(a, b, label, lx, ly, cls, s, mk, off) {
      off = off || 0;
      var gg = g({});
      var p = path('M' + (a[0] + off) + ',' + a[1] + ' L' + (b[0] + off) + ',' + b[1], 'ln ' + (cls || ''), M(mk || 'ink'));
      gg.appendChild(p);
      if (label) gg.appendChild(T(lx, ly, label, 'mono small', { 'text-anchor': 'middle' }));
      if (s != null) step(gg, s);
      return gg;
    }
    svg.appendChild(arrow([c('fl', 'b')[0] - 40, c('fl', 'b')[1] + 2], [c('fa', 't')[0] - 40, c('fa', 't')[1] - 4], 'submit ticket', 70, 216, 'cog-l', null, 'cog'));
    svg.appendChild(arrow([c('fa', 't')[0] + 40, c('fa', 't')[1] - 2], [c('fl', 'b')[0] + 40, c('fl', 'b')[1] + 4], 'update · result', 186, 216, 'dash', null));
    svg.appendChild(arrow([c('fa', 'r')[0] + 2, c('fa', 'r')[1]], [c('ga', 'l')[0] - 4, c('ga', 'l')[1]], 'run gates', 250, 322, '', null));
    svg.appendChild(arrow([c('fl', 'r')[0] + 2, c('fl', 'r')[1]], [c('pr', 'l')[0] - 4, c('pr', 'l')[1]], 'propose', 250, 92, 'hub-l', null, 'hub'));
    svg.appendChild(arrow([c('pr', 'r')[0] + 2, c('pr', 'r')[1]], [c('pm', 'l')[0] - 4, c('pm', 'l')[1]], 'ok', 510, 92, 'hub-l', null, 'hub'));
    svg.appendChild(arrow([c('pm', 'r')[0] + 2, c('pm', 'r')[1]], [c('pb', 'l')[0] - 4, c('pb', 'l')[1]], '', 0, 0, 'hub-l', null, 'hub'));
    svg.appendChild(arrow([c('pb', 'b')[0], c('pb', 'b')[1] + 2], [c('lv', 't')[0], c('lv', 't')[1] - 4], 'healthy', 905, 220, 'ok-l', null, 'ok'));
    svg.appendChild(path('M' + (N.pb[0] + 30) + ',' + (N.pb[1] + bh + 2) + ' C' + (N.pb[0] + 10) + ',220 ' + (N.rb[0] + bw + 40) + ',230 ' + (N.rb[0] + bw + 4) + ',' + (N.rb[1] + 40), 'ln bad-l', M('bad')));
    svg.appendChild(T(752, 214, 'health fails', 'mono small bad-t', { 'text-anchor': 'middle' }));
    // policy file
    var pol = g({ 'data-hl': '5' });
    pol.appendChild(box(290, 176, 180, 50, 'note', 4));
    pol.appendChild(TL(300, 196, ['policy (per coglet):', 'gates · approval · probation'], 'mono small'));
    svg.appendChild(pol);
    svg.appendChild(path('M380,176 L380,162', 'ln dim-l', M('dim')));
    svg.appendChild(T(500, 452, 'Hub coglets (fast loop, factory) get stricter defaults, e.g. human approval early on.', 'small dim', { 'text-anchor': 'middle' }));
    var common = [
      { cap: 'Code changes go through <b>kernel gates</b>, not a merge train. The <b>factory never promotes</b>; the <b>kernel</b> is the only thing that moves a live branch. Spec: <a data-spec="policy"></a>, <a data-spec="hub"></a>.' },
      { cap: 'The fast loop decides a request needs new code and <b>submits a ticket</b> (target repo + base commit, goal + acceptance criteria, context, capabilities, gate, budget). It gets an ID back immediately.', pk: [c('fl', 'b').map(function (v, i) { return i ? v + 2 : v - 40; }), [c('fa', 't')[0] - 40, c('fa', 't')[1] - 4]], pkc: 'cog-f' },
      { cap: 'The factory streams <b>updates</b> (planning, building, gate running, needs input); the fast loop narrates them to the user and relays answers.', pk: [[c('fa', 't')[0] + 40, c('fa', 't')[1] - 2], [c('fl', 'b')[0] + 40, c('fl', 'b')[1] + 4]] },
      { cap: 'Required <b>gates</b> run: replay recorded journals against the candidate, a headless game fork, human approval where policy says so.', pk: [c('fa', 'r'), c('ga', 'l')] },
      { cap: 'Result: <b>done</b> with a signed candidate commit, gate results and a spoken summary (or needs input, failed, abandoned).', pk: [[c('fa', 't')[0] + 40, c('fa', 't')[1] - 2], [c('fl', 'b')[0] + 40, c('fl', 'b')[1] + 4]], pkc: 'cog-f' },
      { cap: '<b>Propose</b>(coglet id, candidate commit, gate results). The kernel verifies the signature and checks the gates against that coglet’s <b>policy file</b>.', pk: [c('fl', 'r'), c('pr', 'l')], pkc: 'hub-f' },
      { cap: '<b>Promote</b>: move the live branch to the candidate at the next step boundary, keeping the previous commit as <b>last-known-good</b>.', pk: [c('pr', 'r'), c('pm', 'l')], pkc: 'hub-f' },
      { cap: '<b>Probation</b>: the kernel watches health for the policy’s probation length.', pk: [c('pm', 'r'), c('pb', 'l')], pkc: 'hub-f' }
    ];
    var ok = { cap: 'Healthy through probation: the candidate is <b>live</b>, visible in <b>status</b> (live commit, candidate, probation state, recent health).', pk: [c('pb', 'b'), c('lv', 't')], pkc: 'ok-f' };
    var bad = { cap: 'Health checks fail during probation: <b>automatic rollback</b> to last-known-good at the next step boundary. Rollback is also available on request.', pk: [[N.pb[0] + 30, N.pb[1] + bh], [N.pb[0] + 10, 210], [N.rb[0] + bw + 4, N.rb[1] + 40]], pkc: 'bad-f' };
    mount(fig, svg, 760);
    var api;
    function build(v) {
      svg.classList.toggle('branch-bad', v === 'bad'); svg.classList.toggle('branch-ok', v !== 'bad');
      var steps = common.concat([v === 'bad' ? bad : ok]);
      var was = api && api.fig.classList.contains('playing');
      if (api) api.stop();
      api = stepper(fig, svg, steps, { dwell: 2500 });
      if (was) api.play();
    }
    variantBar(fig, 'probation', [{ v: 'ok', label: 'healthy' }, { v: 'bad', label: 'health fails' }], function (v) { build(v); if (api) api.go(STATIC ? api.steps.length - 1 : 0, false); });
    build('ok');
  }

  // =====================================================================
  // 5. BARGE-IN TIMELINE — surface reflex then coglet decision
  // =====================================================================
  function bargein(fig) {
    var W = 1000, H = 430, svg = svgRoot(W, H, 'Barge-in timeline: the surface ducks playback within one audio frame, then the coglet decides to stop or resume'), M = defs(svg);
    var X0 = 170, X1 = 975, TMAX = 3.0;
    function X(t) { return X0 + (X1 - X0) * t / TMAX; }
    var tracks = [['mic · VAD', 70, 'srf'], ['STT partials', 120, 'srf'], ['TTS playback', 170, 'srf'], ['surface reflex', 220, 'srf'], ['coglet wakes', 280, 'cog'], ['coglet intents', 330, 'cog']];
    svg.appendChild(box(10, 44, 150, 210, 'band srf-band', 6));
    svg.appendChild(T(24, 149, 'SURFACE · real time', 'tiny srf-t', { transform: 'rotate(-90 24 149)', 'text-anchor': 'middle' }));
    svg.appendChild(box(10, 258, 150, 100, 'band cog-band', 6));
    svg.appendChild(T(24, 308, 'COGLET', 'tiny cog-t', { transform: 'rotate(-90 24 308)', 'text-anchor': 'middle' }));
    tracks.forEach(function (t) {
      svg.appendChild(T(40, t[1] + 5, t[0], 'mono small'));
      svg.appendChild(line(X0, t[1], X1, t[1], 'lane'));
    });
    for (var s = 0; s <= TMAX + 1e-9; s += 0.5) {
      svg.appendChild(line(X(s), 380, X(s), 388, 'tick'));
      svg.appendChild(T(X(s), 404, s.toFixed(1) + ' s', 'mono small dim', { 'text-anchor': 'middle' }));
    }
    svg.appendChild(T(X1, 424, 'illustrative timing · thresholds come from the Face acoustic proofs', 'small dim', { 'text-anchor': 'end' }));
    var clipId = 'bclip' + (++uid);
    var clipR = S('rect', { x: 0, y: 0, width: X(0), height: H });
    svg.appendChild(S('defs', {}, [S('clipPath', { id: clipId }, [clipR])]));
    var content = g({ 'clip-path': 'url(#' + clipId + ')' });
    svg.appendChild(content);
    var head = g({ class: 'playhead' }, [line(0, 40, 0, 380, 'ph-l'), S('circle', { cx: 0, cy: 40, r: 5, class: 'ph-c' })]);
    svg.appendChild(head);
    function bar(t0, t1, y, cls, label) {
      var gg = g({});
      gg.appendChild(S('rect', { x: X(t0), y: y - 9, width: X(t1) - X(t0), height: 18, rx: 3, class: 'tbar ' + cls }));
      if (label) gg.appendChild(T(X(t0) + 6, y + 4, label, 'mono tiny'));
      return gg;
    }
    function mark(t, y, label, cls, below) {
      var gg = g({});
      gg.appendChild(S('circle', { cx: X(t), cy: y, r: 6, class: 'tmark ' + (cls || '') }));
      if (label) gg.appendChild(T(X(t) + 9, y + (below ? 18 : -10), label, 'mono tiny'));
      return gg;
    }
    var scen = {
      stop: {
        draw: function (c) {
          c.appendChild(bar(0, 1.20, 170, 'cog-bar', 'speak_chunk “So the plan for tomorrow is…”'));
          c.appendChild(bar(1.22, 1.55, 170, 'duck-bar', 'ducked'));
          c.appendChild(bar(1.55, 2.4, 170, 'drop-bar', 'rest of reply dropped'));
          c.appendChild(bar(2.55, 3.0, 170, 'cog-bar', 'new reply'));
          c.appendChild(bar(1.20, 2.30, 70, 'srf-bar', 'user: “wait, actually make it Friday”'));
          [1.35, 1.48, 1.62, 1.85, 2.1].forEach(function (t, i) { c.appendChild(mark(t, 120, i === 0 ? 'partial' : null, 'srf-m')); });
          c.appendChild(mark(2.36, 120, 'end_of_utterance', 'srf-m', true));
          c.appendChild(mark(1.22, 220, 'duck playback (≤ 1 frame)', 'srf-m'));
          c.appendChild(mark(1.30, 280, 'speech_start + playback_position', 'cog-m'));
          c.appendChild(mark(1.50, 280, null, 'cog-m'));
          c.appendChild(mark(2.40, 280, 'end_of_utterance', 'cog-m'));
          c.appendChild(mark(1.55, 330, 'stop (drop rest)', 'cog-m'));
          c.appendChild(mark(2.52, 330, 'speak_chunk', 'cog-m', true));
        },
        steps: [
          { t: 0, cap: 'The Face coglet is mid-reply; the surface is playing its <code>speak_chunk</code>s. Audio frames arrive every ~20 ms, too fast for the step loop, so <b>real-time audio stays in the surface</b>. Spec: <a data-spec="speech"></a>.' },
          { t: 1.20, cap: 'The user starts talking. The surface’s <b>voice activity detection</b> (after echo cancellation, so the Face does not hear itself) fires speech start.' },
          { t: 1.24, cap: '<b>Layer 1, reflex:</b> the surface ducks playback immediately, inside the audio path. No coglet round trip.' },
          { t: 1.34, cap: 'The coglet <b>wakes</b> on <code>speech_start</code> with the <code>playback_position</code>, so it knows exactly how much of the reply was heard.' },
          { t: 1.58, cap: '<b>Layer 2, decision:</b> partial transcripts show a real interruption, so the coglet emits <code>stop</code> and drops the rest of the reply.' },
          { t: 2.42, cap: '<code>end_of_utterance</code> wakes the coglet with the final transcript.' },
          { t: 3.0, cap: 'It answers with new <code>speak_chunk</code>s. STT and TTS placement (on device or hub-side) is a capability setting, so we can A/B it without touching the coglet.' }
        ]
      },
      resume: {
        draw: function (c) {
          c.appendChild(bar(0, 1.20, 170, 'cog-bar', 'speak_chunk “So the plan for tomorrow is…”'));
          c.appendChild(bar(1.22, 1.62, 170, 'duck-bar', 'ducked'));
          c.appendChild(bar(1.62, 3.0, 170, 'cog-bar', 'reply resumes'));
          c.appendChild(bar(1.20, 1.45, 70, 'srf-bar', '“mm-hm”'));
          c.appendChild(mark(1.38, 120, 'partial “mm-hm”', 'srf-m'));
          c.appendChild(mark(1.50, 120, 'end_of_utterance', 'srf-m', true));
          c.appendChild(mark(1.22, 220, 'duck playback (≤ 1 frame)', 'srf-m'));
          c.appendChild(mark(1.62, 220, 'unduck', 'srf-m', true));
          c.appendChild(mark(1.30, 280, 'speech_start + playback_position', 'cog-m'));
          c.appendChild(mark(1.54, 280, null, 'cog-m'));
          c.appendChild(mark(1.58, 330, 'resume (backchannel, keep reply)', 'cog-m'));
        },
        steps: [
          { t: 0, cap: 'Same start: the coglet is mid-reply. Spec: <a data-spec="speech"></a>.' },
          { t: 1.20, cap: 'The user says “mm-hm”. VAD fires speech start.' },
          { t: 1.24, cap: '<b>Layer 1, reflex:</b> the surface ducks playback instantly, exactly as before; it cannot know yet whether this is an interruption.' },
          { t: 1.34, cap: 'The coglet wakes on <code>speech_start</code> + <code>playback_position</code>.' },
          { t: 1.60, cap: '<b>Layer 2, decision:</b> the utterance is a backchannel, so the coglet <b>resumes</b> instead of dropping the reply. The surface unducks.' },
          { t: 3.0, cap: 'The reply continues. The surface owns the reflex and the coglet owns the decision, so the same coglet runs on the Mac, the Echo Dot and the EO1.' }
        ]
      }
    };
    var curT = 0, raf = null;
    function setT(t) {
      curT = t; var x = X(t);
      clipR.setAttribute('width', x + 1);
      head.setAttribute('transform', 'translate(' + x + ',0)');
    }
    function tweenTo(t, animate) {
      cancelAnimationFrame(raf);
      if (!animate) { setT(t); return; }
      var from = curT, t0 = null, dur = Math.max(300, Math.abs(t - from) * 900);
      if (t < from) { from = 0; }
      (function f(ts) {
        if (t0 == null) t0 = ts;
        var p = Math.min(1, (ts - t0) / dur);
        setT(from + (t - from) * p);
        if (p < 1) raf = requestAnimationFrame(f);
      })(performance.now());
    }
    mount(fig, svg, 760);
    var api;
    function build(v) {
      while (content.firstChild) content.removeChild(content.firstChild);
      scen[v].draw(content);
      var steps = scen[v].steps.map(function (s) { return { cap: s.cap, t: s.t, dwell: 2600 }; });
      var was = api && api.fig.classList.contains('playing');
      if (api) api.stop();
      api = stepper(fig, svg, steps, { onStep: function (n, prev, animate) { tweenTo(steps[n].t, animate); } });
      if (STATIC) setT(TMAX);
      if (was) api.play();
    }
    variantBar(fig, 'user says', [{ v: 'stop', label: 'an interruption' }, { v: 'resume', label: 'a backchannel' }], function (v) { build(v); api.go(STATIC ? api.steps.length - 1 : 0, false); });
    build('stop');
  }

  // =====================================================================
  // 6. HUB PROTOCOL — one connection, a stream per method, per-method grants
  // =====================================================================
  function proto(fig) {
    var W = 1000, H = 470, svg = svgRoot(W, H, 'One iroh connection with one QUIC stream per operation and per-method grants'), M = defs(svg);
    svg.appendChild(box(16, 40, 230, 400, 'bx srf-b', 10));
    svg.appendChild(T(32, 68, 'DEVICE', 'lbl srf-t'));
    svg.appendChild(TL(32, 92, ['cogletd', 'identity = iroh node key'], 'mono small', 18));
    svg.appendChild(box(754, 40, 230, 400, 'bx hub-b', 10));
    svg.appendChild(T(770, 68, 'HUB KERNEL', 'lbl hub-t'));
    svg.appendChild(TL(770, 92, ['allowlist of enrolled', 'node IDs · grant table'], 'mono small', 18));
    svg.appendChild(box(246, 120, 508, 316, 'pipe', 18));
    svg.appendChild(T(500, 112, '1 connection · 1 ALPN · 1 handshake · versioned schema', 'small', { 'text-anchor': 'middle' }));
    svg.appendChild(step(path('M246,80 L750,80', 'ln srf-l', M('srf')), 1));
    svg.appendChild(step(T(500, 72, 'dial out (QUIC over iroh, NAT-friendly)', 'mono small', { 'text-anchor': 'middle' }), 1));
    var streams = [
      ['enroll', 'manifest of granted capabilities', 2, 'ok'],
      ['lifecycle', 'start · stop · status', 3, 'ok'],
      ['git', 'refs hub / logs / published/main', 3, 'ok'],
      ['events ↑ actions ↓', 'surface events, hub actions', 4, 'ok'],
      ['inbox.append', '→ /data/inbox.jsonl, between steps', 5, 'ok'],
      ['health', 'probation signals', 5, 'ok'],
      ['debug.read', 'tail logs · read /data · inspect step', 6, 'ok'],
      ['debug.write', 'queued writes to /data', 7, 'bad']
    ];
    streams.forEach(function (s, i) {
      var y = 150 + i * 36;
      var gg = g({ 'data-hl': String(s[2]) });
      gg.appendChild(line(262, y, 738, y, 'stream ' + (s[3] === 'bad' ? 'bad-l dash' : '')));
      gg.appendChild(T(272, y - 6, s[0], 'mono small'));
      gg.appendChild(T(560, y - 6, s[1], 'small dim', { 'text-anchor': 'middle' }));
      gg.appendChild(box(684, y - 20, 58, 18, 'grant grant-' + s[3], 9));
      gg.appendChild(T(713, y - 7, s[3] === 'bad' ? 'denied' : 'grant ✓', 'tiny', { 'text-anchor': 'middle' }));
      svg.appendChild(step(gg, s[2]));
    });
    var steps = [
      { cap: 'Everything between a device and the hub is <b>one iroh protocol</b>: one ALPN, one handshake, one auth path, one versioned schema. Spec: <a data-spec="hub"></a>.' },
      { cap: 'The coglet’s runtime <b>dials out</b> to the hub, so it works from inside a game and behind NAT. Identity is the <b>iroh node key</b>; the hub checks it against its allowlist.', pk: [[246, 80], [750, 80]], pkc: 'srf-f' },
      { cap: '<b>enroll</b> carries the manifest of capabilities the surface granted. The hub configures itself from it, e.g. where speech-to-text runs. Schema: <a data-spec="manifest"></a>.', pk: [[262, 150], [738, 150]] },
      { cap: 'Each operation opens <b>its own QUIC stream</b> on the same connection: lifecycle, and the <b>git control plane</b> for refs <code>hub</code>, <code>logs</code>, <code>published/main</code>.', pk: [[[262, 186], [738, 186]], [[738, 222], [262, 222]]], pkc: 'hub-f' },
      { cap: 'Surface events flow up and actions flow down. The hub routes everything; coglets never talk directly.', pk: [[[262, 258], [738, 258]], [[738, 258], [262, 258]]] },
      { cap: '<b>inbox.append</b>: the hub appends to <code>/data/inbox.jsonl</code> between steps (steering messages, LLM responses, factory notes). <b>health</b> feeds probation.', pk: [[738, 294], [262, 294]], pkc: 'hub-f' },
      { cap: 'Grants are <b>per method</b>. A ticket granted <b>debug.read</b> may tail logs, read <code>/data</code>, inspect a step, pause and single-step. Nothing streams unless the hub asks.', pk: [[262, 402], [738, 402]] },
      { cap: '<b>debug.write</b> is a separate grant; without it the stream is refused. With it, writes queue and apply only between steps, and each one lands in an audit log.' }
    ];
    mount(fig, svg, 700);
    stepper(fig, svg, steps, { dwell: 2400 });
  }

  // =====================================================================
  // 7. ENGINE — trait + implementations (static, subtle hash match)
  // =====================================================================
  function engine(fig) {
    var W = 1000, H = 360, svg = svgRoot(W, H, 'Engine trait with WAMR, Wasmtime and mock implementations; same coglet on both engines yields identical state hashes'), M = defs(svg);
    svg.appendChild(box(300, 16, 400, 50, 'bx rt-b', 8));
    svg.appendChild(T(500, 47, 'coglet-runtime  (cogletd · C ABI library)', 'mono small', { 'text-anchor': 'middle' }));
    svg.appendChild(path('M500,66 L500,92', 'ln', M('ink')));
    svg.appendChild(box(220, 96, 560, 76, 'bx trait', 8));
    svg.appendChild(T(240, 122, 'trait Engine', 'mono hd cog-t'));
    svg.appendChild(T(240, 150, 'load · set compute cap · set deadline · wire host fns · call handle', 'mono small'));
    svg.appendChild(T(760, 122, 'crates/coglet-engine', 'small dim', { 'text-anchor': 'end' }));
    var impls = [
      [40, 'coglet-engine-wamr', ['interpreter first (cap + deadline proven)', 'AOT: metering not yet qualified', 'interpreter-only on iOS, EO1'], 'c'],
      [370, 'coglet-engine-wasmtime', ['later: hub and Macs', 'heavier fuzzing/audit'], 's'],
      [700, 'coglet-mock (engine)', ['native fn stands in for a coglet', 'tests runtime without Wasm'], 'g']
    ];
    impls.forEach(function (im) {
      svg.appendChild(path('M' + (im[0] + 130) + ',172 L' + (im[0] + 130) + ',206', 'ln dim-l', M('dim')));
      svg.appendChild(box(im[0], 210, 260, 96, 'bx nd-' + im[3], 8));
      svg.appendChild(T(im[0] + 14, 236, im[1], 'mono small'));
      svg.appendChild(TL(im[0] + 14, 262, im[2], 'small dim', 17));
    });
    var hz = g({ class: 'hash-row' });
    hz.appendChild(path('M170,306 C170,338 300,338 330,338', 'ln dim-l'));
    hz.appendChild(path('M500,306 C500,338 470,338 440,338', 'ln dim-l'));
    hz.appendChild(box(330, 326, 110, 24, 'chip chip-ok', 12));
    hz.appendChild(T(385, 343, 'h(n) = h(n)', 'mono tiny', { 'text-anchor': 'middle' }));
    hz.appendChild(T(460, 343, 'same coglet on both engines → identical state hashes: a free conformance test', 'small', {}));
    svg.appendChild(hz);
    mount(fig, svg, 640);
  }

  // =====================================================================
  // 8. HUB STRUCTURE — kernel + coglets + services
  // =====================================================================
  function hubmap(fig) {
    var W = 1000, H = 400, svg = svgRoot(W, H, 'Hub structure: a fixed kernel, the fast loop and factory coglets, and hub services'), M = defs(svg);
    svg.appendChild(box(16, 16, 968, 300, 'bx hub-b', 12));
    svg.appendChild(T(36, 46, 'HUB', 'lbl hub-t'));
    // kernel
    svg.appendChild(box(36, 64, 440, 232, 'bx nd-k', 8));
    svg.appendChild(T(52, 90, 'kernel', 'mono hd'));
    svg.appendChild(T(460, 90, 'fixed · edited by hand only · crates/coglet-hub', 'small dim', { 'text-anchor': 'end' }));
    ['identity · enroll', 'coglet registry', 'routing', 'promotion', 'policy files', 'probation', 'action arbitration', 'audit log', 'signature verify', 'health · rollback'].forEach(function (c, i) {
      var x = 52 + (i % 2) * 208, y = 104 + Math.floor(i / 2) * 36;
      svg.appendChild(box(x, y, 196, 28, 'chip chip-k', 4));
      svg.appendChild(T(x + 10, y + 19, c, 'mono small'));
    });
    // coglets
    svg.appendChild(box(500, 64, 230, 112, 'bx nd-c', 8));
    svg.appendChild(T(516, 90, 'fast loop', 'mono hd'));
    svg.appendChild(T(714, 90, 'coglet', 'small dim', { 'text-anchor': 'end' }));
    svg.appendChild(TL(516, 114, ['small fast model, always on', 'conversation · routing', 'files tickets'], 'small', 18));
    svg.appendChild(box(500, 184, 230, 112, 'bx nd-c', 8));
    svg.appendChild(T(516, 210, 'factory', 'mono hd'));
    svg.appendChild(T(714, 210, 'coglet', 'small dim', { 'text-anchor': 'end' }));
    svg.appendChild(TL(516, 234, ['ticket → code for any coglet', 'incl. fast loop and itself', 'builds Wasm · requests gates'], 'small', 18));
    // services
    [['LLM gateway', 'routing · spend · recordings', 'coglet-llm'], ['gate service', 'journal replay · game forks', 'coglet-gate'], ['trace store', 'journals · audit · training', 'coglet-trace']].forEach(function (s, i) {
      var y = 64 + i * 78;
      svg.appendChild(box(750, y, 214, 70, 'bx nd-s', 8));
      svg.appendChild(T(764, y + 24, s[0], 'mono small'));
      svg.appendChild(T(764, y + 44, s[1], 'small dim'));
      svg.appendChild(T(764, y + 61, s[2], 'mono tiny dim'));
    });
    // outside
    svg.appendChild(box(440, 340, 420, 50, 'bx ext-b', 8));
    svg.appendChild(TL(456, 360, ['Hermes gateway + Kanban (outside the self-editing loop)', 'generic ticket API, so caos can replace it later'], 'small', 18));
    svg.appendChild(path('M615,296 L615,336', 'ln dash', M('ink')));
    svg.appendChild(T(626, 322, 'factory backend', 'small dim'));
    svg.appendChild(T(36, 350, 'The hub is itself made of coglets,', 'small'));
    svg.appendChild(T(36, 370, 'edited by the same factory, on a small fixed kernel.', 'small'));
    mount(fig, svg, 680);
  }

  // =====================================================================
  // 9. CONFORMANCE — spec → fixtures → harness → real and mock
  // =====================================================================
  function conformance(fig) {
    var W = 1000, H = 330, svg = svgRoot(W, H, 'Every protocol has a spec, fixtures and one black-box suite that both the real implementation and its mock must pass'), M = defs(svg);
    svg.appendChild(box(16, 30, 200, 120, 'bx', 8));
    svg.appendChild(TL(32, 58, ['spec/*.md', 'spec/messages/', '  CDDL + JSON Schema', 'spec/fixtures/', '  golden frames, timelines'], 'mono small', 19));
    svg.appendChild(path('M216,90 L300,90', 'ln', M('ink')));
    svg.appendChild(box(304, 30, 260, 120, 'bx nd-k', 8));
    svg.appendChild(T(320, 58, 'conformance/', 'mono hd'));
    svg.appendChild(TL(320, 84, ['language-neutral black box', 'drives an implementation', 'over the wire', 'suites: abi surface hub engine speech'], 'small', 17));
    var rows = [['real implementation', 'crates/coglet-runtime, face-mac, game adapter, SDKs…', 'ok'], ['its mock', 'coglet-mock: engine · surface · hub · LLM', 'ok']];
    rows.forEach(function (r, i) {
      var y = 26 + i * 70;
      svg.appendChild(path('M564,90 C600,90 600,' + (y + 30) + ' 640,' + (y + 30), 'ln', M('ink')));
      svg.appendChild(box(644, y, 340, 60, 'bx nd-' + (i ? 'g' : 'c'), 8));
      svg.appendChild(T(658, y + 24, r[0], 'mono small'));
      svg.appendChild(T(658, y + 44, r[1], 'small dim'));
      svg.appendChild(box(910, y + 10, 62, 20, 'chip chip-ok', 10));
      svg.appendChild(T(941, y + 25, 'pass', 'tiny', { 'text-anchor': 'middle' }));
    });
    svg.appendChild(T(500, 196, 'Same suite, both sides, so mocks cannot drift. The simulator is the mocks wired together, with real pieces swapped in one at a time.', 'small', { 'text-anchor': 'middle' }));
    var pairs = [['ABI', 'runtime', 'mock engine'], ['surface', 'face-mac · game adapter', 'mock surface'], ['hub', 'coglet-hub', 'mock hub'], ['engine', 'WAMR', 'Wasmtime (state hash)'], ['speech', 'face-mac', 'sim duplex-audio']];
    pairs.forEach(function (p, i) {
      var x = 16 + i * 197;
      svg.appendChild(box(x, 222, 185, 92, 'bx', 8));
      svg.appendChild(T(x + 12, 246, p[0], 'mono small cog-t'));
      svg.appendChild(T(x + 12, 272, p[1], 'small'));
      svg.appendChild(T(x + 12, 296, '≡ ' + p[2], 'small dim'));
    });
    mount(fig, svg, 680);
  }

  // =====================================================================
  // 10. PHASES — the build graph
  // =====================================================================
  function phases(fig) {
    var W = 1000, H = 420, svg = svgRoot(W, H, 'Build phases: P0 skeleton and specs, v0 freeze, P2 parallel components, P3 milestones, P4 product lanes'), M = defs(svg);
    var cols = [
      [16, 'P0 · P1', 'specs + spikes', ['skeleton ✓', 'conventions ✓', 'ABI + journal', 'surface + speech', 'hub protocol', 'policy draft', 'conformance harness', 'codegen', 'spikes: WAMR cap ✓ armv7 ✓', '  hash ✓; iOS build pending']],
      [222, 'gate', 'v0 freeze', ['ADR-0001', 'conventions · ABI', 'surface · speech', 'minimal hub protocol', '', 'the one serial gate']],
      [388, 'P2', 'all parallel', ['engine-wamr', 'mock engine', 'fs (CoW)', 'runtime core + C ABI', 'guest SDK, echo, counter', 'surface SDKs rust py swift', 'hub proto + mock hub', 'kernel part 1', 'mock LLM + gateway', 'release pipeline']],
      [606, 'P3', 'milestones', ['M1 hello coglet', 'M2 sandbox: gridbot,', '  cartpole, CLI, visualizer', 'M2b Face on Mac', 'M3 self-editing loop', '  (policy freeze)', 'parallel: Wasmtime + hash,', '  trace, fault injection']],
      [806, 'P4', 'product lanes', ['Mindister: game adapter,', '  avatar, fast loop,', '  factory, Face→hub→avatar', '', 'Baby Bear: EO1 pilot,', '  Echo Dot/Linux (Go),', '  Mac pod, iOS embedded']]
    ];
    cols.forEach(function (c, i) {
      var w = i === 1 ? 150 : i === 4 ? 178 : 196;
      var gg = g({ class: i === 0 ? 'now' : '' });
      gg.appendChild(box(c[0], 40, w, 360, 'bx ' + (i === 1 ? 'nd-k gate-col' : i === 0 ? 'nd-c' : ''), 8));
      gg.appendChild(T(c[0] + 12, 66, c[1], 'mono hd' + (i === 0 ? ' cog-t' : '')));
      gg.appendChild(T(c[0] + 12, 86, c[2], 'small dim'));
      gg.appendChild(TL(c[0] + 12, 114, c[3], 'small', 26));
      svg.appendChild(gg);
      if (i < 4) svg.appendChild(path('M' + (c[0] + w + 2) + ',220 L' + (cols[i + 1][0] - 4) + ',220', 'ln', M('ink')));
    });
    svg.appendChild(T(16, 26, 'you are here → P0/P1 (2026-11-23): skeleton, conventions spec, WAMR platform + cap spikes merged; ABI, surface, hub, policy specs in review', 'small cog-t'));
    mount(fig, svg, 760);
  }

  var REG = { roles: roles, eventflow: eventflow, gitrefs: gitrefs, promotion: promotion, bargein: bargein, proto: proto, engine: engine, hubmap: hubmap, conformance: conformance, phases: phases };
  function init(root) {
    (root || document).querySelectorAll('figure.dg[data-diagram]').forEach(function (fig) {
      if (fig.dataset.ready) return;
      var f = REG[fig.dataset.diagram];
      if (!f) { console.warn('unknown diagram', fig.dataset.diagram); return; }
      fig.dataset.ready = '1';
      f(fig);
    });
  }
  window.COGLET = window.COGLET || {};
  window.COGLET.initDiagrams = init;
  window.COGLET.STATIC = STATIC;
})();
