"""Prepare a reviewed container build context; building requires explicit --build."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[2]
PINNED = re.compile(r'^[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[a-f0-9]{64}$')


def prepare(destination: Path, base_image: str):
    if not PINNED.fullmatch(base_image):
        raise ValueError('Use an official Python 3.12 Debian image pinned by SHA256 digest')
    if destination.exists():
        raise ValueError('The output directory must not already exist')
    resources = ROOT / 'resources' / 'host'
    manifest = json.loads((resources / 'manifest.json').read_text(encoding='utf-8'))
    entries = {entry['name']: entry for entry in manifest['files']}
    files = {}
    for name in ('host', 'dependencies'):
        entry = entries[name]
        path = (resources / entry['path']).resolve()
        if path.parent != resources.resolve() or hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
            raise ValueError('Host resource verification failed; rebuild Host resources')
        files[name] = path
    with zipfile.ZipFile(files['host']) as wheel:
        for source in (ROOT / 'host/agentdesk_host').rglob('*.py'):
            archive_path = source.relative_to(ROOT / 'host').as_posix()
            if wheel.read(archive_path) != source.read_bytes():
                raise ValueError('Host wheel is stale; rebuild Host resources')
    destination.mkdir(parents=True)
    shutil.copy2(ROOT / 'host/container/Containerfile', destination / 'Containerfile')
    # Preserve the valid wheel filename so pip can inspect its tags/version.
    shutil.copy2(files['host'], destination / files['host'].name)
    containerfile = (destination / 'Containerfile').read_text(encoding='utf-8')
    containerfile = containerfile.replace('agentdesk_host.whl', files['host'].name)
    (destination / 'Containerfile').write_text(containerfile, encoding='utf-8')
    shutil.copy2(files['dependencies'], destination / 'dependencies.txt')
    (destination / 'build.json').write_text(json.dumps({'baseImage': base_image, 'hostVersion': manifest['version']}, indent=2) + '\n', encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-image', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--build', action='store_true')
    args = parser.parse_args()
    prepare(args.output, args.base_image)
    if not args.build:
        print('Container context prepared. No image was built or pulled.')
        return
    executable = shutil.which('podman')
    if not executable:
        parser.error('Podman is required for --build')
    info = json.loads(subprocess.check_output([executable, '--remote=false', 'info', '--format', 'json']))
    if info.get('host', {}).get('security', {}).get('rootless') is not True:
        parser.error('Run the image build as the intended non-root Host user')
    tag = 'localhost/agentdesk-isolated:local'
    subprocess.run([executable, '--remote=false', 'build', '--pull=never', '--build-arg', 'PYTHON_IMAGE=' + args.base_image, '--tag', tag, str(args.output.resolve())], check=True)
    identity = subprocess.check_output([executable, '--remote=false', 'image', 'inspect', '--format', '{{.Id}}', tag], text=True).strip()
    identity = identity.removeprefix('sha256:')
    if not re.fullmatch(r'[a-f0-9]{64}', identity):
        raise ValueError('Built image identity could not be verified')
    print(json.dumps({'isolation': {'image': 'sha256:' + identity}}, indent=2))


if __name__ == '__main__':
    main()
