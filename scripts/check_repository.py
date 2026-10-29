"""Check public candidate paths and Markdown links without accessing services."""
from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
PRIVATE_DIRECTORIES = {
    '.agentdesk-data', '.tmp', '.venv', '.ruff_cache', '.pytest_cache',
    '__pycache__', 'node_modules', 'test-results',
}
PRIVATE_NAMES = {'ssh.txt', 'id_rsa', 'id_ed25519', '.DS_Store', 'Thumbs.db'}
PRIVATE_SUFFIXES = {'.pem', '.key', '.p12', '.pfx', '.db', '.sqlite', '.pyc'}
GENERATED_PREFIXES = ('dist/', 'src-tauri/target/', 'src-tauri/gen/')


def candidate_paths(root: Path) -> list[Path]:
    result = subprocess.run(
        ['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'],
        cwd=root, check=True, capture_output=True,
    )
    return [Path(name) for name in sorted(set(result.stdout.decode('utf-8').split('\0'))) if name]


def path_problem(path: Path) -> str | None:
    name, value = path.name, path.as_posix()
    if (
        set(path.parts) & PRIVATE_DIRECTORIES
        or name in PRIVATE_NAMES
        or name == '.env'
        or name.startswith('.env.') and name != '.env.example'
        or path.suffix.lower() in PRIVATE_SUFFIXES
        or re.search(r'\.(?:db|sqlite)-(?:wal|shm|journal)$', name)
    ):
        return 'private file or local data must not be published'
    if value.startswith(GENERATED_PREFIXES):
        return 'generated build output must not be published'
    if value.startswith(('resources/host/', 'resources/windows/')) and name != 'README.md':
        return 'downloaded release resources must not be published'
    return None


def markdown_problems(root: Path, path: Path, candidates: set[str]) -> list[str]:
    text = (root / path).read_text(encoding='utf-8')
    # Code examples may contain Markdown syntax; only inspect rendered links.
    text = re.sub(r'^```[^\n]*\n.*?^```\s*$', '', text, flags=re.M | re.S)
    problems = []
    for match in re.finditer(r'\[[^\]]*\]\(([^)]+)\)', text):
        link = match.group(1).strip()
        link = link[1:link.index('>')] if link.startswith('<') and '>' in link else link
        if link.lower().startswith('file:') or re.match(r'[A-Za-z]:[\\/]', link):
            problems.append('Markdown link contains a local filesystem path')
            continue
        if re.match(r'[a-zA-Z][a-zA-Z0-9+.-]*:|#|//', link):
            continue
        target = unquote(link.split('#', 1)[0])
        if not target:
            continue
        resolved = ((root / path).parent / target).resolve()
        if not resolved.is_relative_to(root):
            problems.append('Markdown link leaves the public repository')
            continue
        relative = resolved.relative_to(root).as_posix()
        if relative not in candidates and not any(name.startswith(relative.rstrip('/') + '/') for name in candidates):
            problems.append('Markdown link has no public target: ' + target)
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    candidates = candidate_paths(ROOT)
    present = [path for path in candidates if (ROOT / path).exists() or (ROOT / path).is_symlink()]
    names = {path.as_posix() for path in present}
    problems = []
    for path in present:
        absolute = ROOT / path
        if problem := path_problem(path):
            problems.append((path, problem))
            continue
        if absolute.is_symlink():
            if not absolute.resolve().is_relative_to(ROOT):
                problems.append((path, 'symbolic link leaves the repository'))
            continue
        if not absolute.is_file():
            continue
        if absolute.stat().st_size > 10 * 1024 * 1024:
            problems.append((path, 'large file requires review before publication'))
        if path.suffix == '.md':
            problems.extend((path, problem) for problem in markdown_problems(ROOT, path, names))
    for path, problem in problems:
        print(f'{path.as_posix()}: {problem}')
    print(f'Repository check: {len(present)} candidate files, {len(problems)} problems')
    return 1 if problems else 0


if __name__ == '__main__':
    raise SystemExit(main())
