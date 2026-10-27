"""Build separate scalar/SIMD diagnostic modules, without changing the game build."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "logs/simd-decode-labels"
OUT.mkdir(parents=True, exist_ok=True)
common = ["rustc", "--edition=2021", "--target=wasm32-unknown-unknown", "--crate-type=cdylib",
          "-C", "opt-level=3", "-C", "lto=fat", "-C", "panic=abort", str(ROOT / "scripts/simd_decode_labels.rs")]
for name, extra in [("scalar", []), ("simd", ["--cfg", "simd_variant"])]:
    subprocess.run([*common, *extra, "-o", str(OUT / f"{name}.wasm")], check=True)
print(f"Built scalar and SIMD diagnostic modules in {OUT}")
