//! 本地 OCR：通过 Swift 桥接调用系统 Vision 框架。
//!
//! 为什么走系统框架：macOS 自带高质量中文识别模型，零安装、零模型下载、
//! 完全离线，且返回文本块坐标与置信度（引用跳转与高亮的基础）。
//! 跨平台需求出现时，只需新增一个实现相同 JSON 协议的 RapidOCR 桥接，
//! 上层代码不用改。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

pub const ENGINE_NAME: &str = "apple-vision";
pub const BRIDGE_NAME: &str = "meetingdesk-ocr";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBlock {
    pub text: String,
    #[serde(default)]
    pub confidence: f64,
    /// 归一化坐标，原点在左下角（与 Vision 一致）
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub h: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeResult {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub engine_version: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub plain_text: String,
    #[serde(default)]
    pub blocks: Vec<OcrBlock>,
    #[serde(default)]
    pub matched_key_phrase: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// 查找 OCR 桥接可执行文件。
/// 顺序：环境变量 → 开发期目录 → 打包后可执行文件所在目录。
pub fn bridge_path() -> Option<PathBuf> {
    if let Ok(from_env) = std::env::var("MEETINGDESK_OCR_BIN") {
        let path = PathBuf::from(from_env);
        if path.is_file() {
            return Some(path);
        }
    }

    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(BRIDGE_NAME);
    if dev.is_file() {
        return Some(dev);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidates = [
                dir.join("binaries").join(BRIDGE_NAME),
                dir.join("..").join("Resources").join("binaries").join(BRIDGE_NAME),
            ];
            for candidate in candidates {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

pub fn is_ready() -> bool {
    bridge_path().is_some()
}

/// 自测：桥接会自己渲染一张含中文的测试图并识别，用于验证 OCR 链路可用。
pub fn selftest() -> Result<BridgeResult, String> {
    run(&["selftest"])
}

/// 识别图片（PNG / JPEG / WEBP / TIFF，取决于系统解码支持）。
pub fn recognize_image(path: &Path) -> Result<BridgeResult, String> {
    let path_str = path.to_str().ok_or("路径不是合法的 UTF-8")?;
    run(&["recognize", path_str])
}

/// 识别 PDF 的指定页（1 起）。
pub fn recognize_pdf_page(path: &Path, page_no: u32) -> Result<BridgeResult, String> {
    let path_str = path.to_str().ok_or("路径不是合法的 UTF-8")?;
    run(&["recognize", path_str, "--page", &page_no.to_string()])
}

#[derive(Debug, Deserialize)]
struct RenderOutput {
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    error: Option<String>,
}

/// 把 PDF 逐页渲染成 JPEG，返回生成的文件路径（按页顺序）。
///
/// 单个 HTML 的会议包必须这样做：一个文件里没法引用外部 PDF，
/// 只能把每页变成图片内嵌进去。
pub fn render_pdf_pages(pdf: &Path, out_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let bridge = bridge_path().ok_or_else(|| {
        format!("未找到本地渲染桥接（{BRIDGE_NAME}）。请先运行：pnpm ocr-bridge")
    })?;
    let pdf_str = pdf.to_str().ok_or("路径不是合法的 UTF-8")?;
    let out_str = out_dir.to_str().ok_or("输出目录不是合法的 UTF-8")?;

    let output = Command::new(&bridge)
        .args(["render", pdf_str, "--out", out_str])
        .output()
        .map_err(|err| format!("启动渲染桥接失败：{err}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Err(format!(
            "渲染桥接没有返回结果（退出码 {:?}）：{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let parsed: RenderOutput = serde_json::from_str(stdout.trim())
        .map_err(|err| format!("解析渲染结果失败：{err}；原始输出：{stdout}"))?;
    if parsed.files.is_empty() {
        return Err(parsed
            .error
            .unwrap_or_else(|| "没有渲染出任何页面".to_string()));
    }
    Ok(parsed.files.iter().map(|name| out_dir.join(name)).collect())
}

fn run(args: &[&str]) -> Result<BridgeResult, String> {
    let bridge = bridge_path().ok_or_else(|| {
        format!(
            "未找到本地 OCR 桥接（{BRIDGE_NAME}）。请先运行：pnpm ocr-bridge"
        )
    })?;

    let output = Command::new(&bridge)
        .args(args)
        .output()
        .map_err(|err| format!("启动 OCR 桥接失败：{err}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.trim().is_empty() {
        return Err(format!(
            "OCR 桥接没有返回结果（退出码 {:?}）：{stderr}",
            output.status.code()
        ));
    }

    serde_json::from_str::<BridgeResult>(stdout.trim())
        .map_err(|err| format!("解析 OCR 结果失败：{err}；原始输出：{stdout}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 桥接路径查找不会崩溃() {
        // 桥接可能尚未编译，这里只验证查找逻辑是纯查询、不 panic
        let _ = bridge_path();
        let _ = is_ready();
    }
}
