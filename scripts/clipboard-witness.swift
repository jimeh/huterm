import AppKit
import CoreGraphics
import Foundation

struct PasteboardArchive: Codable {
    let items: [[String: Data]]
}

enum WitnessError: Error, CustomStringConvertible {
    case usage
    case invalidArchive
    case missingProcess(String)
    case notReady(String)
    case operationRefused(String, String)
    case stateTimeout(String, String, String)
    case pasteboardWrite

    var description: String {
        switch self {
        case .usage:
            return "usage: clipboard-witness <read|save|restore> <file> | <ready|hide|unhide> <pid>"
        case .invalidArchive:
            return "clipboard archive contains an invalid pasteboard type"
        case let .missingProcess(value):
            return "no running application has pid \(value)"
        case let .notReady(details):
            return "application is not ready: \(details)"
        case let .operationRefused(operation, details):
            return "application refused \(operation): \(details)"
        case let .stateTimeout(operation, expected, details):
            return "timed out after \(operation), expected \(expected): \(details)"
        case .pasteboardWrite:
            return "failed to write the macOS pasteboard"
        }
    }
}

func archive(_ pasteboard: NSPasteboard) -> PasteboardArchive {
    PasteboardArchive(items: (pasteboard.pasteboardItems ?? []).map { item in
        Dictionary(uniqueKeysWithValues: item.types.compactMap { type in
            item.data(forType: type).map { (type.rawValue, $0) }
        })
    })
}

func restore(_ saved: PasteboardArchive, to pasteboard: NSPasteboard) throws {
    let items = try saved.items.map { values in
        let item = NSPasteboardItem()
        for (name, data) in values {
            guard !name.isEmpty else { throw WitnessError.invalidArchive }
            guard item.setData(data, forType: NSPasteboard.PasteboardType(name)) else {
                throw WitnessError.invalidArchive
            }
        }
        return item
    }
    pasteboard.clearContents()
    if !items.isEmpty && !pasteboard.writeObjects(items) {
        throw WitnessError.pasteboardWrite
    }
}

func writeLengthPrefixed(_ data: Data?) throws {
    var length = (data.map { UInt64($0.count) } ?? UInt64.max).bigEndian
    try FileHandle.standardOutput.write(contentsOf: Data(bytes: &length, count: 8))
    if let data { try FileHandle.standardOutput.write(contentsOf: data) }
}

func runningApplication(_ value: String) throws -> NSRunningApplication {
    guard let pid = Int32(value), let app = NSRunningApplication(processIdentifier: pid) else {
        throw WitnessError.missingProcess(value)
    }
    return app
}

func hasOnScreenWindow(_ pid: Int32) -> Bool {
    let windows = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] ?? []
    return windows.contains {
        ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid
    }
}

func activationPolicy(_ policy: NSApplication.ActivationPolicy) -> String {
    switch policy {
    case .regular: return "regular"
    case .accessory: return "accessory"
    case .prohibited: return "prohibited"
    @unknown default: return "unknown(\(policy.rawValue))"
    }
}

func describe(_ app: NSRunningApplication) -> String {
    let bundle = app.bundleURL?.path ?? "<none>"
    let executable = app.executableURL?.path ?? "<none>"
    return [
        "pid=\(app.processIdentifier)",
        "policy=\(activationPolicy(app.activationPolicy))",
        "finished=\(app.isFinishedLaunching)",
        "terminated=\(app.isTerminated)",
        "hidden=\(app.isHidden)",
        "active=\(app.isActive)",
        "on_screen_window=\(hasOnScreenWindow(app.processIdentifier))",
        "bundle=\(bundle)",
        "executable=\(executable)",
    ].joined(separator: " ")
}

func waitForState(
    _ value: String,
    operation: String,
    expected: String,
    check: (NSRunningApplication) -> Bool
) throws {
    let deadline = Date(timeIntervalSinceNow: 3)
    while true {
        let app = try runningApplication(value)
        if check(app) { return }
        if Date() >= deadline {
            throw WitnessError.stateTimeout(operation, expected, describe(app))
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
    }
}

do {
    guard CommandLine.arguments.count == 3 else { throw WitnessError.usage }
    let command = CommandLine.arguments[1]
    let value = CommandLine.arguments[2]
    if command == "hide" || command == "unhide" {
        // Establish the caller's WindowServer connection without taking focus.
        let helper = NSApplication.shared
        let policyAccepted = helper.setActivationPolicy(.prohibited)
        fputs("clipboard-witness caller: policy_accepted=\(policyAccepted) \(describe(NSRunningApplication.current))\n", stderr)
    }
    let pasteboard = NSPasteboard.general
    switch command {
    case "read":
        let text = pasteboard.string(forType: .string)
        try writeLengthPrefixed(text?.data(using: .utf8))
    case "save":
        let data = try PropertyListEncoder().encode(archive(pasteboard))
        try data.write(to: URL(fileURLWithPath: value), options: .atomic)
    case "restore":
        let data = try Data(contentsOf: URL(fileURLWithPath: value))
        try restore(PropertyListDecoder().decode(PasteboardArchive.self, from: data), to: pasteboard)
    case "ready":
        let app = try runningApplication(value)
        guard app.isFinishedLaunching,
              !app.isTerminated,
              app.activationPolicy == .regular,
              app.bundleURL != nil,
              hasOnScreenWindow(app.processIdentifier) else {
            throw WitnessError.notReady(describe(app))
        }
    case "hide":
        let app = try runningApplication(value)
        guard app.hide() else {
            throw WitnessError.operationRefused("hide", describe(app))
        }
        try waitForState(
            value,
            operation: "hide",
            expected: "hidden=true and on_screen_window=false"
        ) { current in
            current.isHidden && !hasOnScreenWindow(current.processIdentifier)
        }
    case "unhide":
        let app = try runningApplication(value)
        guard app.unhide() else {
            throw WitnessError.operationRefused("unhide", describe(app))
        }
        try waitForState(
            value,
            operation: "unhide",
            expected: "hidden=false and on_screen_window=true"
        ) { current in
            !current.isHidden && hasOnScreenWindow(current.processIdentifier)
        }
    default:
        throw WitnessError.usage
    }
} catch {
    fputs("clipboard-witness: \(error)\n", stderr)
    exit(1)
}
