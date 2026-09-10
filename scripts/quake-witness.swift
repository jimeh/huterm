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
        case "key":
            guard parts.count == 4, let code = UInt16(parts[1]), let flags = UInt64(parts[3]) else { throw NSError(domain: "QuakeWitness", code: 3) }
            try postKey(code, parts[2] == "down", CGEventFlags(rawValue: flags))
        case "text":
            guard CGPreflightPostEventAccess() else { try postKey(0, true, []); return }
            guard parts.count == 2 else { throw NSError(domain: "QuakeWitness", code: 4) }
            // Send physical US-layout keystrokes, like the global shortcut probe.
            // One event containing a whole Unicode token does not model typing.
            let keyCodes: [Character: CGKeyCode] = [
                "a": 0, "b": 11, "c": 8, "d": 2, "e": 14, "f": 3,
                "g": 5, "h": 4, "i": 34, "j": 38, "k": 40, "l": 37,
                "m": 46, "n": 45, "o": 31, "p": 35, "q": 12, "r": 15,
                "s": 1, "t": 17, "u": 32, "v": 9, "w": 13, "x": 7,
                "y": 16, "z": 6, "-": 27,
            ]
            for character in parts[1] {
                guard let code = keyCodes[character] else {
                    throw NSError(domain: "QuakeWitness", code: 5, userInfo: [NSLocalizedDescriptionKey: "unsupported physical typing fixture character: \(character)"])
                }
                try postKey(code, true, [])
                try postKey(code, false, [])
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
