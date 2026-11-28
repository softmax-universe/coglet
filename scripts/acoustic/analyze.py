#!/usr/bin/env python3
"""Summarise and gate M2b acoustic-proof runs (ADR-0044). Standard library only.

  analyze.py summarize <run-dir>...            distributions (JSON on stdout, table on stderr)
  analyze.py derive <run-dir>... [--out F]     thresholds from measured distributions (ADR rule)
  analyze.py check <thresholds.json> <run-dir> exit 0 only if every threshold holds
"""
import json
import math
import sys
from pathlib import Path

# metric -> (kind-of-run, direction) ; 'max' metrics are latency-like upper bounds on |value|-style stats.
LATENCY = {
    'barge_to_acoustic_stop_ms': 'barge', 'play_to_acoustic_onset_ms': 'barge',
    'ptt_press_call_ms': 'barge', 'ptt_press_to_first_capture_ms': 'capture',
    'ptt_release_to_end_event_ms': 'capture', 'stimulus_scheduled_to_capture_ms': 'stimulus',
    'barge_to_acoustic_duck_ms': 'duck',
}


def load(dirs):
    runs, sessions = [], []
    for d in dirs:
        d = Path(d)
        sessions.append(json.loads((d / 'session.json').read_text()))
        runs += [json.loads(line) for line in (d / 'runs.jsonl').read_text().splitlines() if line.strip()]
    return runs, sessions


def pct(values, p):
    s = sorted(values)
    if not s:
        return None
    k = (len(s) - 1) * p
    lo, hi = math.floor(k), math.ceil(k)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def dist(values):
    if not values:
        return {'n': 0}
    return {'n': len(values), 'min': round(min(values), 2), 'p50': round(pct(values, .5), 2),
            'p95': round(pct(values, .95), 2), 'p99': round(pct(values, .99), 2), 'max': round(max(values), 2),
            'mean': round(sum(values) / len(values), 2)}


def values(runs, metric, kind=None):
    return [r[metric] for r in runs if metric in r and type(r[metric]) in (int, float) and math.isfinite(r[metric])
            and (kind is None or r.get('kind') == kind)]


def summarize(runs):
    noisy = sum(1 for r in runs if r.get('noise_ok') is False)
    runs = [r for r in runs if r.get('noise_ok') is not False]  # lead-in already above the tone threshold
    out = {'noisy_runs_excluded': noisy, 'latency_ms': {m: dist(values(runs, m, k)) for m, k in LATENCY.items()}}
    barge = [r for r in runs if r.get('kind') == 'barge']
    out['heard_truncation'] = {
        'played_minus_acoustic_ms': dist(values(barge, 'played_minus_acoustic_ms')),
        'abs_error_ms': dist([abs(v) for v in values(barge, 'played_minus_acoustic_ms')]),
        'word_exact_rate': rate([r for r in barge if 'word_exact' in r], 'word_exact'),
        'over_credited_runs': sum(1 for r in barge if r.get('over_credited_words', 0) > 0),
        'runs_with_heard_measure': sum(1 for r in barge if 'word_exact' in r),
    }
    out['conformance'] = {k: {'runs': sum(1 for r in runs if r.get('kind') == k),
                              'conformant': sum(1 for r in runs if r.get('kind') == k and r.get('conformant'))}
                          for k in ('barge', 'capture', 'duck')}
    ducks = [r for r in runs if r.get('kind') == 'duck']
    out['duck'] = {
        'level_ratio': dist(values(ducks, 'duck_level_ratio')),
        'late_level_ratio': dist(values(ducks, 'duck_late_level_ratio')),
        'level_snr_db': dist(values(ducks, 'duck_level_snr_db')),
        'late_snr_db': dist(values(ducks, 'duck_late_snr_db')),
        'render_continued': sum(r.get('duck_render_continued') is True for r in ducks),
        'tone_missing': sum(r.get('tone_present_at_barge') is not True for r in ducks),
    }
    out['stimulus_missed'] = sum(1 for r in runs if r.get('stimulus_missed'))
    out['tone_missing_at_barge'] = sum(1 for r in barge if r.get('tone_present_at_barge') is False)
    direct = [r for r in runs if r.get('kind') == 'ringdown']
    out['direct_flush_diagnostic'] = {
        str(f): dist(values([r for r in direct if r.get('frequency_hz') == f], 'flush_to_acoustic_stop_ms'))
        for f in sorted({r['frequency_hz'] for r in direct})
    }
    return out


def rate(runs, key):
    return round(sum(1 for r in runs if r.get(key)) / len(runs), 4) if runs else None


def ceil5(x):
    return int(math.ceil(x / 5.0) * 5)


def completeness_failures(runs):
    """Version-2 attempts cannot hide failed measurements by dropping rows/fields."""
    fails = []
    if any(r.get('noise_ok') is False for r in runs):
        fails.append('noisy runs invalidate the attempt; retain and repeat the whole attempt')
    for metric, kind in LATENCY.items():
        selected = [r for r in runs if r.get('kind') == kind]
        measured = values(selected, metric)
        if len(measured) != len(selected) or any(v < 0 for v in measured):
            fails.append(f'every {kind} run must have finite nonnegative {metric}')
    barge = [r for r in runs if r.get('kind') == 'barge']
    if any(r.get('tone_present_at_barge') is not True for r in barge):
        fails.append('every barge run must have audible tone at barge')
    if len(values(barge, 'played_minus_acoustic_ms')) != len(barge):
        fails.append('every barge run must have finite heard error')
    if any(type(r.get('word_exact')) is not bool or type(r.get('over_credited_words')) is not int
           for r in barge):
        fails.append('every barge run must have heard word and over-credit measurements')
    direct = [r for r in runs if r.get('kind') == 'ringdown']
    if len(values(direct, 'flush_to_acoustic_stop_ms')) != len(direct):
        fails.append('every direct-flush diagnostic must have a measured acoustic stop')
    snrs = values(direct, 'snr_db')
    if len(snrs) != len(direct) or any(v < 12 for v in snrs):
        fails.append('every direct-flush diagnostic must have at least 12 dB tone SNR')
    return fails


def derive(runs):
    """ADR-0044 rule: for each gated latency, p95 limit = ceil5(2*p95) and max limit = ceil5(2*max)
    (never below p99+20 ms); heard-truncation |error| likewise; counts/rates gated at observed
    worst case (over-crediting 0 of N, conformance 100%) only if that is what was observed."""
    errors = completeness_failures(runs)
    if errors:
        raise ValueError('; '.join(errors))
    s = summarize(runs)
    if s['duck']['level_ratio']['n'] < 5:
        raise ValueError('derive requires at least five measured over-talk-duck runs')
    if any(d['n'] < 5 for d in s['latency_ms'].values()):
        raise ValueError('derive requires at least five samples of every latency metric')
    t = {'version': 2, 'rule': 'p95_limit=ceil5(2*p95); max_limit=ceil5(max(2*max, p99+20)); sample counts are minimums',
         'latency_ms': {}, 'heard_truncation': {}}
    t['duck'] = {'min_n': max(5, s['duck']['level_ratio']['n'] // 4),
                 'level_ratio_min': 0.10, 'level_ratio_max': 0.45, 'snr_db_min': 12}
    for m, d in s['latency_ms'].items():
        if d['n'] >= 5:
            t['latency_ms'][m] = {'p95_max': ceil5(2 * d['p95']), 'max_max': ceil5(max(2 * d['max'], d['p99'] + 20)),
                                  'min_n': max(5, d['n'] // 4)}
    h = s['heard_truncation']
    if h['abs_error_ms'].get('n', 0) >= 5:
        a = h['abs_error_ms']
        t['heard_truncation']['abs_error_ms'] = {'p95_max': ceil5(2 * a['p95']), 'max_max': ceil5(max(2 * a['max'], a['p99'] + 20)),
                                                  'min_n': max(5, a['n'] // 4)}
        t['heard_truncation']['over_credited_runs_max'] = h['over_credited_runs'] if h['over_credited_runs'] else 0
        t['heard_truncation']['word_exact_rate_min'] = round(max(0.0, (h['word_exact_rate'] or 0) - 0.1), 2)
    t['conformance_rate_min'] = 1.0
    t['stimulus_missed_max'] = 0
    return t


def check(thresholds, runs):
    s = summarize(runs)
    fails, notes = [], []
    if thresholds.get('version', 1) >= 2:
        fails.extend(completeness_failures(runs))
        if 'duck' not in thresholds or set(thresholds['latency_ms']) != set(LATENCY):
            fails.append('version-2 thresholds must gate duck and every latency metric')
        if 'abs_error_ms' not in thresholds.get('heard_truncation', {}):
            fails.append('version-2 thresholds must gate heard error')
    for m, lim in thresholds['latency_ms'].items():
        d = s['latency_ms'].get(m, {'n': 0})
        if d['n'] < lim['min_n']:
            fails.append(f'{m}: only {d["n"]} samples (< {lim["min_n"]})'); continue
        for stat, key in (('p95', 'p95_max'), ('max', 'max_max')):
            ok = d[stat] <= lim[key]
            (notes if ok else fails).append(f'{m} {stat}={d[stat]} {"<=" if ok else ">"} {lim[key]}')
    ht = thresholds.get('heard_truncation', {})
    h = s['heard_truncation']
    if 'abs_error_ms' in ht:
        a, lim = h['abs_error_ms'], ht['abs_error_ms']
        if a.get('n', 0) < lim['min_n']:
            fails.append(f'heard abs_error: only {a.get("n", 0)} samples')
        else:
            for stat, key in (('p95', 'p95_max'), ('max', 'max_max')):
                ok = a[stat] <= lim[key]
                (notes if ok else fails).append(f'heard |played-acoustic| {stat}={a[stat]} {"<=" if ok else ">"} {lim[key]}')
        ok = h['over_credited_runs'] <= ht['over_credited_runs_max']
        (notes if ok else fails).append(f'over_credited_runs={h["over_credited_runs"]} max {ht["over_credited_runs_max"]}')
        ok = (h['word_exact_rate'] or 0) >= ht['word_exact_rate_min']
        (notes if ok else fails).append(f'word_exact_rate={h["word_exact_rate"]} min {ht["word_exact_rate_min"]}')
    for k, c in s['conformance'].items():
        if k == 'duck' and 'duck' not in thresholds:
            continue  # Historical version-1 sessions predate duck qualification.
        ok = c['runs'] > 0 and c['conformant'] / c['runs'] >= thresholds['conformance_rate_min']
        (notes if ok else fails).append(f'conformance {k}: {c["conformant"]}/{c["runs"]}')
    ok = s['stimulus_missed'] <= thresholds['stimulus_missed_max']
    (notes if ok else fails).append(f'stimulus_missed={s["stimulus_missed"]}')
    if 'duck' in thresholds:
        lim, d = thresholds['duck'], s['duck']
        ratios = d['level_ratio']
        if ratios['n'] < lim['min_n']:
            fails.append(f'duck level: only {ratios["n"]} samples')
        elif not (lim['level_ratio_min'] <= ratios['min'] <= ratios['max'] <= lim['level_ratio_max']):
            fails.append(f'duck level ratio out of range: {ratios}')
        if d['render_continued'] != s['conformance']['duck']['runs'] or d['tone_missing']:
            fails.append('duck must have pre-barge tone and continued rendered playback in every run')
        n = s['conformance']['duck']['runs']
        if ratios['n'] != n or s['latency_ms']['barge_to_acoustic_duck_ms']['n'] != n:
            fails.append('every accepted duck run must have finite latency and level measurements')
        for name in ('late_level_ratio', 'level_snr_db', 'late_snr_db'):
            metric = d[name]
            if metric['n'] != n or n == 0:
                fails.append(f'every duck run must have {name}')
            elif name == 'late_level_ratio':
                if not (lim['level_ratio_min'] <= metric['min'] <= metric['max'] <= lim['level_ratio_max']):
                    fails.append('duck late level ratio out of range')
            elif metric['min'] < lim['snr_db_min']:
                fails.append(f'duck {name} below required SNR')
    return fails, notes


def main(argv):
    if len(argv) < 3:
        print(__doc__); return 2
    cmd = argv[1]
    if cmd == 'check':
        thresholds = json.loads(Path(argv[2]).read_text())
        runs, sessions = load([argv[3]])
        if not all(x.get('acoustic_coupling_ok') for x in sessions):
            print('FAIL: speaker->microphone coupling check failed in session'); return 1
        fails, notes = check(thresholds, runs)
        for n in notes: print('ok  ', n)
        for f in fails: print('FAIL', f)
        print('PASS' if not fails else 'FAIL'); return 1 if fails else 0
    out = None
    dirs = []
    a = argv[2:]
    while a:
        x = a.pop(0)
        if x == '--out': out = a.pop(0)
        else: dirs.append(x)
    runs, sessions = load(dirs)
    result = derive(runs) if cmd == 'derive' else summarize(runs)
    text = json.dumps(result, indent=2, sort_keys=True)
    if out: Path(out).write_text(text + '\n')
    print(text); return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
