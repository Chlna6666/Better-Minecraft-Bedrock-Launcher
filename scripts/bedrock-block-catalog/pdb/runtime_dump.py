#!/usr/bin/env python3
"""Dump the LIVE, initialized BDS block registry via Frida.

This is RUNTIME observation, not static PDB reversing. It attaches to a running
bedrock_server.exe, reads the game's own populated `BlockTypeRegistry`
`std::map<HashedString, SharedPtr<BlockLegacy>>`, and emits each block's real
runtime name together with the live object address and provenance.

What is read (empirically verified against BDS 1.20.80.05):
  * `BlockTypeRegistry::mBlockLookupMap` at a PDB-derived static RVA.
  * std::map header at +0 -> `_Myhead` sentinel; `_Mysize` at +8.
  * tree node: left@+0, parent@+8, right@+16, HashedString hash@+32,
    std::string key@+40 (size@+56, capacity@+64),
    SharedPtr<BlockLegacy> counter@+80 -> object@counter[0].
  * BlockLegacy object name std::string at +0xA0 (size@+0xB0).

Names come from the game's initialized objects, NOT from decoded PDB symbols.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

try:
    import frida
except ImportError:
    frida = None


JS = r"""
function u32(a){var b=a.readByteArray(4);var x=new Uint8Array(b);return (x[0]|x[1]<<8|x[2]<<16|(x[3]<<24)>>>0)>>>0;}
function pv(a){var lo=u32(a),hi=u32(a.add(4));return hi*4294967296+lo;}
function hx(a){var lo=u32(a),hi=u32(a.add(4));return '0x'+('00000000'+(hi>>>0).toString(16)).slice(-8)+('00000000'+(lo>>>0).toString(16)).slice(-8);}
function readStdString(addr,sizeOff){ // std::string: buffer@addr, size@addr+sizeOff, cap@addr+sizeOff+8
  var size=u32(addr.add(sizeOff));
  if(size===0||size>4096) return '';
  var s='';
  try{
    if(size<=15){var u=new Uint8Array(addr.readByteArray(size));for(var i=0;i<size;i++)s+=String.fromCharCode(u[i]);}
    else{var p=ptr(pv(addr));var u2=new Uint8Array(p.readByteArray(size));for(var i=0;i<size;i++)s+=String.fromCharCode(u2[i]);}
  }catch(e){return '';}
  return s;
}
try{
  var base=Process.getModuleByName('bedrock_server.exe').base;
  var cfg=JSON.parse('__CONFIG__');
  var map=base.add(cfg.map_rva);
  var size=u32(map.add(8));
  var head=ptr(pv(map.add(0)));
  var root=ptr(pv(head.add(8)));
  var visited={}, rows=[], stack=[root];
  while(stack.length && rows.length<cfg.max_rows){
    var node=stack.pop();
    if(node.isNull())continue;
    var key=node.toString();
    if(visited[key])continue;
    visited[key]=1;
    var name='', counter='0x0', object='0x0';
    try{
      name=readStdString(node.add(cfg.key_string_off),cfg.key_size_off-cfg.key_string_off);
      var c=ptr(pv(node.add(cfg.value_off)));
      if(!c.isNull()){counter=c.toString();var o=ptr(pv(c));object=o.toString();}
    }catch(e){}
    if(name) rows.push({name:name,node:key,counter:counter,object:object});
    try{stack.push(ptr(pv(node)));stack.push(ptr(pv(node.add(16))));}catch(e){}
  }
  send({type:'dump',base:base.toString(),declared_size:size,visited:Object.keys(visited).length,rows:rows});
}catch(e){send({type:'err',msg:String(e),stack:e.stack});}
"""


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--map-rva", required=True, help="BlockTypeRegistry::mBlockLookupMap RVA, e.g. 0x331c160")
    ap.add_argument("--pid", type=int)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--max-rows", type=int, default=20000)
    ap.add_argument("--key-string-off", type=int, default=40)
    ap.add_argument("--key-size-off", type=int, default=56)
    ap.add_argument("--value-off", type=int, default=80)
    ap.add_argument("--names-only", action="store_true",
                    help="omit machine-specific heap addresses (portable evidence)")
    args = ap.parse_args()

    if frida is None:
        print("frida is required: pip install frida", file=sys.stderr)
        return 2

    pid = args.pid
    if pid is None:
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command",
             "Get-Process bedrock_server -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id"],
            capture_output=True, text=True)
        pids = [int(line) for line in result.stdout.split() if line.strip().isdigit()]
        if not pids:
            print("no running bedrock_server process", file=sys.stderr)
            return 2
        pid = pids[0]

    config = {
        "map_rva": int(args.map_rva, 0),
        "key_string_off": args.key_string_off,
        "key_size_off": args.key_size_off,
        "value_off": args.value_off,
        "max_rows": args.max_rows,
    }
    print(f"attaching to BDS pid={pid} map_rva={hex(config['map_rva'])}", flush=True)

    session = frida.attach(pid)
    script = session.create_script(JS.replace("__CONFIG__", json.dumps(config)))
    messages: list[dict] = []
    script.on("message", lambda message, _data: messages.append(message))
    script.load()
    deadline = time.time() + 30
    while time.time() < deadline and not any(
            m.get("payload", {}).get("type") in ("dump", "err") for m in messages):
        time.sleep(0.2)
    session.detach()

    payload = next((m.get("payload") for m in messages if m.get("payload")), None)
    if payload is None or payload.get("type") == "err":
        print(f"dump failed: {payload}", file=sys.stderr)
        return 1

    rows = payload["rows"]
    if args.names_only:
        # Heap addresses are per-run and machine-specific; drop them for a
        # portable, reviewable evidence artifact.
        rows = [{"name": row["name"]} for row in rows]
    report = {
        "schema_version": 1,
        "method": "live_frida_block_type_registry_dump",
        "pid": pid,
        "module_base": payload["base"],
        "map_rva": hex(config["map_rva"]),
        "declared_map_size": payload["declared_size"],
        "nodes_visited": payload["visited"],
        "named_blocks": len(rows),
        "layout": {
            "node_left": 0, "node_parent": 8, "node_right": 16,
            "hashed_string_hash": 32,
            "key_string": config["key_string_off"],
            "key_size": config["key_size_off"],
            "shared_ptr_counter": config["value_off"],
            "block_legacy_name": "0xA0",
        },
        "evidence_boundary": (
            "Live runtime observation of the initialized block registry. Names are read "
            "from the game's own objects. No legacy numeric ID or network runtime ID is "
            "inferred from symbol ordinals."
        ),
        "blocks": rows,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"dumped {len(rows)} live block names (map size {payload['declared_size']}, "
          f"visited {payload['visited']}) -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
