import importlib.util
import copy
import os
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), ROOT / 'tools' / f'{name}.py')
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


LICENSES = module('check-licenses')
LANGUAGES = module('check-languages')
PLATFORMS = module('check-platforms')
UX = module('check-browser-ux')
CI = module('check-ci')


def component_tree(tmp_path):
    for path in ['crates/opensided/src/main.rs', 'crates/openside-vault/src/lib.rs',
                 'agent/src/agent.py', 'web/src/App.tsx', 'extension/src/panel.ts']:
        target = tmp_path / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text('fixture')
    return tmp_path


def test_language_audit_reports_components_and_rejects_foreign_implementation(tmp_path):
    root = component_tree(tmp_path)
    report, errors = LANGUAGES.audit(root)
    assert not errors
    assert 'crates/opensided: Rust=1' in report
    (root / 'crates/opensided/src/controller.ts').write_text('fixture')
    _, errors = LANGUAGES.audit(root)
    assert any('controller.ts' in error and 'violates Rust' in error for error in errors)
    assert any('TypeScript outside web/extension' in error for error in errors)


def test_language_audit_ignores_installed_and_generated_code_but_not_nested_source(tmp_path):
    root = component_tree(tmp_path)
    installed = root / 'web/node_modules/vendor/lib.py'
    installed.parent.mkdir(parents=True)
    installed.write_text('fixture')
    assert not LANGUAGES.audit(root)[1]
    (root / 'web/src/server.py').write_text('fixture')
    assert any('server.py' in error for error in LANGUAGES.audit(root)[1])


def test_language_audit_allows_only_named_js_assets_and_no_shell_backend(tmp_path):
    root = component_tree(tmp_path)
    (root / 'web/eslint.config.js').write_text('fixture')
    assert not LANGUAGES.audit(root)[1]
    (root / 'web/server.js').write_text('fixture')
    (root / 'crates/opensided/src/server.sh').write_text('fixture')
    errors = LANGUAGES.audit(root)[1]
    assert any('web/server.js' in error for error in errors)
    assert any('server.sh' in error and 'violates Rust' in error for error in errors)


def test_license_expressions_fail_closed_and_respect_and_or():
    allowed = LICENSES.NPM_ALLOW
    for expression in ['MIT', '(MIT OR GPL-3.0)', 'Apache-2.0 AND MIT', '(MIT OR CC0-1.0)']:
        assert LICENSES.allowed_expression(expression, allowed)
    for expression in ['GPL-3.0', 'MIT AND GPL-3.0', 'UNKNOWN', 'MIT OR', 'MIT; GPL-3.0', '(MIT', 'MIT GPL-3.0', '']:
        assert not LICENSES.allowed_expression(expression, allowed)


def test_rust_embedded_ijg_notice_keeps_copyright_and_binary_acknowledgment(tmp_path):
    (tmp_path / 'LICENSE-MIT').write_text('MIT license fixture')
    source = tmp_path / 'src/fdct.rs'
    source.parent.mkdir()
    header = '/*\n * Independent JPEG Group\n * Copyright (C) fixture author\n * NO WARRANTY\n */'
    source.write_text(header + '\nfn unrelated_implementation() {}')
    package = {'name': 'jpeg-encoder', 'manifest_path': str(tmp_path / 'Cargo.toml')}
    files = dict(LICENSES.rust_license_files(package))
    assert files['LICENSE-MIT'] == 'MIT license fixture'
    assert files['IJG (src/fdct.rs)'] == (
        'This software is based in part on the work of the Independent JPEG Group.\n\n' + header
    )


def test_rust_embedded_ijg_notice_fails_closed_if_upstream_header_disappears(tmp_path):
    import pytest
    source = tmp_path / 'src/fdct.rs'
    source.parent.mkdir()
    source.write_text('/* An unrelated implementation comment. */\nfn transform() {}')
    package = {'name': 'jpeg-encoder', 'manifest_path': str(tmp_path / 'Cargo.toml')}
    with pytest.raises(ValueError, match='missing its embedded IJG notice'):
        LICENSES.rust_license_files(package)


def test_npm_license_inventory_rejects_unknown_license_and_keeps_notice_text(tmp_path, monkeypatch):
    package = tmp_path / 'vendor'
    package.mkdir()
    (package / 'LICENSE').write_text('Copyright fixture author\nPermission is granted')
    data = {'MIT': [{'name': 'fixture', 'versions': ['1.0'], 'paths': [str(package)]}]}
    import json
    for component in ("web", "extension"):
        modules = tmp_path / component / "node_modules/.modules.yaml"
        modules.parent.mkdir(parents=True)
        modules.write_text('storeDir: "/tmp/isolated license store"\n')
    def inventory(command, **kwargs):
        assert "--config.store-dir=/tmp/isolated license store" in command
        return json.dumps(data)
    monkeypatch.setattr(LICENSES.subprocess, 'check_output', inventory)
    records = LICENSES.npm_inventory(tmp_path)
    assert len(records) == 2
    assert records[0]['texts'] == [('LICENSE', 'Copyright fixture author\nPermission is granted')]
    data['GPL-3.0'] = data.pop('MIT')
    import pytest
    with pytest.raises(ValueError, match='prohibited or unknown license'):
        LICENSES.npm_inventory(tmp_path)
    data.clear()
    data['MIT'] = []
    with pytest.raises(ValueError, match='empty license inventory'):
        LICENSES.npm_inventory(tmp_path)


def python_env(root, component, packages):
    lock = '\n'.join(f'[[package]]\nname = "{name}"\nversion = "{version}"\nsource = {{ registry = "https://pypi.org/simple" }}\n'
                     for name, version, _ in packages)
    (root / component).mkdir(parents=True, exist_ok=True)
    (root / component / 'uv.lock').write_text(lock + f'[[package]]\nname = "{component}"\nversion = "0.1.0"\nsource = {{ editable = "." }}\n')
    for name, version, license_line in packages:
        dist = root / component / f'.venv/lib/python3.13/site-packages/{name}-{version}.dist-info'
        (dist / 'licenses').mkdir(parents=True)
        (dist / 'METADATA').write_text(f'Metadata-Version: 2.4\nName: {name}\nVersion: {version}\n{license_line}\n\nbody')
        (dist / 'licenses/LICENSE').write_text(f'{name} licence text')


def test_python_license_inventory_reads_venv_metadata_and_fails_closed(tmp_path):
    import pytest
    python_env(tmp_path, 'agent', [('certifi', '1.0', 'License: MPL-2.0'), ('tenacity', '2.0', 'License: Apache 2.0')])
    python_env(tmp_path, 'bench', [('pytest', '3.0', 'License-Expression: MIT')])
    records = LICENSES.python_inventory(tmp_path)
    assert [(r['component'], r['name'], r['license']) for r in records] == [
        ('agent', 'certifi', 'MPL-2.0'), ('agent', 'tenacity', 'Apache-2.0'), ('bench', 'pytest', 'MIT')]
    assert records[0]['texts'] == [('LICENSE', 'certifi licence text')]
    # MPL-2.0 is accepted only for the named unmodified packages.
    other = tmp_path / 'other'
    python_env(other, 'agent', [('fixture', '1.0', 'License: MPL-2.0')])
    python_env(other, 'bench', [('pytest', '3.0', 'License-Expression: MIT')])
    with pytest.raises(ValueError, match='prohibited or unknown license MPL-2.0'):
        LICENSES.python_inventory(other)
    lock = tmp_path / 'bench/uv.lock'
    lock.write_text(lock.read_text().replace('"3.0"', '"3.1"'))
    with pytest.raises(ValueError, match='installed 3.0, locked 3.1'):
        LICENSES.python_inventory(tmp_path)


def test_platform_compile_checker_executes_only_installed_targets():
    calls = []
    def run(argv, **kwargs):
        calls.append(argv)
        return SimpleNamespace(returncode=0)
    missing, failures = PLATFORMS.check({'x86_64-unknown-linux-gnu'}, run)
    assert len(missing) == len(PLATFORMS.TARGETS) and failures == 0 and not calls
    missing, failures = PLATFORMS.check({'x86_64-apple-darwin'}, run)
    assert len(calls) == 1
    assert calls[0][-1] == 'x86_64-apple-darwin'
    assert '--workspace' in calls[0] and '--all-targets' in calls[0]
    assert calls[0][calls[0].index('-j') + 1] == '4'
    assert not any(arg.startswith('-Z') for arg in calls[0])
    assert len(missing) == len(PLATFORMS.TARGETS) - 1 and failures == 0
    missing, failures = PLATFORMS.check({'x86_64-apple-darwin'}, lambda *args, **kwargs: SimpleNamespace(returncode=1))
    assert len(missing) == len(PLATFORMS.TARGETS) - 1 and failures == 1


def test_platform_compile_checker_builds_std_without_downloading_a_toolchain():
    calls = []
    def run(argv, **kwargs):
        calls.append(argv)
        return SimpleNamespace(returncode=0)
    missing, failures = PLATFORMS.check(
        {'x86_64-apple-darwin'}, run, build_std_toolchain='nightly-fixture')
    assert not missing and failures == 0 and len(calls) == len(PLATFORMS.TARGETS)
    for command in calls:
        assert command[0] == 'cargo' and '--workspace' in command and '--all-targets' in command
        assert '--locked' in command and command[command.index('-j') + 1] == '4'
        if command[command.index('--target') + 1] == 'x86_64-apple-darwin':
            assert command[1] == 'check' and '--target-dir' not in command
        else:
            assert command[1] == '+nightly-fixture'
            assert '-Zbuild-std=std,panic_abort' in command
            assert command[command.index('--target-dir') + 1] == str(PLATFORMS.TARGET_DIR)


def test_platform_compile_checker_distinguishes_external_sdk_and_rust_errors(capsys):
    def missing_sdk(argv, **kwargs):
        return SimpleNamespace(returncode=101, stdout='',
                               stderr="fatal error: 'TargetConditionals.h' file not found\n")
    missing, failures = PLATFORMS.check(set(), missing_sdk, build_std_toolchain='nightly-fixture')
    assert missing == list(PLATFORMS.TARGETS) and failures == 0
    output = capsys.readouterr().out
    assert 'BLOCKED-EXTERNAL' in output and 'User action:' in output
    assert "TargetConditionals.h" in output
    def rust_failure(argv, **kwargs):
        return SimpleNamespace(returncode=101, stdout='',
                               stderr="error[E0432]: unresolved import\nfatal error: 'stdio.h' file not found\n")
    missing, failures = PLATFORMS.check(set(), rust_failure, build_std_toolchain='nightly-fixture')
    assert not missing and failures == len(PLATFORMS.TARGETS)


def test_platform_compile_checker_selects_only_installed_nightly_with_rust_src(tmp_path):
    source = tmp_path / 'ready/lib/rustlib/src/rust/library/Cargo.toml'
    source.parent.mkdir(parents=True)
    source.write_text('[workspace]\n')
    calls = []
    def output(argv, **kwargs):
        calls.append(argv)
        if argv == ['rustup', 'toolchain', 'list']:
            return 'stable-fixture (active)\nnightly-z-missing\nnightly-a-ready\n'
        return str(tmp_path / ('ready' if argv[2] == 'nightly-a-ready' else 'missing'))
    assert PLATFORMS.find_build_std_toolchain(output) == 'nightly-a-ready'
    assert all('install' not in command for command in calls)
    assert PLATFORMS.find_build_std_toolchain(lambda *args, **kwargs: 'stable-fixture') is None


def test_platform_compile_checker_routes_msvc_through_cargo_xwin_when_available(tmp_path):
    xwin = tmp_path / 'bin/cargo-xwin'
    xwin.parent.mkdir()
    xwin.write_text('')
    calls = []
    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return SimpleNamespace(returncode=0)
    missing, failures = PLATFORMS.check(
        {'x86_64-apple-darwin', 'aarch64-apple-darwin', 'x86_64-pc-windows-gnu', 'x86_64-pc-windows-msvc'},
        run, xwin=xwin)
    assert not missing and failures == 0
    by_target = {argv[argv.index('--target') + 1]: (argv, kwargs) for argv, kwargs in calls}
    argv, kwargs = by_target['x86_64-pc-windows-msvc']
    assert argv[:3] == ['cargo', 'xwin', 'check']
    assert kwargs['env']['PATH'].split(os.pathsep)[0] == str(xwin.parent)
    for target in ('x86_64-apple-darwin', 'x86_64-pc-windows-gnu'):
        assert 'xwin' not in by_target[target][0] and 'env' not in by_target[target][1]
    calls.clear()
    missing, failures = PLATFORMS.check(
        set(), run, build_std_toolchain='nightly-fixture', xwin=xwin)
    msvc = next(argv for argv, _ in calls if argv[argv.index('--target') + 1] == 'x86_64-pc-windows-msvc')
    assert msvc[:4] == ['cargo', '+nightly-fixture', 'xwin', 'check']
    assert '-Zbuild-std=std,panic_abort' in msvc
    # Without xwin and without std for the target, MSVC stays blocked-external.
    calls.clear()
    missing, failures = PLATFORMS.check({'x86_64-apple-darwin'}, run)
    assert 'x86_64-pc-windows-msvc' in missing and len(calls) == 1


def test_find_xwin_prefers_the_environment_over_path(tmp_path):
    root = tmp_path / 'xwin-root'
    (root / 'bin').mkdir(parents=True)
    (root / 'bin/cargo-xwin').write_text('')
    assert PLATFORMS.find_xwin({'OPENSIDE_XWIN': str(root)}, lambda name: '/usr/bin/cargo-xwin') == root / 'bin/cargo-xwin'
    assert PLATFORMS.find_xwin({'OPENSIDE_XWIN': str(root / 'bin/cargo-xwin')}, lambda name: None) == root / 'bin/cargo-xwin'
    assert PLATFORMS.find_xwin({'OPENSIDE_XWIN': str(tmp_path / 'nope')}, lambda name: '/usr/bin/cargo-xwin') is None
    assert PLATFORMS.find_xwin({}, lambda name: '/usr/bin/cargo-xwin') == Path('/usr/bin/cargo-xwin')
    assert PLATFORMS.find_xwin({}, lambda name: None) is None


def test_browser_ux_checker_requires_all_items_and_real_evidence_paths(tmp_path):
    evidence = tmp_path / 'tests/ui.rs'
    evidence.parent.mkdir()
    evidence.write_text('fixture')
    text = '\n'.join(f'| {identifier} | Item | Observable AC | UI | implemented | `tests/ui.rs` |' for identifier in sorted(UX.REQUIRED))
    assert not UX.check(tmp_path, text)
    evidence.unlink()
    assert any('evidence does not exist' in error for error in UX.check(tmp_path, text))
    assert any('missing UX-01' in error for error in UX.check(tmp_path, text.replace('| UX-01 ', '| UX-99 ')))
    assert any('implemented row needs test evidence' in error for error in UX.check(tmp_path, text.replace('`tests/ui.rs`', '—')))


def test_browser_ux_exceptions_require_user_action(tmp_path):
    text = '\n'.join(f'| {identifier} | Item | AC | UI | blocked-external | Browser unavailable |' for identifier in sorted(UX.REQUIRED))
    assert any('User action' in error for error in UX.check(tmp_path, text))
    assert not UX.check(tmp_path, text.replace('Browser unavailable', 'Browser unavailable. User action: install browser and run test'))


def ci_document():
    return {
        'name': 'ci', 'on': {'push': {}, 'pull_request': {}}, 'permissions': {'contents': 'read'},
        'jobs': {
            'linux': {'runs-on': 'ubuntu-latest', 'steps': [{'run': 'tools/gate.sh'}]},
            'platform-check': {
                'runs-on': '${{ matrix.os }}', 'strategy': {'matrix': {'os': ['macos-latest', 'windows-latest']}},
                'steps': [{'run': 'cargo check --workspace --all-targets --locked -j 8'}],
            },
        },
    }


def test_ci_schema_rejects_missing_canonical_gate_native_runners_and_compile_command():
    valid = ci_document()
    assert not CI.schema_errors(valid)
    for change, expected in [
        (lambda doc: doc['jobs']['linux']['steps'][0].update(run='echo bypass'), 'canonical tools/gate.sh'),
        (lambda doc: doc['jobs']['linux'].update({'runs-on': 'macos-latest'}), 'ubuntu-latest'),
        (lambda doc: doc['jobs']['platform-check']['strategy']['matrix'].update(os=['macos-latest']), 'Both native platform runners'),
        (lambda doc: doc['jobs']['platform-check'].update({'runs-on': 'ubuntu-latest'}), 'native matrix runner'),
        (lambda doc: doc['jobs']['platform-check']['steps'][0].update(run='cargo check -p one'), 'same workspace'),
        (lambda doc: doc['jobs']['platform-check']['strategy']['matrix'].update(os=[{}]), 'Both native platform runners'),
    ]:
        document = copy.deepcopy(valid)
        change(document)
        assert any(expected in error for error in CI.schema_errors(document))


def test_ci_schema_rejects_invalid_workflow_job_and_step_structure():
    assert CI.schema_errors([])
    assert CI.schema_errors({'jobs': {}})
    for job in [[], {}, {'runs-on': 'ubuntu-latest', 'steps': []},
                {'runs-on': 'ubuntu-latest', 'steps': 'not a list'},
                {'runs-on': 'ubuntu-latest', 'steps': [None]},
                {'runs-on': 'ubuntu-latest', 'steps': [{}]},
                {'runs-on': 'ubuntu-latest', 'steps': [{'run': 7}]},
                {'runs-on': 'ubuntu-latest', 'steps': [{'run': 'echo ok', 'uses': 'action@v1'}]}]:
        document = ci_document()
        document['jobs']['linux'] = job
        assert CI.schema_errors(document)


def test_ci_actionlint_never_bypasses_contract_validation_and_either_failure_fails(monkeypatch):
    monkeypatch.setenv('ACTIONLINT', 'fixture-actionlint')
    monkeypatch.setattr(CI.sys, 'argv', ['check-ci.py'])
    for lint_status, schema_status in [(0, 0), (1, 0), (0, 1)]:
        calls = []
        statuses = iter([lint_status, schema_status])
        def run(command, **kwargs):
            calls.append(command)
            return SimpleNamespace(returncode=next(statuses))
        monkeypatch.setattr(CI.subprocess, 'run', run)
        assert CI.main() == int(bool(lint_status or schema_status))
        assert len(calls) == 2
        assert calls[0][0] == 'fixture-actionlint'
        assert calls[1][0] == 'uv' and calls[1][-1] == '--schema'
        assert 'jsonschema' not in calls[1]


def test_ci_missing_actionlint_is_explicit_and_still_runs_contracts(monkeypatch, capsys):
    monkeypatch.delenv('ACTIONLINT', raising=False)
    monkeypatch.setattr(CI.shutil, 'which', lambda name: None)
    monkeypatch.setattr(CI.sys, 'argv', ['check-ci.py'])
    calls = []
    def run(command, **kwargs):
        calls.append(command)
        return SimpleNamespace(returncode=0)
    monkeypatch.setattr(CI.subprocess, 'run', run)
    assert CI.main() == 0
    assert len(calls) == 1 and calls[0][-1] == '--schema'
    assert 'actionlint is unavailable' in capsys.readouterr().err


def test_license_main_rejects_stale_notice_and_cargo_deny_failure(tmp_path, monkeypatch):
    monkeypatch.setattr(LICENSES, 'ROOT', tmp_path)
    monkeypatch.setattr(LICENSES.sys, 'argv', ['check-licenses.py'])
    (tmp_path / 'deny.toml').write_text('[licenses]\n')
    (tmp_path / 'NOTICE').write_text('stale')
    monkeypatch.setattr(LICENSES, 'notice', lambda root: 'current')
    monkeypatch.setattr(LICENSES.subprocess, 'run', lambda *args, **kwargs: SimpleNamespace(returncode=0))
    assert LICENSES.main() == 1
    assert (tmp_path / 'NOTICE').read_text() == 'stale'
    def fail(command, **kwargs):
        raise LICENSES.subprocess.CalledProcessError(1, command)
    monkeypatch.setattr(LICENSES.subprocess, 'run', fail)
    assert LICENSES.main() == 1
    assert (tmp_path / 'NOTICE').read_text() == 'stale'
