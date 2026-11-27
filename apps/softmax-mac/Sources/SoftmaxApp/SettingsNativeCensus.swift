#if DEBUG
import AppKit

/// Read-only, bounded discovery for the transition probe; never a focus oracle.
@MainActor
enum SettingsNativeCensus {
    enum Role: String, Codable, CaseIterable { case waitEditorFirst, viewFocusFirst, terminal }
    enum Responder: String, Codable { case none, window, fieldEditor, textField, textView, other }
    struct Field: Codable {
        let labelMatches: Bool
        let placeholderMatches: Bool
        let attributedPlaceholderMatches: Bool
        let identifierPresent: Bool
        let editable: Bool
        let enabled: Bool
        let hidden: Bool
        let editorPresent: Bool
        let editorIsResponder: Bool
        let delegateIsResponderField: Bool
    }
    struct Snapshot: Codable {
        let role: Role
        let stepIndex: Int
        let stepTicks: Int
        let contentPresent: Bool
        let keyWindow: Bool
        let active: Bool
        let responder: Responder
        let responderInTargetWindow: Bool
        let visitedViews: Int
        let observedFields: Int
        let walkTruncated: Bool
        let fieldsTruncated: Bool
        let fields: [Field]
    }
    struct Document: Codable {
        let snapshots: [Snapshot]
        let notReached: [Role]
    }
    private static var snapshots: [Snapshot] = []
    static var document: Document {
        Document(snapshots: snapshots, notReached: Role.allCases.filter { role in !snapshots.contains { $0.role == role } })
    }
    static func capture(_ role: Role, window: NSWindow?, stepIndex: Int, stepTicks: Int) {
        guard !snapshots.contains(where: { $0.role == role }) else { return }
        let responder = window?.firstResponder
        let textView = responder as? NSTextView
        let category: Responder
        if responder == nil { category = .none }
        else if responder is NSWindow { category = .window }
        else if textView?.isFieldEditor == true { category = .fieldEditor }
        else if responder is NSTextField { category = .textField }
        else if textView != nil { category = .textView }
        else { category = .other }
        var stack: [(NSView, Int)] = window?.contentView.map { [($0, 0)] } ?? []
        var fields: [Field] = []
        var visited = 0, observed = 0
        var walkTruncated = false, fieldsTruncated = false
        while let (view, depth) = stack.popLast() {
            guard visited < 256 else { walkTruncated = true; break }
            visited += 1
            if let field = view as? NSTextField {
                observed += 1
                if fields.count < 16 {
                    let editor = field.currentEditor()
                    fields.append(Field(
                        labelMatches: field.accessibilityLabel() == "Search settings",
                        placeholderMatches: field.placeholderString == "Filter settings  /",
                        attributedPlaceholderMatches: field.placeholderAttributedString?.string == "Filter settings  /",
                        identifierPresent: !(field.accessibilityIdentifier() ?? "").isEmpty,
                        editable: field.isEditable, enabled: field.isEnabled,
                        hidden: field.isHiddenOrHasHiddenAncestor,
                        editorPresent: editor != nil,
                        editorIsResponder: editor != nil && responder != nil && editor === responder,
                        delegateIsResponderField: textView?.delegate === field))
                } else { fieldsTruncated = true }
            }
            // Bound both traversal and pending storage, including a very wide tree.
            let children = view.subviews
            if depth >= 32 {
                if !children.isEmpty { walkTruncated = true }
            } else {
                let remaining = max(0, 256 - visited - stack.count)
                if children.count > remaining { walkTruncated = true }
                stack.append(contentsOf: children.prefix(remaining).map { ($0, depth + 1) })
            }
        }
        snapshots.append(Snapshot(role: role, stepIndex: stepIndex, stepTicks: stepTicks,
                                  contentPresent: window?.contentView != nil, keyWindow: window?.isKeyWindow == true,
                                  active: NSApp.isActive, responder: category,
                                  responderInTargetWindow: (responder as? NSView)?.window != nil && (responder as? NSView)?.window === window,
                                  visitedViews: visited, observedFields: observed, walkTruncated: walkTruncated,
                                  fieldsTruncated: fieldsTruncated, fields: fields))
    }
}
#endif
