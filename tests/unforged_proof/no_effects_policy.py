"""Bind independently audited replay source bytes; not operator boolean approval.

The committed manifest is populated only after independent source review. Changes
in any selected source/build/manifest file require renewed review and pins. The
source audit complements (does not replace) graph, binary and hosted controls.
"""
import hashlib
from pathlib import Path
import re
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'crates/coglet-gate/python'))
from guarded_transport import strict_json
ROOTS=('tests/unforged_proof/driver/','tools/visualizer/native/',
       'crates/coglet-runtime/','crates/coglet-engine/','crates/coglet-engine-wamr/',
       'crates/coglet-db/','crates/coglet-fs/','crates/coglet-types/','crates/coglet-types-hub0/')

def selected_sources():
    raw=subprocess.check_output(['git','ls-files','-z'],cwd=ROOT)
    paths=[p.decode() for p in raw.split(b'\0') if p]
    return {p:(ROOT/p).read_bytes() for p in paths if
            p in ('Cargo.toml','.cargo/config.toml','rust-toolchain.toml') or
            (p.startswith(ROOTS) and
             (p.endswith(('.rs','.c','.h','.def','.toml','.lock')) or p.endswith('CMakeLists.txt')))}

def verify_files(files,pins):
    if not isinstance(pins,dict) or not pins or set(files)!=set(pins):raise ValueError('source audit file set differs')
    for p,raw in files.items():
        if not isinstance(pins[p],str) or not re.fullmatch('[0-9a-f]{64}',pins[p]) or hashlib.sha256(raw).hexdigest()!=pins[p]:
            raise ValueError('source audit bytes differ: '+p)
    return dict(pins)

def check():
    manifest=strict_json((Path(__file__).parent/'no-effects-source-pins.json').read_bytes())
    if set(manifest)!={'version','files'} or manifest['version']!=1:raise ValueError('source audit schema differs')
    return verify_files(selected_sources(),manifest['files'])
