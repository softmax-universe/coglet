"""Regenerate serving-runtime ABI timelines, without changing the spec corpus.

The runtime intentionally still speaks frozen hub 0.1. Its provenance and
boundary method names therefore stay on that contract; its ABI state hashes
and manifest/record versions are current. No recorded hashes are converted.
Run after generate_abi.py; executable companions require their native generator.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    with tempfile.TemporaryDirectory(prefix='coglet-runtime-abi-') as directory:
        stage = Path(directory)
        fixtures = stage / 'spec/fixtures/abi'
        fixtures.mkdir(parents=True)
        shutil.copyfile(ROOT / 'spec/fixtures/abi/manifest-admission.fixture',
                        fixtures / 'manifest-admission.fixture')
        (stage / 'conformance/hub').mkdir(parents=True)
        shutil.copyfile(ROOT / 'conformance/hub/event-batch-dedup.json',
                        stage / 'conformance/hub/event-batch-dedup.json')
        env = dict(os.environ, COGLET_ABI_GENERATOR_ROOT=str(stage),
                   COGLET_ABI_RUNTIME_CORPUS='1')
        subprocess.run([sys.executable, str(ROOT / 'spec/tests/generate_abi.py')],
                       env=env, check=True)
        import build_boundary_receipts as boundary
        import build_mc_contracts as mc
        boundary.SPEC = mc.SPEC = stage / 'spec'
        boundary.WRITE_METHOD = 'debug.write'
        _, timelines = boundary.generate()
        import build_manifest_admission as admission
        admission.ROOT = stage
        outputs = {'boundary-receipts.json': timelines,
                   'manifest-admission.json': admission.cases()}
        outputs['heard-archive.json'] = mc.outputs()['spec/fixtures/abi/heard-archive-timeline.fixture']
        for name, value in outputs.items():
            (stage / 'conformance/abi' / name).write_text(json.dumps(value, indent=2) + '\n')
        generated = sorted((stage / 'conformance/abi').glob('*.json'))
        for source in generated:
            target = ROOT / 'conformance/abi' / source.name
            if '--check' in sys.argv:
                if not target.exists() or target.read_bytes() != source.read_bytes():
                    raise SystemExit(f'stale runtime ABI timeline: {source.name}')
            else:
                shutil.copyfile(source, target)
        print(f'generated {len(generated)} runtime ABI timeline files')


if __name__ == '__main__':
    main()
