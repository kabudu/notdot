// Write golden orb frames for native adapter tests.
// swift make_goldens.swift <generated-msl-dir> <orb-contract.json> <out-dir> <size>
// Renders one frame of every contract sequence (the last showing a task
// cue, else the last) with the generated
// Metal shaders (runtime compile). scripts/orb_parity shows this path matches
// the canonical GLSL in WebGL exactly; regenerate only with the contract.
import Foundation
import Metal

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}
let args = CommandLine.arguments
guard args.count == 5, let size = Int(args[4]), size > 0 else { fail("usage: make_goldens.swift <msl> <fixture> <out> <size>") }
let msl = URL(fileURLWithPath: args[1])
let fixture = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: args[2]))) as! [String: Any]
let out = URL(fileURLWithPath: args[3])
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { fail("no Metal device") }
let fragment = try device.makeLibrary(source: String(contentsOf: msl.appendingPathComponent("OrbFragment.metal"), encoding: .utf8), options: nil)
let vertex = try device.makeLibrary(source: String(contentsOf: msl.appendingPathComponent("OrbVertex.metal"), encoding: .utf8), options: nil)
let pipelineDescriptor = MTLRenderPipelineDescriptor()
pipelineDescriptor.vertexFunction = vertex.makeFunction(name: "notdot_orb_vertex")
pipelineDescriptor.fragmentFunction = fragment.makeFunction(name: "notdot_orb_fragment")
pipelineDescriptor.colorAttachments[0].pixelFormat = .rgba8Unorm
let vertexDescriptor = MTLVertexDescriptor()
vertexDescriptor.attributes[0].format = .float2
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
var index: [[String: Any]] = []
for sequence in fixture["sequences"] as! [[String: Any]] {
    let name = sequence["name"] as! String
    let frames = sequence["frames"] as! [[String: Any]]
    // Prefer the last frame showing a task cue, so task lights are covered.
    let chosen = frames.lastIndex { ($0["visible_task"] as? String) != "none" } ?? frames.count - 1
    let frame = frames[chosen]
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
    // Raw RGBA (the shader's premultiplied output, top row first),
    // compressed with raw DEFLATE: Foundation (.zlib), Java (Inflater with
    // nowrap) and Python (wbits=-15) all decode it byte for byte. PNG is not
    // used because iOS ImageIO premultiplies on decode.
    let file = "\(name).rgba.deflate"
    let packed = try (Data(pixels) as NSData).compressed(using: .zlib) as Data
    try packed.write(to: out.appendingPathComponent(file))
    index.append(["file": file, "sequence": name, "frame": chosen, "block": block, "packed": frame["packed"]!])
}
let meta: [String: Any] = ["size": size, "contract_version": fixture["contract_version"]!, "shader_sha256": fixture["shader_sha256"]!,
                           "storage": "raw RGBA8 (premultiplied shader output, top row first), raw DEFLATE", "frames": index]
try JSONSerialization.data(withJSONObject: meta, options: [.prettyPrinted, .sortedKeys]).write(to: out.appendingPathComponent("index.json"))
print("wrote \(index.count) goldens at \(size)px")
