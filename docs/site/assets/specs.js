/* Spec link registry: the single place that knows which spec files exist on main.
 *
 * status:
 *   live      the document is written on main
 *   progress  the path exists on main as a placeholder; a Kanban card ("coglet: spec ...") is writing it
 *   planned   the path is named in docs/plan.md but does not exist on main yet
 *   scaffold  a directory created by the P0 skeleton (empty crate/README), implementation pending
 * pr: an open pull request that is writing the file, if any.
 *
 * When a spec lands, flip its status here (follow-up PRs only touch this file).
 * Checked against `gh api repos/softmax-universe/coglet/contents/<path>` on 2026-11-23.
 */
(function () {
  var REPO = 'https://github.com/softmax-universe/coglet';
  var PR = REPO + '/pull/';
  var SPECS = {
    design:      { path: 'docs/design/coglet-runtime.md', label: 'docs/design/coglet-runtime.md', status: 'live' },
    plan:        { path: 'docs/plan.md', label: 'docs/plan.md', status: 'live' },
    review:      { path: 'docs/design/plan-review-2026-11-23.md', label: 'docs/design/plan-review-2026-11-23.md', status: 'live' },
    adrs:        { path: 'docs/adr', dir: true, label: 'docs/adr/', status: 'live' },
    adrTemplate: { path: 'docs/adr/0000-template.md', label: 'docs/adr/0000-template.md', status: 'live' },
    adrFreeze:   { path: 'docs/adr/0001-v0-freeze.md', label: 'ADR-0001 v0 freeze', status: 'planned' },
    adrConv:     { path: 'docs/adr/0002-common-conventions.md', label: 'ADR-0002 common conventions', status: 'live' },
    adrPlatforms:{ path: 'docs/adr/0002-engine-findings-platforms.md', label: 'ADR-0002 WAMR platform findings', status: 'live' },
    adrCap:      { path: 'docs/adr/0001-engine-findings-cap.md', label: 'ADR-0001 WAMR cap/deadline findings', status: 'live' },
    adrDeterm:   { path: 'docs/adr/0003-determinism-rules.md', label: 'ADR-0003 determinism rules', status: 'progress', pr: 5 },
    conventions: { path: 'spec/conventions.md', label: 'spec/conventions.md', status: 'live' },
    abi:         { path: 'spec/abi.md', label: 'spec/abi.md', status: 'progress', pr: 8 },
    surface:     { path: 'spec/surface-protocol.md', label: 'spec/surface-protocol.md', status: 'progress', pr: 9 },
    speech:      { path: 'spec/capabilities/speech.md', label: 'spec/capabilities/speech.md', status: 'progress', pr: 9 },
    expression:  { path: 'spec/capabilities/expression.md', label: 'spec/capabilities/expression.md', status: 'progress', pr: 9 },
    hub:         { path: 'spec/hub-protocol.md', label: 'spec/hub-protocol.md', status: 'progress', pr: 7 },
    manifest:    { path: 'spec/manifest.schema.json', label: 'spec/manifest.schema.json', status: 'progress' },
    policy:      { path: 'spec/policy.schema.json', label: 'spec/policy.schema.json', status: 'progress', pr: 10 },
    messages:    { path: 'spec/messages', dir: true, label: 'spec/messages/', status: 'live' },
    fixtures:    { path: 'spec/fixtures', dir: true, label: 'spec/fixtures/', status: 'live' },
    conformance: { path: 'conformance', dir: true, label: 'conformance/', status: 'scaffold' },
    spikeHash:   { path: 'spikes/cross-engine-hash', dir: true, label: 'spikes/cross-engine-hash/', status: 'progress', pr: 5 },
    spikeWamr:   { path: 'spikes/wamr-platforms', dir: true, label: 'spikes/wamr-platforms/', status: 'live' },
    spikeCap:    { path: 'spikes/wamr-cap', dir: true, label: 'spikes/wamr-cap/', status: 'live' }
  };
  var NOTE = {
    live: 'on main',
    progress: 'in progress',
    planned: 'planned',
    scaffold: 'scaffold'
  };
  var TITLE = {
    live: 'Written and merged on main.',
    progress: 'In progress: the file is a placeholder on main (or exists only in an open PR) while a Kanban card writes it.',
    planned: 'Planned in docs/plan.md; not on main yet.',
    scaffold: 'Directory scaffolded by the P0 skeleton; implementation pending.'
  };

  function href(s) {
    return REPO + (s.dir ? '/tree/main/' : '/blob/main/') + s.path;
  }

  // Paths that exist only in an open PR link to the PR's files instead of a 404 on main.
  var ONLY_IN_PR = { adrDeterm: 1, spikeHash: 1 };

  function linkFor(key) {
    var s = SPECS[key];
    if (!s) return null;
    if (ONLY_IN_PR[key] && s.status === 'progress' && s.pr) return PR + s.pr + '/files';
    if (s.status === 'planned') return REPO + '/blob/main/docs/plan.md#phases-and-dependencies';
    return href(s);
  }

  function decorate(root) {
    (root || document).querySelectorAll('a[data-spec]').forEach(function (a) {
      if (a.dataset.decorated) return;
      var key = a.dataset.spec, s = SPECS[key];
      if (!s) { a.classList.add('spec-unknown'); return; }
      a.href = linkFor(key);
      a.target = '_blank';
      a.rel = 'noopener';
      if (!a.textContent.trim()) a.textContent = s.label;
      a.classList.add('spec', 'spec-' + s.status);
      a.title = TITLE[s.status] + (s.pr ? ' PR #' + s.pr + '.' : '');
      if (s.status !== 'live' && !a.hasAttribute('data-nobadge')) {
        var b = document.createElement('span');
        b.className = 'badge badge-' + s.status;
        b.textContent = NOTE[s.status];
        a.appendChild(b);
      }
      a.dataset.decorated = '1';
    });
    (root || document).querySelectorAll('a[data-repo]').forEach(function (a) {
      var p = a.dataset.repo;
      a.href = REPO + (p.endsWith('/') ? '/tree/main/' + p.slice(0, -1) : '/blob/main/' + p);
      a.target = '_blank'; a.rel = 'noopener';
    });
    (root || document).querySelectorAll('a[data-pr]').forEach(function (a) {
      a.href = PR + a.dataset.pr; a.target = '_blank'; a.rel = 'noopener';
    });
  }

  window.COGLET = window.COGLET || {};
  window.COGLET.REPO = REPO;
  window.COGLET.SPECS = SPECS;
  window.COGLET.specLink = linkFor;
  window.COGLET.decorateSpecs = decorate;
})();
