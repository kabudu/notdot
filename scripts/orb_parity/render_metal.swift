// Render golden orb frames with the naga-generated Metal shaders.
// swift render_metal.swift <generated-msl-dir> <orb-contract.json> <out-dir> <size> <stride>
// Uses Metal's runtime compiler (no offline Metal toolchain needed).
// Writes <out-dir>/<sequence>-<frame>.rgba (premultiplied RGBA8, top row first)
// and <out-dir>/index.json listing the frames.
import Foundation
import Metal

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}
let args = CommandLine.arguments
guard args.count == 6, let size = Int(args[4]), let stride = Int(args[5]), size > 0, stride > 0 else {
    fail("usage: render_metal.swift <msl-dir> <fixture> <out> <size> <stride>")
}
let mslDir = URL(fileURLWithPath: args[1])
let fixture = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: args[2]))) as! [String: Any]
let out = URL(fileURLWithPath: args[3])
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
guard let device = MTLCreateSystemDefaultDevice() else { fail("no Metal device") }
let fragmentSource = try String(contentsOf: mslDir.appendingPathComponent("OrbFragment.metal"), encoding: .utf8)
let vertexSource = try String(contentsOf: mslDir.appendingPathComponent("OrbVertex.metal"), encoding: .utf8)
let fragmentLibrary = try device.makeLibrary(source: fragmentSource, options: nil)
let vertexLibrary = try device.makeLibrary(source: vertexSource, options: nil)
let pipelineDescriptor = MTLRenderPipelineDescriptor()
pipelineDescriptor.vertexFunction = vertexLibrary.makeFunction(name: "main_")
pipelineDescriptor.fragmentFunction = fragmentLibrary.makeFunction(name: "main_")
pipelineDescriptor.colorAttachments[0].pixelFormat = .rgba8Unorm
let vertexDescriptor = MTLVertexDescriptor()
vertexDescriptor.attributes[0].format = .float2
vertexDescriptor.attributes[0].offset = 0
vertexDescriptor.attributes[0].bufferIndex = 1
vertexDescriptor.layouts[1].stride = 8
pipelineDescriptor.vertexDescriptor = vertexDescriptor
let pipeline = try device.makeRenderPipelineState(descriptor: pipelineDescriptor)
let quad: [Float] = [-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]
let quadBuffer = device.makeBuffer(bytes: quad, length: quad.count * 4)!
let textureDescriptor = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: size, height: size, mipmapped: false)
textureDescriptor.usage = [.renderTarget]
textureDescriptor.storageMode = .shared
let target = device.makeTexture(descriptor: textureDescriptor)!
let queue = device.makeCommandQueue()!
var index: [[String: Any]] = []
for sequence in fixture["sequences"] as! [[String: Any]] {
    let name = sequence["name"] as! String
    let frames = sequence["frames"] as! [[String: Any]]
    for (number, frame) in frames.enumerated() where number % stride == stride - 1 || number == frames.count - 1 {
        var block = (frame["block"] as! [NSNumber]).map { $0.floatValue }
        block[0] = Float(size)
        block[1] = Float(size)
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        pass.colorAttachments[0].storeAction = .store
        let commands = queue.makeCommandBuffer()!
        let encoder = commands.makeRenderCommandEncoder(descriptor: pass)!
        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(quadBuffer, offset: 0, index: 1)
        encoder.setFragmentBytes(&block, length: block.count * 4, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 6)
        encoder.endEncoding()
        commands.commit()
        commands.waitUntilCompleted()
        var pixels = [UInt8](repeating: 0, count: size * size * 4)
        target.getBytes(&pixels, bytesPerRow: size * 4, from: MTLRegionMake2D(0, 0, size, size), mipmapLevel: 0)
        let file = "\(name)-\(number).rgba"
        try Data(pixels).write(to: out.appendingPathComponent(file))
        index.append(["file": file, "sequence": name, "frame": number, "packed": frame["packed"]!])
    }
}
try JSONSerialization.data(withJSONObject: ["size": size, "device": device.name, "frames": index]).write(to: out.appendingPathComponent("index.json"))
print("rendered \(index.count) frames on \(device.name)")
