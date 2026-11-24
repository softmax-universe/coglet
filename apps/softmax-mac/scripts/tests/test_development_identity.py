"""Guard development/production identity selection without accessing user secrets."""
import pathlib
import unittest

APP = pathlib.Path(__file__).resolve().parents[2]


class DevelopmentIdentityPolicyTests(unittest.TestCase):
    def test_backend_is_debug_only_and_release_selection_is_explicit(self):
        source = (APP / "Sources/SoftmaxCore/DevelopmentHubStore.swift").read_text()
        self.assertTrue(source.startswith("#if DEBUG\n"))
        self.assertTrue(source.rstrip().endswith("#endif"))
        shell = (APP / "Sources/SoftmaxApp/SoftmaxApp.swift").read_text()
        self.assertIn("#if DEBUG\n            let store: any HubStore = DevelopmentHubStore()\n            #else\n            let store: any HubStore = KeychainHubStore()\n            #endif", shell)
        self.assertNotIn("SecurityHubKeychain", source)
        self.assertNotIn("UserDefaults", source)
        self.assertIn('".softmax-development-hub"', source)

    def test_production_policy_retains_no_legacy_fallback(self):
        source = (APP / "Sources/SoftmaxCore/HubAccount.swift").read_text()
        self.assertIn("kSecUseDataProtectionKeychain as String: true", source)
        self.assertIn("kSecAttrSynchronizable as String: false", source)
        self.assertIn("kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly", source)
        self.assertNotIn("DevelopmentHubStore", source)
        self.assertNotIn("kSecUseDataProtectionKeychain as String: false", source)


if __name__ == "__main__":
    unittest.main()
