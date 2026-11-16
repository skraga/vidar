"""
Download the public test sets used by bench/benchmark.py.

  vbd        VoiceBank+DEMAND test set, 48 kHz (824 clean/noisy pairs + transcripts), 325 MB.
  dns5       DNS Challenge 5 (ICASSP 2023) blind test set, real 48 kHz recordings, 3.9 GB.
  ears-wham  EARS-WHAM v2 test set, 48 kHz studio speech + real noise. Downloads only the six
             EARS test speakers (3.5 GB) and the 160 WHAM! test-split noise files (7.5 GB,
             fetched out of the 73 GB archive with HTTP range requests), then runs the official
             generator (pinned commit) for the test split only. Needs torch + torchaudio.

  python bench/fetch.py vbd --dest ../bench-data/VBD
  python bench/fetch.py dns5 --dest ../bench-data/DNS5
  python bench/fetch.py ears-wham --dest ../bench-data/EARS
"""
import argparse
import csv
import io
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

VBD = "https://datashare.ed.ac.uk/server/api/core/bitstreams/{}/content"
VBD_FILES = {
    "clean_testset_wav.zip": "dec213d3-bf57-4777-9663-c24bdce92d5e",
    "noisy_testset_wav.zip": "13c1bfbf-14a6-41db-9b41-8f7310f01ad5",
    "testset_txt.zip": "09e12592-879e-4749-bf6d-e34cf0d1ddf6",
}
DNS5 = "https://dnschallengepublic.blob.core.windows.net/dns5archive/V5_BlindTestSet.zip"
EARS = "https://github.com/facebookresearch/ears_dataset/releases/download/dataset/{}.zip"
EARS_TEST_SPEAKERS = ["p102", "p103", "p104", "p105", "p106", "p107"]
WHAM48 = "https://my-bucket-a8b4b49c25c811ee9a7e8bba05fa24c7.s3.amazonaws.com/high_res_wham.zip"
EARS_BENCH = "https://raw.githubusercontent.com/sp-uhh/ears_benchmark/36dc8a88cb2ebf7cc746b51cf2b876bb570bc3e0/{}"


def download(url, dst):
    dst = Path(dst)
    if dst.exists():
        return dst
    tmp = dst.with_suffix(dst.suffix + ".part")
    print(f"[get] {url}", flush=True)
    with urllib.request.urlopen(url) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f, 1 << 20)
    tmp.replace(dst)
    return dst


def unzip(path, dest):
    with zipfile.ZipFile(path) as z:
        z.extractall(dest)


def fetch_vbd(dest):
    for name, bid in VBD_FILES.items():
        unzip(download(VBD.format(bid), dest / name), dest)
    print(f"[ok] {len(list((dest / 'clean_testset_wav').glob('*.wav')))} clean test clips in {dest}")


def fetch_dns5(dest):
    unzip(download(DNS5, dest / "V5_BlindTestSet.zip"), dest)
    print(f"[ok] {dest / 'V5_BlindTestSet_mono'} (Track1_Headset, Track2_Speakerphone)")


def fetch_ears_wham(dest):
    from remotezip import RemoteZip
    from tqdm import tqdm
    for spk in EARS_TEST_SPEAKERS:
        if not (dest / "EARS" / spk).is_dir():
            (dest / "EARS").mkdir(parents=True, exist_ok=True)
            z = download(EARS.format(spk), dest / "EARS" / f"{spk}.zip")
            unzip(z, dest / "EARS")
            z.unlink()
    wham = dest / "WHAM48kHz" / "high_res_wham"
    (wham / "audio").mkdir(parents=True, exist_ok=True)
    with RemoteZip(WHAM48) as z:
        meta = wham / "high_res_metadata.csv"
        if not meta.exists():
            meta.write_bytes(z.read("high_res_wham/high_res_metadata.csv"))
        rows = list(csv.DictReader(io.StringIO(meta.read_text("utf-8"))))
        test = [r["Filename"] for r in rows if r["WHAM! Split"].lower() == "test"]
        for name in tqdm([n for n in test if not (wham / "audio" / n).exists()], desc="WHAM! test noise"):
            tmp = wham / "audio" / (name + ".part")
            with z.open(f"high_res_wham/audio/{name}") as src, open(tmp, "wb") as f:
                shutil.copyfileobj(src, f, 1 << 20)
            tmp.replace(wham / "audio" / name)
    if (dest / "EARS-WHAM_v2").exists():
        print(f"[ok] {dest / 'EARS-WHAM_v2'} exists")
        return
    gen = dest / "_gen"
    gen.mkdir(exist_ok=True)
    for f in ("generate_ears_wham.py", "test_files.json"):
        download(EARS_BENCH.format(f), gen / f)
    # The generator splits paths on "/", so run it with POSIX path joins (Windows accepts "/").
    # Its train/valid loops find no speakers here and do nothing; the test split re-seeds
    # numpy itself, so the output matches the official test set.
    boot = ("import os, posixpath, runpy, sys; os.path.join = posixpath.join; "
            "sys.argv = ['generate_ears_wham.py', '--data_dir', sys.argv[1]]; "
            "runpy.run_path('generate_ears_wham.py', run_name='__main__')")
    subprocess.run([sys.executable, "-c", boot, dest.resolve().as_posix()], cwd=gen, check=True)
    print(f"[ok] {dest / 'EARS-WHAM_v2' / 'test'}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("set", choices=["vbd", "dns5", "ears-wham"])
    ap.add_argument("--dest", type=Path, required=True)
    a = ap.parse_args()
    a.dest.mkdir(parents=True, exist_ok=True)
    {"vbd": fetch_vbd, "dns5": fetch_dns5, "ears-wham": fetch_ears_wham}[a.set](a.dest)


if __name__ == "__main__":
    main()
