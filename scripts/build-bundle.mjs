#!/usr/bin/env node
/**
 * 会议包导出原型（M-USB 验证用）
 *
 * 产出：一个可以直接拷进 U 盘、双击 index.html 就能打开的文件夹。
 *
 * 三条铁律（针对 file:// 协议）：
 *   1. 不用 ES module（<script type="module"> 在 file:// 下会被 CORS 拦掉）
 *   2. 不用 fetch / XHR 读数据（同样被拦）→ 数据预烘焙进 data.js
 *   3. 不用 Web Worker / IndexedDB → 会中备注放内存，导出靠下载文件
 *
 * 用法：node scripts/build-bundle.mjs "<会议文件夹1>" ["<会议文件夹2>" ...]
 */
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const outRoot = join(repoRoot, "dist-bundle");

const SKIP_NAMES = new Set([".DS_Store", ".localized", "Thumbs.db"]);
const KIND_BY_EXT = {
  ".pdf": "pdf",
  ".docx": "docx",
  ".doc": "text",
  ".html": "html",
  ".png": "image",
  ".jpg": "image",
  ".jpeg": "image",
  ".webp": "image",
  ".txt": "text",
  ".xlsx": "download",
  ".xls": "download",
  ".csv": "download",
};

/**
 * 会议包自带的渲染引擎（普通 UMD 脚本，file:// 下可用）。
 * docx-preview 依赖全局 JSZip，所以两个都要打包进去。
 */
function vendorFiles() {
  // 直接用文件路径定位，绕开 pnpm 的 exports 限制
  const docxBundle = join(repoRoot, "node_modules", "docx-preview", "dist", "docx-preview.min.js");
  if (!existsSync(docxBundle)) {
    throw new Error("找不到 docx-preview：请先执行 pnpm install");
  }

  const candidates = [];
  try {
    candidates.push(require.resolve("jszip/dist/jszip.min.js"));
  } catch {
    // jszip 是 docx-preview 的传递依赖，pnpm 下不提升到根目录，去 .pnpm 仓库里找
  }
  const storeDir = join(repoRoot, "node_modules", ".pnpm");
  if (existsSync(storeDir)) {
    const hit = readdirSync(storeDir).find((name) => name.startsWith("jszip@"));
    if (hit) {
      candidates.push(join(storeDir, hit, "node_modules", "jszip", "dist", "jszip.min.js"));
    }
  }
  const jszip = candidates.find((path) => existsSync(path));
  if (!jszip) throw new Error("找不到 jszip：请先执行 pnpm install");

  return [
    { from: jszip, to: "jszip.min.js" },
    { from: docxBundle, to: "docx-preview.min.js" },
  ];
}

/** 清洗文件名：换行/控制字符 → 空格，压缩连续空白，供展示与落盘使用。 */
function sanitize(name) {
  return name
    .replace(/[\u0000-\u001f\u007f]/g, " ")
    .replace(/\s+/g, " ")
    .replace(/[\\/:*?"<>|]/g, "_")
    .trim();
}

function walk(dir, prefix = "") {
  const out = [];
  for (const entry of readdirSync(dir).sort((a, b) => a.localeCompare(b, "zh"))) {
    if (SKIP_NAMES.has(entry) || entry.startsWith(".")) continue;
    const full = join(dir, entry);
    const info = statSync(full);
    if (info.isDirectory()) {
      out.push(...walk(full, prefix ? `${prefix}/${entry}` : entry));
      continue;
    }
    out.push({ full, group: prefix });
  }
  return out;
}

/** docx → HTML / 纯文本（用系统自带 textutil，离线，无需额外依赖）。 */
function convertDocxToText(file, outBase) {
  // 只用来喂搜索索引与失败兜底：真正的显示交给浏览器里的 docx-preview
  execFileSync("/usr/bin/textutil", ["-convert", "txt", "-output", `${outBase}.txt`, file]);
}

function buildBundle(sourceDir) {
  // 先确认依赖齐全，再清空目录：避免导出失败时把上一次的包删掉
  const assets = vendorFiles();
  const meetingName = sanitize(basename(sourceDir));
  const bundleDir = join(outRoot, meetingName);
  const materialsDir = join(bundleDir, "materials");
  rmSync(bundleDir, { recursive: true, force: true });
  mkdirSync(materialsDir, { recursive: true });

  const files = walk(sourceDir);
  const groups = new Map();
  const searchIndex = [];
  let counter = 0;

  const vendorDir = join(bundleDir, "vendor");
  mkdirSync(vendorDir, { recursive: true });
  for (const asset of assets) {
    copyFileSync(asset.from, join(vendorDir, asset.to));
  }

  for (const file of files) {
    const ext = extname(file.full).toLowerCase();
    const kind = KIND_BY_EXT[ext];
    if (!kind) continue;

    counter += 1;
    const seq = String(counter).padStart(3, "0");
    const rawName = basename(file.full, extname(file.full));
    const displayTitle = sanitize(rawName);
    const groupName = file.group ? sanitize(file.group) : "会议材料";
    const safeBase = `${seq}-${displayTitle}`.slice(0, 90);

    const item = {
      id: seq,
      title: displayTitle,
      kind,
      sourceExt: ext.replace(".", ""),
      size: statSync(file.full).size,
    };

    if (kind === "docx") {
      // 原始 docx 一并保留：万一渲染出问题，现场可用 WPS/Word 直接打开原文件
      const originalName = `${safeBase}.docx`;
      copyFileSync(file.full, join(materialsDir, originalName));
      item.original = `materials/${originalName}`;

      // docx 内容以 base64 形式放进独立 JS：file:// 下不能 fetch，只能这样带数据
      const payloadName = `${safeBase}.docx.js`;
      const base64 = readFileSync(file.full).toString("base64");
      writeFileSync(
        join(materialsDir, payloadName),
        `window.MEETING_DOCX = window.MEETING_DOCX || {};\nwindow.MEETING_DOCX[${JSON.stringify(
          seq,
        )}] = ${JSON.stringify(base64)};\n`,
      );
      item.payload = `materials/${payloadName}`;

      convertDocxToText(file.full, join(materialsDir, safeBase));
      const textPath = join(materialsDir, `${safeBase}.txt`);
      if (existsSync(textPath)) {
        item.text = readFileSync(textPath, "utf8").slice(0, 20000);
        searchIndex.push({ id: seq, text: item.text });
        rmSync(textPath, { force: true });
      }
    } else if (kind === "text") {
      const target = join(materialsDir, `${safeBase}.txt`);
      execFileSync("/usr/bin/textutil", ["-convert", "txt", "-output", target, file.full]);
      item.file = `materials/${basename(target)}`;
      item.text = existsSync(target) ? readFileSync(target, "utf8").slice(0, 20000) : "";
      searchIndex.push({ id: seq, text: item.text });
    } else {
      const target = join(materialsDir, `${safeBase}${ext}`);
      cpSync(file.full, target);
      item.file = `materials/${basename(target)}`;
    }

    if (!groups.has(groupName)) groups.set(groupName, []);
    groups.get(groupName).push(item);
  }

  const data = {
    generatedAt: new Date().toISOString().slice(0, 19).replace("T", " "),
    meeting: meetingName,
    groups: [...groups.entries()].map(([name, items]) => ({ name, items })),
  };

  writeFileSync(join(bundleDir, "data.js"), `window.MEETING_BUNDLE = ${JSON.stringify(data, null, 1)};\n`);
  writeFileSync(join(bundleDir, "index.html"), INDEX_HTML);
  writeFileSync(join(bundleDir, "app.css"), APP_CSS);
  writeFileSync(join(bundleDir, "app.js"), APP_JS);
  writeFileSync(join(bundleDir, "使用说明.txt"), READ_ME_TEXT);

  return { meetingName, bundleDir, count: counter, groups: groups.size };
}

/* ────────────────────────── 前端模板（普通脚本，兼容 file://） ────────────────────────── */

const INDEX_HTML = `<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<title>会议材料工作台 · 会议包</title>
<link rel="stylesheet" href="app.css">
</head>
<body>
<header id="topbar">
  <span id="meeting-title">会议材料</span>
  <span class="badge">🔒 本地</span>
  <input id="search" type="search" placeholder="搜索材料（标题 / 正文）… 按 / 聚焦">
  <span id="counter"></span>
  <button id="btn-zoom" type="button">字号 100%</button>
  <button id="btn-present" type="button">投屏增强</button>
  <button id="btn-notes" type="button">📝 备注</button>
  <button id="btn-full" type="button">⛶ 全屏（F）</button>
</header>
<main>
  <nav id="toc"></nav>
  <section id="stage">
    <div id="viewer"></div>
    <div id="empty">← 从左侧选择一份材料</div>
  </section>
  <aside id="notes" hidden>
    <h2>我的备注</h2>
    <p class="hint">会中随时记；结束后点“导出备注”，把文件带回工作台归档。</p>
    <textarea id="notes-text" placeholder="针对当前材料记录…"></textarea>
    <div class="note-actions">
      <button id="btn-export-notes" type="button">导出全部备注</button>
      <button id="btn-close-notes" type="button">关闭</button>
    </div>
    <div id="notes-status" class="hint"></div>
  </aside>
</main>
<footer id="bottombar">
  <button id="btn-prev" type="button">← 上一份</button>
  <span id="progress">0 / 0</span>
  <button id="btn-next" type="button">下一份 →</button>
  <span class="sep"></span>
  <button id="btn-prev-page" type="button">PDF 上一页</button>
  <button id="btn-next-page" type="button">PDF 下一页</button>
  <span class="hint">快捷键：← → 上一份/下一份 · ↑ ↓ 翻页 · F 全屏 · / 搜索</span>
</footer>
<script src="vendor/jszip.min.js"></script>
<script src="vendor/docx-preview.min.js"></script>
<script src="data.js"></script>
<script src="app.js"></script>
</body>
</html>
`;

const APP_CSS = `*{box-sizing:border-box}
/* 公文常用字体名 → 本机可用的等价字体。
   文档里写的是「仿宋_GB2312 / 方正小标宋简体 / 黑体」这类 Windows 字体，
   Mac 或没装 Office 的机器上并不存在，浏览器会悄悄换成默认字体
   —— 这正是之前"Word 显示得不好"的主因之一。 */
@font-face{font-family:"仿宋_GB2312";src:local("仿宋_GB2312"),local("FangSong_GB2312"),local("FangSong"),local("STFangsong"),local("华文仿宋")}
@font-face{font-family:"仿宋";src:local("仿宋"),local("FangSong"),local("STFangsong"),local("华文仿宋")}
@font-face{font-family:"宋体";src:local("宋体"),local("SimSun"),local("Songti SC"),local("STSong")}
@font-face{font-family:"黑体";src:local("黑体"),local("SimHei"),local("Heiti SC"),local("STHeiti")}
@font-face{font-family:"楷体";src:local("楷体"),local("KaiTi"),local("Kaiti SC"),local("STKaiti"),local("华文楷体")}
@font-face{font-family:"楷体_GB2312";src:local("楷体_GB2312"),local("KaiTi_GB2312"),local("KaiTi"),local("Kaiti SC")}
@font-face{font-family:"方正小标宋简体";src:local("方正小标宋简体"),local("FZXiaoBiaoSong-B05S"),local("STZhongsong"),local("华文中宋"),local("STSong")}
html,body{height:100%;margin:0}
body{display:flex;flex-direction:column;height:100vh;overflow:hidden;
  font:15px/1.6 -apple-system,BlinkMacSystemFont,"PingFang SC","Microsoft YaHei","Hiragino Sans GB",sans-serif;
  color:#1f2328;background:#f5f6f7}
#topbar{display:flex;align-items:center;gap:12px;padding:8px 14px;background:#fff;border-bottom:1px solid #e0e3e7}
#meeting-title{font-size:17px;font-weight:600}
.badge{font-size:13px;font-weight:600;color:#17594a;background:#e6f2ee;border:1px solid #cfe3dc;border-radius:3px;padding:1px 7px}
#search{flex:1;max-width:420px;padding:5px 9px;font:inherit;border:1px solid #c8ced6;border-radius:3px}
#counter{color:#57606a;font-size:13px;min-width:80px;text-align:right;margin-left:auto}
button{font:inherit;background:#fff;border:1px solid #c8ced6;border-radius:3px;padding:4px 11px;cursor:pointer}
button:hover{background:#fafbfc;border-color:#8b949e}
main{flex:1;display:flex;min-height:0}
#toc{width:320px;flex:none;overflow:auto;background:#fff;border-right:1px solid #e0e3e7;padding:8px 0}
#toc h3{margin:12px 14px 4px;font-size:12px;font-weight:600;color:#8b949e;letter-spacing:.5px}
#toc a{display:flex;gap:8px;align-items:baseline;padding:7px 14px;color:inherit;text-decoration:none;border-left:3px solid transparent;font-size:14px}
#toc a:hover{background:#fafbfc}
#toc a.active{background:#eaf1f9;border-left-color:#1f5fa8;font-weight:600}
#toc .seq{color:#8b949e;font-size:12px;font-variant-numeric:tabular-nums}
#toc .ext{margin-left:auto;color:#8b949e;font-size:11px;text-transform:uppercase}
#toc mark{background:#fdf3e0}
#stage{flex:1;position:relative;min-width:0;background:#eceef0;display:flex}
#viewer{flex:1;display:flex;min-width:0}
#viewer iframe{flex:1;border:0;width:100%;height:100%;background:#eceef0}
#viewer img{max-width:100%;max-height:100%;margin:auto;display:block;background:#fff}
/* Word / 文本：A4 纸张感 + 中文公文友好的排版 */
#viewer .doc{flex:1;overflow:auto;background:#eceef0;padding:24px 16px}
#viewer .doc-inner{max-width:860px;margin:0 auto;background:#fff;padding:46px 58px;
  box-shadow:0 1px 3px rgba(16,24,40,.14);zoom:var(--doc-zoom,1);
  font-family:"FangSong","STFangsong","Songti SC","SimSun","PingFang SC",serif;
  font-size:16px;line-height:1.9;color:#111}
#viewer .doc-inner :is(h1,h2,h3){font-family:"Heiti SC","SimHei","PingFang SC",sans-serif;line-height:1.5;margin:1.1em 0 .6em}
#viewer .doc-inner h1{font-size:1.6em;text-align:center}
#viewer .doc-inner p{margin:.55em 0}
#viewer .doc-inner table{border-collapse:collapse;margin:1em 0;font-size:.94em}
#viewer .doc-inner td,#viewer .doc-inner th{border:1px solid #9aa3ad;padding:5px 9px}
#viewer .fallback{white-space:pre-wrap;word-break:break-word;font:inherit;margin:0}
/* docx-preview 渲染出来的 Word 版面 */
#viewer .docx-host{flex:1;overflow:auto;background:#eceef0;padding:24px 0}
#viewer .docx-wrapper{background:transparent;padding:0;zoom:var(--doc-zoom,1)}
#viewer .docx-wrapper>section.docx{box-shadow:0 1px 3px rgba(16,24,40,.14);margin-bottom:18px}
#viewer .docx-status{padding:40px;text-align:center;color:#57606a}
.docx-error{background:#fdf3e0;border:1px solid #f0e0bc;border-radius:3px;padding:12px 14px;margin:0 auto 16px;max-width:820px;font-size:14px}
.docx-error p{margin:.3em 0}
/* 投屏增强：站在几米外看时，细笔画的书宋/仿宋很难认，统一换黑体类并适当加粗 */
body.present #viewer .docx-wrapper :is(p,span,td,th,div,li,section,article,header,footer),
body.present #viewer .doc-inner :is(p,span,td,th,div,li){
  font-family:"PingFang SC","Heiti SC","Microsoft YaHei","SimHei",sans-serif !important;
  font-weight:500 !important}
body.present #viewer .docx-wrapper :is(b,strong),
body.present #viewer .doc-inner :is(b,strong){font-weight:700 !important}
body.present #viewer :is(.docx-wrapper,.doc-inner){zoom:calc(var(--doc-zoom,1) * 1.15)}
#empty{margin:auto;color:#8b949e}
#notes{width:360px;flex:none;background:#fff;border-left:1px solid #e0e3e7;padding:14px;display:flex;flex-direction:column;gap:8px}
#notes h2{margin:0;font-size:15px}
.hint{color:#8b949e;font-size:12px;margin:0}
#notes textarea{flex:1;min-height:160px;padding:9px;font:inherit;border:1px solid #c8ced6;border-radius:3px;resize:none}
.note-actions{display:flex;gap:8px}
#bottombar{display:flex;align-items:center;gap:10px;padding:7px 14px;background:#fff;border-top:1px solid #e0e3e7;font-size:13px}
#progress{font-variant-numeric:tabular-nums;color:#57606a;min-width:64px;text-align:center}
.sep{width:1px;height:18px;background:#e0e3e7}
body.focus #toc{display:none}
`;

const APP_JS = `(function () {
  var data = window.MEETING_BUNDLE || { groups: [], meeting: "会议材料" };
  var flat = [];
  data.groups.forEach(function (group) {
    group.items.forEach(function (item) {
      item.group = group.name;
      flat.push(item);
    });
  });

  var current = -1;
  var notes = {};
  var toc = document.getElementById("toc");
  var viewer = document.getElementById("viewer");
  var empty = document.getElementById("empty");
  var searchBox = document.getElementById("search");
  var progress = document.getElementById("progress");
  var counter = document.getElementById("counter");
  var notesPane = document.getElementById("notes");
  var notesText = document.getElementById("notes-text");
  var notesStatus = document.getElementById("notes-status");

  document.getElementById("meeting-title").textContent = data.meeting;
  counter.textContent = flat.length + " 份材料";

  function textOf(item) {
    return item.text || "";
  }

  function appendPaper(host, html) {
    var outer = document.createElement("div");
    outer.className = "doc";
    var inner = document.createElement("div");
    inner.className = "doc-inner";
    inner.innerHTML = html;
    outer.appendChild(inner);
    host.appendChild(outer);
  }

  function loadScript(src, onload, onerror) {
    var script = document.createElement("script");
    script.src = src;
    script.onload = onload;
    script.onerror = function () { onerror(new Error("无法加载 " + src)); };
    document.head.appendChild(script);
  }

  function base64ToBytes(base64) {
    var binary = window.atob(base64);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return bytes;
  }

  // Word 渲染：优先用 docx-preview 还原原始版式；失败则退回纯文本，绝不白屏
  function renderDocx(item, host, status) {
    function fallback(message) {
      host.innerHTML = "";
      var box = document.createElement("div");
      box.className = "docx-error";
      box.innerHTML = "<p><strong>Word 文档未能按原版式渲染</strong>：" + escapeHtml(message) + "</p>" +
        (item.original
          ? '<p>可点这里用 WPS/Word 打开原件：<a href="' + encodeURI(item.original) + '" download>' +
            escapeHtml(item.title) + "." + item.sourceExt + "</a></p>"
          : "") +
        "<p>以下为纯文本兜底显示：</p>";
      host.appendChild(box);
      appendPaper(host, '<pre class="fallback">' + escapeHtml(item.text || "（无文本）") + "</pre>");
    }

    function render() {
      var base64 = (window.MEETING_DOCX || {})[item.id];
      if (!base64) { fallback("会议包里缺少该文档的数据"); return; }
      if (!window.docx || !window.docx.renderAsync) { fallback("渲染引擎未加载"); return; }
      var bytes = base64ToBytes(base64);
      window.docx.renderAsync(bytes.buffer, host, null, {
        className: "docx",
        inWrapper: true,
        breakPages: true,
        ignoreWidth: false,
        ignoreHeight: false,
        renderHeaders: true,
        renderFooters: true,
        renderFootnotes: true,
        experimental: true
      }).then(function () {
        if (status.parentNode) status.parentNode.removeChild(status);
      }).catch(function (error) {
        fallback(error && error.message ? error.message : String(error));
      });
    }

    if ((window.MEETING_DOCX || {})[item.id]) render();
    else loadScript(encodeURI(item.payload), render, function (error) { fallback(error.message); });
  }

  function renderToc(filter) {
    toc.innerHTML = "";
    var needle = (filter || "").trim().toLowerCase();
    var shown = 0;
    data.groups.forEach(function (group) {
      var items = group.items.filter(function (item) {
        if (!needle) return true;
        return (item.title + " " + group.name + " " + textOf(item)).toLowerCase().indexOf(needle) >= 0;
      });
      if (items.length === 0) return;
      var head = document.createElement("h3");
      head.textContent = group.name + "（" + items.length + "）";
      toc.appendChild(head);
      items.forEach(function (item) {
        shown += 1;
        var link = document.createElement("a");
        link.href = "#";
        link.dataset.id = item.id;
        link.innerHTML = '<span class="seq">' + item.id + '</span><span class="t">' +
          escapeHtml(item.title) + '</span><span class="ext">' + item.sourceExt + '</span>';
        link.addEventListener("click", function (event) {
          event.preventDefault();
          show(flat.indexOf(item));
        });
        if (flat.indexOf(item) === current) link.classList.add("active");
        toc.appendChild(link);
      });
    });
    if (needle) counter.textContent = "匹配 " + shown + " 份";
    else counter.textContent = flat.length + " 份材料";
  }

  function escapeHtml(value) {
    return String(value).replace(/[&<>"]/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[ch];
    });
  }

  function show(index) {
    if (index < 0 || index >= flat.length) return;
    if (current >= 0) notes[flat[current].id] = notesText.value;
    current = index;
    var item = flat[index];
    viewer.innerHTML = "";
    if (item.kind === "image") {
      var img = document.createElement("img");
      img.src = encodeURI(item.file);
      img.alt = item.title;
      viewer.appendChild(img);
    } else if (item.kind === "docx") {
      var host = document.createElement("div");
      host.className = "docx-host";
      var status = document.createElement("div");
      status.className = "docx-status";
      status.textContent = "正在渲染 Word 文档…";
      host.appendChild(status);
      viewer.appendChild(host);
      renderDocx(item, host, status);
    } else if (item.kind === "text") {
      appendPaper(viewer, '<pre class="fallback">' + escapeHtml(textOf(item)) + "</pre>");
    } else if (item.kind === "download") {
      appendPaper(viewer, "<h2>" + escapeHtml(item.title) + "</h2>" +
        "<p>此格式（" + item.sourceExt + "）需要在 Excel 中查看。</p>" +
        '<p><a href="' + encodeURI(item.file) + '" download>下载原文件</a></p>');
    } else {
      var frame = document.createElement("iframe");
      frame.dataset.base = item.file;
      frame.src = encodeURI(item.file) + (item.kind === "pdf" ? "#page=1&zoom=page-width" : "");
      viewer.appendChild(frame);
    }
    empty.hidden = true;
    progress.textContent = (index + 1) + " / " + flat.length;
    notesText.value = notes[item.id] || "";
    renderToc(searchBox.value);
  }

  function step(delta) { show(current + delta); }

  function stepPdfPage(delta) {
    var frame = viewer.querySelector("iframe");
    if (!frame || !frame.dataset.base) return;
    var match = /#page=(\\d+)/.exec(frame.getAttribute("src") || "");
    var page = Math.max(1, (match ? parseInt(match[1], 10) : 1) + delta);
    frame.setAttribute("src", encodeURI(frame.dataset.base) + "#page=" + page + "&zoom=page-width");
  }

  document.getElementById("btn-prev").onclick = function () { step(-1); };
  document.getElementById("btn-next").onclick = function () { step(1); };

  var zoomSteps = [1, 1.25, 1.5, 1.75, 2];
  var zoomIndex = 0;
  var zoomBtn = document.getElementById("btn-zoom");
  zoomBtn.onclick = function () {
    zoomIndex = (zoomIndex + 1) % zoomSteps.length;
    var value = zoomSteps[zoomIndex];
    document.documentElement.style.setProperty("--doc-zoom", String(value));
    zoomBtn.textContent = "字号 " + Math.round(value * 100) + "%";
  };

  var presentBtn = document.getElementById("btn-present");
  presentBtn.onclick = function () {
    var on = document.body.classList.toggle("present");
    presentBtn.textContent = on ? "投屏增强 ✓" : "投屏增强";
  };
  document.getElementById("btn-prev-page").onclick = function () { stepPdfPage(-1); };
  document.getElementById("btn-next-page").onclick = function () { stepPdfPage(1); };
  document.getElementById("btn-full").onclick = toggleFull;
  document.getElementById("btn-notes").onclick = function () {
    notesPane.hidden = !notesPane.hidden;
    if (!notesPane.hidden) notesText.focus();
  };
  document.getElementById("btn-close-notes").onclick = function () { notesPane.hidden = true; };
  document.getElementById("btn-export-notes").onclick = function () {
    if (current >= 0) notes[flat[current].id] = notesText.value;
    var lines = ["# " + data.meeting + " 会中备注", "", "导出时间：" + new Date().toLocaleString(), ""];
    flat.forEach(function (item) {
      var value = (notes[item.id] || "").trim();
      if (!value) return;
      lines.push("## " + item.id + " " + item.title);
      lines.push("分组：" + item.group);
      lines.push("");
      lines.push(value);
      lines.push("");
    });
    var blob = new Blob([lines.join("\\n")], { type: "text/markdown;charset=utf-8" });
    var link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = data.meeting + "-会中备注.md";
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
    notesStatus.textContent = "已导出：" + link.download;
  };

  searchBox.addEventListener("input", function () { renderToc(searchBox.value); });

  document.addEventListener("keydown", function (event) {
    var typing = /^(INPUT|TEXTAREA)$/.test(document.activeElement.tagName);
    if (event.key === "/" && !typing) { event.preventDefault(); searchBox.focus(); return; }
    if (event.key === "Escape") { document.activeElement.blur(); return; }
    if (typing) return;
    if (event.key === "ArrowRight") step(1);
    else if (event.key === "ArrowLeft") step(-1);
    else if (event.key === "ArrowDown" || event.key === "PageDown") stepPdfPage(1);
    else if (event.key === "ArrowUp" || event.key === "PageUp") stepPdfPage(-1);
    else if (event.key === " ") { event.preventDefault(); step(1); }
    else if (event.key === "f" || event.key === "F") toggleFull();
  });

  function toggleFull() {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(function () {});
    } else {
      document.exitFullscreen();
    }
  }

  renderToc("");
  if (flat.length > 0) show(0);
})();
`;

const READ_ME_TEXT = `会议材料工作台 · 会议包

怎么用
1. 把整个文件夹拷到 U 盘（不要只拷 index.html）
2. 在会议室电脑上双击 index.html，用 Chrome / Edge 打开
3. 按 F 进入全屏，用 ← → 切换材料，↑ ↓ 翻 PDF 页码
4. 需要记东西时点右上角“📝 备注”，会后点“导出备注”，把生成的
   “-会中备注.md” 文件带回去归档

说明
· 完全离线：不联网、不上传、不需要安装任何软件
· 不要在 U 盘里直接双击 PDF（那样只能一个个翻），从这个页面进入才有目录和顺序
· Word 文档按原版式渲染（字体、字号、表格、图片、分页都保留），
  右上角“字号”可整体放大到 200%，站在电视几米外也看得清
· 万一某个 Word 渲染不出来，页面会自动退回纯文本，并给一个“打开原件”的链接
· 会中备注只保存在浏览器内存里，刷新或关掉页面会丢失，离开前记得导出

需要 Office 才能看的文件（xlsx/xls/csv）会提供下载链接
`;

/* ────────────────────────────────── 执行 ────────────────────────────────── */

const targets = process.argv.slice(2);
if (targets.length === 0) {
  console.error("用法：node scripts/build-bundle.mjs <会议文件夹> [更多文件夹…]");
  process.exit(1);
}

mkdirSync(outRoot, { recursive: true });
for (const target of targets) {
  const sourceDir = resolve(target);
  if (!existsSync(sourceDir)) {
    console.error(`跳过（不存在）：${sourceDir}`);
    continue;
  }
  const result = buildBundle(sourceDir);
  console.log(
    `✓ ${result.meetingName}：${result.count} 份材料，${result.groups} 个分组 → ${result.bundleDir}`,
  );
}
