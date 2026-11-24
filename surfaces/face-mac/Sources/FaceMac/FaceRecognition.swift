import Foundation

public struct FaceRecognitionUpdate: Sendable {
    public let revision: UInt64
    public let text: String
    public let final: Bool
    public let positionMS: UInt64
    public init(revision: UInt64, text: String, final: Bool, positionMS: UInt64) {
        self.revision = revision; self.text = text; self.final = final; self.positionMS = positionMS
    }
}
/// Product-configured recognition route; raw PCM never crosses the surface wire.
/// Implementations deliver on MainActor and must not silently change routes.
@MainActor public protocol FaceRecognitionInput: AnyObject {
    func append(_ samples: [Int16]) throws
    func finish()
    func cancel()
}
@MainActor public protocol FaceRecognition: AnyObject {
    func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void) throws -> any FaceRecognitionInput
}
