use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::rp2040::Rp2040Controller;
use crate::vision::{
    capture_primary_screen, capture_wow_window, crop_roi, get_water_center_wide,
    smooth_move_cursor, DetectionBox, YoloDetector,
};
use crate::window::{activate_wow_window, post_key_to_wow};

/// 钓鱼配置项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishingConfig {
    pub cast_key: String,
    pub timeout_secs: u64,
    pub sensitivity: f32,
    pub loot_delay_ms: u64,
    pub post_delay_min_ms: u64,
    pub post_delay_max_ms: u64,
}

impl Default for FishingConfig {
    fn default() -> Self {
        Self {
            cast_key: "1".to_string(),
            timeout_secs: 22,
            sensitivity: 3.0,
            loot_delay_ms: 1500,
            post_delay_min_ms: 1200,
            post_delay_max_ms: 2400,
        }
    }
}

/// 钓鱼状态机当前阶段
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FishingStage {
    Idle,
    Casting,
    WaitingBobber,
    SearchingBobber,
    HoveringBobber,
    WatchingBite { x: i32, y: i32 },
    BiteTriggered { delta_y: f32, miss_count: u32 },
    Hooking,
    WaitingLoot,
    TimeoutRecast,
    Stopped,
}

/// 向前端汇报的钓鱼事件载荷
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FishingEvent {
    pub stage: FishingStage,
    pub catch_count: u32,
    pub message: String,
}

/// 自动钓鱼状态机引擎
pub struct FishingEngine;

impl FishingEngine {
    pub fn start(
        app_handle: AppHandle,
        config: FishingConfig,
        running_flag: Arc<AtomicBool>,
        controller_arc: Arc<std::sync::Mutex<Option<Rp2040Controller>>>,
    ) {
        let detector = Arc::new(YoloDetector::new("models/bobber.onnx", 0.04));

        thread::spawn(move || {
            let mut catch_count = 0u32;

            // 置前台 + 3 秒倒计时
            let wow_ready = activate_wow_window().unwrap_or(false);
            if !wow_ready {
                emit_event(&app_handle, FishingStage::Idle, catch_count,
                    "⚠️ 未能自动获取魔兽前台，请手动点击游戏窗口".to_string());
            }
            for sec in (1..=3).rev() {
                if !running_flag.load(Ordering::SeqCst) { return; }
                emit_event(&app_handle, FishingStage::Idle, catch_count,
                    format!("⏱️ {}s... {}", sec,
                        if sec == 1 { format!("按键 [{}]", config.cast_key) } else { "准备".to_string() }));
                sleep_check(1000, &running_flag);
            }

            while running_flag.load(Ordering::SeqCst) {
                // ── 抛竿 ──────────────────────────────────────────────────────
                let post_ok = post_key_to_wow(&config.cast_key);
                {
                    let mut lock = controller_arc.lock().unwrap();
                    if let Some(c) = lock.as_mut() {
                        let _ = c.key_down(&config.cast_key);
                        thread::sleep(Duration::from_millis(120));
                        let _ = c.key_up(&config.cast_key);
                    }
                }
                emit_event(&app_handle, FishingStage::Casting, catch_count,
                    format!("🎣 抛竿 [{}] PostMsg:{}", config.cast_key,
                        if post_ok { "✓" } else { "✗" }));

                // 等水花平息
                emit_event(&app_handle, FishingStage::WaitingBobber, catch_count,
                    "🌊 入水，等待平息...".to_string());
                sleep_check(1200, &running_flag);
                if !running_flag.load(Ordering::SeqCst) { break; }

                // ── 流水线搜索鱼漂 ────────────────────────────────────────────
                let search_start = Instant::now();
                let mut detected_screen_x = 0i32;
                let mut detected_screen_y = 0i32;
                let mut found_target = false;

                let (frame_tx, frame_rx) = mpsc::sync_channel::<Result<crate::vision::WindowCapture, String>>(2);
                let scan_flag = Arc::clone(&running_flag);
                let cap_thread = thread::spawn(move || {
                    for _ in 0..30 {
                        if !scan_flag.load(Ordering::SeqCst) { break; }
                        let frame = capture_wow_window();
                        if frame_tx.send(frame).is_err() { break; }
                        thread::sleep(Duration::from_millis(18));
                    }
                });

                let mut frame_count = 0u32;
                'pipeline: for frame_result in frame_rx.iter() {
                    if !running_flag.load(Ordering::SeqCst) { break; }
                    let win_cap = match frame_result {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    frame_count += 1;
                    let t0 = Instant::now();

                    let tile = match get_water_center_wide(&win_cap) {
                        Some(t) => t,
                        None => continue,
                    };

                    let detections = detector.detect_candidates(&tile.image, 0.012).unwrap_or_default();
                    let infer_ms = t0.elapsed().as_millis();
                    const LOCK_THRESHOLD: f32 = 0.015;

                    if let Some(top) = detections.first() {
                        if top.confidence >= LOCK_THRESHOLD {
                            let (lx, ly) = top.center();
                            let sx = win_cap.win_x + tile.offset_x as i32 + lx;
                            let sy = win_cap.win_y + tile.offset_y as i32 + ly;
                            detected_screen_x = sx;
                            detected_screen_y = sy;
                            found_target = true;
                            emit_event(&app_handle, FishingStage::SearchingBobber, catch_count,
                                format!("🎯 锁定! {:.1}% ({},{}) 帧#{} {}ms 总:{:.0}ms",
                                    top.confidence * 100.0, sx, sy,
                                    frame_count, infer_ms, search_start.elapsed().as_millis()));
                            break 'pipeline;
                        }
                        // 未达阈值：只打印最高置信度
                        let best_conf = top.confidence * 100.0;
                        let (cx, cy) = top.center();
                        emit_event(&app_handle, FishingStage::SearchingBobber, catch_count,
                            format!("🔍 #{} {:.1}%@({},{}) {}ms", frame_count, best_conf, cx, cy, infer_ms));
                    } else {
                        emit_event(&app_handle, FishingStage::SearchingBobber, catch_count,
                            format!("🔍 #{} 无目标 {}ms", frame_count, infer_ms));
                    }
                }

                drop(frame_rx);
                let _ = cap_thread.join();

                if !found_target {
                    emit_event(&app_handle, FishingStage::TimeoutRecast, catch_count,
                        "⚠️ 未找到鱼漂，重新抛竿".to_string());
                    sleep_check(2000, &running_flag);
                    continue;
                }

                let target_x = detected_screen_x;
                let target_y = detected_screen_y;
                if !running_flag.load(Ordering::SeqCst) { break; }

                // ── 追踪咬钩 ──────────────────────────────────────────────────
                emit_event(&app_handle, FishingStage::WatchingBite { x: target_x, y: target_y },
                    catch_count, format!("👁 追踪 ({},{})", target_x, target_y));

                let start_time = Instant::now();
                let mut hooked = false;
                let mut all_y: Vec<f32> = Vec::new();
                let mut history_x: VecDeque<f32> = VecDeque::with_capacity(30);

                // 去掉上限封顶：noise 高时阈值自然高（近/乱水面），noise 低时阈值低（远/平静）
                // 公式：(noise × 2.0 + 1.5).max(3.0) × sens_scale
                // noise=0.8 → 3.1px | noise=1.6 → 4.7px | noise=4.4 → 10.3px | noise=6.4 → 14.3px
                let sens_scale = (config.sensitivity / 3.0).clamp(0.5, 1.8);

                let mut prev_bobber_y: Option<f32> = None;
                let mut bite_confirm_counter = 0u32;
                let mut prev_bite_dir: i32 = 0; // 上次触发的方向：+1=下沉 -1=上浮 0=未知
                let mut miss_streak = 0u32;
                let mut frame_idx = 0u32;
                const MAX_MISS_STREAK: u32 = 60;
                let mut cur_target_x = target_x;
                let mut cur_target_y = target_y;

                while running_flag.load(Ordering::SeqCst) {
                    if start_time.elapsed().as_secs() >= config.timeout_secs {
                        emit_event(&app_handle, FishingStage::TimeoutRecast, catch_count,
                            "⏱️ 超时，重新抛竿".to_string());
                        break;
                    }

                    // 固定裁切窗口：以初始锁定位置为中心，不再追着当前检测位置跛
                    // 好处：delta_y 完全来自鱼漂真实移动，消除 crop 重心带入的坐标漂移噪声
                    let (crop_cx, crop_cy) = (target_x, target_y); // 固定采样中心
                    let track_result = capture_primary_screen().and_then(|screen| {
                        let (sw, sh) = screen.dimensions();
                        let track_w = 640u32.min(sw);
                        let track_h = 640u32.min(sh);
                        let safe_x = (crop_cx - (track_w / 2) as i32).clamp(0, (sw - track_w) as i32) as u32;
                        let safe_y = (crop_cy - (track_h / 2) as i32).clamp(0, (sh - track_h) as i32) as u32;
                        crop_roi(&screen, safe_x, safe_y, track_w, track_h).map(|img| (img, safe_x, safe_y))
                    });

                    match track_result {
                        Ok((roi, safe_x, safe_y)) => {
                            // local_target: 锁定目标在 crop 内的期望位置
                            let local_target_x = crop_cx - safe_x as i32;
                            let local_target_y = crop_cy - safe_y as i32;

                            match detector.detect_candidates(&roi, 0.012) {
                                Ok(candidates) => {
                                    // 在期望位置周围 160px 内寻找检测框（比之前 100px 稍大，应对鱼漂少量空间漂移）
                                    let mut in_range: Vec<&DetectionBox> = candidates.iter().filter(|b| {
                                        let (cx, cy) = b.center();
                                        let dx = cx - local_target_x;
                                        let dy = cy - local_target_y;
                                        (dx * dx + dy * dy) <= 160 * 160
                                    }).collect();
                                    in_range.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

                                    if let Some(best) = in_range.first().copied() {
                                        miss_streak = 0;
                                        let (local_cx, local_cy) = best.center();
                                        // 屏幕坐标：仅用于鼠标移动和日志显示
                                        let screen_x = safe_x as i32 + local_cx;
                                        let screen_y = safe_y as i32 + local_cy;
                                        let conf = best.confidence;

                                        // 更新屏幕坐标（仅用于鼠标定位）
                                        cur_target_x = screen_x;
                                        cur_target_y = screen_y;
                                        frame_idx += 1;

                                        // 动态跟踪：如果鱼漂漂移超出 crop 边缘 120px，则重新居中以应对暂时漂移
                                        // 注：不用 target_x/y 是 mut，这里用 cur_target 监控位置
                                        // 如需实现动态重居中可加居中逻辑

                                        // 跟踪 bobber_y 用 LOCAL 坐标（crop 内位置），不再用屏幕 Y
                                        // LOCAL 坐标完全反映鱼漂真实移动，消除 crop 漂移噪声
                                        let bobber_x = local_cx as f32;
                                        let bobber_y = local_cy as f32;
                                        if history_x.len() >= 30 { history_x.pop_front(); }
                                        history_x.push_back(bobber_x);

                                        // FishingFun 全历史中位数（限 150 帧防旧数据干扰）
                                        all_y.push(bobber_y);
                                        if all_y.len() > 150 { all_y.remove(0); }
                                        let mut sorted_y = all_y.clone();
                                        sorted_y.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                        let median_y = sorted_y[sorted_y.len() / 2];

                                        let dynamic_noise: f32 = if all_y.len() >= 3 {
                                            all_y.iter().map(|&y| (y - median_y).abs()).sum::<f32>() / all_y.len() as f32
                                        } else { 0.0 };

                                        // 分段阈值：
                                        //   noise < 3px (远/平静)：strike = noise×2+1.5，低至 3px，捕捉远处小幅咬钩
                                        //   noise ≥ 3px (近/乱)  ：strike = 8px 固定，和旧逻辑等效，保证在高噪底下可达
                                        let strike_px = if dynamic_noise >= 3.0 {
                                            8.0f32 * sens_scale
                                        } else {
                                            ((dynamic_noise * 2.0 + 1.5) * sens_scale).max(3.0)
                                        };
                                        // 速度阈值：noise × 1.8，保持统一（高低 noise 均适用）
                                        let jerk_speed = ((dynamic_noise * 1.8) * sens_scale).max(2.5);

                                        let delta_y = bobber_y - median_y;
                                        let instant_speed_y = prev_bobber_y.map(|py| bobber_y - py).unwrap_or(0.0);
                                        prev_bobber_y = Some(bobber_y);

                                        let abs_dy = delta_y.abs();
                                        let abs_spd = instant_speed_y.abs();

                                        // 每 5 帧常规日志；咬钩确认中每帧都打
                                        if frame_idx % 5 == 0 || bite_confirm_counter > 0 {
                                            emit_event(&app_handle,
                                                FishingStage::WatchingBite { x: screen_x, y: screen_y },
                                                catch_count,
                                                format!("📍 #{} ΔY:{:+.1}/{:.1} spd:{:+.1} box:{}x{} n:{:.1} {:.0}%{}",
                                                    frame_idx, delta_y, strike_px,
                                                    instant_speed_y,
                                                    best.width, best.height,
                                                    dynamic_noise, conf * 100.0,
                                                    if bite_confirm_counter > 0 { format!(" ★{}", bite_confirm_counter) } else { String::new() }));
                                        }

                                        // 位移判定（无速度要求，避免遗漏慢速咬钩）
                                        let is_sink = abs_dy >= strike_px;
                                        // 速度判定（单帧急速跳动，辅助捕捉快速咬钩）
                                        let is_step_jerk = abs_spd >= jerk_speed
                                            && abs_dy >= (dynamic_noise * 0.8).max(1.5);

                                        // 方向一致性：咬钩应连续向同一方向移动，水波振荡（+/-交替）不累积
                                        let cur_dir = if delta_y >= 0.0 { 1i32 } else { -1i32 };

                                        if (is_sink || is_step_jerk) && all_y.len() >= 8 {
                                            if prev_bite_dir == 0 || prev_bite_dir == cur_dir {
                                                // 方向一致：累积
                                                bite_confirm_counter += 1;
                                            } else {
                                                // 方向反转（振荡水波）：重置为 1 而非累积
                                                bite_confirm_counter = 1;
                                            }
                                            prev_bite_dir = cur_dir;

                                            // 单帧强烈信号直接触发（不等第2帧）
                                            let is_huge = abs_dy >= strike_px * 2.0 || abs_spd >= jerk_speed * 2.0;
                                            if is_huge || bite_confirm_counter >= 2 {
                                                let reason = if is_step_jerk {
                                                    format!("急坠 spd:{:+.1} ΔY:{:+.1}", instant_speed_y, delta_y)
                                                } else {
                                                    format!("下顿 ΔY:{:+.1} >{:.1}px", delta_y, strike_px)
                                                };
                                                emit_event(&app_handle,
                                                    FishingStage::BiteTriggered { delta_y, miss_count: 0 },
                                                    catch_count,
                                                    format!("💥 上钩! ({},{}) {}", screen_x, screen_y, reason));
                                                hooked = true;
                                                break;
                                            }
                                        } else {
                                            // 衰减而非清零：允许非连续帧在 2 帧窗口内累积
                                            bite_confirm_counter = bite_confirm_counter.saturating_sub(1);
                                            if bite_confirm_counter == 0 { prev_bite_dir = 0; }
                                        }
                                    } else {
                                        miss_streak += 1;
                                        bite_confirm_counter = 0;
                                        prev_bobber_y = None;

                                        if miss_streak % 15 == 0 {
                                            emit_event(&app_handle,
                                                FishingStage::WatchingBite { x: cur_target_x, y: cur_target_y },
                                                catch_count,
                                                format!("👻 遮挡 {}/{}", miss_streak, MAX_MISS_STREAK));
                                        }

                                        if miss_streak >= MAX_MISS_STREAK {
                                            emit_event(&app_handle, FishingStage::TimeoutRecast, catch_count,
                                                format!("🔄 鱼漂消失 {}帧，重新抛竿", miss_streak));
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    emit_event(&app_handle,
                                        FishingStage::WatchingBite { x: cur_target_x, y: cur_target_y },
                                        catch_count, format!("⚠️ 推理错误: {}", e));
                                }
                            }
                        }
                        Err(e) => {
                            emit_event(&app_handle,
                                FishingStage::WatchingBite { x: cur_target_x, y: cur_target_y },
                                catch_count, format!("⚠️ 截屏错误: {}", e));
                        }
                    }

                    thread::sleep(Duration::from_millis(3));
                }

                let target_x = cur_target_x;
                let target_y = cur_target_y;

                if !running_flag.load(Ordering::SeqCst) { break; }

                // ── 收杆提钩 ──────────────────────────────────────────────────
                if hooked {
                    let reaction_ms = rand_range(600, 1100);
                    emit_event(&app_handle, FishingStage::Hooking, catch_count,
                        format!("⚡ 咬钩! 反应 {}ms → ({},{})", reaction_ms, target_x, target_y));
                    sleep_check(reaction_ms, &running_flag);
                    if !running_flag.load(Ordering::SeqCst) { break; }

                    smooth_move_cursor(target_x, target_y);
                    thread::sleep(Duration::from_millis(60));

                    {
                        let mut lock = controller_arc.lock().unwrap();
                        if let Some(c) = lock.as_mut() {
                            let _ = c.mouse_click("right");
                        }
                    }
                    catch_count += 1;

                    emit_event(&app_handle, FishingStage::WaitingLoot, catch_count,
                        format!("🐟 提竿! 第 {} 条，等待拾取...", catch_count));
                    sleep_check(1200, &running_flag);

                    if running_flag.load(Ordering::SeqCst) {
                        // 移到游戏窗口左上角，远离水面中央，不遮挡下一轮鱼漂
                        let park_x;
                        let park_y;
                        if let Ok(win) = capture_wow_window() {
                            park_x = win.win_x + 80;
                            park_y = win.win_y + 80;
                        } else {
                            park_x = 80;
                            park_y = 80;
                        }
                        smooth_move_cursor(park_x, park_y);
                        sleep_check(800, &running_flag);
                    }
                } else {
                    sleep_check(1000, &running_flag);
                }
            }

            emit_event(&app_handle, FishingStage::Stopped, catch_count,
                "🛑 已停止".to_string());

            if let Some(win) = app_handle.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
            }
        });
    }
}

fn emit_event(app: &AppHandle, stage: FishingStage, catch_count: u32, message: String) {
    let _ = app.emit("fishing_event", FishingEvent { stage, catch_count, message });
}

fn sleep_check(ms: u64, flag: &AtomicBool) {
    let step = 20;
    for _ in 0..(ms / step) {
        if !flag.load(Ordering::SeqCst) { break; }
        thread::sleep(Duration::from_millis(step));
    }
}

fn rand_range(min: u64, max: u64) -> u64 {
    if min >= max { return min; }
    let now = Instant::now().elapsed().as_nanos();
    min + ((now as u64) % (max - min + 1))
}
