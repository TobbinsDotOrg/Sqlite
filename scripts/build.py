"""Build precompiled artifacts using the existing native.pebble.json layout.
Run on each native author host, or pass --zig for cross compilation. Consumers
never run this script. The committed lockfile pins the entire dependency graph.
"""
import argparse, json, os, platform, shutil, subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
TARGETS={'windows-amd64':'x86_64-pc-windows-msvc','linux-amd64':'x86_64-unknown-linux-gnu','darwin-arm64':'aarch64-apple-darwin'}
p=argparse.ArgumentParser();p.add_argument('--target',choices=TARGETS,required=True);p.add_argument('--llvm-bin',default=os.environ.get('TOBBINS_NATIVE_LLVM_BIN','C:/Program Files/LLVM/bin' if os.name=='nt' else '/usr/bin'));p.add_argument('--zig');a=p.parse_args()
triple=TARGETS[a.target];env=dict(os.environ)
def run(args):
    r=subprocess.run(list(map(str,args)),cwd=ROOT,env=env,capture_output=True)
    if r.returncode: raise RuntimeError(r.stderr.decode(errors='replace'))
    return r.stdout.decode(errors='replace')
if a.zig:
    # A tiny executable Python wrapper is selected for both cc-rs and rustc.
    zig_target={'linux-amd64':'x86_64-linux-gnu.2.28','darwin-arm64':'aarch64-macos.11.0'}[a.target]
    work=ROOT/'target/tools';work.mkdir(parents=True,exist_ok=True)
    wrapper=work/f'cc-{a.target}.cmd'
    helper=work/f'cc-{a.target}.py'
    helper.write_text('import subprocess,sys\nraise SystemExit(subprocess.call('+repr([str(Path(a.zig).resolve()),'cc','-target',zig_target])+"+[x for x in sys.argv[1:] if not x.startswith('--target=')]))\n")
    wrapper.write_text('@python "'+str(helper)+'" %*\n')
    env['CC_'+triple.replace('-','_')]=str(wrapper)
    env['CARGO_TARGET_'+triple.upper().replace('-','_')+'_LINKER']=str(wrapper)
    env['AR_'+triple.replace('-','_')]=str(Path(a.llvm_bin)/('llvm-ar.exe' if os.name=='nt' else 'llvm-ar'))
    if a.target.startswith('darwin'):
        env['MACOSX_DEPLOYMENT_TARGET']='11.0'
        linkhelper=work/'link-darwin.py';linkwrapper=work/'link-darwin.cmd'
        linker=str(Path(a.llvm_bin)/('ld64.lld.exe' if os.name=='nt' else 'ld64.lld'))
        linkhelper.write_text('import subprocess,sys\nraise SystemExit(subprocess.call(['+repr(linker)+"]+[x for x in sys.argv[1:] if x not in ('-lc','-lm')]))\n")
        linkwrapper.write_text('@python "'+str(linkhelper)+'" %*\n')
        env['CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER']=str(linkwrapper)
        env['CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS']='-C linker-flavor=ld64.lld -C link-arg=-L'+str(Path(a.zig).resolve().parent/'lib/libc/darwin')
if a.target=='darwin-arm64':
    key='CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS'
    extra=' -C link-arg=-install_name -C link-arg=@rpath/libtobbins_sqlite.dylib' if a.zig else ' -C link-arg=-Wl,-install_name,@rpath/libtobbins_sqlite.dylib'
    env[key]=env.get(key,'')+extra
print(run(['cargo','build','--locked','--release','--target',triple]))
source=ROOT/'target'/triple/'release';dest=ROOT/'native'/a.target;dest.mkdir(parents=True,exist_ok=True)
metadata=json.loads((ROOT/'native.pebble.json').read_text())['platforms'][a.target]
dynamic=Path(metadata['dynamic']).name;static=Path(metadata['static']).name
shutil.copyfile(source/dynamic,dest/dynamic)
if a.target.startswith('windows'):shutil.copyfile(source/'tobbins_sqlite.dll.lib',dest/'tobbins_sqlite.dll.lib')
# Independent Rust std implementations must not interpose allocator/panic state
# when statically linked with the Tobbins runtime or another Pebble version.
# Rewrite private implementation symbols and every internal reference together.
ext='.exe' if os.name=='nt' else '';llvm=Path(a.llvm_bin)
symbols=run([llvm/('llvm-nm'+ext),'--defined-only','--extern-only','--format=posix',source/static])
names=set()
for line in symbols.splitlines():
    parts=line.split()
    if len(parts)>=2 and parts[1] in ('T','D','B','R','S','W','V'):
        name=parts[0]
        if name.startswith(('_R','__R','_ZN','__ZN','__rust','rust_','_rust_','sqlite3','_sqlite3')):names.add(name)
mapping=source/'sqlite-symbols.txt'
mapping.write_text(''.join(f'{n} tobbins_sqlite_v0_1_0_private_{n}\n' for n in sorted(names)))
if a.target.startswith('windows'):
    # llvm-objcopy cannot rewrite COFF short-import members. Preserve those
    # system imports byte-for-byte while rewriting actual object members.
    archive=(source/static).read_bytes();at=8;objects=[]
    work=source/'isolated';work.mkdir(exist_ok=True)
    while at<len(archive):
        header=archive[at:at+60];size=int(header[48:58]);data=archive[at+60:at+60+size];name=header[:16].decode().strip();at+=60+size+(size&1)
        if name in ('/','//','/SYM64/'):continue
        file=work/f'member{len(objects):04d}.obj';file.write_bytes(data)
        if not data.startswith(bytes.fromhex('0000ffff0000')):
            run([llvm/('llvm-objcopy'+ext),'--strip-debug','--redefine-syms='+str(mapping),file])
        objects.append(file)
    if (dest/static).exists():(dest/static).unlink()
    run([llvm/('llvm-ar'+ext),'rcs',dest/static,*objects])
else:
    run([llvm/('llvm-objcopy'+ext),'--strip-debug','--redefine-syms='+str(mapping),source/static,dest/static])
if not a.target.startswith('windows'):run([llvm/('llvm-strip'+ext),'--strip-debug',dest/dynamic])
print(f'{a.target}: {dynamic}, {static}; isolated {len(names)} private symbols')
