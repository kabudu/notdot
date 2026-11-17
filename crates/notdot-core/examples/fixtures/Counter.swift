// Disposable desktop fixture for host_control_probe: one window, a count and
// two buttons. Every action is reversible (Increment, then Decrement).
import AppKit

final class Delegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    let label = NSTextField(labelWithString: "Count: 0")
    var count = 0 { didSet { label.stringValue = "Count: \(count)" } }

    func applicationDidFinishLaunching(_ note: Notification) {
        window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 320, height: 140),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "notdot fixture counter"
        let up = NSButton(title: "Increment", target: self, action: #selector(increment))
        let down = NSButton(title: "Decrement", target: self, action: #selector(decrement))
        let stack = NSStackView(views: [label, up, down])
        stack.orientation = .vertical
        stack.frame = window.contentView!.bounds.insetBy(dx: 20, dy: 20)
        window.contentView!.addSubview(stack)
        window.orderFrontRegardless()
    }
    @objc func increment() { count += 1 }
    @objc func decrement() { count -= 1 }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
