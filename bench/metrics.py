"""
Per-clip scores for bench/benchmark.py. Each family scores one (system, clip) pair and returns a
flat dict of numbers; benchmark.py caches them per family and system.

  ref     needs the clean reference. PESQ-WB, STOI, ESTOI (16 kHz); SI-SDR, log-spectral
          distance over the full band and above 8 kHz (48 kHz); how much the start and end of
          each word lose compared with the rest of the speech (the gate's footprint).
  dnsmos  no reference. DNSMOS P.835 (SIG/BAK/OVRL) and P.808 (MOS), Microsoft's ONNX models.
  nn      no reference, GPU if present. UTMOS22 (strong), NISQA v2 (full band, with its
          noisiness/discontinuity/coloration dimensions), TorchAudio SQUIM objective
          (estimated PESQ/STOI/SI-SDR).
  wer     Whisper large-v3-turbo transcripts against a reference text (the data set's own
          transcript, or Whisper's transcript of the clean reference).

Metrics where lower is better are listed in LOWER_BETTER.
"""

import numpy as np

SR = 48000
LOWER_BETTER = {
    "lsd",
    "lsd_hf",
    "onset_loss",
    "offset_loss",
    "onset_clip",
    "wer",
    "lag",
    "out_db",
    "open_pct",
    "pump_db",
}
# Corpus-level ratios: summed numerators over summed denominators, not a mean of per-clip values.
RATIOS = {"wer": ("wer_err", "wer_n")}
LABELS = {
    "pesq": "PESQ-WB",
    "stoi": "STOI",
    "estoi": "ESTOI",
    "sisdr": "SI-SDR dB",
    "lsd": "LSD dB",
    "lsd_hf": "LSD >8k dB",
    "onset_loss": "onset loss dB",
    "offset_loss": "tail loss dB",
    "onset_clip": "onsets cut",
    "sig": "DNSMOS SIG",
    "bak": "DNSMOS BAK",
    "ovrl": "DNSMOS OVRL",
    "p808": "DNSMOS P.808",
    "utmos": "UTMOS",
    "nisqa": "NISQA",
    "nisqa_noi": "NISQA noise",
    "nisqa_dis": "NISQA discont.",
    "nisqa_col": "NISQA color.",
    "squim_pesq": "SQUIM PESQ",
    "squim_stoi": "SQUIM STOI",
    "squim_sisdr": "SQUIM SI-SDR",
    "wer": "WER",
    "out_db": "residual dBFS",
    "atten_db": "attenuation dB",
    "open_pct": "open frames",
    "pump_db": "level wobble dB",
}


def load(path, sr_out):
    import soundfile as sf
    from scipy.signal import resample_poly

    x, sr = sf.read(path, dtype="float32", always_2d=True)
    x = x.mean(axis=1)
    if sr != sr_out:
        g = np.gcd(sr, sr_out)
        x = resample_poly(x, sr_out // g, sr // g).astype(np.float32)
    return x


def _db(p):
    return 10 * np.log10(np.maximum(p, 1e-12))


# ---------------------------------------------------------------- with a clean reference


def si_sdr(ref, est):
    ref = ref.astype(np.float64) - ref.mean()
    est = est.astype(np.float64) - est.mean()
    a = np.dot(est, ref) / (np.dot(ref, ref) + 1e-12)
    t = a * ref
    return float(10 * np.log10(np.dot(t, t) / (np.dot(est - t, est - t) + 1e-12) + 1e-12))


def lsd(ref, est, sr, fullband):
    """Log-spectral distance (dB) on frames where the reference speaks, after matching the
    output's overall gain to the reference. Returns (whole band, above 8 kHz or nan)."""
    from scipy.signal import stft

    est = est * (np.dot(ref, est) / (np.dot(est, est) + 1e-12))
    f, _, R = stft(ref, sr, nperseg=2048, noverlap=1536)
    _, _, E = stft(est, sr, nperseg=2048, noverlap=1536)
    pr, pe = np.abs(R) ** 2, np.abs(E) ** 2
    floor = pr.max() * 1e-10
    lr, le = _db(pr + floor), _db(pe + floor)
    fe = _db(pr.sum(0))
    act = fe > fe.max() - 40
    top = sr / 2 if fullband else 8000
    band = f <= top
    d = (lr - le)[:, act] ** 2
    full = float(np.mean(np.sqrt(d[band].mean(0))))
    hf = float(np.mean(np.sqrt(d[(f > 8000) & band].mean(0)))) if fullband else float("nan")
    return full, hf


def word_edges(ref, est, sr):
    """Energy lost at word starts and ends, relative to the rest of the speech.

    Speech activity comes from the clean reference in 10 ms frames. A word start is an active
    frame after at least 100 ms of pause; its window is the first 50 ms. A word end is the last
    50 ms before such a pause. Loss = (output/reference level ratio over all speech, median)
    minus (the same ratio in the window), in dB: 0 means the edge is kept as well as the middle
    of the word, positive means the edge is quieter, as when a gate opens late or closes early.
    """
    hop = sr // 100
    n = min(len(ref), len(est)) // hop
    if n < 30:
        return float("nan"), float("nan"), float("nan")
    er = (ref[: n * hop].astype(np.float64).reshape(n, hop) ** 2).sum(1)
    ee = (est[: n * hop].astype(np.float64).reshape(n, hop) ** 2).sum(1)
    lr = _db(er)
    # Speech: within 30 dB of the loud frames and at least 15 dB above the reference's own noise
    # floor (VoiceBank's "clean" takes have one, only ~25 dB down for some speakers; a model
    # that removes it must not count as cutting words).
    act = lr > max(np.percentile(lr, 95) - 30, np.percentile(lr, 10) + 15)
    # Only runs of at least 50 ms count as speech.
    edges = np.flatnonzero(np.diff(np.r_[0, act.astype(np.int8), 0]))
    for s, e in zip(edges[::2], edges[1::2]):
        if e - s < 5:
            act[s:e] = False
    if not act.any():
        return float("nan"), float("nan"), float("nan")
    gain = np.median(_db(ee[act]) - lr[act])
    on, off = [], []
    gap = 10
    for i in range(gap, n - 5):
        if act[i] and not act[i - gap : i].any():
            w = slice(i, i + 5)
            on.append(gain - (_db(ee[w].sum()) - _db(er[w].sum())))
    for i in range(5, n - gap):
        if act[i - 1] and not act[i : i + gap].any():
            w = slice(i - 5, i)
            off.append(gain - (_db(ee[w].sum()) - _db(er[w].sum())))
    nan = float("nan")
    return (
        float(np.mean(on)) if on else nan,
        float(np.mean(off)) if off else nan,
        float(np.mean(np.array(on) > 6)) if on else nan,
    )


def score_ref(args):
    """(system, name, clean_path, est_path, fullband) -> dict."""
    system, name, ref_path, est_path, fullband = args
    from pesq import pesq
    from pystoi import stoi

    ref48, est48 = load(ref_path, SR), load(est_path, SR)
    n = min(len(ref48), len(est48))
    ref48, est48 = ref48[:n], est48[:n]
    ref, est = load_16(ref48), load_16(est48)
    m = min(len(ref), len(est))
    ref, est = ref[:m], est[:m]
    # Alignment check: vidar-cli trims its own latency, so the best lag must be ~0.
    seg = slice(1600, m - 1600)
    lag = max(range(-480, 481, 4), key=lambda l: float(np.dot(ref[seg], np.roll(est, -l)[seg])))
    try:
        p = pesq(16000, ref, est, "wb")
    except Exception:
        p = float("nan")
    full, hf = lsd(ref48, est48, SR, fullband)
    on, off, clip = word_edges(ref48, est48, SR)
    return dict(
        pesq=p,
        stoi=stoi(ref, est, 16000, extended=False),
        estoi=stoi(ref, est, 16000, extended=True),
        sisdr=si_sdr(ref48, est48),
        lsd=full,
        lsd_hf=hf,
        onset_loss=on,
        offset_loss=off,
        onset_clip=clip,
        lag=lag,
    )


def load_16(x48):
    from scipy.signal import resample_poly

    return resample_poly(x48, 1, 3).astype(np.float32)


# ---------------------------------------------------------------- DNSMOS (CPU, ONNX)


class DNSMOS:
    """DNSMOS P.835 (sig_bak_ovr.onnx) and P.808 (model_v8.onnx), as Microsoft's dnsmos_local.py."""

    LEN = 144160  # 9.01 s at 16 kHz

    def __init__(self, p835, p808=None):
        import onnxruntime as ort

        o = ort.SessionOptions()
        o.intra_op_num_threads = 1
        o.inter_op_num_threads = 1
        self.sess = ort.InferenceSession(str(p835), o, providers=["CPUExecutionProvider"])
        self.p808 = ort.InferenceSession(str(p808), o, providers=["CPUExecutionProvider"]) if p808 else None

    @staticmethod
    def _mel(seg):
        import librosa

        m = librosa.feature.melspectrogram(y=seg, sr=16000, n_fft=321, hop_length=160, n_mels=120)
        return ((librosa.power_to_db(m, ref=np.max) + 40) / 40).T

    def score(self, x16):
        while len(x16) < self.LEN:
            x16 = np.concatenate([x16, x16])
        n_seg = int(np.floor(len(x16) / 16000) - 9.01) + 1
        res = []
        for i in range(n_seg):
            seg = x16[i * 16000 : i * 16000 + self.LEN]
            if len(seg) < self.LEN:
                continue
            sig, bak, ovr = self.sess.run(None, {"input_1": seg[None, :].astype(np.float32)})[0][0]
            p808 = (
                self.p808.run(None, {"input_1": self._mel(seg[:-160]).astype(np.float32)[None]})[0][0][0]
                if self.p808
                else np.nan
            )
            res.append(
                (
                    np.poly1d([-0.08397278, 1.22083953, 0.0052439])(sig),
                    np.poly1d([-0.13166888, 1.60915514, -0.39604546])(bak),
                    np.poly1d([-0.06766283, 1.11546468, 0.04602535])(ovr),
                    p808,
                )
            )
        sig, bak, ovr, p808 = (float(v) for v in np.mean(res, axis=0))
        return dict(sig=sig, bak=bak, ovrl=ovr, p808=p808)


_dnsmos = None


def dnsmos_init(p835, p808):
    global _dnsmos
    _dnsmos = DNSMOS(p835, p808)


def score_dnsmos(args):
    system, name, path = args
    return _dnsmos.score(load(path, 16000))


# ---------------------------------------------------------------- neural predictors (torch)


class Neural:
    """NISQA v2, SQUIM and (opt-in) UTMOS22 on one device. Weights download on first use.

    UTMOS loads through torch.hub, which runs code from the pinned GitHub repo
    tarepan/SpeechMOS (v1.2.0), so it is only used when asked for (--utmos)."""

    def __init__(self, utmos=False):
        import torch

        self.torch = torch
        self.dev = "cuda" if torch.cuda.is_available() else "cpu"
        self.utmos = (
            torch.hub.load("tarepan/SpeechMOS:v1.2.0", "utmos22_strong", trust_repo=True).to(self.dev).eval()
            if utmos
            else None
        )
        import torchaudio

        self.squim = torchaudio.pipelines.SQUIM_OBJECTIVE.get_model().to(self.dev).eval()
        from torchmetrics.functional.audio.nisqa import non_intrusive_speech_quality_assessment

        self.nisqa = non_intrusive_speech_quality_assessment

    def score(self, path):
        t = self.torch
        x48 = load(path, SR)
        x16 = load_16(x48)
        with t.inference_mode():
            w16 = t.from_numpy(x16)[None].to(self.dev)
            u = float(self.utmos(w16, 16000)[0]) if self.utmos is not None else float("nan")
            # SQUIM was trained on clips up to ~10 s; average 10 s windows.
            sq = [self.squim(w16[:, i : i + 160000]) for i in range(0, max(1, w16.shape[1] - 16000), 160000)]
            st, pe, sd = (float(t.stack([s[k][0] for s in sq]).mean()) for k in range(3))
            nq = self.nisqa(t.from_numpy(x48), SR).cpu().numpy()
        out = dict(
            nisqa=float(nq[0]),
            nisqa_noi=float(nq[1]),
            nisqa_dis=float(nq[2]),
            nisqa_col=float(nq[3]),
            squim_pesq=pe,
            squim_stoi=st,
            squim_sisdr=sd,
        )
        if self.utmos is not None:
            out["utmos"] = u
        return out


# ---------------------------------------------------------------- Whisper word errors


class Transcriber:
    def __init__(self, model="turbo"):
        import torch
        import whisper
        from whisper.normalizers import EnglishTextNormalizer

        self.model = whisper.load_model(model, device="cuda" if torch.cuda.is_available() else "cpu")
        self.norm = EnglishTextNormalizer()
        self.fp16 = torch.cuda.is_available()

    def text(self, path):
        r = self.model.transcribe(
            load(path, 16000),
            language="en",
            temperature=0.0,
            fp16=self.fp16,
            condition_on_previous_text=False,
            without_timestamps=True,
        )
        return self.norm(r["text"])


def word_errors(ref_text, hyp_text):
    import jiwer

    ref, hyp = ref_text.split(), hyp_text.split()
    if not ref:
        return dict(wer_err=float("nan"), wer_n=0, wer=float("nan"))
    o = jiwer.process_words(" ".join(ref), " ".join(hyp) or "<empty>")
    err = o.substitutions + o.deletions + o.insertions
    return dict(wer_err=err, wer_n=len(ref), wer=err / len(ref))


# ---------------------------------------------------------------- noise-only input (gate)


def score_noise(args):
    """Noise with no speech: what is left, how often it passes, and how much its level wobbles."""
    system, name, in_path, out_path = args
    x, y = load(in_path, SR), load(out_path, SR)
    n = min(len(x), len(y))
    x, y = x[SR:n], y[SR:n]  # skip the first second while the gate and models settle
    hop = SR // 100
    m = len(x) // hop
    ex = _db((x[: m * hop].astype(np.float64).reshape(m, hop) ** 2).mean(1))
    ey = np.maximum(_db((y[: m * hop].astype(np.float64).reshape(m, hop) ** 2).mean(1)), -100)
    in_db = _db(np.mean(x.astype(np.float64) ** 2))
    out_db = max(_db(np.mean(y.astype(np.float64) ** 2)), -100.0)
    # 50 ms level track for the wobble (pumping) figure.
    k = m // 5
    ey50 = np.maximum(_db((y[: k * 5 * hop].astype(np.float64).reshape(k, 5 * hop) ** 2).mean(1)), -100)
    return dict(
        out_db=float(out_db),
        atten_db=float(in_db - out_db),
        open_pct=float(np.mean(ex - ey < 6)),
        pump_db=float(np.std(ey50)),
    )
