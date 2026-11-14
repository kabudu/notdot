import MetalKit
import SwiftUI

/// The real orb: canonical shader via Metal, overlays from the contract.
struct OrbView: View {
    let state: OrbState
    @State private var frame: OrbFrame?

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                OrbMetalView(state: state, frame: $frame)
                OrbOverlays(frame: frame, size: geometry.size)
            }
        }
        .accessibilityElement()
        .accessibilityLabel("notdot")
        .accessibilityValue(frame?.accessibilityLabel ?? "Ready.")
        .accessibilityAddTraits(.updatesFrequently)
    }
}

/// Overlay primitives from `OrbFrame.overlays`, drawn exactly as specified.
private struct OrbOverlays: View {
    let frame: OrbFrame?
    let size: CGSize

    var body: some View {
        Canvas { context, size in
            guard let frame else { return }
            let center = CGPoint(x: size.width / 2, y: size.height / 2)
            let radius = min(size.width, size.height) * orbRadiusFraction()
            for overlay in frame.overlays {
                let colour = Color(.sRGB, red: Double(overlay.rgba[0]), green: Double(overlay.rgba[1]),
                                   blue: Double(overlay.rgba[2]), opacity: Double(overlay.rgba[3]))
                switch overlay.kind {
                case .glow:
                    let r = radius * overlay.radius
                    let stops = overlay.stops.map {
                        Gradient.Stop(color: Color(.sRGB, red: Double($0.rgba[0]), green: Double($0.rgba[1]),
                                                   blue: Double($0.rgba[2]), opacity: Double($0.rgba[3])),
                                      location: $0.offset)
                    }
                    let circle = Path(ellipseIn: CGRect(x: center.x - r, y: center.y - r, width: r * 2, height: r * 2))
                    context.fill(circle, with: .radialGradient(Gradient(stops: stops), center: center, startRadius: 0, endRadius: r))
                case .ring:
                    let r = radius * overlay.radius
                    let circle = Path(ellipseIn: CGRect(x: center.x - r, y: center.y - r, width: r * 2, height: r * 2))
                    context.stroke(circle, with: .color(colour), lineWidth: overlay.widthPx)
                case .dot:
                    let x = center.x + radius * overlay.dx, y = center.y + radius * overlay.dy
                    let r = overlay.sizePx
                    context.fill(Path(ellipseIn: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2)), with: .color(colour))
                case .bar:
                    var bar = Path()
                    let y = center.y + radius * overlay.dy
                    bar.move(to: CGPoint(x: center.x - overlay.sizePx, y: y))
                    bar.addLine(to: CGPoint(x: center.x + overlay.sizePx, y: y))
                    context.stroke(bar, with: .color(colour), lineWidth: overlay.widthPx)
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

private struct OrbMetalView: UIViewRepresentable {
    let state: OrbState
    @Binding var frame: OrbFrame?

    func makeCoordinator() -> Coordinator { Coordinator(frame: $frame) }

    func makeUIView(context: Context) -> MTKView {
        let view = MTKView()
        view.isOpaque = false
        view.backgroundColor = .clear
        view.clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        view.colorPixelFormat = .bgra8Unorm
        view.preferredFramesPerSecond = 30
        view.device = context.coordinator.renderer?.device
        view.delegate = context.coordinator
        return view
    }

    func updateUIView(_ view: MTKView, context: Context) {
        context.coordinator.state = state
    }

    final class Coordinator: NSObject, MTKViewDelegate {
        let renderer = try? OrbRenderer()
        let engine = OrbEngine()
        var state: OrbState?
        var frame: Binding<OrbFrame?>
        private var block: [Float]?
        private var lastLabel = ""

        init(frame: Binding<OrbFrame?>) { self.frame = frame }

        func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

        func draw(in view: MTKView) {
            guard let renderer, let state else { return }
            let size = view.drawableSize
            let now = CACurrentMediaTime() * 1000
            if let next = engine.frame(state: state, nowMs: now, width: Float(size.width), height: Float(size.height)) {
                block = next.block
                let label = next.accessibilityLabel
                DispatchQueue.main.async { self.frame.wrappedValue = next }
                if label != lastLabel {
                    if !lastLabel.isEmpty { UIAccessibility.post(notification: .announcement, argument: label) }
                    lastLabel = label
                }
            }
            if let block { renderer.draw(block: block, in: view) }
        }
    }
}
