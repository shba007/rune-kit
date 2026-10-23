"""Run real released tools through the public gateway; fixtures are recorded responses."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import time
from git_gateway_conformance import Gateway

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--binary", type=Path, required=True)
p.add_argument("--time", type=Path, required=True)
p.add_argument("--filesystem", type=Path, required=True)
p.add_argument("--slow", type=Path, required=True)
p.add_argument("--output", type=Path, required=True)
a = p.parse_args()
root = a.output.resolve().parent
root.mkdir(parents=True, exist_ok=True)
os.environ["RUNE_KIT_DATA_DIR"] = str(root / "tools")
(root / "hello.txt").write_text("Hello from the allowed root", encoding="utf-8")
records = {}
for kind, plugin, extra in [("time", a.time.resolve(), ()), ("filesystem", a.filesystem.resolve(), ("-p", "allowed_dir="+root.as_posix()))]:
    g = Gateway(a.binary.resolve(), plugin, True, extra)
    try:
        discovery = g.request("server/discover")
        listing = g.request("tools/list")
        tools = listing["result"]["tools"]
        calls = []
        selected = [t for t in tools if t["name"].endswith("__get_current_time")] if kind == "time" else tools
        for tool in selected:
            arguments = {"timezone":"UTC"} if kind == "time" else {"path":"hello.txt" if tool["name"].endswith("__read_text_file") else "."}
            reply = g.request("tools/call", {"name":tool["name"],"arguments":arguments})
            assert "error" not in reply, reply
            content = json.loads(reply["result"]["content"][0]["text"])
            assert content.get("status") == "success", content
            calls.append(reply)
        records[kind] = {"sha256":hashlib.sha256(plugin.read_bytes()).hexdigest(), "discovery":discovery,"list":listing,"calls":calls}
        if kind == "filesystem":
            read = next(t for t in tools if t["name"].endswith("__read_text_file"))
            outside = g.request("tools/call", {"name":read["name"],"arguments":{"path":"../outside.txt"}})
            assert "Access denied" in json.dumps(outside), outside
            write = g.request("tools/call", {"name":read["name"].rsplit("__",1)[0]+"__write_file","arguments":{"path":"forbidden.txt","content":"bad"}})
            assert write["error"]["code"] == -32600, write
            invalid = g.request("tools/call", {"name":read["name"],"arguments":{"path":42}})
            assert invalid["result"]["isError"] is True, invalid
            records[kind].update(outside=outside, forbidden=write, invalidArguments=invalid)
    finally:
        g.close()
g = Gateway(a.binary.resolve(), a.slow.resolve(), extra_args=("--timeout","1"))
try:
    tools = g.request("tools/list")["result"]["tools"]
    name = tools[0]["name"]
    timeout = g.request("tools/call", {"name":name,"arguments":{}})
    assert timeout["error"]["code"] == -32000 and "timed out" in timeout["error"]["message"], timeout
    meta = {"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}
    g.send({"jsonrpc":"2.0","id":"cancel-me","method":"tools/call","params":{"name":name,"arguments":{},"_meta":meta}})
    time.sleep(0.2)
    g.send({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"cancel-me"}})
    # Any late response arrives before this ping and fails Gateway's ID assertion.
    pong = g.request("ping")
    records["control"] = {"timeout":timeout,"afterCancellation":pong}
    g.send({"jsonrpc":"2.0","id":"missing-meta","method":"tools/list"})
    missing = json.loads(g.output.get(timeout=10))
    assert missing["error"]["code"] == -32602, missing
    wrong = dict(meta, **{"io.modelcontextprotocol/protocolVersion":"2025-03-26"})
    g.send({"jsonrpc":"2.0","id":"wrong-version","method":"tools/list","params":{"_meta":wrong}})
    version = json.loads(g.output.get(timeout=10))
    assert version["error"]["code"] == -32022, version
    records["control"].update(missingMetadata=missing, rejectedVersion=version)
finally:
    g.close()
os.environ["RUNE_FIXTURE_TOOLS"] = '{"tools":[{"name":"read","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true,"readOnly":false}}]}'
g = Gateway(a.binary.resolve(), a.slow.resolve())
try:
    failed = g.request("tools/list")
    assert failed["error"]["code"] == -32000 and "Tool discovery failed" in failed["error"]["message"], failed
    records["failedDiscovery"] = failed
finally:
    g.close()
    os.environ.pop("RUNE_FIXTURE_TOOLS", None)
a.output.write_text(json.dumps(records,indent=2)+"\n",encoding="utf-8")
print("PASS: released time/filesystem calls, root confinement, write refusal, native timeout and cancellation silence")
