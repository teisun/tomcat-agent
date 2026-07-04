#!/usr/bin/env swift

import CoreGraphics
import Foundation

struct WindowBounds: Codable {
    let x: Int
    let y: Int
    let width: Int
    let height: Int
}

struct WindowInfo: Codable {
    let ownerName: String
    let windowName: String
    let windowNumber: Int
    let bounds: WindowBounds
}

func parseBounds(_ raw: Any?) -> WindowBounds? {
    guard let dict = raw as? [String: Any] else {
        return nil
    }
    let x = Int((dict["X"] as? NSNumber)?.doubleValue ?? 0)
    let y = Int((dict["Y"] as? NSNumber)?.doubleValue ?? 0)
    let width = Int((dict["Width"] as? NSNumber)?.doubleValue ?? 0)
    let height = Int((dict["Height"] as? NSNumber)?.doubleValue ?? 0)
    guard width > 0, height > 0 else {
        return nil
    }
    return WindowBounds(x: x, y: y, width: width, height: height)
}

let arguments = Array(CommandLine.arguments.dropFirst())
guard !arguments.isEmpty else {
    fputs("usage: find-macos-window.swift <application-name> [--title <substring>]\n", stderr)
    exit(2)
}

var query = ""
var titleHint: String?
var index = 0
while index < arguments.count {
    let value = arguments[index]
    if value == "--title" {
        guard index + 1 < arguments.count else {
            fputs("missing value for --title\n", stderr)
            exit(2)
        }
        titleHint = arguments[index + 1].trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        index += 2
        continue
    }
    if query.isEmpty {
        query = value
    } else {
        query += " " + value
    }
    index += 1
}

query = query.trimmingCharacters(in: .whitespacesAndNewlines)
guard !query.isEmpty else {
    fputs("usage: find-macos-window.swift <application-name> [--title <substring>]\n", stderr)
    exit(2)
}

let loweredQuery = query.lowercased()
let queryTokens = loweredQuery
    .split { !$0.isLetter && !$0.isNumber }
    .map(String.init)
    .filter { $0.count >= 3 }

func ownerMatches(_ owner: String) -> Bool {
    let loweredOwner = owner.lowercased()
    if loweredOwner.contains(loweredQuery) || loweredQuery.contains(loweredOwner) {
        return true
    }
    return queryTokens.contains { token in
        loweredOwner.contains(token) || token.contains(loweredOwner)
    }
}

func titleMatches(_ title: String) -> Bool {
    guard let titleHint, !titleHint.isEmpty else {
        return true
    }
    return title.lowercased().contains(titleHint)
}

let rawWindows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
let windows = rawWindows as? [[String: Any]] ?? []

for window in windows {
    guard let ownerName = window[kCGWindowOwnerName as String] as? String else {
        continue
    }
    guard ownerMatches(ownerName) else {
        continue
    }
    let windowName = window[kCGWindowName as String] as? String ?? ""
    guard titleMatches(windowName) else {
        continue
    }
    guard let layer = window[kCGWindowLayer as String] as? NSNumber, layer.intValue == 0 else {
        continue
    }
    guard let windowNumber = window[kCGWindowNumber as String] as? NSNumber else {
        continue
    }
    guard let bounds = parseBounds(window[kCGWindowBounds as String]) else {
        continue
    }
    guard bounds.width >= 320, bounds.height >= 220 else {
        continue
    }
    let info = WindowInfo(
        ownerName: ownerName,
        windowName: windowName,
        windowNumber: windowNumber.intValue,
        bounds: bounds
    )
    let encoded = try JSONEncoder().encode(info)
    FileHandle.standardOutput.write(encoded)
    exit(0)
}

exit(1)
