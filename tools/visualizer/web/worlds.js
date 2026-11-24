'use strict';
// Local presentation adapters, NOT new capability schemas or a simulator.
function renderWorld(parent, hex) {
  parent.removeAttribute('data-kind');
  const empty = message => { const p = document.createElement('p'); p.textContent = message; parent.replaceChildren(p); };
  if (!hex) { empty('No world observation at the configured export path'); return; }
  try {
    const world = JSON.parse(decode(hex));
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('role', 'img'); svg.setAttribute('viewBox', '0 0 640 320');
    const shape = (tag, attributes) => {
      const node = document.createElementNS(svg.namespaceURI, tag);
      for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
      svg.append(node); return node;
    };
    const finite = value => typeof value === 'number' && Number.isFinite(value);
    if (world.kind === 'gridbot') {
      const {width, height, robot, goal, walls = []} = world;
      if (![width, height].every(n => Number.isInteger(n) && n > 0 && n <= 128) || !Array.isArray(walls) || walls.length > 16384) throw Error('Invalid grid bounds');
      const point = p => p && [p.x, p.y].every(Number.isInteger) && p.x >= 0 && p.x < width && p.y >= 0 && p.y < height;
      if (!point(robot) || goal && !point(goal)) throw Error('Invalid grid position');
      const size = Math.min(600 / width, 280 / height), ox = (640 - width * size) / 2, oy = 20;
      const cell = (x, y, fill) => shape('rect', {x: ox + x * size, y: oy + y * size, width: size, height: size, fill, stroke: '#293035'});
      for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) cell(x, y, '#101214');
      for (const wall of walls) { if (!Array.isArray(wall) || !point({x: wall[0], y: wall[1]})) throw Error('Invalid wall'); cell(...wall, '#627078'); }
      if (goal) cell(goal.x, goal.y, '#48765e');
      shape('circle', {cx: ox + (robot.x + .5) * size, cy: oy + (robot.y + .5) * size, r: size * .3, fill: '#a3f1cf'});
      svg.setAttribute('aria-label', `Gridbot at ${robot.x}, ${robot.y}; ${width} by ${height} grid`);
    } else if (world.kind === 'cartpole') {
      if (!finite(world.x) || !finite(world.theta) || Math.abs(world.x) > 100 || Math.abs(world.theta) > Math.PI * 2) throw Error('Invalid cartpole observation');
      const x = Math.max(30, Math.min(610, 320 + world.x * 70));
      shape('line', {x1: 20, x2: 620, y1: 260, y2: 260, stroke: '#627078'});
      shape('rect', {x: x - 28, y: 238, width: 56, height: 22, fill: '#a3f1cf'});
      shape('line', {x1: x, y1: 238, x2: x + Math.sin(world.theta) * 170, y2: 238 - Math.cos(world.theta) * 170, stroke: '#e3e8e8', 'stroke-width': 8});
      svg.setAttribute('aria-label', `Cartpole position ${world.x}; angle ${world.theta} radians; display clipped at track edges`);
    } else throw Error('Unknown world kind');
    parent.replaceChildren(svg); parent.dataset.kind = world.kind;
    const caption = document.createElement('p'); caption.textContent = `${world.kind} · exported observation, not simulated by this viewer`; parent.append(caption);
  } catch (error) { empty(`World unavailable: ${error.message}`); }
}
