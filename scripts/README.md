# Scripts

| Script | What it does |
| --- | --- |
| `serve_web.py` | Serves the browser build from the repository root, so the page and the levels are reachable. Run from the repository root: `python3 scripts/serve_web.py --port 8800`. |
| `fetch_upstream.py` | Downloads, or checks, the exact upstream reference files listed in `third_party/upstream-lock.json`. Refuses to overwrite modified files. |
| `download_supersplat.py` | Downloads a splat asset from a SuperSplat scene URL. Run `--help` for options. |
| `make_test_hdri.py` | Writes a synthetic equirectangular HDRI for testing the environment lighting. |

All of these run with Python 3.
