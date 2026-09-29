// 会议材料工作台 —— 本地 OCR 桥接
//
// 用系统自带的 Vision 框架做中文 / 中英混排文字识别：
//   · 完全离线，不需要下载模型，不联网
//   · 返回文本块坐标与置信度（供引用跳转与高亮使用）
//
// 协议（stdout 输出单行 JSON，诊断信息走 stderr）：
//   meetingdesk-ocr selftest
//   meetingdesk-ocr recognize <文件路径> [--page N] [--lang zh-Hans,en-US]
//   meetingdesk-ocr render <PDF路径> --out <目录> [--scale 2.0] [--quality 0.8] [--max-width 1800]
//
// 输出：
//   {"ok":true,"engine":"vision","engineVersion":"…","elapsedMs":123,
//    "plainText":"…","blocks":[{"text":"…","confidence":0.9,"x":0.1,"y":0.2,"w":0.3,"h":0.04}],
//    "matchedKeyPhrase":true,"error":null}

import Foundation
import CoreGraphics
import CoreText
import ImageIO
import Vision

let engineName = "vision"
let selfTestKeyPhrase = "预算方案"

// MARK: - 输出

func emit(_ payload: [String: Any]) {
    let data = (try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]))
        ?? Data("{}".utf8)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    emit([
        "ok": false,
        "engine": engineName,
        "engineVersion": engineVersion(),
        "elapsedMs": 0,
        "plainText": "",
        "blocks": [],
        "matchedKeyPhrase": false,
        "error": message,
    ])
    exit(1)
}

func engineVersion() -> String {
    let os = ProcessInfo.processInfo.operatingSystemVersion
    return "macOS \(os.majorVersion).\(os.minorVersion).\(os.patchVersion) / Vision"
}

// MARK: - 识别

struct Block {
    let text: String
    let confidence: Float
    let box: CGRect
}

func recognize(_ image: CGImage, languages: [String]) throws -> [Block] {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.recognitionLanguages = languages
    request.usesLanguageCorrection = true

    try VNImageRequestHandler(cgImage: image, options: [:]).perform([request])

    var blocks: [Block] = []
    for observation in request.results ?? [] {
        guard let candidate = observation.topCandidates(1).first else { continue }
        blocks.append(
            Block(
                text: candidate.string,
                confidence: candidate.confidence,
                box: observation.boundingBox
            )
        )
    }
    return sortReadingOrder(blocks)
}

/// Vision 的坐标原点在左下角、y 向上；这里按"从上到下、从左到右"重排，
/// 使 plainText 与人的阅读顺序一致。
func sortReadingOrder(_ blocks: [Block]) -> [Block] {
    guard !blocks.isEmpty else { return [] }

    let tolerance: CGFloat = 0.02
    var rows: [[Int]] = []

    for (index, block) in blocks.enumerated() {
        if let rowIndex = rows.firstIndex(where: { row in
            guard let first = row.first else { return false }
            return abs(blocks[first].box.midY - block.box.midY) <= tolerance
        }) {
            rows[rowIndex].append(index)
        } else {
            rows.append([index])
        }
    }

    return rows
        .sorted { lhs, rhs in
            guard let a = lhs.first, let b = rhs.first else { return false }
            return blocks[a].box.midY > blocks[b].box.midY
        }
        .flatMap { (row: [Int]) -> [Block] in
            row
                .sorted { blocks[$0].box.minX < blocks[$1].box.minX }
                .map { index in blocks[index] }
        }
}

func jsonBlocks(_ blocks: [Block]) -> [[String: Any]] {
    blocks.map { block in
        [
            "text": block.text,
            "confidence": Double(block.confidence),
            "x": Double(block.box.minX),
            "y": Double(block.box.minY),
            "w": Double(block.box.width),
            "h": Double(block.box.height),
        ]
    }
}

// MARK: - 图像装载

func renderPDFPage(_ page: CGPDFPage, scale: CGFloat) -> CGImage? {
    let box = page.getBoxRect(.mediaBox)
    let width = Int((box.width * scale).rounded())
    let height = Int((box.height * scale).rounded())
    guard width > 0, height > 0 else { return nil }

    guard let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { return nil }

    context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    context.scaleBy(x: scale, y: scale)
    context.drawPDFPage(page)
    return context.makeImage()
}

func downscaleIfNeeded(_ image: CGImage, maxDimension: Int) -> CGImage {
    let longest = max(image.width, image.height)
    guard longest > maxDimension else { return image }

    let ratio = CGFloat(maxDimension) / CGFloat(longest)
    let width = Int(CGFloat(image.width) * ratio)
    let height = Int(CGFloat(image.height) * ratio)
    guard let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { return image }

    context.interpolationQuality = .high
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    return context.makeImage() ?? image
}

func loadImage(path: String, page: Int?) -> CGImage? {
    let url = URL(fileURLWithPath: path)
    guard FileManager.default.fileExists(atPath: path) else { return nil }

    if url.pathExtension.lowercased() == "pdf" {
        guard let document = CGPDFDocument(url as CFURL) else { return nil }
        let pageNumber = max(1, min(page ?? 1, document.numberOfPages))
        guard let pdfPage = document.page(at: pageNumber) else { return nil }
        return renderPDFPage(pdfPage, scale: 2.0)
    }

    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else { return nil }
    return downscaleIfNeeded(image, maxDimension: 5000)
}

// MARK: - PDF 逐页渲染成图片
//
// 单个 HTML 会议包必需：一个文件里没法引用外部 PDF，
// 所以导出时把每页渲染成 JPEG 内嵌进去，任何浏览器都能直接翻页。

func renderPDF(
    path: String,
    outDir: String,
    scale: CGFloat,
    quality: CGFloat,
    maxWidth: Int
) throws -> [String] {
    guard let document = CGPDFDocument(URL(fileURLWithPath: path) as CFURL) else {
        throw NSError(
            domain: "meetingdesk", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "无法打开 PDF：\(path)"])
    }
    try FileManager.default.createDirectory(
        atPath: outDir, withIntermediateDirectories: true)

    var written: [String] = []
    for pageIndex in 1...max(1, document.numberOfPages) {
        guard let page = document.page(at: pageIndex) else { continue }
        let box = page.getBoxRect(.mediaBox)
        guard box.width > 0, box.height > 0 else { continue }

        var width = Int((box.width * scale).rounded())
        var height = Int((box.height * scale).rounded())
        if maxWidth > 0 && width > maxWidth {
            let ratio = CGFloat(maxWidth) / CGFloat(width)
            width = maxWidth
            height = Int((CGFloat(height) * ratio).rounded())
        }
        guard width > 0, height > 0 else { continue }

        guard let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else { continue }

        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.interpolationQuality = .high
        context.scaleBy(x: CGFloat(width) / box.width, y: CGFloat(height) / box.height)
        context.translateBy(x: -box.origin.x, y: -box.origin.y)
        context.drawPDFPage(page)

        guard let image = context.makeImage() else { continue }
        let name = String(format: "page-%03d.jpg", pageIndex)
        let target = URL(fileURLWithPath: outDir).appendingPathComponent(name)
        guard let destination = CGImageDestinationCreateWithURL(
            target as CFURL, "public.jpeg" as CFString, 1, nil
        ) else { continue }
        CGImageDestinationAddImage(
            destination, image,
            [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
        if CGImageDestinationFinalize(destination) {
            written.append(name)
        }
    }
    return written
}

// MARK: - 自测用测试图

func renderSelfTestImage() -> CGImage? {
    let text = """
    附件1：关于人事处2026年度预算方案的请示
    一、会议决定事项：同意按 1,280,000 元安排预算。
    责任部门：财务处　截止时间：2026年10月15日
    """

    let width = 1500
    let height = 420
    guard let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { return nil }

    context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))

    let font = CTFontCreateWithName("PingFangSC-Regular" as CFString, 34, nil)
    let attributed = NSAttributedString(
        string: text,
        attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String):
                CGColor(red: 0, green: 0, blue: 0, alpha: 1),
        ]
    )
    let frame = CTFramesetterCreateFrame(
        CTFramesetterCreateWithAttributedString(attributed),
        CFRangeMake(0, 0),
        CGPath(rect: CGRect(x: 50, y: 50, width: width - 100, height: height - 100), transform: nil),
        nil
    )
    CTFrameDraw(frame, context)
    return context.makeImage()
}

// MARK: - 命令解析

var arguments = Array(CommandLine.arguments.dropFirst())
guard let command = arguments.first else {
    fail("用法：meetingdesk-ocr selftest | recognize <路径> [--page N] [--lang zh-Hans,en-US]")
}
arguments.removeFirst()

var targetPath: String?
var pageNumber: Int?
var languages = ["zh-Hans", "en-US"]
var outDirectory: String?
var renderScale: CGFloat = 2.0
var renderQuality: CGFloat = 0.8
var maxWidth = 1800

var index = 0
while index < arguments.count {
    switch arguments[index] {
    case "--page":
        index += 1
        if index < arguments.count { pageNumber = Int(arguments[index]) }
    case "--lang":
        index += 1
        if index < arguments.count {
            languages = arguments[index].split(separator: ",").map(String.init)
        }
    case "--out":
        index += 1
        if index < arguments.count { outDirectory = arguments[index] }
    case "--scale":
        index += 1
        if index < arguments.count, let value = Double(arguments[index]) {
            renderScale = CGFloat(value)
        }
    case "--quality":
        index += 1
        if index < arguments.count, let value = Double(arguments[index]) {
            renderQuality = CGFloat(value)
        }
    case "--max-width":
        index += 1
        if index < arguments.count, let value = Int(arguments[index]) { maxWidth = value }
    default:
        if targetPath == nil { targetPath = arguments[index] }
    }
    index += 1
}

// MARK: - 执行

switch command {
case "selftest":
    guard let image = renderSelfTestImage() else { fail("生成自测图失败") }
    let started = Date()
    do {
        let blocks = try recognize(image, languages: languages)
        let plainText = blocks.map(\.text).joined(separator: "\n")
        emit([
            "ok": !blocks.isEmpty,
            "engine": engineName,
            "engineVersion": engineVersion(),
            "elapsedMs": Int(Date().timeIntervalSince(started) * 1000),
            "plainText": plainText,
            "blocks": jsonBlocks(blocks),
            "matchedKeyPhrase": plainText.contains(selfTestKeyPhrase),
            "error": blocks.isEmpty ? "未识别到任何文字" : NSNull(),
        ])
    } catch {
        fail("识别失败：\(error.localizedDescription)")
    }

case "render":
    guard let path = targetPath else { fail("缺少 PDF 路径") }
    guard let outDir = outDirectory else { fail("缺少输出目录 --out <目录>") }
    let started = Date()
    do {
        let files = try renderPDF(
            path: path, outDir: outDir, scale: renderScale,
            quality: renderQuality, maxWidth: maxWidth)
        emit([
            "ok": !files.isEmpty,
            "engine": "coregraphics",
            "engineVersion": engineVersion(),
            "elapsedMs": Int(Date().timeIntervalSince(started) * 1000),
            "plainText": "",
            "blocks": [],
            "matchedKeyPhrase": false,
            "pageCount": files.count,
            "files": files,
            "error": files.isEmpty ? "没有渲染出任何页面" : NSNull(),
        ])
    } catch {
        fail("渲染失败：\(error.localizedDescription)")
    }

case "recognize":
    guard let path = targetPath else { fail("缺少文件路径") }
    guard let image = loadImage(path: path, page: pageNumber) else {
        fail("无法读取文件（不支持的类型或文件已损坏）：\(path)")
    }
    let started = Date()
    do {
        let blocks = try recognize(image, languages: languages)
        emit([
            "ok": true,
            "engine": engineName,
            "engineVersion": engineVersion(),
            "elapsedMs": Int(Date().timeIntervalSince(started) * 1000),
            "plainText": blocks.map(\.text).joined(separator: "\n"),
            "blocks": jsonBlocks(blocks),
            "matchedKeyPhrase": false,
            "error": NSNull(),
        ])
    } catch {
        fail("识别失败：\(error.localizedDescription)")
    }

default:
    fail("未知命令：\(command)")
}
