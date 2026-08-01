#!/usr/bin/env swift
// Listen-only scroll tap for Terminal-Fenster. Emits one JSON object per line:
// {"dx":0.0,"dy":-1.2,"phase":1}
//
// Requires Accessibility permission (System Settings → Privacy → Accessibility).
// Only forwards events while a Kitty-capable terminal is frontmost.

import ApplicationServices
import AppKit
import Foundation

let allowedBundles: Set<String> = [
    "com.mitchellh.ghostty",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "dev.cmux.cmux",
    "com.googlecode.iterm2",
]

func frontmostAllowed() -> Bool {
    guard let app = NSWorkspace.shared.frontmostApplication,
          let id = app.bundleIdentifier else { return false }
    return allowedBundles.contains(id)
}

var gTap: CFMachPort?

let callback: CGEventTapCallBack = { _, type, event, _ in
    if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
        if let tap = gTap {
            CGEvent.tapEnable(tap: tap, enable: true)
        }
        return Unmanaged.passUnretained(event)
    }
    guard type == .scrollWheel, frontmostAllowed() else {
        return Unmanaged.passUnretained(event)
    }
    let dy = event.getDoubleValueField(.scrollWheelEventDeltaAxis1)
    let dx = event.getDoubleValueField(.scrollWheelEventDeltaAxis2)
    let phase = event.getIntegerValueField(.scrollWheelEventScrollPhase)
    if dx == 0 && dy == 0 { return Unmanaged.passUnretained(event) }
    let line = "{\"dx\":\(dx),\"dy\":\(dy),\"phase\":\(phase)}\n"
    FileHandle.standardOutput.write(line.data(using: .utf8)!)
    return Unmanaged.passUnretained(event)
}

let mask = CGEventMask(1 << CGEventType.scrollWheel.rawValue)
guard let tap = CGEvent.tapCreate(
    tap: .cgSessionEventTap,
    place: .headInsertEventTap,
    options: .listenOnly,
    eventsOfInterest: mask,
    callback: callback,
    userInfo: nil
) else {
    fputs("{\"error\":\"event_tap_failed\"}\n", stderr)
    exit(1)
}
gTap = tap
let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
CGEvent.tapEnable(tap: tap, enable: true)
RunLoop.main.run()
