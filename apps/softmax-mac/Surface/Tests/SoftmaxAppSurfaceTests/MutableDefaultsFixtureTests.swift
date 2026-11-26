import Foundation
import XCTest

final class MutableDefaultsFixtureTests: XCTestCase {
    func testArgumentOverrideTakesPrecedenceOverPersistentWrites() throws {
        // A private suite and unique keys: never read/write the user's standard
        // persistent defaults or change the process launch arguments.
        let suite = "SoftmaxMutableDefaultsFixture-\(UUID().uuidString)"
        let overriddenKey = "overridden-\(UUID().uuidString)"
        let mutableKey = "mutable-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let originalArguments = defaults.volatileDomain(forName: UserDefaults.argumentDomain)
        defer {
            defaults.setVolatileDomain(originalArguments, forName: UserDefaults.argumentDomain)
            defaults.removePersistentDomain(forName: suite)
        }
        var arguments = originalArguments
        arguments[overriddenKey] = "Light"
        defaults.setVolatileDomain(arguments, forName: UserDefaults.argumentDomain)
        XCTAssertEqual(defaults.string(forKey: overriddenKey), "Light")
        defaults.set("Dark", forKey: overriddenKey)
        defaults.set("Dark", forKey: mutableKey)
        let persistent = try XCTUnwrap(defaults.persistentDomain(forName: suite))
        XCTAssertEqual(persistent[overriddenKey] as? String, "Dark")
        XCTAssertEqual(persistent[mutableKey] as? String, "Dark")
        XCTAssertEqual(defaults.string(forKey: overriddenKey), "Light",
            "Launch arguments are overrides, not mutable setting seeds")
        XCTAssertEqual(defaults.string(forKey: mutableKey), "Dark",
            "Without a launch override, the effective value reflects the persistent write")
        // No assertion about post-removal Foundation search-list invalidation:
        // this fixture qualifies precedence, not cache behavior or AppStorage UI.
    }
}
