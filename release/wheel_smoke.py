"""Run using a clean interpreter after installing the actual release wheel."""
from importlib.metadata import version
from importlib.resources import files
import json
import os
from pathlib import Path

import coglet_surface
from coglet_surface import generated

expected = os.environ.get('RELEASE_TAG', 'v0.0.0-rc1')[1:].replace('-rc', 'rc')
assert version('coglet-surface') == expected
assert callable(coglet_surface.Surface)
assert generated.__file__
assert json.loads(files('coglet_surface').joinpath('bindings.json').read_text())
assert 'site-packages' in str(Path(coglet_surface.__file__).resolve())
print(f'Installed coglet-surface {expected}: API, generated codec and packaged bindings load')
