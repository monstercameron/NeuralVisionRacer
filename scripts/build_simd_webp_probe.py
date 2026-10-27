"""Build isolated scalar/SIMD WebP diagnostics using the game's locked libraries."""
import os
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
CRATE = ROOT / "tools/simd-webp-probe"
OUT = ROOT / "logs/simd-webp-probe"
OUT.mkdir(parents=True, exist_ok=True)

lock = CRATE / "Cargo.lock"
if not lock.exists():
    # Start with the production lock so Cargo preserves its exact selections.
    shutil.copy2(ROOT / "crates/splat-web/Cargo.lock", lock)
    subprocess.run(["cargo", "metadata", "--offline", "--format-version=1", "--manifest-path", str(CRATE / "Cargo.toml")],
                   stdout=subprocess.DEVNULL, check=True)

def packages(path):
    result = set()
    for block in path.read_text().split("[[package]]")[1:]:
        fields = dict(re.findall(r'^(name|version|checksum) = "([^"]+)"$', block, re.M))
        result.add((fields["name"], fields["version"], fields.get("checksum")))
    return result

probe = packages(lock)
installed = packages(ROOT / "crates/splat-web/Cargo.lock")
different = sorted(p for p in probe if p[0] != "simd-webp-probe" and p not in installed)
if different:
    raise SystemExit(f"Probe dependencies differ from splat-web/Cargo.lock: {different}")

for name, feature in [("scalar", "-simd128"), ("simd", "+simd128")]:
    env = os.environ.copy()
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    env["RUSTFLAGS"] = f"-Ctarget-feature={feature}"
    subprocess.run(["cargo", "build", "--locked", "--offline", "--release", "--target", "wasm32-unknown-unknown",
                    "--manifest-path", str(CRATE / "Cargo.toml")], check=True, env=env)
    shutil.copy2(ROOT / "target/wasm32-unknown-unknown/release/simd_webp_probe.wasm", OUT / f"{name}.wasm")
print(f"Built lockfile-matched standalone WebP diagnostics in {OUT}")
