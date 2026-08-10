(function () {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const send = (obj) => window.ipc.postMessage(JSON.stringify(obj));

  let mode = "local";

  // ---- 模式切换 ----
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      mode = tab.dataset.mode;
      document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("is-active", t === tab));
      document.querySelectorAll(".panel").forEach((p) => {
        p.classList.toggle("hidden", p.dataset.panel !== mode);
      });
    });
  });

  // ---- 选择目录 / 输出 ----
  $("pickFolder").addEventListener("click", () => send({ action: "pickFolder" }));
  $("pickOutput").addEventListener("click", () => {
    const t = ($("title").value || "app").trim().replace(/[\\/:*?"<>|]/g, "_");
    send({ action: "pickOutput", defaultName: (t || "app") + ".exe" });
  });

  // ---- 打包 ----
  const packBtn = $("pack");
  packBtn.addEventListener("click", () => {
    const data = {
      mode: mode,
      url: $("url").value.trim(),
      folder: $("folder").value.trim(),
      entry: $("entry").value.trim() || "index.html",
      title: $("title").value.trim() || "App",
      width: parseFloat($("width").value) || 1024,
      height: parseFloat($("height").value) || 720,
      resizable: $("resizable").checked,
      output: $("output").value.trim(),
    };

    // 前端基础校验，给小白即时反馈
    if (mode === "url" && !/^https?:\/\//i.test(data.url)) {
      return setStatus("请输入以 http:// 或 https:// 开头的网址", "err");
    }
    if (mode === "local" && !data.folder) {
      return setStatus("请先选择本地网页目录", "err");
    }
    if (!data.output) {
      return setStatus("请先选择输出 exe 的保存位置", "err");
    }

    packBtn.disabled = true;
    setStatus("正在打包，请稍候…", "busy");
    send({ action: "pack", data: data });
  });

  // ---- 关于弹层 ----
  const overlay = $("aboutOverlay");
  $("openAbout").addEventListener("click", () => overlay.classList.remove("hidden"));
  $("closeAbout").addEventListener("click", () => overlay.classList.add("hidden"));
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) overlay.classList.add("hidden");
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") overlay.classList.add("hidden");
  });

  // ---- 状态显示 ----
  function setStatus(text, kind) {
    const el = $("status");
    el.textContent = text;
    el.className = "status" + (kind ? " " + kind : "");
  }

  // ---- 供 Rust 回调 ----
  window.__setFolder = (path) => {
    $("folder").value = path;
    setStatus("已选择目录：" + path, "");
  };
  window.__setOutput = (path) => {
    $("output").value = path;
    setStatus("输出到：" + path, "");
  };
  window.__packResult = (ok, msg) => {
    packBtn.disabled = false;
    setStatus(msg, ok ? "ok" : "err");
  };
  window.__log = (msg) => setStatus(msg, "");
})();
