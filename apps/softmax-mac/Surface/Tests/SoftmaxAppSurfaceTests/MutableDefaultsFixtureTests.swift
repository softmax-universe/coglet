import Foundation
import XCTest

final class MutableDefaultsFixtureTests: XCTestCase {
    func testArgumentOverrideShadowsPersistentWritesUntilRemoved() throws {
        // A private suite and unique key: never read/write the user's standard
        // defaults or change the process launch arguments.
        let suite = "SoftmaxMutableDefaultsFixture-\(UUID().uuidString)"
        let key = "selection-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer {
            defaults.removeVolatileDomain(forName: UserDefaults.argumentDomain)
            defaults.removePersistentDomain(forName: suite)
        }
        defaults.setVolatileDomain([key: "Light"], forName: UserDefaults.argumentDomain)
        XCTAssertEqual(defaults.string(forKey: key), "Light")
        defaults.set("Dark", forKey: key)
        XCTAssertEqual(defaults.persistentDomain(forName: suite)?[key] as? String, "Dark")
        XCTAssertEqual(defaults.string(forKey: key), "Light",
            "Launch arguments are overrides, not mutable setting seeds")
        defaults.removeVolatileDomain(forName: UserDefaults.argumentDomain)
        XCTAssertEqual(defaults.string(forKey: key), "Dark")
    }
}
