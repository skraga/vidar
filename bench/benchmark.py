"""
Score Vidar on public noise-suppression test sets, through vidar-cli (the full chain:
rumble filter -> AI model -> transient suppressor -> gate -> limiter).

  snsd  MS-SNSD (github.com/microsoft/MS-SNSD): clean speech mixed with real noise at
        0/5/10/20 dB SNR. Reference scores: PESQ-WB and STOI at 16 kHz.
  dns   DNS Challenge blind test set (github.com/microsoft/DNS-Challenge, branch
        interspeech2020/master, datasets/blind_test_set): real noisy recordings.
        No-reference scores: DNSMOS P.835 (SIG speech, BAK background, OVRL overall).

Dev-only dependencies:  pip install -r bench/requirements.txt
Examples:
  python bench/benchmark.py snsd --snsd-dir MS-SNSD --work bench-out
  python bench/benchmark.py dns --blind-dir blind_test_set --dnsmos sig_bak_ovr.onnx --work bench-out
"""
import argparse
import csv
import glob
import json
import os
import subprocess
import sys
import time
from collections import defaultdict
from multiprocessing import Pool
from pathlib import Path

import numpy as np

SR = 48000
SNRS = (0, 5, 10, 20)
DEFAULT_CLI = Path(__file__).resolve().parent.parent / "target" / "release" / "vidar-cli.exe"

# Vidar configurations to score: name -> vidar-cli flags.
SYSTEMS = {
    "voice": ["--engine", "rnnoise"],
    "voice_hd": ["--engine", "dpdfnet", "--dpdfnet-vad", "rnnoise"],
    "voice_hd_spectral": ["--engine", "dpdfnet", "--dpdfnet-vad", "spectral"],
    "voice_no_gate": ["--engine", "rnnoise", "--no-gate"],
}


def _load(path, sr_out):
    import soundfile as sf
    from scipy.signal import resample_poly
    x, sr = sf.read(path, dtype="float32", always_2d=True)
    x = x.mean(axis=1)
    if sr != sr_out:
        g = np.gcd(sr, sr_out)
        x = resample_poly(x, sr_out // g, sr // g).astype(np.float32)
    return x


def _write(path, x, sr):
    import soundfile as sf
    sf.write(path, np.clip(x, -1, 1), sr, subtype="PCM_16")


def _rms(x):
    return float(np.sqrt(np.mean(x.astype(np.float64) ** 2)) + 1e-12)


# ---------------------------------------------------------------- MS-SNSD mixing

def prepare_snsd(snsd_dir, work, clips_per_noise, seed):
    rng = np.random.default_rng(seed)
    clean_files = sorted(glob.glob(os.path.join(snsd_dir, "clean_test", "*.wav")))
    noise_files = sorted(glob.glob(os.path.join(snsd_dir, "noise_test", "*.wav")))
    by_type = defaultdict(list)
    for f in noise_files:
        by_type[Path(f).stem.rsplit("_", 1)[0]].append(f)
    picks = rng.choice(len(clean_files), size=clips_per_noise * len(by_type), replace=False)

    mix_dir = Path(work, "snsd", "noisy"); mix_dir.mkdir(parents=True, exist_ok=True)
    ref_dir = Path(work, "snsd", "clean16"); ref_dir.mkdir(parents=True, exist_ok=True)
    meta, i = [], 0
    from scipy.signal import resample_poly
    for ntype in sorted(by_type):
        for _ in range(clips_per_noise):
            clean = _load(clean_files[picks[i]], SR); i += 1
            clean *= 10 ** (-25 / 20) / _rms(clean)  # MS-SNSD convention: -25 dBFS speech
            noise = _load(by_type[ntype][rng.integers(len(by_type[ntype]))], SR)
            if len(noise) < len(clean):
                noise = np.tile(noise, int(np.ceil(len(clean) / len(noise))))
            off = rng.integers(0, len(noise) - len(clean) + 1)
            noise = noise[off:off + len(clean)]
            for snr in SNRS:
                mix = clean + noise * (_rms(clean) / _rms(noise)) / (10 ** (snr / 20))
                peak = np.max(np.abs(mix))
                scale = 0.95 / peak if peak > 0.95 else 1.0
                name = f"{ntype}_{i:03d}_snr{snr:02d}"
                _write(mix_dir / f"{name}.wav", mix * scale, SR)
                _write(ref_dir / f"{name}.wav", resample_poly(clean * scale, 1, 3), 16000)
                meta.append(dict(name=name, noise=ntype, snr=snr))
    with open(Path(work, "snsd", "meta.json"), "w") as f:
        json.dump(meta, f, indent=1)
    print(f"[prep] {len(meta)} mixtures in {mix_dir}")


# ---------------------------------------------------------------- processing

def run_systems(cli, in_dir, out_root, systems, jobs):
    """Clean the folder once per system with vidar-cli (parallel inside the CLI)."""
    for s in systems:
        out = Path(out_root, s)
        out.mkdir(parents=True, exist_ok=True)
        if all((out / f.name).exists() for f in Path(in_dir).glob("*.wav")):
            continue
        t0 = time.time()
        subprocess.run([str(cli), "--clean", str(in_dir), str(out), "--no-progress", "--no-stats", "--jobs", str(jobs),
                        *SYSTEMS[s]], check=True)
        for f in out.glob("*_cleaned.wav"):
            f.replace(out / (f.name[: -len("_cleaned.wav")] + ".wav"))
        print(f"[run] {s}: {time.time() - t0:.1f}s", flush=True)


# ---------------------------------------------------------------- scoring

def _score_ref(args):
    system, name, ref_path, est_path = args
    from pesq import pesq
    from pystoi import stoi
    ref, est = _load(ref_path, 16000), _load(est_path, 16000)
    n = min(len(ref), len(est))
    ref, est = ref[:n], est[:n]
    # Alignment check: vidar-cli trims its own latency, so the best lag must be ~0.
    seg = slice(1600, n - 1600)
    lag = max(range(-480, 481, 4), key=lambda l: float(np.dot(ref[seg], np.roll(est, -l)[seg])))
    try:
        p = pesq(16000, ref, est, "wb")
    except Exception:
        p = float("nan")
    return dict(system=system, name=name, pesq=p, stoi=stoi(ref, est, 16000, extended=False), lag=lag)


def summarize(rows, keys, group=None):
    out = defaultdict(lambda: defaultdict(list))
    for r in rows:
        g = r[group] if group else "all"
        for k in keys:
            if not np.isnan(r[k]):
                out[(r["system"], g)][k].append(r[k])
    return {sg: {k: float(np.mean(v)) for k, v in d.items()} for sg, d in out.items()}


def run_snsd(args, systems):
    if not Path(args.work, "snsd", "meta.json").exists():
        prepare_snsd(args.snsd_dir, args.work, args.clips_per_noise, args.seed)
    run_systems(args.cli, Path(args.work, "snsd", "noisy"), Path(args.work, "snsd", "out"), systems, args.jobs)
    meta = {m["name"]: m for m in json.load(open(Path(args.work, "snsd", "meta.json")))}
    tasks = []
    for s in ["noisy"] + systems:
        d = Path(args.work, "snsd", "noisy" if s == "noisy" else f"out/{s}")
        for name in meta:
            tasks.append((s, name, str(Path(args.work, "snsd", "clean16", f"{name}.wav")), str(d / f"{name}.wav")))
    with Pool(args.jobs) as pool:
        rows = pool.map(_score_ref, tasks, chunksize=8)
    for r in rows:
        r.update(noise=meta[r["name"]]["noise"], snr=meta[r["name"]]["snr"])
    write_csv(Path(args.work, "snsd_scores.csv"), rows)
    for group in (None, "snr", "noise"):
        print(f"\n== by {group or 'overall'}")
        for (s, g), v in sorted(summarize(rows, ["pesq", "stoi"], group).items(), key=lambda t: (str(t[0][1]), t[0][0])):
            print(f"{s:<18} {g!s:<22} PESQ {v['pesq']:.3f}  STOI {v['stoi']:.4f}")
    lags = defaultdict(list)
    for r in rows:
        lags[r["system"]].append(abs(r["lag"]))
    print("\nmedian |alignment lag| (16 kHz samples):", {s: float(np.median(v)) for s, v in lags.items()})


class DNSMOS:
    """DNSMOS P.835 (sig_bak_ovr.onnx) with Microsoft's published polynomial calibration."""
    LEN = 144160  # 9.01 s at 16 kHz

    def __init__(self, path):
        import onnxruntime as ort
        o = ort.SessionOptions(); o.intra_op_num_threads = 1; o.inter_op_num_threads = 1
        self.sess = ort.InferenceSession(path, o, providers=["CPUExecutionProvider"])
        self.inp = self.sess.get_inputs()[0].name

    def score(self, x16):
        while len(x16) < self.LEN:
            x16 = np.concatenate([x16, x16])
        n_seg = int(np.floor(len(x16) / 16000) - 9.01) + 1  # as in Microsoft's dnsmos_local.py
        res = []
        for i in range(n_seg):
            seg = x16[i * 16000:i * 16000 + self.LEN]
            if len(seg) < self.LEN:
                continue
            sig, bak, ovr = self.sess.run(None, {self.inp: seg[None, :].astype(np.float32)})[0][0]
            res.append((np.poly1d([-0.08397278, 1.22083953, 0.0052439])(sig),
                        np.poly1d([-0.13166888, 1.60915514, -0.39604546])(bak),
                        np.poly1d([-0.06766283, 1.11546468, 0.04602535])(ovr)))
        return [float(v) for v in np.mean(res, axis=0)]


_dnsmos = None


def _dnsmos_init(path):
    global _dnsmos
    _dnsmos = DNSMOS(path)


def _score_dnsmos(args):
    system, name, path = args
    sig, bak, ovr = _dnsmos.score(_load(path, 16000))
    return dict(system=system, name=name, kind=name.split("_")[0], sig=sig, bak=bak, ovrl=ovr)


def run_dns(args, systems):
    files = sorted(glob.glob(os.path.join(args.blind_dir, "**", "*.wav"), recursive=True))
    in_dir = Path(args.work, "dns", "noisy"); in_dir.mkdir(parents=True, exist_ok=True)
    for f in files:
        dst = in_dir / Path(f).name
        if not dst.exists():
            _write(dst, _load(f, SR), SR)
    run_systems(args.cli, in_dir, Path(args.work, "dns", "out"), systems, args.jobs)
    tasks = [("noisy", f.stem, str(f)) for f in sorted(in_dir.glob("*.wav"))]
    for s in systems:
        tasks += [(s, f.stem, str(f)) for f in sorted(Path(args.work, "dns", "out", s).glob("*.wav"))]
    with Pool(args.jobs, initializer=_dnsmos_init, initargs=(args.dnsmos,)) as pool:
        rows = pool.map(_score_dnsmos, tasks, chunksize=4)
    write_csv(Path(args.work, "dns_scores.csv"), rows)
    for group in (None, "kind"):
        print(f"\n== by {group or 'overall'}")
        for (s, g), v in sorted(summarize(rows, ["sig", "bak", "ovrl"], group).items(), key=lambda t: (str(t[0][1]), t[0][0])):
            print(f"{s:<18} {g!s:<10} SIG {v['sig']:.3f}  BAK {v['bak']:.3f}  OVRL {v['ovrl']:.3f}")


def write_csv(path, rows):
    with open(path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
        w.writeheader()
        w.writerows(rows)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("snsd"); a.add_argument("--snsd-dir", required=True)
    a.add_argument("--clips-per-noise", type=int, default=8); a.add_argument("--seed", type=int, default=7)
    b = sub.add_parser("dns"); b.add_argument("--blind-dir", required=True); b.add_argument("--dnsmos", required=True)
    for p in (a, b):
        p.add_argument("--work", required=True)
        p.add_argument("--systems", default="voice,voice_hd")
        p.add_argument("--jobs", type=int, default=os.cpu_count())
        p.add_argument("--cli", type=Path, default=DEFAULT_CLI, help="path to vidar-cli.exe")
    args = ap.parse_args()
    systems = args.systems.split(",")
    unknown = [s for s in systems if s not in SYSTEMS]
    if unknown:
        ap.error(f"unknown systems {unknown}; choose from {list(SYSTEMS)}")
    if not args.cli.is_file():
        ap.error(f"{args.cli} not found; build it with: cargo build --release -p vidar-cli")
    (run_snsd if args.cmd == "snsd" else run_dns)(args, systems)
    return 0


if __name__ == "__main__":
    sys.exit(main())
