// Real session pointer events for title-bar smokes. The window server
// decides title-bar drags and double-clicks from HID input; NSEvents an
// application posts to itself never reach that path.
//
//   hid-pointer frame <pid>                     prints "x y w h" for the pid's largest on-screen window
//   hid-pointer visible                         prints "x y w h" for the main screen's visible frame
//   hid-pointer at <x> <y>                      lists the on-screen windows under a point, front to back
//   hid-pointer place <pid> <x> <y>             moves the pid's first window through Accessibility
//   hid-pointer click <x> <y> [count] [right]   clicks count times at a global point
//   hid-pointer drag <x0> <y0> <x1> <y1>        presses, drags, and releases the left button
//
// Points are global display coordinates with a top-left origin, matching
// CGWindowList bounds.
import AppKit
import ApplicationServices

func fail(_ message: String) -> Never {
    fputs("hid-pointer: \(message)\n", stderr)
    exit(1)
}

func number(_ index: Int) -> Double {
    let arguments = CommandLine.arguments
    guard index < arguments.count, let value = Double(arguments[index]) else {
        fail("argument \(index) must be a number")
    }
    return value
}

func show(_ rect: CGRect) {
    print("\(rect.origin.x) \(rect.origin.y) \(rect.size.width) \(rect.size.height)")
}

func windowFrame(pid: Int32) -> CGRect? {
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return windows
        .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
        .compactMap { info -> CGRect? in
            guard let bounds = info[kCGWindowBounds as String] as? NSDictionary else { return nil }
            return CGRect(dictionaryRepresentation: bounds as CFDictionary)
        }
        .max { $0.width * $0.height < $1.width * $1.height }
}

let source = CGEventSource(stateID: .hidSystemState)

// Human-scale gaps between synthesized events. The window server tracks
// drags and double-clicks by event timing, so this paces input generation;
// callers still wait on observable state for every outcome.
func pace() {
    usleep(25_000)
}

func post(_ type: CGEventType, _ point: CGPoint, _ button: CGMouseButton = .left, clicks: Int64 = 1) {
    guard CGPreflightPostEventAccess() else {
        fail("macOS denied session event posting; grant Accessibility event-posting permission to the smoke host")
    }
    guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button) else {
        fail("cannot create a \(type.rawValue) event")
    }
    event.setIntegerValueField(.mouseEventClickState, value: clicks)
    event.post(tap: .cghidEventTap)
    pace()
}

let arguments = CommandLine.arguments
guard arguments.count >= 2 else { fail("expected a command") }
switch arguments[1] {
case "frame":
    guard arguments.count == 3, let pid = Int32(arguments[2]) else { fail("frame <pid>") }
    guard let frame = windowFrame(pid: pid) else { fail("no on-screen window for pid \(pid)") }
    show(frame)
case "at":
    // Diagnostics: the on-screen windows under a point, front to back.
    let point = CGPoint(x: number(2), y: number(3))
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    for info in windows {
        guard let bounds = info[kCGWindowBounds as String] as? NSDictionary,
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary), rect.contains(point) else { continue }
        let owner = info[kCGWindowOwnerName as String] as? String ?? "?"
        let name = info[kCGWindowName as String] as? String ?? ""
        print("\(owner) pid=\(info[kCGWindowOwnerPID as String] ?? 0) layer=\(info[kCGWindowLayer as String] ?? 0) name=\(name) bounds=\(rect)")
    }
case "place":
    guard arguments.count == 5, let pid = Int32(arguments[2]) else { fail("place <pid> <x> <y>") }
    guard AXIsProcessTrusted() else { fail("macOS denied Accessibility access to the smoke host") }
    var windows: CFTypeRef?
    let application = AXUIElementCreateApplication(pid)
    guard AXUIElementCopyAttributeValue(application, kAXWindowsAttribute as CFString, &windows) == .success,
          let window = (windows as? [AXUIElement])?.first else { fail("no Accessibility window for pid \(pid)") }
    var origin = CGPoint(x: number(3), y: number(4))
    guard let position = AXValueCreate(.cgPoint, &origin),
          AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, position) == .success else {
        fail("cannot move the window of pid \(pid)")
    }
case "visible":
    guard let screen = NSScreen.main, let primary = NSScreen.screens.first else { fail("no screen") }
    // AppKit's origin is the primary display's bottom-left corner.
    let visible = screen.visibleFrame
    show(CGRect(x: visible.minX, y: primary.frame.maxY - visible.maxY, width: visible.width, height: visible.height))
case "click":
    let point = CGPoint(x: number(2), y: number(3))
    let count = arguments.count > 4 ? Int64(number(4)) : 1
    let right = arguments.count > 5 && arguments[5] == "right"
    post(.mouseMoved, point)
    for click in 1...max(count, 1) {
        post(right ? .rightMouseDown : .leftMouseDown, point, right ? .right : .left, clicks: click)
        post(right ? .rightMouseUp : .leftMouseUp, point, right ? .right : .left, clicks: click)
    }
case "drag":
    let start = CGPoint(x: number(2), y: number(3))
    let end = CGPoint(x: number(4), y: number(5))
    post(.mouseMoved, start)
    post(.leftMouseDown, start)
    // Real pointer motion arrives in small increments. Large jumps can
    // leave a 32-point title row before its first drag event.
    let steps = max(12, Int((hypot(end.x - start.x, end.y - start.y) / 8).rounded(.up)))
    for step in 1...steps {
        let fraction = Double(step) / Double(steps)
        post(.leftMouseDragged, CGPoint(x: start.x + (end.x - start.x) * fraction, y: start.y + (end.y - start.y) * fraction))
        // Hold after the first motion, as a hand settles into a drag: an
        // application-started window move reaches the window server late.
        if step == 1 { usleep(250_000) }
    }
    usleep(250_000)
    post(.leftMouseUp, end)
default:
    fail("unknown command \(arguments[1])")
}
