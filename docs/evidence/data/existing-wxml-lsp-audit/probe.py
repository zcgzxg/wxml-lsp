import json
import os
import pathlib
import queue
import subprocess
import tempfile
import threading
import time

# Prerequisites: build the audited service and point PROBE_BIN at the binary
# (default: $PROBE_ROOT/target/debug/wxml-lsp). PROBE_ROOT defaults to a
# scratch directory under the system temp directory; cases.py is read from
# this file's directory and results are written under PROBE_ROOT.
HERE = pathlib.Path(__file__).resolve().parent
ROOT = pathlib.Path(
    os.environ.get("PROBE_ROOT", pathlib.Path(tempfile.gettempdir()) / "wxml-lsp-audit")
)
BIN = pathlib.Path(os.environ.get("PROBE_BIN", ROOT / "target/debug/wxml-lsp"))
ROOT.mkdir(parents=True, exist_ok=True)
proc = subprocess.Popen(
    [str(BIN)],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=(ROOT / "server.stderr").open("w"),
)
messages = queue.Queue()


def reader():
    while True:
        headers = {}
        while True:
            line = proc.stdout.readline()
            if not line:
                return
            if line == b"\r\n":
                break
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        messages.put(json.loads(proc.stdout.read(int(headers["content-length"]))))


threading.Thread(target=reader, daemon=True).start()
seq = 0
notifications = []


def send(method, params, request=False):
    global seq
    message = {"jsonrpc": "2.0", "method": method, "params": params}
    if params is None:
        message.pop("params")
    if request:
        seq += 1
        message["id"] = seq
    body = json.dumps(message).encode()
    proc.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    proc.stdin.flush()
    return message.get("id")


def receive(predicate):
    deadline = time.monotonic() + 15
    while True:
        message = messages.get(timeout=max(0.01, deadline - time.monotonic()))
        if predicate(message):
            return message
        notifications.append(message)


def request(method, params):
    ident = send(method, params, True)
    result = receive(lambda message: message.get("id") == ident)
    if "error" in result:
        raise RuntimeError(result)
    return result.get("result")


def position(text, index):
    prefix = text[:index]
    return {
        "line": prefix.count("\n"),
        "character": len(prefix.rsplit("\n", 1)[-1].encode("utf-16-le")) // 2,
    }


def query_hover(uri, text, needle, delta=0, occurrence=0):
    start = -1
    for _ in range(occurrence + 1):
        start = text.index(needle, start + 1)
    return request(
        "textDocument/hover",
        {"textDocument": {"uri": uri}, "position": position(text, start + delta)},
    )


results = []


def case(name, ts, wxml, queries, extra=None):
    directory = ROOT / "fixtures" / name
    directory.mkdir(parents=True, exist_ok=True)
    for filename, content in (extra or {}).items():
        path = directory / filename
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    (directory / "index.ts").write_text(ts)
    path = directory / "index.wxml"
    path.write_text(wxml)
    uri = path.as_uri()
    send(
        "textDocument/didOpen",
        {
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": wxml,
            }
        },
    )
    published = receive(
        lambda message: (
            message.get("method") == "textDocument/publishDiagnostics"
            and message["params"]["uri"] == uri
        )
    )
    result = {
        "name": name,
        "ts": ts,
        "wxml": wxml,
        "diagnostics": published["params"]["diagnostics"],
        "hover": [],
    }
    for query in queries:
        args = (query,) if isinstance(query, str) else query
        result["hover"].append({"query": args, "result": query_hover(uri, wxml, *args)})
    results.append(result)
    return uri


request(
    "initialize",
    {"processId": None, "rootUri": (ROOT / "fixtures").as_uri(), "capabilities": {}},
)
send("initialized", {})
exec((HERE / "cases.py").read_text())
(ROOT / "results.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
for result in results:
    simplified = {
        "case": result["name"],
        "diagnostics": [
            {"message": item["message"], "range": item["range"]}
            for item in result["diagnostics"]
        ],
        "hover": [
            {
                "query": item["query"],
                "value": (item["result"] or {}).get("contents"),
                "range": (item["result"] or {}).get("range"),
            }
            for item in result["hover"]
        ],
    }
    if "hover_after_unsaved_ts_open" in result:
        simplified["hover_after_unsaved_ts_open"] = result[
            "hover_after_unsaved_ts_open"
        ]
    print(json.dumps(simplified, ensure_ascii=False))
request("shutdown", None)
send("exit", None)
proc.stdin.close()
proc.wait(timeout=10)
