import AppKit
import CoreGraphics
import Foundation

struct PasteboardArchive: Codable {
    let items: [[String: Data]]
}

enum WitnessError: Error, CustomStringConvertible {
    case usage
    case invalidArchive
    case invalidProcess(String)
    case pasteboardWrite

    var description: String {
        switch self {
        case .usage:
            return "usage: clipboard-witness <read|save|restore> <file> | <ready|hide|activate> <pid>"
        case .invalidArchive:
            return "clipboard archive contains an invalid pasteboard type"
        case let .invalidProcess(value):
            return "no running application has pid \(value)"
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
        throw WitnessError.invalidProcess(value)
    }
    return app
}

do {
    guard CommandLine.arguments.count == 3 else { throw WitnessError.usage }
    let command = CommandLine.arguments[1]
    let value = CommandLine.arguments[2]
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
        let pid = try runningApplication(value).processIdentifier
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        guard windows.contains(where: {
            ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid
        }) else {
            throw WitnessError.invalidProcess(value)
        }
    case "hide":
        let app = try runningApplication(value)
        guard app.hide() else { throw WitnessError.invalidProcess(value) }
    case "activate":
        let app = try runningApplication(value)
        let options: NSApplication.ActivationOptions = [.activateAllWindows, .activateIgnoringOtherApps]
        guard app.activate(options: options) else {
            throw WitnessError.invalidProcess(value)
        }
    default:
        throw WitnessError.usage
    }
} catch {
    fputs("clipboard-witness: \(error)\n", stderr)
    exit(1)
}
