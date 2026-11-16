"""
Score Vidar on public noise-suppression test sets, through vidar-cli (the full chain:
rumble filter -> AI model -> transient suppressor -> gate -> limiter).

With a clean reference (all audio scored at 48 kHz unless noted):
  vbd    VoiceBank+DEMAND test set, 48 kHz (824 clips, 2 speakers, transcripts included).
         The standard benchmark; papers (DeepFilterNet, DPDFNet, ...) report on it.
  ears   EARS-WHAM v2 test set: 48 kHz studio speech in real WHAM! noise, -2.5 to 17.5 dB SNR.
  snsd   MS-SNSD: 16 kHz speech mixed with real noise at 0/5/10/20 dB SNR (no content above
         8 kHz, so the full-band scores are left out).
  pairs  any folder of clean/noisy pairs (e.g. DNS synthetic test sets, URGENT validation).

Without a reference:
  blind  real noisy recordings (DNS Challenge 2020 or DNS 5 blind sets); `dns` is an alias.
  noise  noise with no speech at all, at several levels: what the gate lets through.

Listening test:
  mushra          export a MUSHRA test for webMUSHRA from a scored reference set.
  mushra-results  summarize the CSV webMUSHRA writes.

Score families (--metrics, default: every one whose dependencies are present):
  ref     PESQ-WB, STOI, ESTOI, SI-SDR, log-spectral distance (full band and >8 kHz), word
          start/end loss (gate footprint)
  dnsmos  DNSMOS P.835 SIG/BAK/OVRL + P.808 (needs --dnsmos)
  nn      NISQA v2, TorchAudio SQUIM, UTMOS with --utmos (needs torch)
  wer     Whisper large-v3-turbo word error rate (needs torch + openai-whisper)

Every result comes with a 95% confidence interval and a paired bootstrap test against
--baseline (default: the untouched input). Per-clip scores are cached in WORK/<set>/scores,
so adding a system only scores that system.

  python bench/benchmark.py vbd --vbd-dir ../bench-data/VBD --dnsmos ../bench-data/DNSmaster/DNSMOS/DNSMOS --work bench-out
  python bench/benchmark.py blind --blind-dir V5_BlindTestSet --name dns5 --dnsmos DNSMOS --work bench-out
  python bench/benchmark.py noise --noise-dir MS-SNSD/noise_test --work bench-out
"""
import argparse
import csv
import glob
import json
import os
import random
import subprocess
import sys
import time
from collections import defaultdict
from multiprocessing import Pool
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metrics as M  # noqa: E402
import stats  # noqa: E402

SR = 48000
SNRS = (0, 5, 10, 20)
DEFAULT_CLI = Path(__file__).resolve().parent.parent / "target" / "release" / "vidar-cli.exe"

# Vidar configurations to score: name -> vidar-cli flags.
SYSTEMS = {
    "voice": ["--engine", "rnnoise"],
    "voice_hd": ["--engine", "dpdfnet", "--dpdfnet-vad", "rnnoise"],
    "voice_hd_spectral": ["--engine", "dpdfnet", "--dpdfnet-vad", "spectral"],
    "voice_no_gate": ["--engine", "rnnoise", "--no-gate"],
    "voice_hd_no_gate": ["--engine", "dpdfnet", "--dpdfnet-vad", "rnnoise", "--no-gate"],
    "voice_hd_no_declick": ["--engine", "dpdfnet", "--dpdfnet-vad", "rnnoise", "--no-declick"],
    "voice_hd_gate_out": ["--engine", "dpdfnet", "--dpdfnet-vad", "rnnoise", "--gate-level", "output"],
    "voice_hd_8": ["--engine", "dpdfnet", "--dpdfnet-vad", "rnnoise", "--dpdfnet-model", "dpdfnet8_48khz_hr"],
    "no_model": ["--no-rnnoise"],
}

FAMILY_METRICS = {
    "ref": ["pesq", "stoi", "estoi", "sisdr", "lsd", "lsd_hf", "onset_loss", "offset_loss", "onset_clip"],
    "dnsmos": ["sig", "bak", "ovrl", "p808"],
    "nn": ["utmos", "nisqa", "nisqa_noi", "nisqa_dis", "nisqa_col", "squim_pesq", "squim_stoi", "squim_sisdr"],
    "wer": ["wer"],
    "noise": ["out_db", "atten_db", "open_pct", "pump_db"],
}


def _write(path, x, sr, subtype="PCM_16"):
    import soundfile as sf
    sf.write(path, np.clip(x, -1, 1) if subtype == "PCM_16" else x, sr, subtype=subtype)


def _rms(x):
    return float(np.sqrt(np.mean(x.astype(np.float64) ** 2)) + 1e-12)


# ---------------------------------------------------------------- data sets
# Each loader returns items: dict(name, noisy=<source path>, clean=<source path or None>,
# text=<transcript or None>, plus grouping fields). prepare() converts them into
# WORK/<set>/noisy (48 kHz PCM, what vidar-cli reads) and WORK/<set>/clean48 (float).

def items_vbd(a):
    root = Path(a.vbd_dir)
    out = []
    for c in sorted((root / "clean_testset_wav").glob("*.wav")):
        t = root / "testset_txt" / (c.stem + ".txt")
        out.append(dict(name=c.stem, noisy=str(root / "noisy_testset_wav" / c.name), clean=str(c),
                        text=t.read_text(errors="replace").strip() if t.exists() else None,
                        speaker=c.stem.split("_")[0]))
    return out


def items_ears(a):
    root = Path(a.ears_dir)
    if (root / "test").is_dir():
        root = root / "test"
    out = []
    for n in sorted((root / "noisy").glob("*/*.wav")):
        cid, snr = n.stem.split("_")
        snr = float(snr[:-2])
        band = "<2.5" if snr < 2.5 else "2.5-7.5" if snr < 7.5 else "7.5-12.5" if snr < 12.5 else ">12.5"
        out.append(dict(name=f"{n.parent.name}_{cid}", noisy=str(n), clean=str(root / "clean" / n.parent.name / f"{cid}.wav"),
                        text=None, speaker=n.parent.name, snr=band))
    return out


def items_pairs(a):
    clean = {Path(p).relative_to(a.clean_dir).with_suffix("").as_posix(): p
             for p in glob.glob(os.path.join(a.clean_dir, "**", "*.wav"), recursive=True)}
    out = []
    for p in sorted(glob.glob(os.path.join(a.noisy_dir, "**", "*.wav"), recursive=True)):
        rel = Path(p).relative_to(a.noisy_dir).with_suffix("").as_posix()
        c = clean.get(rel)
        if c is None:  # DNS synthetic style: noisy_..._fileid_N.wav <-> clean_fileid_N.wav
            fid = rel.rsplit("fileid_", 1)[-1]
            c = next((v for k, v in clean.items() if k.endswith(f"fileid_{fid}")), None) if "fileid_" in rel else None
        if c:
            out.append(dict(name=rel.replace("/", "__"), noisy=p, clean=c, text=None,
                            kind=rel.split("/")[0] if "/" in rel else "all"))
    return out


def items_snsd(a):
    """Mix MS-SNSD clean speech with its test noise (fixed seed); writes the mixtures itself."""
    work = Path(a.work, a.name)
    rng = np.random.default_rng(a.seed)
    clean_files = sorted(glob.glob(os.path.join(a.snsd_dir, "clean_test", "*.wav")))
    noise_files = sorted(glob.glob(os.path.join(a.snsd_dir, "noise_test", "*.wav")))
    by_type = defaultdict(list)
    for f in noise_files:
        by_type[Path(f).stem.rsplit("_", 1)[0]].append(f)
    picks = rng.choice(len(clean_files), size=a.clips_per_noise * len(by_type), replace=False)
    mix_dir, ref_dir = work / "noisy", work / "clean48"
    mix_dir.mkdir(parents=True, exist_ok=True); ref_dir.mkdir(parents=True, exist_ok=True)
    items, i = [], 0
    for ntype in sorted(by_type):
        for _ in range(a.clips_per_noise):
            clean = M.load(clean_files[picks[i]], SR); i += 1
            clean *= 10 ** (-25 / 20) / _rms(clean)  # MS-SNSD convention: -25 dBFS speech
            noise = M.load(by_type[ntype][rng.integers(len(by_type[ntype]))], SR)
            if len(noise) < len(clean):
                noise = np.tile(noise, int(np.ceil(len(clean) / len(noise))))
            off = rng.integers(0, len(noise) - len(clean) + 1)
            noise = noise[off:off + len(clean)]
            for snr in SNRS:
                name = f"{ntype}_{i:03d}_snr{snr:02d}"
                if not (ref_dir / f"{name}.wav").exists() or not (mix_dir / f"{name}.wav").exists():
                    mix = clean + noise * (_rms(clean) / _rms(noise)) / (10 ** (snr / 20))
                    peak = np.max(np.abs(mix))
                    scale = 0.95 / peak if peak > 0.95 else 1.0
                    _write(mix_dir / f"{name}.wav", mix * scale, SR)
                    _write(ref_dir / f"{name}.wav", clean * scale, SR, "FLOAT")
                items.append(dict(name=name, noisy=str(mix_dir / f"{name}.wav"),
                                  clean=str(ref_dir / f"{name}.wav"), text=None, noise=ntype, snr=snr))
    return items


def items_blind(a):
    root = Path(a.blind_dir)
    files = [Path(p) for p in sorted(glob.glob(os.path.join(root, "**", "*.wav"), recursive=True))]
    files = [f for f in files if not any("enrol" in part.lower() for part in f.relative_to(root).parts)]
    out = []
    for f in files:
        rel = f.relative_to(root)
        if len(rel.parts) > 1:  # e.g. DNS 5: Track1_Headset/noisy/x.wav
            kind = rel.parts[0]
            name = f"{kind}__{f.stem}"
        else:  # DNS 2020: noreverb_fileid_0.wav
            kind, name = f.stem.split("_")[0], f.stem
        out.append(dict(name=name, noisy=str(f), clean=None, text=None, kind=kind))
    return out


def items_noise(a):
    """Noise-only clips: `--clips-per-type` files of each noise type, `--seconds` long, at each
    of `--levels` dBFS RMS."""
    work = Path(a.work, a.name)
    (work / "noisy").mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(a.seed)
    by_type = defaultdict(list)
    for f in sorted(glob.glob(os.path.join(a.noise_dir, "**", "*.wav"), recursive=True)):
        by_type[Path(f).stem.rsplit("_", 1)[0]].append(f)
    items = []
    n_len = int(a.seconds * SR)
    for ntype, files in sorted(by_type.items()):
        for j, f in enumerate(rng.permutation(files)[: a.clips_per_type]):
            x = None
            for lvl in a.levels:
                name = f"{ntype}_{j}_{abs(lvl):02d}dB"
                dst = work / "noisy" / f"{name}.wav"
                if not dst.exists():
                    if x is None:
                        x = M.load(f, SR)
                        x = np.tile(x, int(np.ceil(n_len / len(x))))[:n_len]
                    y = x * (10 ** (lvl / 20) / _rms(x))
                    _write(dst, y * min(1.0, 0.95 / (np.max(np.abs(y)) + 1e-12)), SR)
                items.append(dict(name=name, noisy=str(dst), clean=None, text=None, noise=ntype, level=f"{lvl} dBFS"))
    return items


def prepare(work, items):
    (work / "noisy").mkdir(parents=True, exist_ok=True)
    (work / "clean48").mkdir(parents=True, exist_ok=True)
    for it in items:
        dst = work / "noisy" / f"{it['name']}.wav"
        if Path(it["noisy"]).resolve() != dst.resolve() and not dst.exists():
            _write(dst, M.load(it["noisy"], SR), SR)
        if it["clean"]:
            c = work / "clean48" / f"{it['name']}.wav"
            if Path(it["clean"]).resolve() != c.resolve() and not c.exists():
                _write(c, M.load(it["clean"], SR), SR, "FLOAT")
    with open(work / "items.json", "w") as f:
        json.dump(items, f, indent=1)


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


def audio_path(work, system, name):
    if system == "noisy":
        return work / "noisy" / f"{name}.wav"
    if system == "clean":
        return work / "clean48" / f"{name}.wav"
    return work / "out" / system / f"{name}.wav"


# ---------------------------------------------------------------- scoring with a cache

def cached(work, family, system, names, compute):
    """Scores for `names` from WORK/scores/<family>/<system>.json, computing the missing ones.
    The cache is dropped when any of the system's audio is newer than it."""
    path = work / "scores" / family / f"{system}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    cache = {}
    if path.exists():
        newest = max((audio_path(work, system, n).stat().st_mtime for n in names), default=0)
        if newest <= path.stat().st_mtime:
            cache = json.load(open(path))
    todo = [n for n in names if n not in cache]
    if todo:
        t0 = time.time()
        cache.update(compute(todo))
        with open(path, "w") as f:
            json.dump(cache, f)
        print(f"[score] {family:<6} {system:<20} {len(todo)} clips in {time.time() - t0:.0f}s", flush=True)
    return {n: cache[n] for n in names}


def score_all(a, work, items, systems, families):
    names = [it["name"] for it in items]
    has_clean = all(it["clean"] for it in items)
    rows = {}
    base = ["noisy"] + systems
    for fam in families:
        sys_list = (["clean"] + base) if has_clean and fam in ("dnsmos", "nn", "wer") else base
        for s in sys_list:
            if fam == "ref":
                def compute(todo, s=s):
                    tasks = [(s, n, str(work / "clean48" / f"{n}.wav"), str(audio_path(work, s, n)), a.fullband)
                             for n in todo]
                    with Pool(a.jobs) as pool:
                        return dict(zip(todo, pool.map(M.score_ref, tasks, chunksize=4)))
            elif fam == "noise":
                def compute(todo, s=s):
                    tasks = [(s, n, str(work / "noisy" / f"{n}.wav"), str(audio_path(work, s, n))) for n in todo]
                    with Pool(a.jobs) as pool:
                        return dict(zip(todo, pool.map(M.score_noise, tasks, chunksize=4)))
            elif fam == "dnsmos":
                def compute(todo, s=s):
                    p835, p808 = dnsmos_paths(a.dnsmos)
                    with Pool(a.jobs, initializer=M.dnsmos_init, initargs=(p835, p808)) as pool:
                        return dict(zip(todo, pool.map(M.score_dnsmos, [(s, n, str(audio_path(work, s, n))) for n in todo],
                                                       chunksize=4)))
            elif fam == "nn":
                def compute(todo, s=s):
                    nn = _model("nn", lambda: M.Neural(utmos=a.utmos))
                    return {n: nn.score(str(audio_path(work, s, n))) for n in todo}
            elif fam == "wer":
                refs = reference_texts(a, work, items)

                def compute(todo, s=s, refs=refs):
                    tr = _model("whisper", lambda: M.Transcriber(a.whisper_model))
                    out = {}
                    for n in todo:
                        hyp = tr.text(str(audio_path(work, s, n)))
                        out[n] = dict(text=hyp, **M.word_errors(refs[n], hyp))
                    return out
            fam_key = "nn_utmos" if fam == "nn" and a.utmos else fam
            for n, v in cached(work, fam_key, s, names, compute).items():
                rows.setdefault((s, n), {}).update({k: x for k, x in v.items() if k != "text"})
    meta = {it["name"]: it for it in items}
    out = []
    for (s, n), v in rows.items():
        r = dict(system=s, name=n)
        r.update({k: meta[n][k] for k in meta[n] if k not in ("name", "noisy", "clean", "text")})
        r.update(v)
        out.append(r)
    return out


_models = {}


def _model(key, make):
    if key not in _models:
        _models[key] = make()
    return _models[key]


def reference_texts(a, work, items):
    """The data set's transcript where it has one, else Whisper's transcript of the clean audio."""
    from whisper.normalizers import EnglishTextNormalizer
    norm = EnglishTextNormalizer()
    given = {it["name"]: norm(it["text"]) for it in items if it.get("text")}
    missing = [it["name"] for it in items if it["name"] not in given]
    if missing:
        tr = _model("whisper", lambda: M.Transcriber(a.whisper_model))
        got = cached(work, "transcript", "clean", missing, lambda todo: {n: tr.text(str(audio_path(work, "clean", n)))
                                                                        for n in todo})
        given.update(got)
    return given


def dnsmos_paths(p):
    p = Path(p)
    if p.is_dir():
        return p / "sig_bak_ovr.onnx", (p / "model_v8.onnx") if (p / "model_v8.onnx").exists() else None
    p808 = p.parent / "model_v8.onnx"
    return p, p808 if p808.exists() else None


def have_torch():
    try:
        import torch  # noqa: F401
        return True
    except ImportError:
        return False


def have_whisper():
    try:
        import whisper  # noqa: F401
        import jiwer  # noqa: F401
        return True
    except ImportError:
        return False


# ---------------------------------------------------------------- reporting

def report(a, work, rows, systems, families, groups):
    write_csv(Path(a.work, f"{a.name}_scores.csv"), rows)
    order = [s for s in ["clean", "noisy"] + systems if any(r["system"] == s for r in rows)]
    mets = [m for f in families for m in FAMILY_METRICS[f]]
    res = stats.compare(rows, order, mets, a.baseline if a.baseline in order else order[0])
    stats.print_compare(res, a.baseline)
    for g in groups:
        stats.print_groups(rows, order, mets, g)
    if res:
        write_csv(Path(a.work, f"{a.name}_summary.csv"), res)
    if "ref" in families:
        lags = defaultdict(list)
        for r in rows:
            if "lag" in r:
                lags[r["system"]].append(abs(r["lag"]))
        print("\nmedian |alignment lag| (16 kHz samples, should be 0):",
              {s: float(np.median(v)) for s, v in lags.items()})


def write_csv(path, rows):
    keys = []
    for r in rows:
        keys += [k for k in r if k not in keys]
    with open(path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=keys)
        w.writeheader()
        w.writerows(rows)


# ---------------------------------------------------------------- MUSHRA listening test

def export_mushra(a):
    """Write a webMUSHRA config + loudness-matched stimuli for `--trials` clips of a scored set."""
    import pyloudnorm as pyln
    work = Path(a.work, a.name)
    items = json.load(open(work / "items.json"))
    systems = a.systems.split(",")
    meter = pyln.Meter(SR)
    pool = [it for it in items if it["clean"] and 3 <= sf_seconds(work / "clean48" / f"{it['name']}.wav") <= 10
            and all(audio_path(work, s, it["name"]).exists() for s in systems)]
    random.Random(a.seed).shuffle(pool)
    if a.max_snr is not None:
        pool = [it for it in pool if isinstance(it.get("snr"), (int, float)) and it["snr"] <= a.max_snr] or pool
    pool = pool[: a.trials]
    out = Path(a.out)
    audio_root = Path("configs", "resources", "audio", f"vidar_{a.name}")
    pages = []
    for it in pool:
        n = it["name"]
        stims = {"reference": audio_path(work, "clean", n), "noisy": audio_path(work, "noisy", n)}
        stims.update({s: audio_path(work, s, n) for s in systems})
        sig = {k: M.load(str(p), SR) for k, p in stims.items()}
        length = min(len(x) for x in sig.values())
        d = out / audio_root / n
        d.mkdir(parents=True, exist_ok=True)
        for k, x in sig.items():
            x = x[:length].astype(np.float64)
            loud = meter.integrated_loudness(x)
            if np.isfinite(loud):
                x = pyln.normalize.loudness(x, loud, -26.0)
            _write(d / f"{k}.wav", x, SR)
        pages.append({
            "type": "mushra", "id": n, "name": "Rate each version against the reference",
            "content": "How close is each version to the reference? Noise, missing word parts and "
                       "a dull or robotic voice all count against it.",
            "showWaveform": False, "enableLooping": True, "randomize": True, "showConditionNames": False,
            "createAnchor35": True, "createAnchor70": False,
            "reference": (audio_root / n / "reference.wav").as_posix(),
            "stimuli": {k: (audio_root / n / f"{k}.wav").as_posix() for k in sig if k != "reference"},
        })
    first = (audio_root / pool[0]["name"] / "reference.wav").as_posix()
    cfg = {
        "testname": "Vidar noise suppression", "testId": f"vidar_{a.name}", "bufferSize": 2048,
        "stopOnErrors": True, "showButtonPreviousPage": True, "remoteService": "service/write.php",
        "pages": [
            {"type": "generic", "id": "intro", "name": "Welcome",
             "content": "You will hear short recordings cleaned in different ways. Use headphones in a quiet room."},
            {"type": "volume", "id": "volume", "name": "Volume", "content": "Set a comfortable level.",
             "stimulus": first, "defaultVolume": 0.5},
            ["random"] + pages,
            {"type": "finish", "name": "Thank you", "content": "Thank you for taking part.",
             "showResults": True, "writeResults": True,
             "questionnaire": [{"type": "text", "label": "Name or nickname", "name": "listener"}]},
        ],
    }
    (out / "configs").mkdir(parents=True, exist_ok=True)
    # JSON is valid YAML, and webMUSHRA reads configs with a YAML parser.
    with open(out / "configs" / f"vidar_{a.name}.yaml", "w") as f:
        json.dump(cfg, f, indent=2)
    print(f"[mushra] {len(pool)} trials -> {out / 'configs'}. Copy that folder into webMUSHRA and open "
          f"?config=vidar_{a.name}.yaml")


def sf_seconds(p):
    import soundfile as sf
    return sf.info(str(p)).duration


def mushra_results(a):
    """Post-screen listeners (ITU-R BS.1534-3: drop anyone who rates the hidden reference below
    90 in more than 15% of trials) and print each condition's mean with a 95% bootstrap CI."""
    rows = list(csv.DictReader(open(a.csv, newline="", encoding="utf-8")))
    who = "session_uuid" if "session_uuid" in rows[0] else "session_test_id"
    ref = defaultdict(list)
    for r in rows:
        if r["rating_stimulus"] == "reference":
            ref[r[who]].append(float(r["rating_score"]))
    keep = {l for l, v in ref.items() if np.mean(np.array(v) < 90) <= 0.15}
    print(f"listeners: {len(ref)}, kept after post-screening: {len(keep)}")
    by = defaultdict(list)
    for r in rows:
        if r[who] in keep:
            by[r["rating_stimulus"]].append(float(r["rating_score"]))
    rng = np.random.default_rng(0)
    for s, v in sorted(by.items(), key=lambda t: -np.mean(t[1])):
        v = np.array(v)
        boot = v[rng.integers(0, len(v), (stats.B, len(v)))].mean(1)
        lo, hi = np.percentile(boot, [2.5, 97.5])
        print(f"  {s:<20} {v.mean():5.1f} [{lo:5.1f}, {hi:5.1f}]  n={len(v)}")


# ---------------------------------------------------------------- CLI

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p_vbd = sub.add_parser("vbd"); p_vbd.add_argument("--vbd-dir", required=True)
    p_ears = sub.add_parser("ears"); p_ears.add_argument("--ears-dir", required=True,
                                                         help="EARS-WHAM_v2 (or its test folder)")
    p_snsd = sub.add_parser("snsd"); p_snsd.add_argument("--snsd-dir", required=True)
    p_snsd.add_argument("--clips-per-noise", type=int, default=8); p_snsd.add_argument("--seed", type=int, default=7)
    p_pairs = sub.add_parser("pairs"); p_pairs.add_argument("--clean-dir", required=True)
    p_pairs.add_argument("--noisy-dir", required=True)
    p_pairs.add_argument("--wideband", action="store_true", help="the clean audio has nothing above 8 kHz")
    p_blind = sub.add_parser("blind", aliases=["dns"]); p_blind.add_argument("--blind-dir", required=True)
    p_noise = sub.add_parser("noise"); p_noise.add_argument("--noise-dir", required=True)
    p_noise.add_argument("--levels", default="-50,-35,-20", help="dBFS RMS of the noise")
    p_noise.add_argument("--clips-per-type", type=int, default=3); p_noise.add_argument("--seconds", type=float, default=12)
    p_noise.add_argument("--seed", type=int, default=7)
    scored = (p_vbd, p_ears, p_snsd, p_pairs, p_blind, p_noise)
    defaults = {p_vbd: "vbd", p_ears: "ears", p_snsd: "snsd", p_pairs: "pairs", p_blind: "dns", p_noise: "noise"}
    for p in scored:
        p.add_argument("--work", required=True)
        p.add_argument("--name", default=defaults[p], help="subfolder of --work and prefix of the CSVs")
        p.add_argument("--systems", default="voice,voice_hd")
        p.add_argument("--jobs", type=int, default=os.cpu_count())
        p.add_argument("--cli", type=Path, default=DEFAULT_CLI, help="path to vidar-cli.exe")
        p.add_argument("--metrics", default="auto", help="comma list of ref,dnsmos,nn,wer (default: all available)")
        p.add_argument("--dnsmos", help="DNSMOS folder with sig_bak_ovr.onnx and model_v8.onnx (or the .onnx)")
        p.add_argument("--utmos", action="store_true", help="add UTMOS (runs code from github.com/tarepan/SpeechMOS)")
        p.add_argument("--whisper-model", default="turbo")
        p.add_argument("--baseline", default="noisy", help="system the others are tested against")
        p.add_argument("--limit", type=int, help="only the first N clips (quick checks)")
    p_mu = sub.add_parser("mushra"); p_mu.add_argument("--work", required=True)
    p_mu.add_argument("--name", default="vbd"); p_mu.add_argument("--systems", default="voice,voice_hd")
    p_mu.add_argument("--trials", type=int, default=12); p_mu.add_argument("--seed", type=int, default=1)
    p_mu.add_argument("--max-snr", type=float, help="only clips at or below this SNR (MS-SNSD)")
    p_mu.add_argument("--out", required=True, help="where to write configs/ for webMUSHRA")
    p_mr = sub.add_parser("mushra-results"); p_mr.add_argument("--csv", required=True)
    a = ap.parse_args()

    if a.cmd == "mushra":
        return export_mushra(a)
    if a.cmd == "mushra-results":
        return mushra_results(a)

    systems = a.systems.split(",")
    unknown = [s for s in systems if s not in SYSTEMS]
    if unknown:
        ap.error(f"unknown systems {unknown}; choose from {list(SYSTEMS)}")
    if not a.cli.is_file():
        ap.error(f"{a.cli} not found; build it with: cargo build --release -p vidar-cli")
    a.levels = [int(x) for x in a.levels.split(",")] if a.cmd == "noise" else None
    a.fullband = a.cmd in ("vbd", "ears") or (a.cmd == "pairs" and not a.wideband)

    loaders = {"vbd": items_vbd, "ears": items_ears, "snsd": items_snsd, "pairs": items_pairs,
               "blind": items_blind, "dns": items_blind, "noise": items_noise}
    groups = {"vbd": ["speaker"], "ears": ["snr", "speaker"], "snsd": ["snr", "noise"], "pairs": ["kind"],
              "blind": ["kind"], "dns": ["kind"], "noise": ["level", "noise"]}[a.cmd]
    with_ref = a.cmd in ("vbd", "ears", "snsd", "pairs")
    if a.cmd == "noise":
        possible = ["noise"]
    else:
        possible = (["ref"] if with_ref else []) + ["dnsmos", "nn"] + (["wer"] if with_ref else [])
    if a.metrics == "auto":
        families = [f for f in possible if f in ("ref", "noise")
                    or (f == "dnsmos" and a.dnsmos) or (f == "nn" and have_torch()) or (f == "wer" and have_whisper())]
    else:
        families = a.metrics.split(",")
        bad = [f for f in families if f not in possible]
        if bad:
            ap.error(f"metrics {bad} do not apply to {a.cmd}; choose from {possible}")
        if "dnsmos" in families and not a.dnsmos:
            ap.error("--dnsmos is needed for the dnsmos scores")
    print(f"[{a.name}] systems: {systems}; scores: {families}", flush=True)

    work = Path(a.work, a.name)
    work.mkdir(parents=True, exist_ok=True)
    items = loaders[a.cmd](a)[: a.limit]
    if not items:
        ap.error("no audio found")
    prepare(work, items)
    run_systems(a.cli, work / "noisy", work / "out", systems, a.jobs)
    rows = score_all(a, work, items, systems, families)
    report(a, work, rows, systems, families, groups)
    return 0


if __name__ == "__main__":
    sys.exit(main())
