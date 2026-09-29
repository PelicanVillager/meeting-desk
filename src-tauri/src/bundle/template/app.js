(function () {
  var data = window.MEETING_BUNDLE || { groups: [], meeting: "会议材料" };
  var flat = [];
  data.groups.forEach(function (group) {
    group.items.forEach(function (item) {
      item.group = group.name;
      item.department = group.department || null;
      flat.push(item);
    });
  });

  var current = -1;
  var pageIndex = 0;
  var notes = {};
  var rotations = {};   // 每份材料记住旋转角度（会中看倒着的扫描件用）
  var collapsed = {};   // 目录里哪些部门/议题被收起了

  var toc = document.getElementById("toc");
  var viewer = document.getElementById("viewer");
  var empty = document.getElementById("empty");
  var searchBox = document.getElementById("search");
  var progress = document.getElementById("progress");
  var counter = document.getElementById("counter");
  var notesPane = document.getElementById("notes");
  var notesText = document.getElementById("notes-text");
  var notesStatus = document.getElementById("notes-status");
  var zoomBtn = document.getElementById("btn-zoom");
  var presentBtn = document.getElementById("btn-present");
  var tocBtn = document.getElementById("btn-toc");
  var notesBtn = document.getElementById("btn-notes");
  var zoomSteps = [1, 1.25, 1.5, 2];
  var zoomIndex = 0;

  document.getElementById("meeting-title").textContent = data.meeting;
  document.getElementById("meeting-date").textContent = data.date || "";

  function escapeHtml(value) {
    return String(value).replace(/[&<>"]/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[ch];
    });
  }

  function base64ToBytes(base64) {
    var binary = window.atob(base64);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return bytes;
  }

  function download(item) {
    if (!item.data) return;
    var blob = new Blob([base64ToBytes(item.data)], { type: "application/octet-stream" });
    var link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = item.filename || item.title + "." + item.ext;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  }

  // ── 目录：按部门分组，部门与议题都能点开/收起 ──────────────────

  function groupRow(text, key, kind, count, isCollapsed) {
    var row = document.createElement(kind === "department" ? "h2" : "h3");
    row.className = kind === "department" ? "dept" : "topic";
    row.title = isCollapsed ? "点击展开" : "点击收起";

    var chev = document.createElement("span");
    chev.className = "chev";
    chev.textContent = isCollapsed ? "▸" : "▾";
    row.appendChild(chev);
    row.appendChild(document.createTextNode(text));
    if (count != null) {
      var badge = document.createElement("span");
      badge.className = "count";
      badge.textContent = "（" + count + "）";
      row.appendChild(badge);
    }
    row.addEventListener("click", function () {
      collapsed[key] = !collapsed[key];
      renderToc(searchBox.value);
    });
    return row;
  }

  function renderToc(filter) {
    toc.innerHTML = "";
    var needle = (filter || "").trim().toLowerCase();
    var shown = 0;
    var lastDept = null;
    var deptCollapsed = false;

    data.groups.forEach(function (group, groupIndex) {
      var items = group.items.filter(function (item) {
        if (!needle) return true;
        var haystack = item.title + " " + group.name + " " + (group.department || "");
        return haystack.toLowerCase().indexOf(needle) >= 0;
      });
      if (items.length === 0) return;

      // 部门标题（搜索时强制展开，避免"搜到了却看不见"）
      if (group.department && group.department !== lastDept) {
        lastDept = group.department;
        var deptKey = "dept:" + group.department;
        deptCollapsed = !needle && !!collapsed[deptKey];
        toc.appendChild(groupRow(group.department, deptKey, "department", null, deptCollapsed));
      } else if (!group.department) {
        lastDept = null;
        deptCollapsed = false;
      }

      var topicKey = "topic:" + groupIndex;
      var topicCollapsed = !needle && (deptCollapsed || !!collapsed[topicKey]);
      toc.appendChild(groupRow(group.name, topicKey, "topic", items.length, topicCollapsed));
      if (topicCollapsed) return;

      items.forEach(function (item) {
        shown += 1;
        var link = document.createElement("a");
        link.href = "#";
        link.innerHTML =
          '<span class="mark">' +
          (item.role === "main" ? "📄" : item.role === "agenda" ? "📋" : item.role === "attachment" ? "📎" : "📃") +
          "</span>" +
          (item.label ? '<span class="label">' + escapeHtml(item.label) + "</span>" : "") +
          '<span class="t">' + escapeHtml(item.title) + "</span>" +
          '<span class="ext">' + escapeHtml(item.ext) + "</span>";
        link.addEventListener("click", function (event) {
          event.preventDefault();
          show(flat.indexOf(item));
        });
        if (flat.indexOf(item) === current) link.classList.add("active");
        toc.appendChild(link);
      });
    });

    counter.textContent = needle ? "匹配 " + shown + " 份" : flat.length + " 份材料";
  }

  // ── 阅读区 ──────────────────────────────────────────────────────

  function show(index) {
    if (index < 0 || index >= flat.length) return;
    if (current >= 0) notes[flat[current].id] = notesText.value;
    current = index;
    pageIndex = 0;
    var item = flat[index];

    viewer.innerHTML = "";
    viewer.scrollTop = 0;
    if (item.kind === "pdf") renderPdf(item);
    else if (item.kind === "docx") renderDocx(item);
    else if (item.kind === "image") renderImage(item);
    else if (item.kind === "text") renderText(item);
    else renderOther(item);

    empty.hidden = true;
    progress.textContent = index + 1 + " / " + flat.length;
    notesText.value = notes[item.id] || "";
    renderToc(searchBox.value);
    updateRotateButton();
  }

  function renderPdf(item) {
    if (!item.pages || item.pages.length === 0) {
      renderText({ title: item.title, text: "这份 PDF 没有渲染出页面。" });
      return;
    }
    var wrap = document.createElement("div");
    wrap.className = "page-wrap";
    var img = document.createElement("img");
    wrap.appendChild(img);

    var bar = document.createElement("div");
    bar.className = "page-bar";
    bar.innerHTML =
      '<button id="page-prev">← 上一页</button>' +
      '<span id="page-pos"></span>' +
      '<button id="page-next">下一页 →</button>' +
      '<span class="hint">↑ ↓ 翻页</span>';
    viewer.appendChild(wrap);
    viewer.appendChild(bar);

    bar.querySelector("#page-prev").onclick = function () { stepPage(-1); };
    bar.querySelector("#page-next").onclick = function () { stepPage(1); };
    updatePage();
  }

  function updatePage() {
    var item = flat[current];
    if (!item || item.kind !== "pdf" || !item.pages) return;
    var img = viewer.querySelector(".page-wrap img");
    var pos = viewer.querySelector("#page-pos");
    if (img) img.src = item.pages[pageIndex];
    if (pos) pos.textContent = "第 " + (pageIndex + 1) + " / " + item.pages.length + " 页";
    viewer.scrollTop = 0;
    applyRotation(item);
  }

  function stepPage(delta) {
    var item = flat[current];
    if (!item || item.kind !== "pdf" || !item.pages) return;
    pageIndex = Math.max(0, Math.min(item.pages.length - 1, pageIndex + delta));
    updatePage();
  }

  function renderDocx(item) {
    var host = document.createElement("div");
    host.className = "docx-host";
    var status = document.createElement("div");
    status.className = "docx-status";
    status.textContent = "正在渲染 Word 文档…";
    host.appendChild(status);
    viewer.appendChild(host);

    var fallback = function (message) {
      host.innerHTML = "";
      var box = document.createElement("div");
      box.className = "notice-box";
      box.innerHTML =
        "<p><strong>Word 文档未能按原版式渲染</strong>：" + escapeHtml(message) + "</p>" +
        '<p><a href="#" id="dl">下载原始 Word 文件</a></p>' +
        "<p>以下为纯文本兜底显示：</p>";
      host.appendChild(box);
      var holder = document.createElement("div");
      holder.className = "doc";
      var inner = document.createElement("div");
      inner.className = "doc-inner";
      var pre = document.createElement("pre");
      pre.className = "fallback";
      pre.textContent = item.text || "（无文本）";
      inner.appendChild(pre);
      holder.appendChild(inner);
      host.appendChild(holder);
      var dl = host.querySelector("#dl");
      if (dl) dl.onclick = function (event) { event.preventDefault(); download(item); };
    };

    if (!window.docx || !window.docx.renderAsync) { fallback("渲染引擎未加载"); return; }
    if (!item.data) { fallback("会议包里缺少该文档的数据"); return; }
    try {
      var bytes = base64ToBytes(item.data);
      window.docx
        .renderAsync(bytes.buffer, host, null, {
          className: "docx", inWrapper: true, breakPages: true,
          ignoreWidth: false, ignoreHeight: false,
          renderHeaders: true, renderFooters: true, renderFootnotes: true,
          experimental: true,
        })
        .then(function () { if (status.parentNode) status.parentNode.removeChild(status); })
        .catch(function (error) { fallback(error && error.message ? error.message : String(error)); });
    } catch (error) {
      fallback(error && error.message ? error.message : String(error));
    }
  }

  function renderImage(item) {
    var wrap = document.createElement("div");
    wrap.className = "image-wrap";
    var img = document.createElement("img");
    img.src = item.dataUri;
    img.alt = item.title;
    wrap.appendChild(img);
    viewer.appendChild(wrap);
    applyRotation(item);
  }

  function renderText(item) {
    var holder = document.createElement("div");
    holder.className = "doc";
    var inner = document.createElement("div");
    inner.className = "doc-inner";
    var pre = document.createElement("pre");
    pre.className = "fallback";
    pre.textContent = item.text || "（无文本）";
    inner.appendChild(pre);
    holder.appendChild(inner);
    viewer.appendChild(holder);
  }

  function renderOther(item) {
    var holder = document.createElement("div");
    holder.className = "doc";
    var inner = document.createElement("div");
    inner.className = "doc-inner";
    inner.innerHTML =
      "<h2>" + escapeHtml(item.title) + "</h2>" +
      "<p>这种格式（" + escapeHtml(item.ext) + "）需要 Office 才能查看，可以下载后在电脑上打开。</p>" +
      '<p><button id="dl">下载这份材料</button></p>';
    holder.appendChild(inner);
    viewer.appendChild(holder);
    var dl = inner.querySelector("#dl");
    if (dl) dl.onclick = function () { download(item); };
  }

  // ── 旋转：扫描件经常是倒的或横的 ─────────────────────────────────

  function rotatable(item) {
    return item && (item.kind === "pdf" || item.kind === "image");
  }

  function applyRotation(item) {
    var img = viewer.querySelector(".page-wrap img, .image-wrap img");
    if (!img) return;
    var deg = rotations[item.id] || 0;
    img.style.transform = deg ? "rotate(" + deg + "deg)" : "";
    img.classList.toggle("rotated", deg % 180 === 90);
    if (deg % 180 === 90) {
      // 转 90/270 后，能放多宽取决于容器宽度，交给 CSS 变量控制
      var wrap = img.parentElement;
      img.style.maxHeight = Math.max(240, wrap.clientWidth - 40) + "px";
      img.style.width = "auto";
      img.style.maxWidth = "none";
    } else {
      img.style.maxHeight = "";
      img.style.width = "";
      img.style.maxWidth = "";
    }
  }

  function rotate(delta) {
    var item = flat[current];
    if (!rotatable(item)) return;
    rotations[item.id] = ((rotations[item.id] || 0) + delta + 360) % 360;
    applyRotation(item);
    updateRotateButton();
  }

  function updateRotateButton() {
    var btn = document.getElementById("btn-rotate");
    var item = flat[current];
    btn.disabled = !rotatable(item);
    var deg = item ? rotations[item.id] || 0 : 0;
    btn.textContent = deg ? "旋转 " + deg + "°" : "旋转";
  }

  // ── 控件 ────────────────────────────────────────────────────────

  function toggleToc() {
    var hidden = document.body.classList.toggle("no-toc");
    tocBtn.textContent = hidden ? "目录" : "目录 ✓";
    tocBtn.title = hidden ? "显示左侧目录（T）" : "隐藏左侧目录（T）";
  }

  function toggleNotes() {
    notesPane.hidden = !notesPane.hidden;
    notesBtn.textContent = notesPane.hidden ? "📝 备注" : "📝 备注 ✓";
    if (!notesPane.hidden) notesText.focus();
  }

  document.getElementById("btn-prev").onclick = function () { show(current - 1); };
  document.getElementById("btn-next").onclick = function () { show(current + 1); };
  document.getElementById("btn-full").onclick = function () {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(function () {});
    } else {
      document.exitFullscreen();
    }
  };
  tocBtn.onclick = toggleToc;
  notesBtn.onclick = toggleNotes;
  document.getElementById("btn-rotate").onclick = function () { rotate(90); };
  document.getElementById("btn-close-notes").onclick = toggleNotes;
  zoomBtn.onclick = function () {
    zoomIndex = (zoomIndex + 1) % zoomSteps.length;
    var value = zoomSteps[zoomIndex];
    document.documentElement.style.setProperty("--doc-zoom", String(value));
    zoomBtn.textContent = "字号 " + Math.round(value * 100) + "%";
    var item = flat[current];
    if (rotatable(item)) applyRotation(item);
  };
  presentBtn.onclick = function () {
    var on = document.body.classList.toggle("present");
    presentBtn.textContent = on ? "投屏增强 ✓" : "投屏增强";
  };
  document.getElementById("btn-export-notes").onclick = function () {
    if (current >= 0) notes[flat[current].id] = notesText.value;
    var lines = ["# " + data.meeting + " 会中备注", "", "导出时间：" + new Date().toLocaleString(), ""];
    flat.forEach(function (item) {
      var value = (notes[item.id] || "").trim();
      if (!value) return;
      lines.push("## " + (item.label ? item.label + " " : "") + item.title);
      lines.push("分组：" + (item.department ? item.department + " / " : "") + item.group);
      lines.push("");
      lines.push(value);
      lines.push("");
    });
    var blob = new Blob([lines.join("\n")], { type: "text/markdown;charset=utf-8" });
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
    if (event.key === "ArrowRight") show(current + 1);
    else if (event.key === "ArrowLeft") show(current - 1);
    else if (event.key === "ArrowDown" || event.key === "PageDown") { event.preventDefault(); stepPage(1); }
    else if (event.key === "ArrowUp" || event.key === "PageUp") { event.preventDefault(); stepPage(-1); }
    else if (event.key === " ") { event.preventDefault(); show(current + 1); }
    else if (event.key === "r" || event.key === "R") rotate(90);
    else if (event.key === "t" || event.key === "T") toggleToc();
    else if (event.key === "n" || event.key === "N") toggleNotes();
    else if (event.key === "f" || event.key === "F") document.getElementById("btn-full").click();
  });

  window.addEventListener("resize", function () {
    var item = flat[current];
    if (rotatable(item)) applyRotation(item);
  });

  renderToc("");
  updateRotateButton();
  if (flat.length > 0) show(0);
})();
