exec /usr/bin/python3 -c '
import json,os,sys
from pathlib import Path
root=Path(os.environ["FAKE_LOG"])
r=json.loads(sys.stdin.readline()) if sys.argv[1:]==["connect"] else None
if r: p=r["params"];op=r["method"].split(".")[1]
else:
 op=sys.argv[1];p={"terminal":sys.argv[2]};assert op in ["kill","rm"]
def emit(v):
 if r: print(json.dumps({"jsonrpc":"2.0","id":r["id"],"result":v}))
def number(name,default):
 f=root/name
 return int(f.read_text()) if f.exists() else default
def terminal(addr):
 return {"address":addr,"state":"live","argv":[],"cwd":"/nonexistent","created_ms":0,"screen":"exact","cols":80,"rows":24,"labels":{}}
a=[op]
if op=="run":
 a += ["--bg","--no-parent","--outlive-parent","--name",p["name"],"--cwd",p["cwd"]]
 for k in ["vega.role","vega.review","herdr.space"]:
  if k in p["labels"]: a += ["--label",k+"="+p["labels"][k]]
 a += ["--"]+p["argv"]
else:
 a += [p["terminal"]]
 if op=="send": a += [p["text"]]
with (root/"hc.log").open("a") as f: f.write(" ".join(a)+"\n")
if op=="get":
 n=number("hc-get-n",0)+1; (root/"hc-get-n").write_text(str(n))
 if n==number("hc-get-fail-at",-1):
  print(json.dumps({"jsonrpc":"2.0","id":r["id"],"error":{"code":-32000,"message":"author lookup failed","data":{"kind":"not_found"}}}))
  sys.exit()
 f=root/("hc-"+p["terminal"]+".json")
 if f.exists(): emit(json.loads(f.read_text()));sys.exit()
 t=terminal(p["terminal"])
 t.update(pid=2 if (root/"hc-stale").exists() else os.getppid(),status={"app":"claude-code","state":"working"},foreground={"pid":1,"name":"claude","argv":["claude"],"cwd":"/nonexistent"},labels={"herdr.space":"space-1","vega.review":(root/"hc-review").read_text() if (root/"hc-review").exists() else "none"})
 emit(t)
elif op=="run":
 n=number("hc-n",10)+1; (root/"hc-n").write_text(str(n));emit(terminal("t"+str(n)))
elif op=="rm":
 n=number("hc-rm-busy",0)
 if n>0:
  (root/"hc-rm-busy").write_text(str(n-1))
  if r: print(json.dumps({"jsonrpc":"2.0","id":r["id"],"error":{"code":-32000,"message":"still running","data":{"kind":"live"}}}))
  sys.exit(6)
 emit({"removed":p["terminal"]})
elif op=="send": emit({"turn":0})
elif op=="kill":
 t=terminal(p["terminal"]);t["state"]="finished";emit(t)
else: raise AssertionError(op)
' "$@"
