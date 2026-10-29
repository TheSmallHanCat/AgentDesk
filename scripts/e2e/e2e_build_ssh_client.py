"""Prepare an isolated official SSH client for the legacy dedicated acceptance server."""

from __future__ import annotations

import argparse
import json
from e2e_common import configured, connect, run

REMOTE = r'''
import hashlib,json,os,pathlib,shutil,subprocess,tarfile,urllib.request
root=pathlib.Path.home()/'.agentdesk/acceptance/ssh-client'
root.mkdir(parents=True,exist_ok=True,mode=0o700)
assets=[
 ('zig-linux-x86_64-0.13.0','https://ziglang.org/download/0.13.0/zig-linux-x86_64-0.13.0.tar.xz','d45312e61ebcc48032b77bc4cf7fd6915c11fa16e4aad116b66c9468211230ea'),
 ('libressl-4.3.4','https://cdn.openbsd.org/pub/OpenBSD/LibreSSL/libressl-4.3.4.tar.gz','57c8b2f8c6f9505587abaeddab003ea243d615edb7ed71957968121fc6316bd6'),
 ('openssh-10.6p1','https://cdn.openbsd.org/pub/OpenBSD/OpenSSH/portable/openssh-10.6p1.tar.gz','a9dc9565dffe8640f64d863cd29a32bc4a3dbdec0566a7fc44c5d6ee767d5f39'),
]
def state(value):
 (root/'state.json').write_text(json.dumps(value))
def command(arguments,cwd,environment,log):
 result=subprocess.run(arguments,cwd=cwd,env=environment,stdout=log,stderr=subprocess.STDOUT,timeout=3600)
 if result.returncode: raise RuntimeError('Build step failed: '+str(arguments[0]))
try:
 with (root/'build.log').open('a') as log:
  for name,url,digest in assets:
   state({'phase':'download','component':name})
   archive=root/(name+('.tar.xz' if name.startswith('zig-') else '.tar.gz'))
   if not archive.exists():
    with urllib.request.urlopen(url,timeout=60) as source,archive.open('wb') as destination:
     shutil.copyfileobj(source,destination)
   if hashlib.sha256(archive.read_bytes()).hexdigest()!=digest: raise RuntimeError('Official archive SHA256 mismatch')
   if not (root/name).exists():
    with tarfile.open(archive) as source: source.extractall(root,filter='data')
  zig=str(root/assets[0][0]/'zig')
  environment=dict(os.environ,CC=zig+' cc -target x86_64-linux-musl',AR=zig+' ar',RANLIB=zig+' ranlib',CFLAGS='-O2',LDFLAGS='-static')
  state({'phase':'build','component':'libressl'})
  crypto=root/'crypto'
  if not (crypto/'lib/libcrypto.a').is_file():
   source=root/assets[1][0]
   command(['./configure','--prefix='+str(crypto),'--disable-shared','--enable-static','--disable-tests'],source,environment,log)
   command(['make','-j2'],source,environment,log)
   command(['make','install'],source,environment,log)
  state({'phase':'build','component':'openssh'})
  source=root/assets[2][0]
  if not (source/'Makefile').is_file():
   command(['./configure','--cache-file='+str(root/'openssh-config.cache'),'--prefix='+str(root),'--with-ssl-dir='+str(crypto),'--without-zlib','--without-pam','--without-selinux','--without-kerberos5'],source,environment,log)
  command(['make','-j2','ssh'],source,environment,log)
  (root/'bin').mkdir(exist_ok=True)
  shutil.copy2(source/'ssh',root/'bin/ssh')
  (root/'licenses').mkdir(exist_ok=True)
  shutil.copy2(source/'LICENCE',root/'licenses/OpenSSH-LICENCE')
  shutil.copy2(root/assets[1][0]/'COPYING',root/'licenses/LibreSSL-COPYING')
  version=subprocess.run([str(root/'bin/ssh'),'-V'],capture_output=True,check=True).stderr.decode().strip()
  state({'phase':'complete','version':version,'binarySha256':hashlib.sha256((root/'bin/ssh').read_bytes()).hexdigest(),'assets':[{'name':name,'sha256':digest,'url':url} for name,url,digest in assets]})
except Exception as error:
 state({'phase':'failed','reason':str(error)})
 raise
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--status", action="store_true")
    args = parser.parse_args()
    if not configured():
        return
    client = connect()
    try:
        if args.status:
            status, output = run(client, 'cat "$HOME/.agentdesk/acceptance/ssh-client/state.json"', check=False)
            if status:
                print("SSH client preparation has not started")
            else:
                value = json.loads(output)
                print(json.dumps(value, ensure_ascii=False))
            return
        run(client, 'umask 077; mkdir -p "$HOME/.agentdesk/acceptance/ssh-client"')
        sftp = client.open_sftp()
        try:
            home = sftp.normalize(".")
            with sftp.open(home + "/.agentdesk/acceptance/ssh-client/build.py", "w") as target:
                target.write(REMOTE)
        finally:
            sftp.close()
        run(client, 'setsid nohup "$HOME/.agentdesk/host/current/venv/bin/python" "$HOME/.agentdesk/acceptance/ssh-client/build.py" </dev/null >"$HOME/.agentdesk/acceptance/ssh-client/launcher.log" 2>&1 &')
        print("Started isolated official SSH client preparation")
    finally:
        client.close()


if __name__ == "__main__":
    main()
