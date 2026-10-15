"""Submit this repository using Tobbins' established immutable publisher."""
import json, urllib.request
from pathlib import Path
root=Path(__file__).resolve().parents[1]
metadata=json.loads((root/'native.pebble.json').read_text())
for target,files in metadata['platforms'].items():
    for kind in ('dynamic','static'):
        path=root/files[kind]
        if not path.is_file():raise SystemExit(f'Missing {target} {kind}: {path}')
request=urllib.request.Request('https://tobbins.org/api/v1/publish',data=json.dumps({'repository':'https://github.com/TobbinsDotOrg/Sqlite'}).encode(),headers={'Content-Type':'application/json'},method='POST')
with urllib.request.urlopen(request,timeout=180) as response:print(response.read().decode())
