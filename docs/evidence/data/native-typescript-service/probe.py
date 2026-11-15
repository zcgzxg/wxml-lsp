import json
import os
import pathlib
import queue
import subprocess
import tempfile
import threading
import time

# Prerequisites: point PROBE_TSC at a TypeScript native binary. PROBE_ROOT
# defaults to a scratch directory under the system temp directory so fixtures
# and output are not written into the repository.
HERE = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(
    os.environ.get(
        "PROBE_ROOT", pathlib.Path(tempfile.gettempdir()) / "wxml-ts-lsp-probe"
    )
)
BIN = pathlib.Path(os.environ.get("PROBE_TSC", str(HERE / "package/lib/tsc")))
MODEL = "export interface User {\n  /** Display name from the real model. */\n  Name: string;\n}\n"
VIRTUAL = "import type { User } from './model';\ndeclare const user: User;\nuser.Name;\nuser.DoesNotExist;\n"


class Client:
    def __init__(self, cwd):
        self.proc = subprocess.Popen(
            [str(BIN), "--lsp", "--stdio"],
            cwd=cwd,
            env={**os.environ, "PATH": "/nonexistent"},
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.messages = queue.Queue()
        self.id = 0
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.proc.stdout.readline()
                    if not line:
                        return
                    if line in (b"\r\n", b"\n"):
                        break
                    name, val = line.decode().split(":", 1)
                    headers[name.lower()] = val.strip()
                self.messages.put(
                    json.loads(self.proc.stdout.read(int(headers["content-length"])))
                )
        except Exception as exc:
            self.messages.put({"reader_error": str(exc)})

    def send(self, obj):
        data = json.dumps({"jsonrpc": "2.0", **obj}).encode()
        self.proc.stdin.write(f"Content-Length: {len(data)}\r\n\r\n".encode() + data)
        self.proc.stdin.flush()

    def notify(self, method, params):
        self.send({"method": method, "params": params})

    def request(self, method, params):
        self.id += 1
        rid = self.id
        self.send({"id": rid, "method": method, "params": params})
        until = time.monotonic() + 15
        while time.monotonic() < until:
            msg = self.messages.get(timeout=max(0.1, until - time.monotonic()))
            if msg.get("id") == rid and "method" not in msg:
                return msg
            if "id" in msg and "method" in msg:
                result = (
                    [{} for _ in msg.get("params", {}).get("items", [])]
                    if msg["method"] == "workspace/configuration"
                    else None
                )
                self.send({"id": msg["id"], "result": result})
        raise TimeoutError(method)

    def close(self):
        try:
            self.request("shutdown", None)
            self.notify("exit", None)
            self.proc.stdin.close()
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            pass
        finally:
            if self.proc.poll() is None:
                self.proc.kill()


def probe(case, config, scheme=False):
    workspace = ROOT / "fixtures" / case
    (workspace / "src").mkdir(parents=True, exist_ok=True)
    (workspace / "tsconfig.json").write_text(json.dumps(config))
    model = workspace / "src/model.ts"
    model.write_text(MODEL)
    virtual = workspace / "src/page.wxml.ts"
    uri = "virtual://wxml/src/page.wxml.ts" if scheme else virtual.as_uri()
    c = Client(workspace)
    try:
        init = c.request(
            "initialize",
            {
                "processId": os.getpid(),
                "rootUri": workspace.as_uri(),
                "workspaceFolders": [{"uri": workspace.as_uri(), "name": case}],
                "capabilities": {
                    "general": {"positionEncodings": ["utf-16"]},
                    "textDocument": {
                        "hover": {"contentFormat": ["markdown"]},
                        "diagnostic": {},
                    },
                    "workspace": {"configuration": True},
                },
            },
        )
        c.notify("initialized", {})
        c.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": model.as_uri(),
                    "languageId": "typescript",
                    "version": 1,
                    "text": MODEL,
                }
            },
        )
        c.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": uri,
                    "languageId": "typescript",
                    "version": 1,
                    "text": VIRTUAL,
                }
            },
        )
        doc = {"textDocument": {"uri": uri}}
        info = c.request("custom/projectInfo", doc)
        hover = c.request(
            "textDocument/hover", {**doc, "position": {"line": 2, "character": 6}}
        )
        diag = c.request("textDocument/diagnostic", doc)
        result = {
            "case": case,
            "virtualExistsOnDisk": virtual.exists(),
            "positionEncoding": init.get("result", {})
            .get("capabilities", {})
            .get("positionEncoding"),
            "projectInfo": info,
            "hover": hover,
            "diagnostic": diag,
        }
        if case == "included":
            updated = VIRTUAL.replace("user.DoesNotExist;", "user.Name;")
            c.notify(
                "textDocument/didChange",
                {
                    "textDocument": {"uri": uri, "version": 2},
                    "contentChanges": [{"text": updated}],
                },
            )
            result["afterFix"] = c.request("textDocument/diagnostic", doc)
            changed_model = MODEL.replace(
                "Display name from the real model.", "Updated unsaved JSDoc."
            )
            c.notify(
                "textDocument/didChange",
                {
                    "textDocument": {"uri": model.as_uri(), "version": 2},
                    "contentChanges": [{"text": changed_model}],
                },
            )
            result["unsavedDependencyHover"] = c.request(
                "textDocument/hover", {**doc, "position": {"line": 2, "character": 6}}
            )
        c.notify("textDocument/didClose", doc)
        return result
    finally:
        c.close()


base = {
    "compilerOptions": {
        "strict": True,
        "noEmit": True,
        "target": "es2022",
        "module": "esnext",
        "moduleResolution": "bundler",
        "types": [],
    }
}
results = [
    probe("included", {**base, "include": ["src/**/*.ts"]}),
    probe("explicit-files", {**base, "files": ["src/model.ts"]}),
    probe("custom-scheme", {**base, "include": ["src/**/*.ts"]}, scheme=True),
]
output = {
    "version": subprocess.check_output([str(BIN), "--version"], text=True).strip(),
    "childPath": "/nonexistent",
    "results": results,
}
(ROOT / os.environ.get("PROBE_OUTPUT", "probe-results.json")).write_text(
    json.dumps(output, indent=2)
)
print(json.dumps(output, indent=2))
