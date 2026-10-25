"""Serves the browser build of Splat Racer.

usage: python3 scripts/serve_web.py [--port 8800] [--root .]

WebGPU only works in a secure context. `http://localhost` counts as secure; `http://192.168.x.x` does not, so a
browser on this Mac that opens the network address gets no WebGPU. This server redirects requests that come from
this machine but name a network address to `http://localhost:<port>` with the same path, so either address works here.

It also sends `Cache-Control: no-store`, so a reload always picks up the latest build.

Other machines on the network still need a secure origin (HTTPS) for WebGPU; the page says so when that happens.
"""

import argparse
import http.server
import os
import socketserver

LOCAL_HOSTS = ("localhost", "127.0.0.1", "[::1]")


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def _is_this_machine(self):
        client = self.client_address[0]
        local = self.connection.getsockname()[0]
        return client in ("127.0.0.1", "::1") or client == local

    def _needs_localhost(self):
        """Only page navigations are redirected. Scripts, wasm and data are served where they were asked for, because a
        cross-origin redirect would block a module script and leave the page stuck."""
        host = (self.headers.get("Host") or "").split(":")[0]
        dest = self.headers.get("Sec-Fetch-Dest")
        path = self.path.split("?")[0]
        is_page = dest == "document" if dest else (path.endswith("/") or path.endswith(".html"))
        return is_page and self._is_this_machine() and host not in LOCAL_HOSTS

    def do_GET(self):
        if self.path.split("?")[0] == "/__splat/is-local":
            # Lets a page loaded from a cached copy ask whether it runs on this machine, so it can move to localhost.
            body = b'{"local": true}' if self._is_this_machine() else b'{"local": false}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self._needs_localhost():
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{self.server.server_address[1]}{self.path}")
            self.end_headers()
            return
        super().do_GET()

    def do_HEAD(self):
        if self._needs_localhost():
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{self.server.server_address[1]}{self.path}")
            self.end_headers()
            return
        super().do_HEAD()


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8800)
    parser.add_argument("--root", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    args = parser.parse_args()
    root = os.path.abspath(args.root)
    os.chdir(root)
    with Server(("0.0.0.0", args.port), Handler) as server:
        print(f"serving {root} on port {args.port}; open http://localhost:{args.port}/web/index.html", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
