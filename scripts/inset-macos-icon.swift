import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// Legacy ICNS needs the macOS inset; Icon Composer supplies it to Assets.car at runtime.
let arguments = CommandLine.arguments
guard arguments.count == 3 else { throw CocoaError(.fileReadInvalidFileName) }
let input = URL(fileURLWithPath: arguments[1])
let output = URL(fileURLWithPath: arguments[2])
guard let source = CGImageSourceCreateWithURL(input as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
      image.width == 1024, image.height == 1024,
      let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
      let context = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8,
                              bytesPerRow: 0, space: colorSpace,
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { throw CocoaError(.fileReadCorruptFile) }
context.interpolationQuality = .high
context.draw(image, in: CGRect(x: 100, y: 100, width: 824, height: 824))
guard let padded = context.makeImage(),
      let destination = CGImageDestinationCreateWithURL(output as CFURL, UTType.png.identifier as CFString, 1, nil)
else { throw CocoaError(.fileWriteUnknown) }
CGImageDestinationAddImage(destination, padded, nil)
guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
