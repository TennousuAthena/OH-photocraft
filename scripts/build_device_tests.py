#!/usr/bin/env python3
"""Build signed instrument HAPs in a private stage; never rewrite normal outputs."""
from __future__ import annotations

import copy
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import threading
import time


class BuildError(RuntimeError):
    pass


def json5_load(path: Path, env: dict[str, str]) -> dict:
    # Use DevEco's own JSON5 parser; do not echo credential-bearing profiles.
    node = env['CRAFT_NODE']
    parser = Path(env['CRAFT_DEVECO_CONTENTS']) / 'tools/hvigor/hvigor-ohos-plugin/node_modules/json5'
    result = subprocess.run([node, '-e',
        'process.stdout.write(JSON.stringify(require(process.argv[1]).parse(require("fs").readFileSync(process.argv[2],"utf8"))))',
        str(parser), str(path)], capture_output=True, text=True, env=env)
    if result.returncode:
        raise BuildError(f'Cannot parse {path.name}; inspect this file locally.')
    return json.loads(result.stdout)


def _run(argv: list[str], cwd: Path, env: dict[str, str], report: Path, name: str,
         *, private: bool = False, timeout: int = 1800) -> None:
    started = time.monotonic()
    print(f'{name}: running', flush=True)
    log_path = report / f'{name}.log'
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace',
                               start_new_session=True)
    def stop_process():
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
    with log_path.open('w', encoding='utf-8') as log:
        def read_output():
            for line in process.stdout:
                # Signer commands can include passwords. Only retain safe task
                # summaries; unsafe lines never reach the log or terminal.
                safe = not private or any(marker in line for marker in
                    ('Finished :', 'UP-TO-DATE :', 'BUILD SUCCESSFUL', 'BUILD FAILED'))
                if safe:
                    log.write(line)
                    log.flush()
                    if private and any(task in line for task in (
                        'CompileArkTS', 'OhosTestCompileArkTS', 'BuildNative', 'SignHap', 'BUILD SUCCESSFUL', 'BUILD FAILED')):
                        print(line.rstrip(), flush=True)
        reader = threading.Thread(target=read_output, daemon=True)
        reader.start()
        try:
            while True:
                remaining = timeout - (time.monotonic() - started)
                if remaining <= 0:
                    raise subprocess.TimeoutExpired(argv, timeout)
                try:
                    process.wait(timeout=min(30, remaining))
                    break
                except subprocess.TimeoutExpired:
                    if time.monotonic() - started >= timeout:
                        raise
                    print(f'{name}: still running ({time.monotonic() - started:.0f}s)', flush=True)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            stop_process()
            reader.join(timeout=5)
            if time.monotonic() - started >= timeout:
                raise BuildError(f'{name} timed out after {timeout}s.') from None
            raise
        finally:
            reader.join(timeout=5)
            if not reader.is_alive():
                process.stdout.close()
    print(f'{name}: {"passed" if process.returncode == 0 else "failed"} ({time.monotonic() - started:.1f}s)', flush=True)
    if process.returncode:
        suffix = ' Private Hvigor logs remain in the restricted stage.' if private else f' See {name}.log.'
        raise BuildError(f'{name} exited {process.returncode}.{suffix}')


def clone_file(source: Path, destination: Path) -> None:
    """Use APFS copy-on-write when available; keep huge native artifacts off disk twice."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    if sys_platform_is_macos():
        result = subprocess.run(['cp', '-c', str(source), str(destination)], capture_output=True)
        if result.returncode == 0:
            return
    shutil.copy2(source, destination)


def sys_platform_is_macos() -> bool:
    import sys
    return sys.platform == 'darwin'


def prepare_stage(project: Path, report: Path, archive: Path, env: dict[str, str]) -> Path:
    stage = report / 'stage' / 'harmonyos'
    if stage.exists():
        raise BuildError('Build stage already exists; use a fresh run id.')
    stage.parent.mkdir(mode=0o700)
    shutil.copytree(project / 'harmonyos', stage, ignore=shutil.ignore_patterns(
        '.hvigor', 'build', 'oh_modules', '.cxx', 'libs', 'local.properties'))
    stage.chmod(0o700)
    # ohosTest is a module test target. Applying it to the root product causes
    # Hvigor 00303072 and must never be added to app.modules[].targets.
    module_profile = json5_load(stage / 'entry/build-profile.json5', env)
    options = module_profile.setdefault('buildOption', {})
    options.setdefault('arkOptions', {}).setdefault('buildProfileFields', {})['DEVICE_TESTS'] = True
    native_options = options.setdefault('externalNativeOptions', {})
    index_tool = Path(__file__).resolve().parent / 'archive_symbol_index.py'
    if any(char.isspace() for char in str(archive) + str(index_tool)):
        raise BuildError('Hvigor native test archive/tool paths must not contain whitespace.')
    native_options['arguments'] = (native_options.get('arguments', '') +
        f' -DPHOTOCRAFT_DEVICE_TESTS=ON -DPHOTOCRAFT_RUST_ARCHIVE={archive}' +
        f' -DPHOTOCRAFT_ARCHIVE_INDEX_TOOL={index_tool}').strip()
    if not any(t.get('name') == 'ohosTest' for t in module_profile.get('targets', [])):
        module_profile.setdefault('targets', []).append({'name': 'ohosTest'})
    (stage / 'entry/build-profile.json5').write_text(json.dumps(module_profile, indent=2) + '\n')
    return stage


def source_fingerprint(project: Path) -> str:
    """Hash app/bridge sources and local Rust inputs for safe signed-build reuse."""
    digest = hashlib.sha256()
    files = set()
    for relative in ('Cargo.toml', 'Cargo.lock', 'harmonyos/build-profile.json5',
                     'harmonyos/entry/build-profile.json5', 'harmonyos/oh-package.json5',
                     'harmonyos/oh-package-lock.json5', 'harmonyos/entry/oh-package.json5',
                     'harmonyos/hvigorfile.ts', 'harmonyos/entry/hvigorfile.ts'):
        path = project / relative
        if path.is_file():
            files.add(path)
    for relative in ('apps', 'harmonyos/entry/src', 'harmonyos/AppScope'):
        directory = project / relative
        if directory.is_dir():
            files.update(path for path in directory.rglob('*') if path.is_file() and '.DS_Store' not in path.parts)
    extensions = {'.rs', '.toml', '.wgsl', '.spv', '.ttf', '.otf', '.png', '.svg', '.inc'}
    for relative in ('vendor', 'upstream'):
        directory = project / relative
        if directory.is_dir():
            files.update(path for path in directory.rglob('*') if path.is_file() and path.suffix in extensions
                         and not any(part in ('.git', 'target', 'node_modules') for part in path.relative_to(directory).parts))
    for path in sorted(files):
        digest.update(str(path.relative_to(project)).encode())
        digest.update(b'\0')
        with path.open('rb') as stream:
            digest.update(hashlib.file_digest(stream, 'sha256').digest())
    return digest.hexdigest()


def reuse_artifacts(source: Path, project: Path, bundle: str) -> dict:
    if source.is_dir():
        source = source / 'build-artifacts.json'
    if not source.is_file():
        raise BuildError('Reusable build metadata is missing; use a successful signed test build directory.')
    artifacts = json.loads(source.read_text())
    if 'artifacts' in artifacts:
        artifacts = artifacts['artifacts']
    if artifacts.get('bundle') != bundle or artifacts.get('buildMode') != 'debug-device-tests' or artifacts.get('signed') is not True:
        raise BuildError('Reusable build is not a signed device-tests build for this bundle.')
    if artifacts.get('sourceFingerprint') != source_fingerprint(project):
        raise BuildError('Reusable signed build is stale: source content changed.')
    for target in ('default', 'ohosTest'):
        item = artifacts.get(target, {})
        hap = Path(item.get('path', ''))
        if not hap.is_file() or hashlib.sha256(hap.read_bytes()).hexdigest() != item.get('sha256'):
            raise BuildError(f'Reusable signed {target} HAP is missing or its SHA256 changed.')
    return artifacts


def signed_stage_profile(project: Path, stage: Path, env: dict[str, str], bundle: str) -> dict:
    profile = json5_load(stage / 'build-profile.json5', env)
    if profile.get('app', {}).get('signingConfigs'):
        raise BuildError('Public profile unexpectedly contains signing material.')
    private = json5_load(project / '.signing/build-profile.json5', env)
    configs = copy.deepcopy(private.get('app', {}).get('signingConfigs', []))
    if not configs:
        raise BuildError('Private signing profile has no signingConfigs.')
    for config in configs:
        material = config['material']
        # Resolve relative materials against the original project, before staging.
        for key in ('certpath', 'storeFile', 'profile'):
            if key in material:
                source = Path(material[key]).expanduser()
                material[key] = str(source if source.is_absolute() else (project / 'harmonyos' / source).resolve())
                if not Path(material[key]).is_file():
                    raise BuildError(f'Private signing material {key} is unavailable.')
        decoded = subprocess.run(['openssl', 'cms', '-verify', '-inform', 'DER', '-in', material['profile'], '-noverify'],
                                 capture_output=True)
        if decoded.returncode:
            raise BuildError('Cannot decode the private provisioning profile.')
        allowed = json.loads(decoded.stdout).get('bundle-info', {}).get('bundle-name')
        if allowed not in ('*', bundle):
            raise BuildError('Private provisioning profile does not match this app bundle.')
    profile['app']['signingConfigs'] = configs
    products = {item['name']: item for item in private['app'].get('products', [])}
    names = {item['name'] for item in configs}
    for product in profile['app'].get('products', []):
        selected = products.get(product['name'], {}).get('signingConfig')
        if not selected and len(names) == 1:
            selected = next(iter(names))
        if selected not in names:
            raise BuildError('Private signing selection is ambiguous.')
        product['signingConfig'] = selected
    return profile


def build(project: Path, report: Path, env: dict[str, str], bundle: str) -> dict:
    if not (project / 'apps/photocraft-ohos/Cargo.toml').is_file() or not (project / 'harmonyos/entry/src/ohosTest/module.json5').is_file():
        raise BuildError('Device tests require the PhotoCraft application and instrument-test module.')
    if not (project / '.signing/build-profile.json5').is_file():
        raise BuildError('Device tests not executed: this app has no private debug signing profile.')
    source_before = source_fingerprint(project)
    manifest = project / 'Cargo.toml'
    # Cargo's dependency cache is large. Preserve normal top-level outputs while
    # reusing cached dependencies, then pass a separate feature archive to CMake.
    native_target = Path(env.get('CRAFT_TEST_DEVICE_TARGET_DIR', str(project / 'target')))
    build_env = env.copy()
    build_env['CARGO_TARGET_DIR'] = str(native_target)
    cached_archive = native_target / env['CRAFT_RUST_TARGET'] / 'release/libphotocraft_ohos.a'
    archive = report / 'native/libphotocraft_ohos.a'
    native_lock_path = project / '.cache/tests/native-build.lock'
    native_lock_path.parent.mkdir(parents=True, exist_ok=True)
    with native_lock_path.open('a') as native_lock:
        try:
            fcntl.flock(native_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise BuildError('Another device native build is running.') from None
        originals = {}
        for source in (cached_archive, cached_archive.with_suffix('.d'), cached_archive.with_suffix('.rlib')):
            original = report / 'native-original' / source.name
            if source.is_file():
                clone_file(source, original)
                originals[source] = original
            else:
                originals[source] = None
        try:
            _run(['cargo', 'build', '--locked', '--manifest-path', str(manifest), '--target', env['CRAFT_RUST_TARGET'],
                  '-p', 'photocraft-ohos', '--release', '--features', 'device-tests'], project, build_env, report, 'rust-device-build')
            if not cached_archive.is_file():
                raise BuildError('Feature build did not produce its native archive.')
            # Cargo can regard restored top-level output as fresh on a later
            # run. Select its actual feature fingerprint in deps instead.
            fingerprints = cached_archive.parent / '.fingerprint'
            feature_archives = []
            for fingerprint in fingerprints.glob('photocraft-ohos-*/lib-photocraft_ohos.json'):
                info = json.loads(fingerprint.read_text())
                if 'device-tests' not in json.loads(info.get('features', '[]')):
                    continue
                suffix = fingerprint.parent.name.removeprefix('photocraft-ohos-')
                candidate = cached_archive.parent / f'deps/libphotocraft_ohos-{suffix}.a'
                if candidate.is_file():
                    feature_archives.append(candidate)
            if not feature_archives:
                raise BuildError('Feature build has no device-tests fingerprint archive.')
            feature_archive = max(feature_archives, key=lambda path: path.stat().st_mtime_ns)
            clone_file(feature_archive, archive)
        finally:
            for source, original in originals.items():
                if original is not None:
                    source.unlink(missing_ok=True)
                    clone_file(original, source)
                else:
                    source.unlink(missing_ok=True)
    stage = prepare_stage(project, report, archive, env)
    build_env['HVIGOR_USER_HOME'] = str(project / '.cache/hvigor')
    hvigor_modules = Path(build_env['HVIGOR_USER_HOME']) / 'node_modules/@ohos'
    hvigor_modules.mkdir(parents=True, exist_ok=True)
    for package in ('hvigor', 'hvigor-ohos-plugin'):
        link = hvigor_modules / package
        if not link.exists() and not link.is_symlink():
            link.symlink_to(Path(env['CRAFT_DEVECO_CONTENTS']) / 'tools/hvigor' / package, target_is_directory=True)
    build_env['NODE_PATH'] = str(hvigor_modules.parent) + (os.pathsep + env['NODE_PATH'] if env.get('NODE_PATH') else '')
    _run([env['CRAFT_NODE'], env['CRAFT_OHPM'], 'install', '--all', '--cache', env['CRAFT_OHPM_CACHE'],
          '--no-experimental-concurrently-safe'], stage, build_env, report, 'ohpm-tests-install')
    profile_path = stage / 'build-profile.json5'
    original = profile_path.read_bytes()
    # Reuse the normal signer lock, but only the restricted stage receives secrets.
    with (project / '.signing/sign-local.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise BuildError('Another local signing build is already running.') from None
        try:
            profile_path.write_text(json.dumps(signed_stage_profile(project, stage, env, bundle), indent=2) + '\n')
            profile_path.chmod(0o600)
            for target in ('default', 'ohosTest'):
                _run([env['CRAFT_NODE'], env['CRAFT_HVIGOR'], '--mode', 'module', '-p', 'product=default',
                      '-p', f'module=entry@{target}', '-p', 'buildMode=debug', '-p', 'enableSignTask=true',
                      'assembleHap', '--no-daemon', '--no-parallel', '--no-stacktrace', '--no-analyze'],
                     stage, build_env, report, f'assemble-{target}-signed', private=True)
        finally:
            profile_path.write_bytes(original)
            profile_path.chmod(0o644)
    artifacts = {}
    for target in ('default', 'ohosTest'):
        candidates = list((stage / 'entry/build').glob(f'**/entry-{target}-signed.hap'))
        if len(candidates) != 1:
            raise BuildError(f'Expected exactly one signed {target} HAP, found {len(candidates)}.')
        hap = candidates[0]
        artifacts[target] = {'path': str(hap), 'sha256': hashlib.sha256(hap.read_bytes()).hexdigest(), 'bytes': hap.stat().st_size}
    artifacts['nativeArchive'] = str(archive)
    artifacts['stage'] = str(stage)
    artifacts['bundle'] = bundle
    artifacts['buildMode'] = 'debug-device-tests'
    artifacts['signed'] = True
    artifacts['sourceFingerprint'] = source_before
    if source_fingerprint(project) != source_before:
        raise BuildError('App sources changed during the signed build; rebuild before reusing these HAPs.')
    (report / 'build-artifacts.json').write_text(json.dumps(artifacts, indent=2) + '\n')
    return artifacts
