#!/usr/bin/env python3
# Run the panel UI in a normal browser against a scratch database:
#   cargo build -p copypasta && COPYPASTA_HOME=/tmp/cp-demo target/debug/copypasta demo
#   COPYPASTA_HOME=/tmp/cp-demo scripts/ui-dev-server.py crates/copypasta-app/ui target/debug/copypasta 8766
# then open http://localhost:8766/. Store calls go through `copypasta api`;
# restore/hide are stubbed (see window.__restores).
import http.server, json, os, subprocess, sys
UI, BIN, PORT = sys.argv[1], sys.argv[2], int(sys.argv[3])
MOCK = r"""
<script>
window.__restores = [];
window.__TAURI__ = {
  core: { invoke: async (cmd, args) => {
    if (cmd === "api") {
      const r = await fetch("/api", { method: "POST", body: JSON.stringify(args) });
      const t = await r.text();
      if (!r.ok) throw t;
      return JSON.parse(t);
    }
    if (cmd === "status") {
      const stats = await window.__TAURI__.core.invoke("api", { cmd: "stats", args: {} });
      return { paste_tracking: true, accessibility: true, stats };
    }
    if (cmd === "restore") { window.__restores.push(args); console.log("restore", JSON.stringify(args)); return null; }
    console.log("invoke", cmd, JSON.stringify(args)); return null;
  } },
  event: { listen: async () => () => {} },
};
</script>
"""
class H(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k): super().__init__(*a, directory=UI, **k)
    def log_message(self, *a): pass
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()
    def do_GET(self):
        if self.path.split("?")[0] in ("/", "/index.html"):
            html = open(os.path.join(UI, "index.html")).read().replace('<script src="dag.js">', MOCK + '<script src="dag.js">')
            b = html.encode(); self.send_response(200); self.send_header("Content-Type", "text/html"); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b); return
        return super().do_GET()
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        p = subprocess.run([BIN, "api", body["cmd"], json.dumps(body.get("args") or {})], capture_output=True, text=True)
        out = (p.stdout if p.returncode == 0 else p.stderr).encode()
        self.send_response(200 if p.returncode == 0 else 500); self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(out)
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
