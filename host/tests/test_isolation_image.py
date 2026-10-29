"""Build context integrity checks without pulling images or accessing services."""
import hashlib
import importlib.util
import json
from pathlib import Path
import zipfile

import pytest


@pytest.fixture
def builder(tmp_path, monkeypatch):
    root = Path(__file__).resolve().parents[2]
    spec = importlib.util.spec_from_file_location('prepare_isolation_image', root / 'scripts/build/prepare_isolation_image.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    source = tmp_path / 'host/agentdesk_host'
    source.mkdir(parents=True)
    (source / '__init__.py').write_text('VERSION = "test"\n')
    resources = tmp_path / 'resources/host'
    resources.mkdir(parents=True)
    wheel = resources / 'agentdesk_host-0.2.0-py3-none-any.whl'
    with zipfile.ZipFile(wheel, 'w') as archive:
        archive.write(source / '__init__.py', 'agentdesk_host/__init__.py')
    dependencies = resources / 'dependencies.txt'
    dependencies.write_text('')
    entries = [{'name': name, 'path': path.name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()} for name, path in [('host', wheel), ('dependencies', dependencies)]]
    (resources / 'manifest.json').write_text(json.dumps({'version': '0.2.0', 'files': entries}))
    container = tmp_path / 'host/container'
    container.mkdir()
    (container / 'Containerfile').write_bytes((root / 'host/container/Containerfile').read_bytes())
    monkeypatch.setattr(module, 'ROOT', tmp_path)
    def unexpected(*args, **kwargs): raise AssertionError('Prepare must not execute a build or external command')
    monkeypatch.setattr(module.subprocess, 'run', unexpected)
    monkeypatch.setattr(module.subprocess, 'check_output', unexpected)
    return module


def test_prepare_only_preserves_valid_wheel_filename(builder, tmp_path):
    destination = tmp_path / 'context'
    image = 'python@sha256:' + 'a' * 64
    builder.prepare(destination, image)
    container = (destination / 'Containerfile').read_text()
    assert 'agentdesk_host-0.2.0-py3-none-any.whl' in container
    assert 'agentdesk_host.whl' not in container
    assert json.loads((destination / 'build.json').read_text())['baseImage'] == image


@pytest.mark.parametrize('invalid', ['unpinned', 'existing', 'stale', 'hash', 'outside'])
def test_prepare_rejects_invalid_or_changed_inputs_without_writing(builder, tmp_path, invalid):
    destination = tmp_path / 'context'
    image = 'python@sha256:' + 'a' * 64
    if invalid == 'unpinned': image = 'python:latest'
    if invalid == 'existing': destination.mkdir()
    if invalid == 'stale': (tmp_path / 'host/agentdesk_host/__init__.py').write_text('VERSION = "changed"\n')
    if invalid == 'hash': (tmp_path / 'resources/host/dependencies.txt').write_text('changed')
    if invalid == 'outside':
        manifest_path = tmp_path / 'resources/host/manifest.json'
        manifest = json.loads(manifest_path.read_text())
        manifest['files'][0]['path'] = '../../outside.whl'
        manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(ValueError): builder.prepare(destination, image)
    assert not destination.exists() or not list(destination.iterdir())
