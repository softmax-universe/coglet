import XCTest
@testable import SoftmaxCore

final class FaceTests: XCTestCase {
    func testStatesMapToDesignParametersAndClampLevels() {
        XCTAssertEqual(FaceParameters(state: .idle).mouthHeight, 16)
        XCTAssertEqual(FaceParameters(state: .listening(level: 2)).headScale, 1.025)
        XCTAssertEqual(FaceParameters(state: .thinking).eyeHeight, 22)
        XCTAssertEqual(FaceParameters(state: .speaking(level: 1)).mouthHeight, 88)
        XCTAssertEqual(FaceParameters(state: .speaking(level: -.infinity)).mouthHeight, 16)
        XCTAssertEqual(FaceParameters(state: .speaking(level: .nan)).mouthHeight, 16)
        XCTAssertEqual(FaceParameters(state: .listening(level: 1), reduceMotion: true).headScale, 1)
    }
}
