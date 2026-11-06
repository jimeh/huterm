// An external Accessibility client for the terminal accessibility smoke. It
// reads an application's focused element as assistive technology does,
// through the same AX attributes VoiceOver and voice-control tools use.
//
//   ax-probe read <pid> [points] [lines]   prints one JSON object describing the focused element
//   ax-probe observe <pid>                 prints "ready", then one line per text notification
//   ax-probe tree <pid>                    prints the application's elements, for diagnosing a failed read
//
// `points` is "x,y;x,y" in global display coordinates with a top-left
// origin; for each point the probe reports AXRangeForPosition, the string
// and AXBoundsForRange of that range. `lines` is "0,3" and reports
// AXRangeForLine and its string for each line. The read also reports
// AXRangeForLine for AXInsertionPointLineNumber.
import AppKit
import ApplicationServices

func fail(_ message: String) -> Never {
    fputs("ax-probe: \(message)\n", stderr)
    exit(1)
}

func pid(_ index: Int) -> pid_t {
    let arguments = CommandLine.arguments
    guard index < arguments.count, let value = Int32(arguments[index]) else {
        fail("argument \(index) must be a process id")
    }
    return value
}

func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}

func parameterized(_ element: AXUIElement, _ name: String, _ parameter: AnyObject) -> AnyObject? {
    var value: AnyObject?
    guard AXUIElementCopyParameterizedAttributeValue(element, name as CFString, parameter, &value) == .success else {
        return nil
    }
    return value
}

func range(_ value: AnyObject?) -> CFRange? {
    guard let value, CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var range = CFRange()
    return AXValueGetValue(value as! AXValue, .cfRange, &range) ? range : nil
}

func rect(_ value: AnyObject?) -> CGRect? {
    guard let value, CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var rect = CGRect.zero
    return AXValueGetValue(value as! AXValue, .cgRect, &rect) ? rect : nil
}

func rangeValue(_ range: CFRange) -> AXValue {
    var range = range
    return AXValueCreate(.cfRange, &range)!
}

func json(_ range: CFRange?) -> Any {
    guard let range else { return NSNull() }
    return [range.location, range.length]
}

func json(_ rect: CGRect?) -> Any {
    guard let rect else { return NSNull() }
    return [rect.origin.x, rect.origin.y, rect.size.width, rect.size.height]
}

func string(_ element: AXUIElement, _ range: CFRange?) -> Any {
    guard let range else { return NSNull() }
    return parameterized(element, kAXStringForRangeParameterizedAttribute, rangeValue(range)) as? String ?? NSNull()
}

func bounds(_ element: AXUIElement, _ range: CFRange?) -> CGRect? {
    guard let range else { return nil }
    return rect(parameterized(element, kAXBoundsForRangeParameterizedAttribute, rangeValue(range)))
}

func focused(_ pid: pid_t) -> AXUIElement? {
    let application = AXUIElementCreateApplication(pid)
    guard let element = attribute(application, kAXFocusedUIElementAttribute) else { return nil }
    return (element as! AXUIElement)
}

/// The focused text area inside the application's focused window, for an
/// application whose own focused element stops short of it.
func focusedTextArea(_ pid: pid_t) -> AXUIElement? {
    let application = AXUIElementCreateApplication(pid)
    guard let window = attribute(application, kAXFocusedWindowAttribute) else { return nil }
    var pending = [window as! AXUIElement]
    while let element = pending.popLast() {
        if attribute(element, kAXRoleAttribute) as? String == kAXTextAreaRole,
           attribute(element, kAXFocusedAttribute) as? Bool == true {
            return element
        }
        pending.append(contentsOf: attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? [])
    }
    return nil
}

func read(_ pid: pid_t, points: String?, lines: String?) {
    guard AXIsProcessTrusted() else { fail("macOS denied Accessibility access to the probe's host") }
    var result: [String: Any] = ["trusted": true]
    // The application's focused element is what clients such as voice
    // control ask for first; it is reported, and the read continues with
    // the focused text area when it stops short of one.
    let applicationFocus = focused(pid)
    result["applicationFocusRole"] = applicationFocus.flatMap { attribute($0, kAXRoleAttribute) as? String } ?? NSNull()
    let element: AXUIElement
    if let applicationFocus, attribute(applicationFocus, kAXRoleAttribute) as? String == kAXTextAreaRole {
        element = applicationFocus
    } else if let textArea = focusedTextArea(pid) {
        element = textArea
    } else {
        result["focused"] = false
        emit(result)
        return
    }
    result["focused"] = true
    result["role"] = attribute(element, kAXRoleAttribute) as? String ?? NSNull()
    result["title"] = attribute(element, kAXTitleAttribute) as? String ?? NSNull()
    result["description"] = attribute(element, kAXDescriptionAttribute) as? String ?? NSNull()
    result["value"] = attribute(element, kAXValueAttribute) as? String ?? NSNull()
    result["characters"] = attribute(element, kAXNumberOfCharactersAttribute) as? Int ?? NSNull()
    let visible = range(attribute(element, kAXVisibleCharacterRangeAttribute))
    result["visible"] = json(visible)
    result["visibleString"] = string(element, visible)
    let selected = range(attribute(element, kAXSelectedTextRangeAttribute))
    result["selected"] = json(selected)
    result["selectedText"] = attribute(element, kAXSelectedTextAttribute) as? String ?? NSNull()
    result["selectedBounds"] = json(bounds(element, selected))
    let line = attribute(element, kAXInsertionPointLineNumberAttribute) as? Int
    result["insertionLine"] = line ?? NSNull()
    if let line {
        let lineRange = range(parameterized(element, kAXRangeForLineParameterizedAttribute, line as NSNumber))
        result["insertionLineRange"] = json(lineRange)
    }
    var frame = CGRect.zero
    if let position = attribute(element, kAXPositionAttribute), let size = attribute(element, kAXSizeAttribute) {
        var origin = CGPoint.zero
        var extent = CGSize.zero
        AXValueGetValue(position as! AXValue, .cgPoint, &origin)
        AXValueGetValue(size as! AXValue, .cgSize, &extent)
        frame = CGRect(origin: origin, size: extent)
    }
    result["frame"] = json(frame)
    if let points {
        result["points"] = points.split(separator: ";").map { pair -> Any in
            let values = pair.split(separator: ",").compactMap { Double($0) }
            guard values.count == 2 else { fail("points take x,y pairs") }
            var point = CGPoint(x: values[0], y: values[1])
            let found = range(parameterized(element, kAXRangeForPositionParameterizedAttribute, AXValueCreate(.cgPoint, &point)!))
            return [
                "point": [point.x, point.y],
                "range": json(found),
                "string": string(element, found),
                "bounds": json(bounds(element, found)),
            ] as [String: Any]
        }
    }
    if let lines {
        result["lines"] = lines.split(separator: ",").map { item -> Any in
            guard let line = Int(item) else { fail("lines take numbers") }
            let found = range(parameterized(element, kAXRangeForLineParameterizedAttribute, line as NSNumber))
            return ["line": line, "range": json(found), "string": string(element, found)] as [String: Any]
        }
    }
    emit(result)
}

func emit(_ result: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) else {
        fail("cannot encode the result")
    }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
}

let watched = [
    kAXValueChangedNotification,
    kAXSelectedTextChangedNotification,
    kAXFocusedUIElementChangedNotification,
    kAXLayoutChangedNotification,
]

func observe(_ pid: pid_t) -> Never {
    guard AXIsProcessTrusted() else { fail("macOS denied Accessibility access to the probe's host") }
    var observer: AXObserver?
    let callback: AXObserverCallback = { _, element, notification, _ in
        var role: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &role)
        print("\(notification as String) \(role as? String ?? "-")")
        fflush(stdout)
    }
    guard AXObserverCreate(pid, callback, &observer) == .success, let observer else {
        fail("cannot create an observer for \(pid)")
    }
    let application = AXUIElementCreateApplication(pid)
    for notification in watched {
        let status = AXObserverAddNotification(observer, application, notification as CFString, nil)
        guard status == .success || status == .notificationAlreadyRegistered else {
            fail("cannot observe \(notification): \(status.rawValue)")
        }
    }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), AXObserverGetRunLoopSource(observer), .defaultMode)
    print("ready")
    fflush(stdout)
    // The smoke closes stdin to stop the probe.
    FileHandle.standardInput.readabilityHandler = { handle in
        if handle.availableData.isEmpty { exit(0) }
    }
    CFRunLoopRun()
    exit(0)
}

func tree(_ element: AXUIElement, depth: Int) {
    let role = attribute(element, kAXRoleAttribute) as? String ?? "-"
    let title = attribute(element, kAXTitleAttribute) as? String ?? ""
    let focused = attribute(element, kAXFocusedAttribute) as? Bool ?? false
    print("\(String(repeating: "  ", count: depth))\(role) \(title.debugDescription)\(focused ? " focused" : "")")
    guard depth < 6, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] else { return }
    for child in children { tree(child, depth: depth + 1) }
}

let arguments = CommandLine.arguments
guard arguments.count >= 3 else { fail("usage: ax-probe read <pid> [points] [lines] | observe <pid>") }
switch arguments[1] {
case "read":
    read(pid(2), points: arguments.count > 3 ? arguments[3] : nil, lines: arguments.count > 4 ? arguments[4] : nil)
case "observe":
    observe(pid(2))
case "tree":
    guard AXIsProcessTrusted() else { fail("macOS denied Accessibility access to the probe's host") }
    let application = AXUIElementCreateApplication(pid(2))
    print("focused window: \(attribute(application, kAXFocusedWindowAttribute) != nil)")
    print("focused element: \(attribute(application, kAXFocusedUIElementAttribute) != nil)")
    tree(application, depth: 0)
default:
    fail("unknown command \(arguments[1])")
}
