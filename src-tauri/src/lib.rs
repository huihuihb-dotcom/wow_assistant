pub mod bot;
pub mod rp2040;
pub mod vision;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};

use bot::{FishingConfig, FishingEngine, MiningConfig, MiningEngine};
use rp2040::Rp2040Controller;

pub struct AppState {
    pub controller: Arc<Mutex<Option<Rp2040Controller>>>,
    pub fishing_running: Arc<AtomicBool>,
    pub mining_running: Arc<AtomicBool>,
}

// 1. 自动连接 RP2040 硬件
#[tauri::command]
fn connect_device(state: State<AppState>, port: Option<String>) -> Result<String, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    let controller = match port {
        Some(p) if !p.trim().is_empty() => Rp2040Controller::connect(&p)?,
        _ => Rp2040Controller::connect_auto()?,
    };
    let port_name = controller.port_name().to_string();
    *lock = Some(controller);
    Ok(format!("成功连接 RP2040 硬件外设 [{}]", port_name))
}

// 2. 检查硬件状态与 PING 握手
#[tauri::command]
fn ping_device(state: State<AppState>) -> Result<bool, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    match lock.as_mut() {
        Some(c) => c.ping(),
        None => Err("硬件尚未连接".to_string()),
    }
}

// 3. 【核心】片上纯硬件拟人移动 (WindMouse 物理算法解算)
#[tauri::command]
fn human_move(
    state: State<AppState>,
    dx: i32,
    dy: i32,
    speed: Option<f32>,
) -> Result<String, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    match lock.as_mut() {
        Some(c) => c.human_move(dx, dy, speed),
        None => Err("硬件尚未连接".to_string()),
    }
}

// 4. 鼠标点击 (left / right / middle)
#[tauri::command]
fn mouse_click(state: State<AppState>, button: String) -> Result<String, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    match lock.as_mut() {
        Some(c) => c.mouse_click(&button),
        None => Err("硬件尚未连接".to_string()),
    }
}

// 5. 键盘敲击按键
#[tauri::command]
fn key_press(state: State<AppState>, key: String) -> Result<String, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    match lock.as_mut() {
        Some(c) => c.key_press(&key),
        None => Err("硬件尚未连接".to_string()),
    }
}

// 6. 设置板载 WS2812 RGB 彩灯
#[tauri::command]
fn set_device_led(state: State<AppState>, r: u8, g: u8, b: u8) -> Result<String, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    match lock.as_mut() {
        Some(c) => c.set_led(r, g, b),
        None => Err("硬件尚未连接".to_string()),
    }
}

// 7. 安全复位所有按键与鼠标按键
#[tauri::command]
fn reset_device(state: State<AppState>) -> Result<String, String> {
    let mut lock = state.controller.lock().map_err(|e| e.to_string())?;
    match lock.as_mut() {
        Some(c) => c.reset_all(),
        None => Err("硬件尚未连接".to_string()),
    }
}

// 8. 启动自动钓鱼状态机
#[tauri::command]
fn start_fishing(
    app: AppHandle,
    state: State<AppState>,
    config: FishingConfig,
) -> Result<String, String> {
    if state.fishing_running.load(Ordering::Relaxed) {
        return Ok("钓鱼已在运行中".to_string());
    }

    // 借鉴 FishingFun 核心机制：启动后立即将 Tauri 界面最小化！
    // Windows 核心调度铁律：当前前台窗口最小化时，系统必然无条件将前台焦点还给魔兽世界！
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.minimize();
    }

    state.fishing_running.store(true, Ordering::Relaxed);
    FishingEngine::start(
        app,
        config,
        state.fishing_running.clone(),
        state.controller.clone(),
    );
    Ok("钓鱼引擎启动成功，界面已自动最小化让位".to_string())
}

// 9. 停止自动钓鱼
#[tauri::command]
fn stop_fishing(app: AppHandle, state: State<AppState>) -> Result<String, String> {
    state.fishing_running.store(false, Ordering::SeqCst);
    if let Ok(mut lock) = state.controller.lock() {
        if let Some(c) = lock.as_mut() {
            let _ = c.reset_all();
        }
    }
    // 钓鱼停止后，自动恢复主窗口显示
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok("已停止钓鱼，已强制刹停硬件".to_string())
}

// 10. 启动采矿状态机
#[tauri::command]
fn start_mining(
    app: AppHandle,
    state: State<AppState>,
    config: MiningConfig,
) -> Result<String, String> {
    if state.mining_running.load(Ordering::Relaxed) {
        return Ok("采矿已在运行中".to_string());
    }

    state.mining_running.store(true, Ordering::Relaxed);
    MiningEngine::start(
        app,
        config,
        state.mining_running.clone(),
        state.controller.clone(),
    );
    Ok("采矿巡航引擎启动成功".to_string())
}

// 11. 停止采矿状态机
#[tauri::command]
fn stop_mining(state: State<AppState>) -> Result<String, String> {
    state.mining_running.store(false, Ordering::SeqCst);
    if let Ok(mut lock) = state.controller.lock() {
        if let Some(c) = lock.as_mut() {
            let _ = c.reset_all();
        }
    }
    Ok("已停止采矿，已强制刹停硬件".to_string())
}

// 12. 查询 YOLO 模型加载状态
#[tauri::command]
fn check_model_status() -> Result<serde_json::Value, String> {
    let search_dirs = [
        "models",
        "../models",
        "d:/projects/rp2040/wow_assistant/models",
    ];
    let file_names = ["bobber.onnx", "best.onnx", "yolov8n_wow_bobber.onnx"];

    for dir in &search_dirs {
        for fname in &file_names {
            let full_path = std::path::Path::new(dir).join(fname);
            if let Ok(meta) = std::fs::metadata(&full_path) {
                let size_mb = meta.len() as f64 / 1024.0 / 1024.0;
                return Ok(serde_json::json!({
                    "found": true,
                    "filename": fname,
                    "size_mb": format!("{:.1}", size_mb),
                    "path": full_path.to_string_lossy(),
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "found": false,
        "filename": "未检测到模型 (采用保底启发式特征)",
        "size_mb": "0",
        "path": "",
    }))
}

// 13. 激活置顶游戏窗口
#[tauri::command]
fn focus_game_window() -> Result<bool, String> {
    window::activate_wow_window()
}

// 14. 一键用资源管理器打开调试图片目录
#[tauri::command]
fn open_debug_dir() -> Result<String, String> {
    let dir = std::path::Path::new("../debug_images");
    let target = if dir.exists() {
        dir
    } else {
        std::path::Path::new("debug_images")
    };
    let _ = std::fs::create_dir_all(target);
    let full_path = std::fs::canonicalize(target).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&full_path)
        .spawn()
        .map_err(|e| format!("打开文件夹失败: {}", e))?;
    Ok(full_path.to_string_lossy().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 程序启动时立即禁用 Windows 前台激活锁
    // 这是跨进程激活游戏窗口的必要前提
    window::disable_foreground_lock();

    tauri::Builder::default()
        .manage(AppState {
            controller: Arc::new(Mutex::new(None)),
            fishing_running: Arc::new(AtomicBool::new(false)),
            mining_running: Arc::new(AtomicBool::new(false)),
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            connect_device,
            ping_device,
            human_move,
            mouse_click,
            key_press,
            set_device_led,
            reset_device,
            start_fishing,
            stop_fishing,
            start_mining,
            stop_mining,
            check_model_status,
            focus_game_window,
            open_debug_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
