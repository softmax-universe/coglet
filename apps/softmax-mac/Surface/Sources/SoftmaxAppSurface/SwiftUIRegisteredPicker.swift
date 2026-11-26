#if canImport(SwiftUI)
import SwiftUI

/// Registration for an ORIGINAL SwiftUI `Picker` and its original choice views.
/// Nothing here renders a wrapper or a replacement control: both modifiers call
/// the existing `.coglet` registration on the view they are applied to. The
/// closures retained by that registration read and write the supplied Binding
/// live, so they must be given a Binding backed by the real model state.
@available(macOS 14.0, *)
public extension View {
    /// A choice inside a picker: role `button`, verb `press`, live value
    /// `boolean(selection == choice)`. Press ALWAYS assigns `choice` through
    /// the same Binding the user path uses; pressing the selected choice
    /// re-assigns it and never toggles it off.
    @MainActor
    func cogletPickerChoice<Selection: Equatable>(
        id: String, registry: ControlRegistry, window: String, route: String, label: String,
        selection: Binding<Selection>, choice: Selection,
        failure: @escaping (ControlRegistryFailure) -> Void
    ) -> some View {
        coglet(id: id, registry: registry, window: window, route: route,
            role: "button", label: label, verbs: ["press"],
            value: { .map([.init("kind", .text("boolean")),
                           .init("value", .boolean(selection.wrappedValue == choice))]) },
            failure: failure,
            invoke: { _ in selection.wrappedValue = choice })
    }

    /// The picker itself: role `picker`, verb `select`. The projected value is
    /// the wire of the FIRST choice equal to the live selection, or kind `none`
    /// when the model is off-grid. `select` accepts only a `value` text equal to
    /// a declared wire and mutates only the supplied Binding. Invalid or
    /// duplicate declarations fail registration (value getter throws), leaving
    /// the control unregistered.
    @MainActor
    func cogletPickerSelection<Selection: Equatable>(
        id: String, registry: ControlRegistry, window: String, route: String, label: String,
        selection: Binding<Selection>, choices: [(value: Selection, wire: String)],
        failure: @escaping (ControlRegistryFailure) -> Void
    ) -> some View {
        let declared = choices
        let wires = declared.map(\.wire)
        let wellFormed = !declared.isEmpty && declared.count <= 127 && Set(wires).count == wires.count
            && wires.allSatisfy { (try? CBORCodec.validate("app-selection", .text($0))) != nil }
        return coglet(id: id, registry: registry, window: window, route: route,
            role: "picker", label: label, verbs: ["select"],
            value: {
                guard wellFormed else { throw ControlRegistryFailure.invalidRegistration }
                let current = selection.wrappedValue
                guard let match = declared.first(where: { $0.value == current }) else {
                    return .map([.init("kind", .text("none"))])
                }
                return .map([.init("kind", .text("selection")), .init("value", .text(match.wire))])
            },
            failure: failure,
            invoke: { args in
                guard wellFormed, case .map(let fields) = args,
                      let field = fields.first(where: { $0.key == "value" }), case .text(let wire) = field.value,
                      let match = declared.first(where: { $0.wire == wire }) else {
                    throw ControlRegistryFailure.unsupported
                }
                selection.wrappedValue = match.value
            })
    }
}
#endif
