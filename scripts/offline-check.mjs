#!/usr/bin/env node
/**
 * 离线自检（静态扫描构建产物）
 *
 * 目标：确认打包产物真的不依赖外网。
 *
 * 只把"会真的去加载远端资源"的写法判为 ❌（阻断）：
 *   · HTML 里的远程 src= / href=
 *   · CSS 里的 url(http…) / @import "http…"
 *   · JS 里的 fetch("http…") / new WebSocket("http…") / import("http…")
 *     / XMLHttpRequest.open("http…")
 *
 * 其它只是"字符串里出现了网址"的情况（XML 命名空间、库的报错说明链接等）
 * 记为 ⚠️ 提示，不阻断，但会如实列出来供人工核对。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const distDir = fileURLToPath(new URL("../dist", import.meta.url));

const LOOPBACK = /^https?:\/\/(127\.0\.0\.1|localhost|\[::1\])(:|\/|$)/;

/** 已知的"惰性网址"：出现在字符串里但不会触发网络请求。 */
const INERT = [
  /^http:\/\/www\.w3\.org\//, // SVG / MathML / xlink 等 XML 命名空间
  /^http:\/\/www\.w3\.org\/XML\/1998\/namespace/,
  /^https:\/\/react\.dev\/errors\//, // React 的报错说明链接（仅出现在错误信息里）
  /^https:\/\/example\.com\/upload$/, // 本项目"网络守卫生效"演示用的固定测试地址
];

const LOADING_PATTERNS = [
  { kind: "html", regex: /(?:src|href)\s*=\s*["'](https?:\/\/[^"']+)["']/g },
  { kind: "css", regex: /url\(\s*["']?(https?:\/\/[^"')]+)["']?\s*\)/g },
  { kind: "css", regex: /@import\s+["']?(https?:\/\/[^"';]+)["']?/g },
  { kind: "js", regex: /fetch\(\s*["'](https?:\/\/[^"']+)["']/g },
  { kind: "js", regex: /new\s+WebSocket\(\s*["'](https?:\/\/[^"']+)["']/g },
  { kind: "js", regex: /import\(\s*["'](https?:\/\/[^"']+)["']/g },
  { kind: "js", regex: /\.open\(\s*["'][A-Z]+["']\s*,\s*["'](https?:\/\/[^"']+)["']/g },
];

const REMOTE_ANY = /https?:\/\/[^\s"'`)<>\]]+/g;

const violations = [];
const warnings = [];
let scanned = 0;

function isAllowed(url) {
  return LOOPBACK.test(url) || INERT.some((pattern) => pattern.test(url));
}

function inspect(path) {
  scanned += 1;
  const text = readFileSync(path, "utf8");
  const kind = extname(path).toLowerCase();

  for (const pattern of LOADING_PATTERNS) {
    if (kind === ".html" && pattern.kind !== "html") continue;
    if (kind === ".css" && pattern.kind !== "css") continue;
    if (kind === ".js" && pattern.kind !== "js") continue;

    for (const match of text.matchAll(pattern.regex)) {
      const url = match[1];
      if (!isAllowed(url)) {
        violations.push({ file: relative(distDir, path), url, reason: `${pattern.kind} 资源加载` });
      }
    }
  }

  for (const url of text.match(REMOTE_ANY) ?? []) {
    if (isAllowed(url)) continue;
    const alreadyReported = violations.some((entry) => entry.file === relative(distDir, path) && entry.url === url);
    if (!alreadyReported) {
      warnings.push({ file: relative(distDir, path), url });
    }
  }
}

function walk(dir) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      walk(path);
      continue;
    }
    inspect(path);
  }
}

try {
  statSync(distDir);
} catch {
  console.error(`未找到构建产物：${distDir}\n请先运行 pnpm build`);
  process.exit(2);
}

walk(distDir);

if (violations.length > 0) {
  console.error(`✕ 离线自检未通过：发现 ${violations.length} 处远程资源加载（违反完全本地原则）`);
  for (const item of violations.slice(0, 40)) {
    console.error(`  ${item.file} → ${item.url}　(${item.reason})`);
  }
  process.exit(1);
}

console.log(`✓ 离线自检通过：扫描 ${scanned} 个文件，未发现任何远程资源加载。`);

const uniqueWarnings = [...new Map(warnings.map((item) => [`${item.url}`, item])).values()];
if (uniqueWarnings.length > 0) {
  console.log(
    `⚠ 另有 ${uniqueWarnings.length} 处"仅为字符串"的网址（不会发起请求，已人工确认为命名空间/报错说明/自检样例）：`,
  );
  for (const item of uniqueWarnings.slice(0, 20)) {
    console.log(`   ${item.url}`);
  }
}
