#!/usr/bin/env python3
"""Fetch the exact MIT reference source files recorded in upstream-lock.json."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path, PurePosixPath
import time
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]


def blob_sha(data):
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()


def fetch_file(job):
    source, entry = job
    relative = PurePosixPath(entry["path"])
    if relative.is_absolute() or ".." in relative.parts:
        raise ValueError("Unsafe upstream path")
    target = ROOT / "third_party" / source["name"] / relative
    expected = entry["git_blob_sha"]
    if target.exists():
        if blob_sha(target.read_bytes()) == expected:
            return "cached"
        raise ValueError(f"Locally modified file; refusing to overwrite: {target}")
    url = f"https://raw.githubusercontent.com/{source['repository']}/{source['commit']}/{relative}"
    for attempt in range(3):
        try:
            request = Request(url, headers={"User-Agent": "splat-racer-research/1"})
            with urlopen(request, timeout=45) as response:
                data = response.read()
            if blob_sha(data) != expected:
                raise ValueError(f"Git blob hash mismatch: {url}")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            return "downloaded"
        except Exception:
            if attempt == 2:
                raise
            time.sleep(attempt + 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workers", type=int, default=8)
    args = parser.parse_args()
    if not 1 <= args.workers <= 16:
        parser.error("--workers must be between 1 and 16")
    lock = json.loads((ROOT / "upstream-lock.json").read_text())
    jobs = [(source, entry) for source in lock["sources"] for entry in source["files"]]
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        results = list(pool.map(fetch_file, jobs))
    print(f"Verified {len(results)} files: {results.count('downloaded')} downloaded, {results.count('cached')} cached.")


if __name__ == "__main__":
    main()
