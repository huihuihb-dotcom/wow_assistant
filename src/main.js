// Tauri 2 原生 IPC 接口
const tauriCore = window.__TAURI__ ? window.__TAURI__.core : { invoke: async () => "Mock" };
const tauriEvent =
  window.__TAURI__ && window.__TAURI__.event ? window.__TAURI__.event : { listen: async () => {} };
const { invoke } = tauriCore;
const { listen } = tauriEvent;

// 界面元素
const hwStatusBadge = document.getElementById("hw-status-badge");
const hwStatusText = document.getElementById("hw-status-text");
const hwDot = document.getElementById("hw-dot");
const btnReconnect = document.getElementById("btn-reconnect");
const consoleBox = document.getElementById("console-log");
const btnClearLog = document.getElementById("btn-clear-log");
const btnCopyLog = document.getElementById("btn-copy-log");

if (btnCopyLog) {
  btnCopyLog.addEventListener("click", async () => {
    const items = Array.from(consoleBox.querySelectorAll(".log-item"));
    const text = items.map((el) => el.innerText).join("\n");
    if (!text) return;

    try {
      await navigator.clipboard.writeText(text);
      const oldText = btnCopyLog.innerText;
      btnCopyLog.innerText = "✅ 已复制";
      setTimeout(() => {
        btnCopyLog.innerText = oldText;
      }, 1500);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
      const oldText = btnCopyLog.innerText;
      btnCopyLog.innerText = "✅ 已复制";
      setTimeout(() => {
        btnCopyLog.innerText = oldText;
      }, 1500);
    }
  });
}

// 选项卡切换
const tabBtns = document.querySelectorAll(".tab-btn");
const tabPanels = document.querySelectorAll(".tab-panel");

tabBtns.forEach((btn) => {
  btn.addEventListener("click", () => {
    tabBtns.forEach((b) => b.classList.remove("active"));
    tabPanels.forEach((p) => p.classList.remove("active"));
    btn.classList.add("active");
    const target = btn.getAttribute("data-tab");
    const panel = document.getElementById(`panel-${target}`);
    if (panel) panel.classList.add("active");
  });
});

// 日志工具
function appendLog(text, level = "info") {
  const item = document.createElement("div");
  item.className = `log-item ${level}`;
  const timeStr = new Date().toLocaleTimeString();
  item.innerText = `[${timeStr}] ${text}`;
  consoleBox.appendChild(item);
  consoleBox.scrollTop = consoleBox.scrollHeight;
}

btnClearLog.addEventListener("click", () => {
  consoleBox.innerHTML = "";
});

// 硬件连接
async function connectHardware() {
  appendLog("正在探测 RP2040 硬件外设...", "info");
  hwStatusText.innerText = "连接中...";
  try {
    const res = await invoke("connect_device", { port: null });
    appendLog(res, "success");
    hwStatusText.innerText = "RP2040 就绪 (COM3)";
    hwDot.style.background = "#10b981";
  } catch (err) {
    appendLog(`连接硬件失败: ${err}`, "error");
    hwStatusText.innerText = "未连接硬件";
    hwDot.style.background = "#ef4444";
  }
}

btnReconnect.addEventListener("click", connectHardware);

// 硬件调试按钮事件
document.getElementById("test-ping")?.addEventListener("click", async () => {
  try {
    const ok = await invoke("ping_device");
    appendLog(`PING 握手测试: ${ok ? "成功 (PONG 收到)" : "失败"}`, ok ? "success" : "warn");
  } catch (e) {
    appendLog(`PING 异常: ${e}`, "error");
  }
});

document.getElementById("test-human-move")?.addEventListener("click", async () => {
  try {
    appendLog("触发片上拟人移动 HM,150,100...", "info");
    const resp = await invoke("human_move", { dx: 150, dy: 100, speed: 1.0 });
    appendLog(`开发板物理引擎响应: ${resp}`, "success");
  } catch (e) {
    appendLog(`拟人移动失败: ${e}`, "error");
  }
});

document.getElementById("test-click")?.addEventListener("click", async () => {
  try {
    const resp = await invoke("mouse_click", { button: "right" });
    appendLog(`右键点击执行: ${resp}`, "success");
  } catch (e) {
    appendLog(`点击失败: ${e}`, "error");
  }
});

let ledColorIdx = 0;
const testColors = [
  { r: 255, g: 0, b: 0, name: "红色" },
  { r: 0, g: 255, b: 0, name: "绿色" },
  { r: 0, g: 0, b: 255, name: "蓝色" },
  { r: 0, g: 30, b: 0, name: "微弱呼吸常态" },
];
document.getElementById("test-led")?.addEventListener("click", async () => {
  const c = testColors[ledColorIdx % testColors.length];
  ledColorIdx++;
  try {
    await invoke("set_device_led", { r: c.r, g: c.g, b: c.b });
    appendLog(`彩灯切换为: ${c.name}`, "info");
  } catch (e) {
    appendLog(`设置彩灯失败: ${e}`, "error");
  }
});

document.getElementById("test-reset")?.addEventListener("click", async () => {
  try {
    const resp = await invoke("reset_device");
    appendLog(`硬件按键复位成功: ${resp}`, "warn");
  } catch (e) {
    appendLog(`复位失败: ${e}`, "error");
  }
});

// ==================== 钓鱼模块 ====================
let isFishing = false;
const btnStartFish = document.getElementById("btn-start-fish");
const btnStopFish = document.getElementById("btn-stop-fish");
const fishStage = document.getElementById("fish-stage");
const fishCount = document.getElementById("fish-count");
const fishCoords = document.getElementById("fish-coords");
const inputFishKey = document.getElementById("fish-key");
const inputFishTimeout = document.getElementById("fish-timeout");

btnStartFish.addEventListener("click", async () => {
  const castKey = inputFishKey.value.trim() || "1";
  const timeoutSecs = parseInt(inputFishTimeout.value, 10) || 22;

  try {
    const focused = await invoke("focus_game_window");
    if (focused) {
      appendLog("🎮 已自动锁定并将《魔兽世界》窗口切至前台", "info");
    } else {
      appendLog("⚠️ 未检测到魔兽窗口，建议将游戏置于前台", "warn");
    }

    await invoke("start_fishing", {
      config: {
        cast_key: castKey,
        timeout_secs: timeoutSecs,
        sensitivity: 3.0,
        loot_delay_ms: 1500,
        post_delay_min_ms: 1200,
        post_delay_max_ms: 2400,
      },
    });

    isFishing = true;
    btnStartFish.disabled = true;
    btnStopFish.disabled = false;
    fishStage.innerText = "准备启动...";
    appendLog("🎣 自动钓鱼状态机已启动，窗口已自动最小化让位给《魔兽世界》", "success");
  } catch (e) {
    appendLog(`启动钓鱼失败: ${e}`, "error");
  }
});

btnStopFish.addEventListener("click", async () => {
  try {
    await invoke("stop_fishing");
    try { await invoke("reset_device"); } catch {}
    isFishing = false;
    btnStartFish.disabled = false;
    btnStopFish.disabled = true;
    fishStage.innerText = "已停止 (IDLE)";
    appendLog("🛑 自动钓鱼已强制停止，已下发硬件复位指令！", "warn");
  } catch (e) {
    appendLog(`停止钓鱼失败: ${e}`, "error");
  }
});

document.getElementById("btn-open-debug-dir")?.addEventListener("click", async () => {
  try {
    const dir = await invoke("open_debug_dir");
    appendLog(`📂 已在系统文件管理器中打开截屏排查目录: ${dir}`, "info");
  } catch (e) {
    appendLog(`打开目录失败: ${e}`, "error");
  }
});

// 监听钓鱼后端实时事件
listen("fishing_event", (event) => {
  const data = event.payload;
  if (!data) return;

  if (typeof data.catch_count === "number") {
    fishCount.innerText = `${data.catch_count} 次`;
  }

  const stage = data.stage;
  let stageName = "未知";

  if (typeof stage === "string") {
    stageName = stage;
  } else if (stage && stage.type) {
    stageName = stage.type;
    if (stage.data && stage.data.x !== undefined) {
      fishCoords.innerText = `(${stage.data.x}, ${stage.data.y})`;
    }
  }

  const stageMap = {
    Idle: "空闲待命",
    Casting: "正在抛竿...",
    WaitingBobber: "等待落水平息",
    SearchingBobber: "视觉搜寻鱼漂",
    HoveringBobber: "拟人滑向鱼漂",
    WatchingBite: "监视咬钩抖动中",
    BiteTriggered: "💥 鱼漂咬钩！",
    Hooking: "提竿收钩！",
    WaitingLoot: "拾取渔获中...",
    TimeoutRecast: "超时防脱重抛",
    Stopped: "已停止 (IDLE)",
  };

  fishStage.innerText = stageMap[stageName] || stageName;
  if (data.message) {
    const level =
      stageName === "BiteTriggered" || stageName === "Hooking"
        ? "success"
        : stageName === "Stopped" || stageName === "TimeoutRecast"
          ? "warn"
          : "info";
    appendLog(data.message, level);
  }

  if (stageName === "Stopped") {
    isFishing = false;
    btnStartFish.disabled = false;
    btnStopFish.disabled = true;
  }
});

// ==================== 采矿模块 ====================


let isMining = false;
const btnStartMine = document.getElementById("btn-start-mine");
const btnStopMine = document.getElementById("btn-stop-mine");
const mineStage = document.getElementById("mine-stage");
const mineCrosshair = document.getElementById("mine-crosshair");
const checkDruid = document.getElementById("druid-mode");
const inputMountKey = document.getElementById("mount-key");
const inputMineDuration = document.getElementById("mine-duration");

btnStartMine.addEventListener("click", async () => {
  const isDruid = checkDruid.checked;
  const mountKey = inputMountKey.value.trim() || "F1";
  const mineDuration = parseInt(inputMineDuration.value, 10) || 3200;

  try {
    await invoke("start_mining", {
      config: {
        is_druid: isDruid,
        mount_key: mountKey,
        mine_duration_ms: mineDuration,
      },
    });

    isMining = true;
    btnStartMine.disabled = true;
    btnStopMine.disabled = false;
    mineStage.innerText = "巡航寻矿中...";
    appendLog("⛏️ 采矿巡航状态机已启动", "success");
  } catch (e) {
    appendLog(`启动采矿失败: ${e}`, "error");
  }
});

btnStopMine.addEventListener("click", async () => {
  try {
    await invoke("stop_mining");
    isMining = false;
    btnStartMine.disabled = false;
    btnStopMine.disabled = true;
    mineStage.innerText = "已停止";
    appendLog("采矿巡航已停止", "warn");
  } catch (e) {
    appendLog(`停止采矿失败: ${e}`, "error");
  }
});

// 监听采矿后端实时事件
listen("mining_event", (event) => {
  const data = event.payload;
  if (!data) return;

  const stage = data.stage;
  let stageName = typeof stage === "string" ? stage : stage?.type || "未知";

  const stageMap = {
    Idle: "待命",
    Cruising: "空中巡航中",
    TargetAcquired: "🎯 锁定矿点准星",
    Approaching: "俯冲降落中",
    MiningWait: "采矿读条中...",
    Looting: "拾取矿石",
    Takeoff: "变鸟升空巡航",
    Stopped: "已停止",
  };

  mineStage.innerText = stageMap[stageName] || stageName;
  if (data.message) {
    appendLog(data.message, stageName === "TargetAcquired" ? "success" : "info");
  }

  if (stageName === "Stopped") {
    isMining = false;
    btnStartMine.disabled = false;
    btnStopMine.disabled = true;
  }
});

// 全局快捷键 F9 (启动钓鱼) / F10 (全局紧急停止)
window.addEventListener("keydown", (e) => {
  if (e.key === "F9") {
    if (!isFishing) btnStartFish.click();
  } else if (e.key === "F10") {
    if (isFishing) btnStopFish.click();
    if (isMining) btnStopMine.click();
  }
});

// 页面加载完成后自动尝试连接硬件
window.addEventListener("DOMContentLoaded", () => {
  connectHardware();
});

