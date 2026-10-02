use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::rp2040::Rp2040Controller;
use crate::vision::{capture_primary_screen, get_cursor_pos};

/// 采矿与飞行配置项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningConfig {
    pub is_druid: bool,
    pub mount_key: String,
    pub mine_duration_ms: u64,
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            is_druid: true,
            mount_key: "F1".to_string(),
            mine_duration_ms: 3200,
        }
    }
}

/// 采矿状态机当前阶段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MiningStage {
    Idle,
    Cruising,
    TargetAcquired { x: i32, y: i32 },
    Approaching,
    MiningWait,
    Looting,
    Takeoff,
    Stopped,
}

/// 采矿实时事件数据包
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningEvent {
    pub stage: MiningStage,
    pub mined_count: u32,
    pub message: String,
}

/// 采矿巡航状态机引擎
pub struct MiningEngine;

impl MiningEngine {
    /// 启动采矿状态机后台工作线程
    pub fn start(
        app_handle: AppHandle,
        config: MiningConfig,
        running_flag: Arc<AtomicBool>,
        controller_arc: Arc<std::sync::Mutex<Option<Rp2040Controller>>>,
    ) {
        thread::spawn(move || {
            let mut mined_count = 0u32;

            emit_event(
                &app_handle,
                MiningStage::Idle,
                mined_count,
                "⛏️ 采矿巡航状态机已启动，进入巡航监控...".to_string(),
            );

            while running_flag.load(Ordering::Relaxed) {
                // 1. 检查硬件
                {
                    let lock = controller_arc.lock().unwrap();
                    if lock.is_none() {
                        emit_event(
                            &app_handle,
                            MiningStage::Stopped,
                            mined_count,
                            "❌ 硬件未连接，请先连接 RP2040".to_string(),
                        );
                        break;
                    }
                }

                // 2. 巡航阶段 (Cruising)
                emit_event(
                    &app_handle,
                    MiningStage::Cruising,
                    mined_count,
                    "空中巡航中，视觉检测矿点准星 / 小地图矿标...".to_string(),
                );

                // 模拟巡航扫描（当接入实际 YOLO 准星模型时在此截屏预测）
                #[allow(unused_mut)]
                let mut target_found = false;
                #[allow(unused_mut)]
                let mut target_x = 0;
                #[allow(unused_mut)]
                let mut target_y = 0;

                for _ in 0..10 {
                    if !running_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(screen) = capture_primary_screen() {
                        let (w, h) = screen.dimensions();
                        // 预留准星/矿标探测逻辑
                        // 此处为示范探测：可检测屏幕中心准星或特定高亮黄色矿点
                        let _ = (w, h);
                    }
                    thread::sleep(Duration::from_millis(300));
                }

                if !running_flag.load(Ordering::Relaxed) {
                    break;
                }

                // 若发现矿点准星（此处支持外部信号或模型识别到准星时执行着陆与采矿）
                if target_found {
                    emit_event(
                        &app_handle,
                        MiningStage::TargetAcquired {
                            x: target_x,
                            y: target_y,
                        },
                        mined_count,
                        format!("🎯 锁定矿点准星 ({}, {})，俯冲接近...", target_x, target_y),
                    );

                    // 拟人移动并右键采矿
                    let (cur_x, cur_y) = get_cursor_pos();
                    let dx = target_x - cur_x;
                    let dy = target_y - cur_y;

                    {
                        let mut lock = controller_arc.lock().unwrap();
                        if let Some(c) = lock.as_mut() {
                            let _ = c.human_move(dx, dy, Some(1.2));
                            thread::sleep(Duration::from_millis(150));
                            let _ = c.mouse_click("right");
                        }
                    }

                    // 3. 采矿读条等待 (MiningWait)
                    emit_event(
                        &app_handle,
                        MiningStage::MiningWait,
                        mined_count,
                        format!("正在采集矿石，读条等待 {} ms...", config.mine_duration_ms),
                    );
                    sleep_check(config.mine_duration_ms, &running_flag);

                    // 4. 拾取等待
                    emit_event(
                        &app_handle,
                        MiningStage::Looting,
                        mined_count,
                        "等待拾取矿石与宝石...".to_string(),
                    );
                    sleep_check(1200, &running_flag);

                    mined_count += 1;

                    // 5. 变鸟 / 坐骑起飞升空 (Takeoff)
                    emit_event(
                        &app_handle,
                        MiningStage::Takeoff,
                        mined_count,
                        if config.is_druid {
                            format!("🦅 德鲁伊瞬发变鸟 [{}]，空格升空巡航...", config.mount_key)
                        } else {
                            format!("🐎 上坐骑 [{}] 读条起飞...", config.mount_key)
                        },
                    );

                    {
                        let mut lock = controller_arc.lock().unwrap();
                        if let Some(c) = lock.as_mut() {
                            let _ = c.key_press(&config.mount_key);

                            if !config.is_druid {
                                // 普通坐骑需读条 1.6 秒
                                thread::sleep(Duration::from_millis(1600));
                            } else {
                                thread::sleep(Duration::from_millis(300));
                            }

                            // 敲击并长按空格键 1.2 秒以升入安全巡航高度
                            let _ = c.key_down("SPACE");
                            thread::sleep(Duration::from_millis(1200));
                            let _ = c.key_up("SPACE");
                        }
                    }
                }

                // 巡航心跳
                sleep_check(1000, &running_flag);
            }

            emit_event(
                &app_handle,
                MiningStage::Stopped,
                mined_count,
                "🛑 采矿巡航已安全停止。".to_string(),
            );
        });
    }
}

fn emit_event(app: &AppHandle, stage: MiningStage, mined_count: u32, message: String) {
    let evt = MiningEvent {
        stage,
        mined_count,
        message,
    };
    let _ = app.emit("mining_event", evt);
}

fn sleep_check(ms: u64, flag: &AtomicBool) {
    let step = 50;
    let iterations = ms / step;
    for _ in 0..iterations {
        if !flag.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(step));
    }
}
