use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::rp2040::Rp2040Controller;
use crate::vision::{
    capture_primary_screen, capture_wow_window, crop_roi, get_cursor_pos, get_water_center_wide,
    smooth_move_cursor, DetectionBox, VisionDetector, YoloDetector,
};
use crate::window::{activate_wow_window, is_wow_foreground, post_key_to_wow};

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

                // FishingFun strikeValue=7px（直接屏幕像素）
                // 我们同样使用屏幕坐标，基准 6px，按 sensitivity 缩放
                let sens_scale = (config.sensitivity / 3.0).clamp(0.5, 1.8);
                let strike_px_base = (6.0 * sens_scale).clamp(3.0, 14.0);

                let mut prev_bobber_y: Option<f32> = None;
                let mut bite_confirm_counter = 0u32;
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

                    let track_result = capture_primary_screen().and_then(|screen| {
                        let (sw, sh) = screen.dimensions();
                        let track_w = 640u32.min(sw);
                        let track_h = 640u32.min(sh);
                        let safe_x = (cur_target_x - (track_w / 2) as i32).clamp(0, (sw - track_w) as i32) as u32;
                        let safe_y = (cur_target_y - (track_h / 2) as i32).clamp(0, (sh - track_h) as i32) as u32;
                        crop_roi(&screen, safe_x, safe_y, track_w, track_h).map(|img| (img, safe_x, safe_y))
                    });

                    match track_result {
                        Ok((roi, safe_x, safe_y)) => {
                            let local_target_x = cur_target_x - safe_x as i32;
                            let local_target_y = cur_target_y - safe_y as i32;

                            match detector.detect_candidates(&roi, 0.012) {
                                Ok(candidates) => {
                                    let mut in_range: Vec<&DetectionBox> = candidates.iter().filter(|b| {
                                        let (cx, cy) = b.center();
                                        let dx = cx - local_target_x;
                                        let dy = cy - local_target_y;
                                        (dx * dx + dy * dy) <= 100 * 100
                                    }).collect();
                                    in_range.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

                                    if let Some(best) = in_range.first().copied() {
                                        miss_streak = 0;
                                        let (local_cx, local_cy) = best.center();
                                        let screen_x = safe_x as i32 + local_cx;
                                        let screen_y = safe_y as i32 + local_cy;
                                        let bobber_x = screen_x as f32;
                                        let bobber_y = screen_y as f32;
                                        let conf = best.confidence;

                                        cur_target_x = screen_x;
                                        cur_target_y = screen_y;
                                        frame_idx += 1;

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

                                        // 平静水面自动收紧阈值（noise<1px → 4px，否则用基准 6px）
                                        let strike_px = if dynamic_noise < 1.0 {
                                            (strike_px_base * 0.65).max(3.0)
                                        } else {
                                            strike_px_base
                                        };

                                        let delta_y = bobber_y - median_y;
                                        let instant_speed_y = prev_bobber_y.map(|py| bobber_y - py).unwrap_or(0.0);
                                        prev_bobber_y = Some(bobber_y);

                                        // 每 5 帧常规日志；咬钩确认中每帧都打
                                        if frame_idx % 5 == 0 || bite_confirm_counter > 0 {
                                            emit_event(&app_handle,
                                                FishingStage::WatchingBite { x: screen_x, y: screen_y },
                                                catch_count,
                                                format!("📍 #{} ΔY:{:+.1}/{:.0} spd:{:+.1} n:{:.1} {:.0}%{}",
                                                    frame_idx, delta_y, strike_px,
                                                    instant_speed_y, dynamic_noise, conf * 100.0,
                                                    if bite_confirm_counter > 0 { format!(" ★{}", bite_confirm_counter) } else { String::new() }));
                                        }

                                        let is_sink = delta_y >= strike_px;
                                        let is_step_jerk = instant_speed_y >= (strike_px * 0.4).max(2.5)
                                            && delta_y >= strike_px * 0.4;

                                        if (is_sink || is_step_jerk) && all_y.len() >= 8 {
                                            bite_confirm_counter += 1;
                                            let is_huge = delta_y >= strike_px + 3.0 || instant_speed_y >= strike_px * 0.5;
                                            if is_huge || bite_confirm_counter >= 2 {
                                                let reason = if is_step_jerk {
                                                    format!("急坠 spd:{:+.1} ΔY:{:+.1}", instant_speed_y, delta_y)
                                                } else {
                                                    format!("下顿 ΔY:{:+.1} >{:.0}px", delta_y, strike_px)
                                                };
                                                emit_event(&app_handle,
                                                    FishingStage::BiteTriggered { delta_y, miss_count: 0 },
                                                    catch_count,
                                                    format!("💥 上钩! ({},{}) {}", screen_x, screen_y, reason));
                                                hooked = true;
                                                break;
                                            }
                                        } else {
                                            bite_confirm_counter = 0;
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
                        let (cur_x, cur_y) = get_cursor_pos();
                        smooth_move_cursor((cur_x - 280).max(80), cur_y);
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
