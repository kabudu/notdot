import Metal
import MetalKit

/// Draws the canonical orb shader. Both Metal stages are generated from
/// crates/notdot-orb/src/orb.frag by the Rust core; this type only binds the
/// uniform block and draws a quad. It contains no colour, timing or state.
final class OrbRenderer {
    let device: MTLDevice
    private let pipeline: MTLRenderPipelineState
    private let quad: MTLBuffer
    private let queue: MTLCommandQueue

    enum Failure: Error { case noDevice, noFunction(String), noBuffer }

    init(device: MTLDevice? = MTLCreateSystemDefaultDevice(), pixelFormat: MTLPixelFormat = .bgra8Unorm) throws {
        guard let device, let queue = device.makeCommandQueue() else { throw Failure.noDevice }
        self.device = device
        self.queue = queue
        let fragmentLibrary = try device.makeLibrary(source: orbMetalFragment(), options: nil)
        let vertexLibrary = try device.makeLibrary(source: orbMetalVertex(), options: nil)
        guard let vertex = vertexLibrary.makeFunction(name: "notdot_orb_vertex") else { throw Failure.noFunction("vertex") }
        guard let fragment = fragmentLibrary.makeFunction(name: "notdot_orb_fragment") else { throw Failure.noFunction("fragment") }
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = vertex
        descriptor.fragmentFunction = fragment
        let vertexDescriptor = MTLVertexDescriptor()
        vertexDescriptor.attributes[0].format = .float2
        vertexDescriptor.attributes[0].offset = 0
        vertexDescriptor.attributes[0].bufferIndex = 1
        vertexDescriptor.layouts[1].stride = MemoryLayout<Float>.stride * 2
        descriptor.vertexDescriptor = vertexDescriptor
        let attachment = descriptor.colorAttachments[0]!
        attachment.pixelFormat = pixelFormat
        // The shader outputs premultiplied colour, as the desktop canvas uses.
        attachment.isBlendingEnabled = true
        attachment.sourceRGBBlendFactor = .one
        attachment.sourceAlphaBlendFactor = .one
        attachment.destinationRGBBlendFactor = .oneMinusSourceAlpha
        attachment.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        pipeline = try device.makeRenderPipelineState(descriptor: descriptor)
        let vertices = orbQuad()
        guard let quad = device.makeBuffer(bytes: vertices, length: vertices.count * MemoryLayout<Float>.stride) else { throw Failure.noBuffer }
        self.quad = quad
    }

    /// Encode one frame. `block` is `OrbFrame.block` (16 floats).
    func encode(block: [Float], into encoder: MTLRenderCommandEncoder) {
        precondition(block.count == 16, "uniform block is four vec4s")
        var block = block
        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(quad, offset: 0, index: 1)
        encoder.setFragmentBytes(&block, length: block.count * MemoryLayout<Float>.stride, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6)
    }

    /// Render to RGBA8 pixels, top row first (tests and snapshots).
    func renderPixels(block: [Float], size: Int) throws -> [UInt8] {
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: size, height: size, mipmapped: false)
        descriptor.usage = [.renderTarget]
        descriptor.storageMode = .shared
        guard let target = device.makeTexture(descriptor: descriptor) else { throw Failure.noBuffer }
        let offscreen = try OrbRenderer(device: device, pixelFormat: .rgba8Unorm)
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        pass.colorAttachments[0].storeAction = .store
        guard let commands = queue.makeCommandBuffer(), let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else { throw Failure.noBuffer }
        offscreen.encode(block: block, into: encoder)
        encoder.endEncoding()
        commands.commit()
        commands.waitUntilCompleted()
        var pixels = [UInt8](repeating: 0, count: size * size * 4)
        target.getBytes(&pixels, bytesPerRow: size * 4, from: MTLRegionMake2D(0, 0, size, size), mipmapLevel: 0)
        return pixels
    }

    func draw(block: [Float], in view: MTKView) {
        guard let pass = view.currentRenderPassDescriptor, let drawable = view.currentDrawable,
              let commands = queue.makeCommandBuffer(),
              let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else { return }
        encode(block: block, into: encoder)
        encoder.endEncoding()
        commands.present(drawable)
        commands.commit()
    }
}
