"""Source-policy regression checks; hosted execution is a separate CI gate."""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
# Keep published release qualification pinned; only disposable proofs use the
# currently working hosted image. Both are native Apple-Silicon images.
ROUTE = "${{ github.ref_type == 'tag' && 'macos-15' || 'macos-26' }}"


class ReleaseMacRoutingTests(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / '.github/workflows/release.yml').read_text()

    def test_binary_and_swift_proofs_use_same_tag_preserving_route(self):
        mac = next(line for line in self.workflow.splitlines()
                   if 'platform: macos-arm64' in line)
        self.assertIn('target: aarch64-apple-darwin', mac)
        self.assertIn('runner: "' + ROUTE + '"', mac)
        swift = self.workflow.split('  swift-package:\n', 1)[1].split('  inventory:', 1)[0]
        self.assertIn('    runs-on: ' + ROUTE, swift)
        self.assertNotIn('self-hosted', self.workflow)

    def test_no_checks_or_publication_prerequisites_removed(self):
        self.assertIn('cargo build --locked --release --target "$TARGET"', self.workflow)
        self.assertIn('"${RUNNER[@]}" "target/$TARGET/release/cogletd" --version', self.workflow)
        self.assertIn('swift build', self.workflow)
        self.assertIn('swift test --package-path sdk/surface-swift', self.workflow)
        self.assertIn('PinProbe', self.workflow)
        self.assertIn('needs: [binaries, native-armv7, packages, swift-package]', self.workflow)
        publish = self.workflow.split('  publish:\n', 1)[1]
        self.assertIn("if: github.event_name == 'push' && github.ref_type == 'tag'", publish)
        self.assertIn('needs: inventory', publish)
        self.assertIn('cancel-in-progress: false', self.workflow)

    def test_linux_matrix_and_timeouts_unchanged(self):
        for runner in ('ubuntu-24.04', 'ubuntu-24.04-arm'):
            self.assertIn('runner: ' + runner + '}', self.workflow)
        self.assertEqual(re.findall(r'^    timeout-minutes: (\d+)$', self.workflow, re.M),
                         ['45', '45', '15', '20', '10', '15'])


if __name__ == '__main__':
    unittest.main()
