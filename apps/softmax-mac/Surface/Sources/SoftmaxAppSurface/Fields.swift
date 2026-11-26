import Foundation

// Product-private exact UTF-8 lookup; generated CBOR maps intentionally aren't
// Swift dictionaries (which normalize Unicode equality).
extension CBORValue {
    subscript(_ key: String) -> CBORValue? {
        guard case .map(let fields) = self else { return nil }
        return fields.first { $0.key.utf8.elementsEqual(key.utf8) }?.value
    }
}
