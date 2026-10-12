#!/usr/bin/env python3
"""Validate the canonical CI contracts and, when installed, actionlint syntax."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def schema_errors(document):
    if not isinstance(document, dict):
        return ['Workflow must be an object']
    errors = []
    for key, expected in [('name', str), ('on', dict), ('permissions', dict), ('jobs', dict)]:
        if not isinstance(document.get(key), expected):
            errors.append(f'{key} must be a {expected.__name__}')
    jobs = document.get('jobs')
    if not isinstance(jobs, dict):
        return errors
    if not jobs:
        errors.append('jobs must not be empty')
    for name, job in jobs.items():
        if not isinstance(job, dict):
            errors.append(f'{name}: job must be an object')
            continue
        if not isinstance(job.get('runs-on'), str):
            errors.append(f'{name}: runs-on must be a string')
        steps = job.get('steps')
        if not isinstance(steps, list) or not steps:
            errors.append(f'{name}: steps must be a nonempty list')
            continue
        for step in steps:
            if not isinstance(step, dict):
                errors.append(f'{name}: step must be an object')
            elif sum(key in step for key in ('run', 'uses')) != 1:
                errors.append(f'{name}: step must specify exactly one of run or uses')
            elif not isinstance(step.get('run', step.get('uses')), str):
                errors.append(f'{name}: run/uses must be a string')
    if errors:
        return errors
    jobs = document['jobs']
    if jobs.get('linux', {}).get('runs-on') != 'ubuntu-latest':
        errors.append('Linux gate must run on ubuntu-latest')
    if not any(step.get('run') == 'tools/gate.sh' for step in jobs.get('linux', {}).get('steps', [])):
        errors.append('Linux CI must invoke the canonical tools/gate.sh')
    platform = jobs.get('platform-check', {})
    strategy = platform.get('strategy', {})
    matrix = strategy.get('matrix', {}) if isinstance(strategy, dict) else {}
    runners = matrix.get('os') if isinstance(matrix, dict) else None
    if not isinstance(runners, list) or not all(isinstance(runner, str) for runner in runners) or set(runners) != {'macos-latest', 'windows-latest'}:
        errors.append('Both native platform runners are required')
    if platform.get('runs-on') != '${{ matrix.os }}':
        errors.append('Platform jobs must use their native matrix runner')
    if not any('cargo check --workspace --all-targets --locked -j 8' == step.get('run') for step in platform.get('steps', [])):
        errors.append('Platform matrix must compile the same workspace')
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--schema', action='store_true')
    args = parser.parse_args()
    path = ROOT / '.github/workflows/ci.yml'
    actionlint = os.environ.get('ACTIONLINT') or shutil.which('actionlint')
    if not args.schema:
        lint_status = 0
        if actionlint:
            try:
                lint_status = subprocess.run([actionlint, str(path)], cwd=ROOT, check=False).returncode
            except OSError as error:
                print(f'Cannot run actionlint: {error}', file=sys.stderr)
                lint_status = 1
        else:
            print('actionlint is unavailable: running YAML structure/canonical-contract validation only; Actions syntax is not certified.', file=sys.stderr)
        schema_status = subprocess.run(['uv', 'run', '--no-project', '--with', 'pyyaml', 'python', str(Path(__file__)), '--schema'], cwd=ROOT, check=False).returncode
        return int(bool(lint_status or schema_status))
    import yaml
    # BaseLoader avoids YAML 1.1 coercing GitHub's "on" key to a boolean.
    document = yaml.load(path.read_text(), Loader=yaml.BaseLoader)
    errors = schema_errors(document)
    if errors:
        print('\n'.join(errors), file=sys.stderr)
        return 1
    print('CI YAML structure and canonical gate/matrix contracts: pass')
    return 0


if __name__ == '__main__':
    sys.exit(main())
