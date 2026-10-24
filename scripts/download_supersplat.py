#!/usr/bin/env python3
"""Download a PLY/SOG using the asset link from a SuperSplat scene's Download dialog.

Uses explicit download links rather than guessing private/legacy CDN paths.
Records source, creator, license, credit, and SHA-256 beside the downloaded file.
Python 3.9+; standard library only. Does not resolve share pages or streamed SOG.
"""
import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
from urllib.parse import urlsplit, urlunsplit
from urllib.request import Request, urlopen
import zipfile


def copy_limited(source, target, limit):
    total = 0
    digest = hashlib.sha256()
    while True:
        block = source.read(min(1024 * 1024, limit - total + 1))
        if not block:
            return total, digest.hexdigest()
        total += len(block)
        if total > limit:
            raise ValueError(f"Asset exceeds size limit ({limit} bytes)")
        digest.update(block)
        target.write(block)


def validate(path, suffix):
    if suffix == ".ply":
        with path.open("rb") as stream:
            if stream.readline(16).strip() != b"ply":
                raise ValueError("Response is not a PLY file (possibly a share/login page)")
    elif suffix == ".sog":
        with zipfile.ZipFile(path) as archive:
            info = archive.getinfo("meta.json")
            if info.file_size > 1024 * 1024:
                raise ValueError("SOG metadata exceeds 1 MiB")
            meta = json.loads(archive.read(info))
            if "version" not in meta or "count" not in meta:
                raise ValueError("SOG meta.json is missing version/count")
    else:
        raise ValueError("Output must end in .ply or .sog")


def public_url(url):
    parts = urlsplit(url)
    # Signed query strings may contain temporary credentials.
    return urlunsplit((parts.scheme, parts.netloc, parts.path, "", ""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--asset-url", required=True, help="Explicit HTTPS PLY/SOG download URL")
    parser.add_argument("--scene-url", required=True, help="Original superspl.at scene URL")
    parser.add_argument("--creator", required=True)
    parser.add_argument("--license-url", required=True, help="License shown in the Download dialog")
    parser.add_argument("--credit", required=True, help="Credit line supplied by the creator/dialog")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-mib", type=int, default=1024)
    args = parser.parse_args()
    if urlsplit(args.asset_url).scheme != "https":
        parser.error("--asset-url must use HTTPS")
    scene = urlsplit(args.scene_url)
    if scene.scheme != "https" or scene.hostname not in {"superspl.at", "www.superspl.at"}:
        parser.error("--scene-url must be an HTTPS superspl.at URL")
    if args.max_mib <= 0:
        parser.error("--max-mib must be positive")
    output = args.output.resolve()
    if output.suffix.lower() not in {".ply", ".sog"}:
        parser.error("--output must end in .ply or .sog")
    sidecar = output.with_name(output.name + ".source.json")
    if output.exists() or sidecar.exists():
        parser.error("Output or provenance file already exists; choose a new name")
    output.parent.mkdir(parents=True, exist_ok=True)
    limit = args.max_mib * 1024 * 1024
    with tempfile.TemporaryDirectory(prefix="splat-download-", dir=output.parent) as tmp:
        raw = Path(tmp) / "download"
        request = Request(args.asset_url, headers={"User-Agent": "splat-racer-research/1"})
        with urlopen(request, timeout=60) as response, raw.open("wb") as target:
            if urlsplit(response.geturl()).scheme != "https":
                raise ValueError("Download redirected to a non-HTTPS URL")
            resolved_url = public_url(response.geturl())
            copy_limited(response, target, limit)
        with raw.open("rb") as source:
            is_gzip = source.read(2) == b"\x1f\x8b"
        payload = Path(tmp) / "payload"
        if is_gzip:
            with gzip.open(raw, "rb") as source, payload.open("wb") as target:
                size, digest = copy_limited(source, target, limit)
        else:
            raw.rename(payload)
            with payload.open("rb") as source:
                size, digest = copy_limited(source, _Discard(), limit)
        validate(payload, output.suffix.lower())
        provenance = {
            "scene_url": args.scene_url,
            "asset_url_without_query": public_url(args.asset_url),
            "resolved_url_without_query": resolved_url,
            "creator": args.creator,
            "license_url": args.license_url,
            "credit": args.credit,
            "downloaded_at_utc": datetime.now(timezone.utc).isoformat(),
            "gzip_decoded": is_gzip,
            "bytes": size,
            "sha256": digest,
        }
        with output.open("xb") as target, payload.open("rb") as source:
            shutil.copyfileobj(source, target)
        with sidecar.open("x") as target:
            json.dump(provenance, target, indent=2)
            target.write("\n")
    print(f"Saved {output} ({size} bytes); SHA-256 {digest}")


class _Discard:
    def write(self, data):
        return len(data)


if __name__ == "__main__":
    main()
