import Foundation
import XCTest
@testable import SoftmaxAppSurface

final class AppSurfaceProvisioningTests: XCTestCase {
    private func config(profile: Any? = nil, catalog: Bool = false, names: [String] = [], resources: [String] = [], pin: String? = nil) throws -> Data {
        var setup: [String: Any] = ["manifest": ["subscriptions": names, "required_capabilities": names, "exclusive_outputs": resources]]
        if let profile { setup["contract_profile"] = profile }
        var root: [String: Any] = ["setup": setup]
        if catalog {
            let base = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            root["trusted_extension_catalog"] = ["json": try String(contentsOf: base.appendingPathComponent("catalog.v1.json"), encoding: .utf8),
                "sha256": pin ?? AppSurfaceProvisioning.catalogPin]
        }
        return try JSONSerialization.data(withJSONObject: root)
    }
    func testDefaultRetainsSpeechAndDoesNotInferCatalogOrAuthority() throws {
        let p = try AppSurfaceProvisioning(configuration: config(names: ["speech.speak_chunk"]))
        XCTAssertEqual(p.profile.rawValue, "surface-0.1")
        XCTAssertNil(p.catalog)
        XCTAssertFalse(p.enablesApp)
        XCTAssertEqual(p.operationNames, ["speech.start", "speech.end", "speech.speak_chunk"])
        XCTAssertFalse(p.resources.contains("softmax.app_control"))
        for selector: Any in ["", "surface-0.3", "0.2", 2, NSNull()] {
            XCTAssertThrowsError(try AppSurfaceProvisioning(configuration: config(profile: selector)))
        }
        XCTAssertThrowsError(try AppSurfaceProvisioning(configuration: config(profile: "surface-0.2", names: ["softmax.app.state"])))
        XCTAssertThrowsError(try AppSurfaceProvisioning(configuration: config(catalog: true, names: ["softmax.app.state"])))
    }
    func testReviewedCatalogIsPinnedAndManifestSelectionNeverGrantsSensitiveResource() throws {
        let p = try AppSurfaceProvisioning(configuration: config(profile: "surface-0.2", catalog: true,
            names: ["softmax.app.state", "softmax.app.invoke_sensitive"], resources: ["softmax.app_control"]))
        XCTAssertTrue(p.enablesApp)
        XCTAssertEqual(Set(p.operations.map(\.name)), ["speech.start", "speech.end", "softmax.app.state", "softmax.app.invoke_sensitive"])
        XCTAssertFalse(p.resources.contains("softmax.app_sensitive_control"))
        XCTAssertTrue(p.resources.contains("softmax.app_control"))
        let base = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        XCTAssertEqual(try String(contentsOf: base.appendingPathComponent("catalog.v1.sha256"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), AppSurfaceProvisioning.catalogPin)
        XCTAssertThrowsError(try AppSurfaceProvisioning(configuration: config(profile: "surface-0.2", catalog: true, pin: String(repeating: "0", count: 64))))
        XCTAssertThrowsError(try AppSurfaceProvisioning(configuration: config(profile: "surface-0.2", catalog: true, names: ["softmax.app.unknown"])))
    }
}
