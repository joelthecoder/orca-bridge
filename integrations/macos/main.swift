import AppKit
import CoreServices

// A tiny macOS URL entry point. Rust validates the URL and resolves exact ownership.
if CommandLine.arguments.contains("--check-handler") {
    let handler = NSWorkspace.shared.urlForApplication(toOpen: URL(string: "orca-bridge://session/probe")!)
        .flatMap { Bundle(url: $0)?.bundleIdentifier }
    print(handler ?? "unregistered")
    exit(handler == "dev.joel.orca-bridge.url" ? 0 : 1)
}

final class BridgeDelegate: NSObject, NSApplicationDelegate {
    func applicationWillFinishLaunching(_ notification: Notification) {
        NSAppleEventManager.shared().setEventHandler(
            self, andSelector: #selector(openURL(_:reply:)),
            forEventClass: AEEventClass(kInternetEventClass),
            andEventID: AEEventID(kAEGetURL))
    }

    @objc func openURL(_ event: NSAppleEventDescriptor, reply: NSAppleEventDescriptor) {
        guard let url = event.paramDescriptor(forKeyword: keyDirectObject)?.stringValue else { return }
        launch(url)
    }

    func launch(_ url: String) {
        guard let binary = Bundle.main.url(forResource: "orca-bridge", withExtension: nil) else {
            showError("The bundled Orca Bridge CLI is missing. Run make url-install again.")
            return
        }
        DispatchQueue.global().async {
            let process = Process()
            process.executableURL = binary
            process.arguments = ["open", url, "--json"]
            process.standardOutput = FileHandle.nullDevice
            let errors = Pipe()
            process.standardError = errors
            do {
                try process.run()
                let data = errors.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                if process.terminationStatus != 0 {
                    let message = String(data: data, encoding: .utf8) ?? "Orca Bridge could not open the session."
                    DispatchQueue.main.async { self.showError(String(message.prefix(2000))) }
                }
            } catch {
                DispatchQueue.main.async { self.showError(error.localizedDescription) }
            }
        }
    }

    func showError(_ message: String) {
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Orca Bridge"
        alert.informativeText = message
        alert.runModal()
    }
}

let application = NSApplication.shared
let delegate = BridgeDelegate()
application.delegate = delegate
application.setActivationPolicy(.accessory)
application.run()
