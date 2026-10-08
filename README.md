# 魔兽世界智能硬件助手 (WoW Hardware Assistant)

[![Platform](https://img.shields.io/badge/Platform-Windows%2011%20%7C%20Windows%2010-blue.svg)](https://microsoft.com)
[![Framework](https://img.shields.io/badge/Framework-Tauri%202.x-orange.svg)](https://tauri.app)
[![Language](https://img.shields.io/badge/Language-Rust%20%2B%20HTML5%2FVanilla%20JS-green.svg)](https://www.rust-lang.org)
[![Inference Engine](https://img.shields.io/badge/Inference-DirectML%20(DirectX%2012)-purple.svg)](https://github.com/microsoft/DirectML)
[![Hardware](https://img.shields.io/badge/Hardware-RP2040%20USB%20HID-yellowgreen.svg)](https://www.raspberrypi.com)
[![Security](https://img.shields.io/badge/Security-BlackBox%20%7C%20Non--Invasive-brightgreen.svg)](#-安全与设计理念)

魔兽世界智能硬件助手（**WoW Hardware Assistant**）是一款专为《魔兽世界》（怀旧服与正式服）打造的**黑盒计算机视觉 + 纯硬件级外设模拟**桌面端自动化辅助系统。

系统基于 **Tauri 2.0 + Rust** 高性能后端构建，前端采用现代化响应式控制台。系统通过外置 DirectML GPU 硬件加速的目标检测神经网络（YOLOv8 ONNX）实时感知游戏水面与目标，并通过 USB 串口直驱树莓派 **RP2040 单片机**，由单片机直接向电脑输出标准的硬件级 USB 键盘与鼠标信号。

---

## 🌟 核心特性亮点

### 1. 🛡️ 纯物理外设黑盒安全体系
- **零内存注入 / 零 Hook / 零封包分析**：严守安全红线，绝不读取、修改游戏内存，不注入 DLL，不挂钩 DirectX，不拦截网络封包。
- **真实硬件级外设响应**：所有键鼠操作均由 RP2040 单片机物理模拟，系统与游戏将单片机识别为与普通物理键盘、鼠标无异的 USB HID 复合设备。
- **全链路物理急刹**：界面点击停止或按 F10 时，毫秒级向单片机下发 `RESET` 指令，即刻打断单片机位移与释放按键。

### 2. ⚡ DirectML 跨平台 GPU 硬件加速
- **跨显卡原生支持**：采用微软官方 DirectX 12 DirectML API (`ort 2.0`)，无需配置复杂的 CUDA、cuDNN 或 ROCm 环境。
- **毫秒级极速推理**：开箱即通用支持 **NVIDIA RTX/GTX、AMD Radeon 及 Intel Arc（如实测 Intel Arc B580 独显）** 全系列独显与核显，640×640 图像单次推理耗时低至 **4.3ms**！
- **优雅保底回退（Graceful CPU Fallback）**：针对虚拟机或特殊环境，系统初始化自动探测 GPU，遇异常无缝回退至 CPU 运算，杜绝崩溃闪退。

### 3. 🎯 极速流水线切片扫描 (Pipeline Tile Scan)
- **1:1 原画无损直通**：水面落漂区域按 640×640 步长生成多切片送入模型，彻底解决整图压缩缩放造成的羽毛细节模糊与置信度骤降问题。
- **生产者-消费者流水线**：独立截屏线程（~18ms 节奏）与 GPU 推理主线程并发运行，首帧延迟极低，吞吐量相较串行扫描提升十倍以上。

### 4. 🌊 稳健动态姿态追踪与咬钩检测
- **50 FPS 零抽帧纯内存流水线**：截屏与推理全程在内存流转，彻底移除写盘开销，确保高频捕获浮漂垂直急坠。
- **中位数（Median）水面基线与自适应门限**：基于滑动窗口动态计算水面基准线与波动幅度，平静水面灵敏触发，大浪水面自动防抖。
- **空间物理隔离滤波**：姿态追踪时限定 120px 邻域约束，彻底免疫远处猎人宠物头像、装饰墙壁或水鸟等假特征干扰。
- **拟人生理反应缓冲**：检测到真实咬钩后，执行 600ms ~ 1100ms 随机生理反应延时，使浮漂充分咬稳并模拟真人反应，随后平滑滑向目标右键提竿。
- **提竿防遮挡归位**：收杆后光标平滑移动至屏幕空旷区域，防止光标阻挡下一次抛竿的视觉检测。

### 5. 🪟 Windows 11 穿透级焦点与抛竿保障
- **主动最小化让位（Minimize-to-Yield）**：启动钓鱼后 Tauri 界面主动最小化，顺应 Windows DWM 焦点调度机制，100% 确保系统焦点无缝归还游戏。
- **双通道保险抛竿**：Win32 消息队列直投 (`PostMessageW`) 与 RP2040 硬件按键双通道触发，杜绝前台锁漏键。

---

## 🏗️ 系统架构

```mermaid
flowchart TD
    subgraph UI[前端表现层 (Tauri Webview)]
        Dash[控制台 Dashboard: 参数配置 / 状态看板 / 调试面板]
        Log[实时高亮日志控制台]
    end

    subgraph RustCore[后端核心 (Rust / Tauri 2.0)]
        IPC[Tauri Command 调度中心]
        FSM[业务有限状态机: 自动钓鱼 / 巡航采矿]
        Capture[无损视觉采集: xcap + Win32 客户区校准]
        Detector[YOLOv8 DirectML GPU 推理引擎 (ort 2.0)]
        Tracker[50 FPS 中位数姿态追踪与下沉分析]
        Driver[RP2040 串口通信驱动 (serialport)]
    end

    subgraph Hardware[执行层 (物理外设)]
        MCU[微雪 RP2040-Zero 单片机 (CircuitPython 10.x)]
        Engine[片上 WindMouse 拟人动力学物理引擎]
        HID[USB HID 复合设备 (键盘 + 鼠标)]
    end

    subgraph Target[运行环境]
        Game[《魔兽世界》客户端 (怀旧服 / 正式服)]
    end

    Dash -->|invoke| IPC
    IPC --> FSM
    FSM --> Capture
    Capture --> Detector
    Detector --> Tracker
    Tracker -->|触发动作| Driver
    Driver -->|USB 串口指令 HM/MC/KP/RESET| MCU
    MCU --> Engine
    Engine --> HID
    HID -->|硬件物理输入| Game
    FSM -.->|实时状态事件| Log
```

---

## 📁 目录结构说明

```text
wow_assistant/
├── .antigravity/            # 项目基准架构与演进维护文档
├── models/                  # 深度学习模型存储目录
│   └── bobber.onnx          # YOLOv8 鱼漂目标检测 ONNX 模型
├── src/                     # 前端界面源码 (HTML / CSS / Vanilla JS)
│   ├── assets/              # 前端静态图标资源
│   ├── index.html           # 助手主界面骨架
│   ├── main.js              # 前端业务逻辑与 Tauri API 交互
│   └── styles.css           # 现代化深色玻璃质感样式表
├── src-tauri/               # Rust 后端核心源码
│   ├── Cargo.toml           # Rust 依赖声明 (tauri, ort, xcap, serialport 等)
│   ├── tauri.conf.json      # Tauri 配置文件 (应用标题、资源打包、窗口设置)
│   ├── build.rs             # 构建脚本 (图标监听与资源绑定)
│   ├── icons/               # 跨平台应用图标 (ico, icns, png)
│   └── src/
│       ├── main.rs          # 桌面端程序入口
│       ├── lib.rs           # Tauri IPC 命令注册与生命周期管理
│       ├── window.rs        # Win32 穿透级窗口激活与消息队列投递
│       ├── bot/             # 自动化状态机实现
│       │   ├── mod.rs       # 状态机模块声明
│       │   ├── fishing.rs   # 自动钓鱼引擎 (流水线扫描、姿态追踪、起竿拾取)
│       │   └── mining.rs    # 巡航采矿引擎 (准星巡航、右键采集、变鸟起飞)
│       ├── rp2040/          # RP2040 单片机硬件驱动库
│       │   ├── mod.rs       # 驱动模块声明
│       │   └── controller.rs# 串口握手、WindMouse 移动、按键下发与硬件急刹
│       └── vision/          # 计算机视觉管线
│           ├── mod.rs       # 视觉模块声明
│           ├── capture.rs   # 屏幕捕获、切片生成、物理坐标平滑缓动
│           └── detector.rs  # YOLOv8 DirectML GPU / CPU 推理
├── package.json             # 前端项目配置与开发脚本
└── README.md                # 助手客户端专属使用与说明文档
```

---

## 🛠️ 环境准备与硬件连接

### 1. 硬件准备
- **微雪 Waveshare RP2040-Zero**（或其他兼容 RP2040 板卡）。
- 确保开发板已刷入 CircuitPython 固件并部署了配套的 `board/code.py` 镜像（详见父目录 [RP2040 全局说明](../README.md)）。
- 使用**支持全双工数据传输**的 Type-C 数据线将开发板接入电脑。

### 2. 软件与模型准备
1. 确保已安装系统基础环境：
   - Windows 10 / Windows 11 (64位)
   - [Node.js](https://nodejs.org/) (v18+)
   - [Rust 工具链](https://rustup.rs/) (1.75+)
2. 模型文件放置：
   - 确保 `models/bobber.onnx` 存在于当前目录的 `models/` 文件夹下。

---

## 🚀 开发与打包编译

### 1. 本地开发与调试

在 `wow_assistant` 目录下打开终端（PowerShell 或 CMD）：

```powershell
# 1. 安装前端依赖
npm install

# 2. 启动 Tauri 开发环境 (自动编译 Rust 并拉起桌面端窗口)
npm run dev
```

> [!TIP]
> 首次运行 `npm run dev` 时，Cargo 将下载并编译依赖（包括 DirectML 推理绑定与图像库）。`Cargo.toml` 中已针对开发模式配置了 `opt-level = 3` 深度优化，确保在调试模式下也能保持 GPU 高频满血运行。

### 2. 生产分发包构建 (Build)

```powershell
npm run build
```

构建完成后，独立的单文件 Windows 中文安装包将输出至：
`src-tauri/target/release/bundle/nsis/魔兽世界智能助手_0.1.0_x64-setup.exe`

- **单文件向导安装包**：`src-tauri/target/release/bundle/nsis/魔兽世界智能助手_0.1.0_x64-setup.exe`（约 16MB，内置图标向导与自动注册桌面快捷方式）
- **免安装绿色便携包**：`src-tauri/target/release/bundle/wow_assistant_0.1.0_x64_portable.zip`（约 20MB，解压即用，包含 `wow_assistant.exe` 与内置 `models/`）


---

## 📖 界面操作指南

### 1. 自动连接与状态自检
- 启动应用后，右上角会显示 RP2040 硬件连接状态徽章（绿色为在线，红色为未连接）。
- 若未自动识别，可点击“**重连**”按钮重新扫描系统 USB 虚拟串口。

### 2. 🎣 自动钓鱼模式 (Fishing)
1. **配置按键**：将“**游戏内抛竿快捷键**”设为你在魔兽世界动作条中放置“钓鱼”技能的键位（默认为 `1`）。
2. **设置超时**：默认 22 秒，若超时未捕获咬钩则自动重新抛投。
3. **启动运行**：
   - 点击界面上的 **▶ 启动钓鱼** 或在任意界面按下快捷键 **F9**；
   - 助手将自动最小化让位，并将前台焦点切换至《魔兽世界》窗口；
   - 自动执行抛竿 $\rightarrow$ 流水线 GPU 水面扫描 $\rightarrow$ 锁定鱼漂 $\rightarrow$ 50 FPS 追踪姿态 $\rightarrow$ 识别咬钩下沉 $\rightarrow$ 拟人反应延时 $\rightarrow$ 硬件右键收线 $\rightarrow$ 拾取归位循环。
4. **紧急刹停**：
   - 按下界面上的 **⏹ 停止** 或按下快捷键 **F10**，系统立即中断所有动作，单片机硬件发送 `RESET` 释放按键，并自动弹回助手主界面。

### 3. ⛏️ 巡航采矿模式 (Mining)
- 支持德鲁伊瞬发变鸟或普通飞行坐骑配置；
- 支持设定采矿读条等待时间（默认 3200ms）；
- 结合准星巡航引导、自动接近、右键采集与飞升循环。

### 4. 🎮 硬件调试控制台 (Hardware)
- 提供底层直通测试按钮：
  - **PING 握手测试**：检测串口连通与往返耗时；
  - **测试片上拟人移动 (HM)**：让光标执行一次单片机片上算解的 WindMouse 轨迹；
  - **测试硬件右键点击**：下发 `MC,right` 测试物理点击；
  - **切换板载彩灯颜色**：动态设置板载 WS2812 RGB 灯光；
  - **紧急释放全部按键 (RESET)**：一键安全复位。

---

## 🔍 调试排查与日志

- **查看实时日志**：主界面底部提供高亮日志控制台，逐帧透明输出当前动作、鱼漂坐标 `(X, Y)`、中位数基准 `median_y`、实时位移 $\Delta Y$ 及置信度。
- **一键复制日志**：点击控制台右上角的“**📋 复制日志**”按钮，可直接将完整诊断信息粘贴给团队或社区分析。
- **排查截屏目录**：点击“**📂 查看截屏排查目录**”按钮，系统将自动通过 Windows 文件资源管理器打开调试目录，方便查看无损截片与模型识别情况。

---

## ⚖️ 免责声明与使用规范

1. 本项目仅供嵌入式硬件开发、计算机视觉与自动化控制技术研究与交流使用。
2. 本软件遵守黑盒非入侵规范，不破解、不篡改任何游戏客户端文件与通信数据。
3. 请合理规范使用辅助技术，遵守相关游戏的使用协议与社区准则，开发者不对因使用本工具导致的任何账号风险承担法律或连带责任。
