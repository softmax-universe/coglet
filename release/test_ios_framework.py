"""Distribution checks, not native build or simulator execution proof."""
import importlib.util
import json
from pathlib import Path
import plistlib
import tempfile
import unittest
import zipfile

SPEC = importlib.util.spec_from_file_location("ios_framework", Path(__file__).with_name("ios_framework.py"))
assert SPEC is not None and SPEC.loader is not None


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.framework = self.root / "CogletRuntime.xcframework"
        self.framework.mkdir()
        self.libs = []
        for ident, variant in [("ios-arm64", None), ("ios-arm64-simulator", "simulator")]:
            entry = {"LibraryIdentifier": ident, "LibraryPath": "libCogletRuntime.a", "HeadersPath": "Headers", "SupportedArchitectures": ["arm64"], "SupportedPlatform": "ios"}
            if variant:
                entry["SupportedPlatformVariant"] = variant
            self.libs.append(entry)
            folder = self.framework / ident
            (folder / "Headers").mkdir(parents=True)
            (folder / "libCogletRuntime.a").write_bytes(b"!<arch>\nfixture only")
            (folder / "Headers/coglet_runtime.h").write_text("fixture header")
            (folder / "Headers/module.modulemap").write_text("fixture module")
        self.save_plist()

    def save_plist(self):
        (self.framework / "Info.plist").write_bytes(plistlib.dumps({"AvailableLibraries": self.libs, "XCFrameworkFormatVersion": "1.0"}))

    def module(self):
        module = importlib.util.module_from_spec(SPEC)
        SPEC.loader.exec_module(module)
        return module

    def seal(self):
        return self.module().seal(self.framework, self.root / "out", "ios-runtime-v0.0.0-rc1", "a" * 40, "fixture rust", "fixture Xcode")

    def test_sealed_inventory_and_zip_are_bound(self):
        manifest = self.seal()
        data = json.loads(manifest.read_text())
        self.assertEqual(data["source_sha"], "a" * 40)
        self.assertEqual(len(data["slices"]), 2)
        self.module().verify(manifest.parent)
        archive = manifest.parent / data["archive"]["name"]
        with zipfile.ZipFile(archive) as z:
            self.assertEqual(len(z.namelist()), 7)
        archive.write_bytes(archive.read_bytes() + b"corruption")
        with self.assertRaises(ValueError):
            self.module().verify(manifest.parent)

    def test_missing_simulator_is_refused(self):
        self.libs.pop()
        self.save_plist()
        with self.assertRaises(ValueError):
            self.seal()

    def test_symlink_is_refused(self):
        (self.framework / "external").symlink_to(self.root)
        with self.assertRaises(ValueError):
            self.seal()

    def test_unknown_platform_and_path_are_refused(self):
        self.libs[1]["SupportedPlatform"] = "macos"
        self.save_plist()
        with self.assertRaises(ValueError):
            self.seal()
        self.libs[1]["SupportedPlatform"] = "ios"
        self.libs[1]["LibraryIdentifier"] = "../outside"
        self.save_plist()
        with self.assertRaises(ValueError):
            self.seal()

    def test_existing_output_and_invalid_identity_are_refused(self):
        self.seal()
        with self.assertRaises(FileExistsError):
            self.seal()
        with self.assertRaises(ValueError):
            self.module().seal(self.framework, self.root / "other", "v0.0.0-rc1", "a" * 40, "r", "x")


if __name__ == "__main__":
    unittest.main()
