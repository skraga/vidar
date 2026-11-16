"""Paired bootstrap confidence intervals for bench/benchmark.py."""
from collections import defaultdict

import numpy as np

from metrics import LABELS, LOWER_BETTER, RATIOS

B = 10000


def _values(rows, system, metric, names):
    by = {r["name"]: r for r in rows if r["system"] == system}
    if metric in RATIOS:
        a, b = RATIOS[metric]
        return np.array([[by[n].get(a, np.nan), by[n].get(b, np.nan)] for n in names], dtype=float)
    return np.array([by[n].get(metric, np.nan) for n in names], dtype=float)


def _stat(v, idx=None):
    """Mean of v (or summed ratio for 2-column v) over idx resamples; nan-safe."""
    if v.ndim == 2:
        num, den = v[:, 0], v[:, 1]
        if idx is None:
            return np.nansum(num) / max(np.nansum(den), 1e-12)
        return np.nansum(num[idx], -1) / np.maximum(np.nansum(den[idx], -1), 1e-12)
    if idx is None:
        return np.nanmean(v)
    return np.nanmean(v[idx], -1)


def compare(rows, systems, metrics, baseline, seed=0, group=None):
    """For each metric: per system mean and 95% CI, and the paired difference from `baseline`
    with its CI, over the clips that every system has a score for.

    Returns a list of dicts (one per system x metric)."""
    rng = np.random.default_rng(seed)
    out = []
    for m in metrics:
        names = None
        present = [s for s in systems if any(r["system"] == s and _ok(r, m) for r in rows)]
        if baseline not in present:
            continue
        for s in present:
            have = {r["name"] for r in rows if r["system"] == s and _ok(r, m)}
            names = have if names is None else names & have
        names = sorted(names or [])
        if len(names) < 2:
            continue
        idx = rng.integers(0, len(names), (B, len(names)))
        base = _values(rows, baseline, m, names)
        bboot = _stat(base, idx)
        for s in present:
            v = _values(rows, s, m, names)
            boot = _stat(v, idx)
            d = boot - bboot
            lo, hi = np.nanpercentile(boot, [2.5, 97.5])
            dlo, dhi = np.nanpercentile(d, [2.5, 97.5])
            out.append(dict(group=group or "all", metric=m, system=s, n=len(names), mean=float(_stat(v)),
                            lo=float(lo), hi=float(hi), delta=float(_stat(v) - _stat(base)),
                            dlo=float(dlo), dhi=float(dhi),
                            significant=bool(s != baseline and (dlo > 0 or dhi < 0))))
    return out


def _ok(r, m):
    if m in RATIOS:
        a, b = RATIOS[m]
        return a in r and r.get(b) and not np.isnan(r[a])
    return m in r and r[m] is not None and not (isinstance(r[m], float) and np.isnan(r[m]))


def print_compare(res, baseline):
    if not res:
        return
    print(f"\n== overall: mean [95% CI]; delta from {baseline} [95% CI], * = significant "
          f"(paired bootstrap, {B} resamples)")
    by = defaultdict(list)
    for r in res:
        by[r["metric"]].append(r)
    for m, rs in by.items():
        lab = LABELS.get(m, m) + (" (lower is better)" if m in LOWER_BETTER else "")
        print(f"{lab}  n={rs[0]['n']}")
        for r in rs:
            line = f"  {r['system']:<20} {_f(m, r['mean'])} [{_f(m, r['lo'])}, {_f(m, r['hi'])}]"
            if r["system"] != baseline:
                line += (f"   {_f(m, r['delta'], True)} [{_f(m, r['dlo'], True)}, {_f(m, r['dhi'], True)}]"
                         f"{' *' if r['significant'] else ''}")
            print(line)


def _f(m, x, sign=False):
    if m in ("wer", "onset_clip", "open_pct"):
        return f"{x * 100:+6.1f}%" if sign else f"{x * 100:5.1f}%"
    if m in ("stoi", "estoi", "squim_stoi"):
        return f"{x:+.4f}" if sign else f"{x:.4f}"
    return f"{x:+.3f}" if sign else f"{x:.3f}"


def print_groups(rows, systems, metrics, group):
    """Plain means per group value (no CIs), one line per system x group."""
    groups = sorted({r[group] for r in rows}, key=str)
    cols = [m for m in metrics if any(_ok(r, m) for r in rows)]
    print(f"\n== by {group}")
    print(f"{'':<20} {'':<22} " + " ".join(f"{LABELS.get(m, m)[:12]:>12}" for m in cols))
    for g in groups:
        for s in systems:
            sub = [r for r in rows if r["system"] == s and r[group] == g]
            if not sub:
                continue
            vals = []
            for m in cols:
                names = [r["name"] for r in sub if _ok(r, m)]
                vals.append(_f(m, _stat(_values(sub, s, m, names))) if names else "-")
            print(f"{s:<20} {g!s:<22} " + " ".join(f"{v:>12}" for v in vals))
