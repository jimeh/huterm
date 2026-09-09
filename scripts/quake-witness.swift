// Separate AppKit application for focus observation and real session key events.
import AppKit
import ApplicationServices

let directory = URL(fileURLWithPath: CommandLine.arguments[1])
func publish(_ name: String, _ text: String) throws {
    try text.write(to: directory.appendingPathComponent(name), atomically: true, encoding: .utf8)
}
func postKey(_ code: CGKeyCode, _ down: Bool, _ flags: CGEventFlags) throws {
    guard CGPreflightPostEventAccess() else {
        throw NSError(domain: "QuakeWitness", code: 1, userInfo: [NSLocalizedDescriptionKey: "macOS denied session event posting. Grant Accessibility event-posting permission to the smoke host; internal NSEvent dispatch is not a substitute for global-hotkey evidence."])
    }
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else {
        throw NSError(domain: "QuakeWitness", code: 2)
    }
    event.flags = flags
    event.post(tap: .cghidEventTap)
}

final class Witness: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var timer: Timer?
    var sequence = 0
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 500, height: 320), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Quake external focus witness"
        window.contentView = NSTextField(labelWithString: "External application. Quake must return focus here.")
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        try! publish("witness-ready", "pid=\(ProcessInfo.processInfo.processIdentifier)\nposting=\(CGPreflightPostEventAccess())\n")
        timer = Timer.scheduledTimer(withTimeInterval: 0.01, repeats: true) { [self] _ in
            do {
                let active = NSApp.isActive && window.isKeyWindow
                let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
                try publish("witness-state", "active=\(active)\nfront_pid=\(pid)\n")
                let commandURL = directory.appendingPathComponent("witness-command-\(sequence)")
                guard let command = try? String(contentsOf: commandURL, encoding: .utf8) else { return }
                var outcome = "ok"
                do { try execute(command) } catch { outcome = "error: \(error.localizedDescription)" }
                try publish("witness-result-\(sequence)", outcome)
                sequence += 1
            } catch {
                fputs("Quake witness failed: \(error)\n", stderr)
                NSApp.terminate(nil)
            }
        }
    }
    func execute(_ command: String) throws {
        let parts = command.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
        switch parts[0] {
        case "focus":
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
        case "activate":
            guard parts.count == 2, let pid = Int32(parts[1]), let app = NSRunningApplication(processIdentifier: pid), app.activate(options: [.activateIgnoringOtherApps]) else { throw NSError(domain: "QuakeWitness", code: 7) }
        case "key":
            guard parts.count == 4, let code = UInt16(parts[1]), let flags = UInt64(parts[3]) else { throw NSError(domain: "QuakeWitness", code: 3) }
            try postKey(code, parts[2] == "down", CGEventFlags(rawValue: flags))
        case "text":
            guard CGPreflightPostEventAccess() else { try postKey(0, true, []); return }
            guard parts.count == 2 else { throw NSError(domain: "QuakeWitness", code: 4) }
            let characters = Array(parts[1].utf16)
            for down in [true, false] {
                guard let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down) else { throw NSError(domain: "QuakeWitness", code: 5) }
                event.flags = []
                event.keyboardSetUnicodeString(stringLength: characters.count, unicodeString: characters)
                event.post(tap: .cghidEventTap)
            }
        case "quit": NSApp.terminate(nil)
        default: throw NSError(domain: "QuakeWitness", code: 6, userInfo: [NSLocalizedDescriptionKey: "unknown witness command"])
        }
    }
}
let app = NSApplication.shared
let delegate = Witness()
app.setActivationPolicy(.regular)
app.delegate = delegate
app.run()
