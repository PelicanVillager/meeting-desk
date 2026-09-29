// 生成应用图标（离线，不依赖任何外部素材）。
// 风格克制：深蓝底 + 白色文档 + 本地模式锁标记。
// 用法：swift scripts/make-icon.swift <输出路径>

import Foundation
import CoreGraphics
import ImageIO

let output = CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : "src-tauri/icons/icon.png"

let size = 1024
guard let context = CGContext(
    data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpaceCreateDeviceRGB(),
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else {
    FileHandle.standardError.write(Data("无法创建绘图上下文\n".utf8))
    exit(1)
}

func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> CGColor {
    CGColor(red: r / 255, green: g / 255, blue: b / 255, alpha: 1)
}

func fill(_ rect: CGRect, radius: CGFloat, color: CGColor) {
    context.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
    context.setFillColor(color)
    context.fillPath()
}

// 背景
fill(CGRect(x: 72, y: 72, width: 880, height: 880), radius: 190, color: rgb(31, 95, 168))

// 文档纸张
let page = CGRect(x: 300, y: 250, width: 420, height: 560)
fill(page, radius: 30, color: rgb(255, 255, 255))

// 折角
context.beginPath()
context.move(to: CGPoint(x: page.maxX - 130, y: page.maxY))
context.addLine(to: CGPoint(x: page.maxX, y: page.maxY))
context.addLine(to: CGPoint(x: page.maxX, y: page.maxY - 130))
context.closePath()
context.setFillColor(rgb(214, 224, 236))
context.fillPath()

// 正文行
let lineColor = rgb(150, 163, 178)
let lineWidths: [CGFloat] = [300, 300, 260, 300, 210]
for (index, width) in lineWidths.enumerated() {
    let y = page.maxY - 220 - CGFloat(index) * 62
    fill(CGRect(x: page.minX + 60, y: y, width: width, height: 26), radius: 13, color: lineColor)
}

// 本地模式锁标记
let lockColor = rgb(23, 89, 74)
let body = CGRect(x: 640, y: 250, width: 224, height: 168)
fill(body, radius: 32, color: lockColor)

context.setStrokeColor(lockColor)
context.setLineWidth(34)
context.setLineCap(.round)
context.addArc(center: CGPoint(x: body.midX, y: body.maxY), radius: 58,
               startAngle: 0, endAngle: .pi, clockwise: false)
context.strokePath()

fill(CGRect(x: body.midX - 26, y: body.midY - 40, width: 52, height: 84), radius: 26,
     color: rgb(255, 255, 255))

guard let image = context.makeImage() else {
    FileHandle.standardError.write(Data("生成图像失败\n".utf8))
    exit(1)
}

let url = URL(fileURLWithPath: output)
try? FileManager.default.createDirectory(
    at: url.deletingLastPathComponent(), withIntermediateDirectories: true)

guard let destination = CGImageDestinationCreateWithURL(
    url as CFURL, "public.png" as CFString, 1, nil
) else {
    FileHandle.standardError.write(Data("无法写入 \(output)\n".utf8))
    exit(1)
}

CGImageDestinationAddImage(destination, image, nil)
CGImageDestinationFinalize(destination)
print("已生成图标：\(output)")
