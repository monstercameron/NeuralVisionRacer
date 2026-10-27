#!/usr/bin/env python3
"""Retain the current compiled browser build and post shader for paired WebGPU checks.

Build web/pkg first. Run from anywhere; use --replace only to intentionally start a new comparison.
"""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--replace', action='store_true')
    parser.add_argument('--name', default='browser-benchmark', help='Snapshot directory name under logs/')
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,63}', args.name):
        parser.error('--name must be a short lowercase directory name, not a path')
    out = ROOT / 'logs' / args.name
    package = ROOT / 'web/pkg'
    if not (package / 'splat_web_bg.wasm').is_file():
        parser.error('Build web/pkg first; see README browser build instructions.')
    if out.exists():
        if not args.replace:
            parser.error(f'{out} already exists; use --replace to start a new comparison.')
        shutil.rmtree(out)
    out.mkdir(parents=True)
    shutil.copytree(package, out / 'baseline-pkg')
    shutil.copy2(ROOT / 'crates/splat-render/src/shaders/post.wgsl', out / 'post-baseline.wgsl')
    shutil.copy2(ROOT / 'crates/splat-render/src/shaders/dof.wgsl', out / 'dof.wgsl')
    files = [p for p in out.rglob('*') if p.is_file()]
    manifest = {
        'created_utc': datetime.now(timezone.utc).isoformat(),
        'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'working_tree': subprocess.check_output(['git', 'status', '--short'], cwd=ROOT, text=True).splitlines(),
        'sha256': {str(p.relative_to(out)): hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
        'note': 'Snapshot of existing web/pkg; build it from the intended sources before running this script.',
    }
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Browser comparison baseline saved to {out}')


if __name__ == '__main__':
    main()
