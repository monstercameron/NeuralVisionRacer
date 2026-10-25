# Browser page

The page that runs the game in a browser: `index.html` (the layout and styles) and `main.js` (loads the assets, handles
input, runs the pause menu and the level selector, and drives the WebAssembly engine).

The WebAssembly module is generated into `web/pkg/` (ignored by git). Build it as described in the
[README](../README.md#running-in-the-browser), then serve with `python3 scripts/serve_web.py --port 8800` from the
repository root.
