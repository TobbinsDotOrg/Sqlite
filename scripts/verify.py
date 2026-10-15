"""Windows AMD64 JIT/AOT integration via real tob add and the registry protocol.
--public uses the public registry and ordinary global cache after publication.
"""
import argparse, hashlib, io, json, os, platform, shutil, subprocess, threading, time, zipfile
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
ROOT=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--compiler',required=True);p.add_argument('--public',action='store_true');p.add_argument('--jit-only',action='store_true');a=p.parse_args()
assert os.name=='nt' and platform.machine().lower() in ('amd64','x86_64')
compiler=Path(a.compiler).resolve();work=ROOT/'tests/work'/str(time.time_ns());work.mkdir(parents=True)
env=dict(os.environ);env.pop('TOBBINS_REGISTRY',None)
def run(args,expected=0,environment=None,cwd=work):
    r=subprocess.run(list(map(str,args)),cwd=cwd,env=environment or env,capture_output=True,timeout=180)
    if r.returncode!=expected:raise AssertionError(f'{args}: {r.returncode}\n{r.stdout.decode(errors="replace")}\n{r.stderr.decode(errors="replace")}')
    return r
server=None
if not a.public:
    stream=io.BytesIO()
    with zipfile.ZipFile(stream,'w',zipfile.ZIP_DEFLATED) as z:
        for path in [ROOT/'tobbins.json',ROOT/'native.pebble.json',*sorted((ROOT/'src').rglob('*.tob')),*sorted((ROOT/'native').rglob('*'))]:
            if path.is_file():z.write(path,path.relative_to(ROOT).as_posix())
    archive=stream.getvalue();sha=hashlib.sha256(archive).hexdigest();manifest=json.loads((ROOT/'tobbins.json').read_text());release=dict(name=manifest['name'],version=manifest['version'],manifest=manifest,sha256=sha,commit='c'*40)
    class Registry(BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):
            if self.path.endswith('/download'):body=json.dumps(dict(url=base+'/archive',complete_url=base+'/complete')).encode()
            elif self.path.endswith('/archive'):body=archive
            else:body=json.dumps(dict(versions=[release])).encode()
            self.send_response(200);self.send_header('Content-Length',str(len(body)));self.send_header('X-Tobbins-Receipt','sqlite-verification');self.end_headers();self.wfile.write(body)
        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length','0')));body=b'{"counted":true}';self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    server=ThreadingHTTPServer(('127.0.0.1',0),Registry);base=f'http://127.0.0.1:{server.server_port}/api/v1';threading.Thread(target=server.serve_forever,daemon=True).start()
    env.update(TOBBINS_REGISTRY=base,TOBBINS_CACHE_DIR=str(work/'cache'))
env.update(LANG_CACHE_DIR=str(work/'jit'))
try:
    print(run([compiler,'add','Tobbins.Database.Sqlite']).stdout.decode())
    run([compiler,'install']);run([compiler,'update'])
    shutil.copyfile(ROOT/'tests/integration.tob',work/'app.tob')
    first=run([compiler,'app.tob']);print(first.stdout.decode())
    assert b'SQLite OK:' in first.stdout
    assert run([compiler,'app.tob']).stdout==first.stdout
    if not a.jit_only:
        run([compiler,'--build','app.tob'])
        clean=work/'standalone';clean.mkdir();shutil.copyfile(work/'app.exe',clean/'app.exe')
        cleanenv={k:v for k,v in os.environ.items() if k.upper() in ('SYSTEMROOT','WINDIR','COMSPEC','NUMBER_OF_PROCESSORS','PROCESSOR_ARCHITECTURE')}
        cleanenv.update(PATH=str(Path(os.environ['SYSTEMROOT'])/'System32'),TEMP=str(clean),TMP=str(clean),LOCALAPPDATA=str(clean/'empty-profile'))
        assert run([clean/'app.exe'],environment=cleanenv,cwd=clean).stdout==first.stdout
    # Cached/prepared source paths must revalidate locked compatibility too.
    lock=json.loads((work/'tobbins.lock').read_text());key=next(iter(lock['packages']));lock['packages'][key]['manifest']['tobbins']='>=99.0.0';(work/'tobbins.lock').write_text(json.dumps(lock))
    for args in [[compiler,'app.tob'],[compiler,'--build','app.tob']]:
        r=run(args,expected=1);assert b'requires Tobbins >=99.0.0' in r.stderr,r.stderr
    report=dict(platform='windows-amd64',public_registry=a.public,install=True,jit=True,aot=not a.jit_only,compatibility_recheck=True,work=str(work))
    (ROOT/'tests'/('public-results.json' if a.public else 'results.json')).write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
finally:
    if server:server.shutdown()
