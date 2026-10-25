#!/usr/bin/env python3
"""Builds the GitHub Pages site into docs/.

The site is the browser build of the engine, laid out exactly as the repository is, so the page's relative paths
(`../engine.conf`, `../levels/...`, `../assets/...`) resolve the same way on Pages as they do under `web/`:

    docs/index.html            redirects to web/index.html
    docs/web/                  the page (index.html, main.js) and the WebAssembly build (pkg/)
    docs/engine.conf           a copy of engine.conf
    docs/levels/<name>/        the one level the site includes (LEVEL)
    docs/assets/car/           the car model and its attribution
    docs/crates/.../specs,designs   the car specification and the dashboard design the config names
    docs/.nojekyll             stops Pages from running Jekyll, which would drop the pkg/ and levels/ folders

Run from anywhere: `python3 scripts/build_pages.py`. Needs the wasm32 target and wasm-bindgen-cli 0.2.129.
"""
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
# The one level the site includes (the smallest playable scene). Change it here and rebuild.
LEVEL = "saburi-alley-yazd"
WASM = ROOT / "target/wasm32-unknown-unknown/release/splat_web.wasm"


def copy_tree(src: Path, dst: Path) -> None:
    shutil.copytree(src, dst, dirs_exist_ok=True)


def main() -> None:
    subprocess.run(["cargo", "build", "-q", "--target", "wasm32-unknown-unknown", "--release"], cwd=ROOT / "crates/splat-web", check=True)

    # Start clean: only the generated parts, so nothing stale survives a rebuild.
    for name in ["web", "levels", "assets", "crates", "engine.conf", "index.html", ".nojekyll"]:
        path = DOCS / name
        if path.is_dir():
            shutil.rmtree(path)
        elif path.exists():
            path.unlink()
    DOCS.mkdir(exist_ok=True)

    subprocess.run(["wasm-bindgen", "--target", "web", "--out-dir", str(DOCS / "web/pkg"), str(WASM)], cwd=ROOT, check=True)
    (DOCS / "web").mkdir(exist_ok=True)
    for name in ["index.html", "main.js"]:
        shutil.copy2(ROOT / "web" / name, DOCS / "web" / name)

    config = (ROOT / "engine.conf").read_text()
    config, n = re.subn(r"^path = levels/[\w.-]+", f"path = levels/{LEVEL}", config, count=1, flags=re.M)
    assert n == 1, "engine.conf has no [level] path to point at the site's level"
    (DOCS / "engine.conf").write_text(config)
    copy_tree(ROOT / "assets/car", DOCS / "assets/car")
    copy_tree(ROOT / "levels" / LEVEL, DOCS / "levels" / LEVEL)
    copy_tree(ROOT / "crates/splat-car/specs", DOCS / "crates/splat-car/specs")
    copy_tree(ROOT / "crates/splat-hud/designs", DOCS / "crates/splat-hud/designs")

    (DOCS / "index.html").write_text(
        '<!doctype html><meta charset="utf-8"><title>Splat Racer</title>\n'
        '<meta http-equiv="refresh" content="0; url=web/index.html">\n'
        '<a href="web/index.html">Open Splat Racer</a>\n'
    )
    (DOCS / ".nojekyll").write_text("")
    size = sum(f.stat().st_size for f in DOCS.rglob("*") if f.is_file())
    print(f"docs/ built: {size / 1e6:.0f} MB in {sum(1 for f in DOCS.rglob('*') if f.is_file())} files")


if __name__ == "__main__":
    main()
