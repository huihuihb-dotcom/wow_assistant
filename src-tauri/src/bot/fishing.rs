use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::rp2040::Rp2040Controller;
use crate::vision::{
    capture_primary_screen, crop_roi, get_cursor_pos, smooth_move_cursor, DetectionBox,
    VisionDetector, YoloDetector,
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
    /// 启动自动钓鱼循环线程
    pub fn start(
        app_handle: AppHandle,
        config: FishingConfig,
        running_flag: Arc<AtomicBool>,
        controller_arc: Arc<std::sync::Mutex<Option<Rp2040Controller>>>,
    ) {
        let detector = Arc::new(YoloDetector::new("models/bobber.onnx", 0.04));

        thread::spawn(move || {
            let mut catch_count = 0u32;

            // 1. 尝试将《魔兽世界》窗口置于前台，若无法置顶则透明提示用户手动激活
            let wow_ready = activate_wow_window().unwrap_or(false);

            if wow_ready {
                emit_event(
                    &app_handle,
                    FishingStage::Idle,
                    catch_count,
                    "🎮 已自动将《魔兽世界》切换至系统前台！3 秒倒计时准备...".to_string(),
                );
            } else {
                emit_event(
                    &app_handle,
                    FishingStage::Idle,
                    catch_count,
                    "⚠️ 未能自动获取魔兽前台焦点，请立即手动点击激活《魔兽世界》窗口！3 秒后将直接抛竿...".to_string(),
                );
            }

            // 2. 给予充足的 3 秒缓冲倒计时，确保用户或 DirectX 彻底就绪
            for sec in (1..=3).rev() {
                if !running_flag.load(Ordering::SeqCst) {
                    return;
                }
                let status_text = if is_wow_foreground() {
                    "前台已就绪"
                } else {
                    "等待焦点中"
                };
                let action_hint = if sec == 1 {
                    format!("即将按下按键 [{}] 抛竿...", config.cast_key)
                } else {
                    "准备就绪...".to_string()
                };
                emit_event(
                    &app_handle,
                    FishingStage::Idle,
                    catch_count,
                    format!("⏱️ 倒计时 {} 秒 [{}]：{}", sec, status_text, action_hint),
                );
                sleep_check(1000, &running_flag);
            }

            // 正式启动钓鱼主状态循环
            while running_flag.load(Ordering::SeqCst) {
                // 校验魔兽窗口状态，若未在前台如实提示
                let is_front = is_wow_foreground();
                emit_event(
                    &app_handle,
                    FishingStage::Casting,
                    catch_count,
                    if is_front {
                        "🎮 游戏窗口已在前台".to_string()
                    } else {
                        "⚠️ 提示: 魔兽窗口当前可能未处于系统最前台，若按键无效请手动点击游戏窗口".to_string()
                    },
                );

                // 尝试向游戏直投按键（Windows 消息管道）
                let post_ok = post_key_to_wow(&config.cast_key);

                // 同步通过 RP2040 真实物理外设按下按键（双保险）
                {
                    let mut lock = controller_arc.lock().unwrap();
                    if let Some(c) = lock.as_mut() {
                        let _ = c.key_down(&config.cast_key);
                        thread::sleep(Duration::from_millis(120)); // 120ms 硬件级按键保持时长
                        let _ = c.key_up(&config.cast_key);
                    }
                }

                emit_event(
                    &app_handle,
                    FishingStage::Casting,
                    catch_count,
                    format!(
                        "🎣 执行双通道抛竿 [{}] (PostMessage 直投: {}, RP2040 硬件: 已按下)",
                        config.cast_key,
                        if post_ok { "成功" } else { "未获取窗口" }
                    ),
                );

                // 3. 抛竿后快速准备搜索 (等待 1.2 秒浮漂充分入水并露出羽毛)
                emit_event(
                    &app_handle,
                    FishingStage::WaitingBobber,
                    catch_count,
                    "🌊 抛竿完毕，浮漂入水中，等待水花平息后启动 GPU 神经网络搜寻...".to_string(),
                );
                sleep_check(1200, &running_flag);
                if !running_flag.load(Ordering::SeqCst) {
                    break;
                }

                // 4. 视觉窗口渐进式高频扫描（最多扫描 12 次，每次间隔约 120ms，全程日志透明）
                let search_start = Instant::now();
                let mut detected_screen_x = 0;
                let mut detected_screen_y = 0;
                let mut found_target = false;

                for attempt in 1..=12 {
                    if !running_flag.load(Ordering::SeqCst) {
                        break;
                    }

                    let t_start = Instant::now();
                    let win_cap = match crate::vision::capture_wow_window() {
                        Ok(c) => c,
                        Err(e) => {
                            emit_event(
                                &app_handle,
                                FishingStage::SearchingBobber,
                                catch_count,
                                format!("⚠️ [扫描 #{}] 截屏失败: {}", attempt, e),
                            );
                            sleep_check(100, &running_flag);
                            continue;
                        }
                    };
                    let t_cap_ms = t_start.elapsed().as_millis();

                    // 优先在水面黄金区检索 (10%~88% 真实落水带)
                    let (water_roi, roi_off_x, roi_off_y) = crate::vision::get_fishing_water_roi(&win_cap);

                    let t_infer_start = Instant::now();
                    let mut max_conf = 0.0f32;
                    let roi_detections = detector.detect(&water_roi).unwrap_or_default();
                    if !roi_detections.is_empty() {
                        max_conf = roi_detections[0].confidence;
                    }
                    let t_infer_ms = t_infer_start.elapsed().as_millis();
                    let total_ms = t_start.elapsed().as_millis();

                    // 命中水面鱼漂 (YOLO 神经网络置信度 >= 0.04)
                    if !roi_detections.is_empty() && max_conf >= 0.04 {
                        let target_box = &roi_detections[0];
                        let (rx, ry) = target_box.center();
                        let win_target_x = roi_off_x as i32 + rx;
                        let win_target_y = roi_off_y as i32 + ry;
                        detected_screen_x = win_cap.win_x + win_target_x;
                        detected_screen_y = win_cap.win_y + win_target_y;
                        found_target = true;

                        emit_event(
                            &app_handle,
                            FishingStage::SearchingBobber,
                            catch_count,
                            format!(
                                "🎯 [第 {} 次水面扫描命中！] 置信度: {:.1}% | 窗口屏幕原点: ({}, {}) | 物理目标: ({}, {}) | 本轮耗时: {}ms (截屏: {}ms, GPU推理: {}ms)",
                                attempt, max_conf * 100.0, win_cap.win_x, win_cap.win_y, detected_screen_x, detected_screen_y, total_ms, t_cap_ms, t_infer_ms
                            ),
                        );
                        break;
                    }

                    // 如果到了第 6 次（约 1.5 秒后，水花已完全平息）水面仍未命中，尝试一次全屏 Letterbox 兜底
                    if attempt >= 6 && attempt % 3 == 0 {
                        emit_event(
                            &app_handle,
                            FishingStage::SearchingBobber,
                            catch_count,
                            format!("🔍 [第 {} 次扫描] 水面检出 {:.1}%，尝试全屏大范围扫描兜底...", attempt, max_conf * 100.0),
                        );

                        if let Ok(full_detections) = detector.detect(&win_cap.image) {
                            if !full_detections.is_empty() && full_detections[0].confidence >= 0.04 {
                                let target_box = &full_detections[0];
                                let (wx, wy) = target_box.center();
                                detected_screen_x = win_cap.win_x + wx;
                                detected_screen_y = win_cap.win_y + wy;
                                found_target = true;

                                emit_event(
                                    &app_handle,
                                    FishingStage::SearchingBobber,
                                    catch_count,
                                    format!(
                                        "🎯 [全屏大范围扫描命中！] 置信度: {:.1}% | 窗口屏幕原点: ({}, {}) | 物理目标: ({}, {}) | 耗时: {:.2}s",
                                        target_box.confidence * 100.0, win_cap.win_x, win_cap.win_y, detected_screen_x, detected_screen_y, search_start.elapsed().as_secs_f32()
                                    ),
                                );
                                break;
                            }
                        }
                    } else {
                        emit_event(
                            &app_handle,
                            FishingStage::SearchingBobber,
                            catch_count,
                            format!(
                                "🔍 [第 {} 次扫描] 水面搜寻中... 当前最高置信: {:.1}% (水花平息中) | 本轮耗时: {}ms (截屏: {}ms, GPU推理: {}ms)",
                                attempt, max_conf * 100.0, total_ms, t_cap_ms, t_infer_ms
                            ),
                        );
                    }

                    sleep_check(120, &running_flag);
                }

                // 如果多轮扫描均未找到鱼漂，稍作等待重新抛竿
                if !found_target {
                    emit_event(
                        &app_handle,
                        FishingStage::TimeoutRecast,
                        catch_count,
                        "⚠️ 连续 12 次扫描未捕获到显著鱼漂目标，准备重新抛竿...".to_string(),
                    );
                    sleep_check(2000, &running_flag);
                    continue;
                }

                let target_x = detected_screen_x;
                let target_y = detected_screen_y;

                if !running_flag.load(Ordering::SeqCst) {
                    break;
                }

                // 5. 鱼漂已锁定，立即启动零抽帧全速动态振幅监测（鼠标驻留防遮挡视野）
                emit_event(
                    &app_handle,
                    FishingStage::WatchingBite { x: target_x, y: target_y },
                    catch_count,
                    format!("🎯 鱼漂精准锁定: ({}, {})！鼠标驻留防遮挡，立即启动全帧率动态振幅监测...", target_x, target_y),
                );

                let start_time = Instant::now();
                let mut hooked = false;

                // 25 帧滑动窗口（在 ~45 FPS 下对应约 0.5 秒时间跨度）
                const SLIDING_WINDOW: usize = 25;
                let mut history_y: VecDeque<f32> = VecDeque::with_capacity(SLIDING_WINDOW);
                let mut history_x: VecDeque<f32> = VecDeque::with_capacity(SLIDING_WINDOW);

                let mut prev_bobber_y: Option<f32> = None;
                let mut bite_confirm_counter = 0u32; // 连续确认计数器，消除单帧噪点
                let mut miss_streak = 0u32;
                let mut frame_idx = 0u32;
                let mut cur_target_x = target_x;
                let mut cur_target_y = target_y;

                while running_flag.load(Ordering::SeqCst) {
                    if start_time.elapsed().as_secs() >= config.timeout_secs {
                        emit_event(
                            &app_handle,
                            FishingStage::TimeoutRecast,
                            catch_count,
                            "⏱️ 达到防脱钩超时阈值，准备重新起竿抛投...".to_string(),
                        );
                        break;
                    }

                    // 以当前目标点为中心裁出 640x640 区域（1:1 直通 YOLOv8，纯内存推理）
                    let track_result = capture_primary_screen().and_then(|screen| {
                        let (sw, sh) = screen.dimensions();
                        let track_w = 640u32.min(sw);
                        let track_h = 640u32.min(sh);
                        let half_w = (track_w / 2) as i32;
                        let half_h = (track_h / 2) as i32;
                        let safe_x = (cur_target_x - half_w).clamp(0, (sw - track_w) as i32) as u32;
                        let safe_y = (cur_target_y - half_h).clamp(0, (sh - track_h) as i32) as u32;
                        crop_roi(&screen, safe_x, safe_y, track_w, track_h).map(|img| (img, safe_x, safe_y))
                    });

                    match track_result {
                        Ok((roi, safe_x, safe_y)) => {
                            let local_target_x = cur_target_x - safe_x as i32;
                            let local_target_y = cur_target_y - safe_y as i32;

                            // 极低门限 0.012 提取视野内候选目标
                            match detector.detect_candidates(&roi, 0.012) {
                                Ok(candidates) => {
                                    // 空间滤波与近邻匹配：寻找距离当前鱼漂中心 100 像素以内的候选
                                    let mut in_range: Vec<&DetectionBox> = candidates.iter().filter(|b| {
                                        let (cx, cy) = b.center();
                                        let dx = cx - local_target_x;
                                        let dy = cy - local_target_y;
                                        (dx * dx + dy * dy) <= 100 * 100
                                    }).collect();

                                    // 优先按置信度排序
                                    in_range.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
                                    let local_target = in_range.first().copied();

                                    if let Some(best) = local_target {
                                        miss_streak = 0;
                                        let (local_cx, local_cy) = best.center();
                                        let screen_x = safe_x as i32 + local_cx;
                                        let screen_y = safe_y as i32 + local_cy;
                                        let bobber_x = screen_x as f32;
                                        let bobber_y = screen_y as f32;
                                        let conf = best.confidence;

                                        // 跟手更新屏幕坐标中心
                                        cur_target_x = screen_x;
                                        cur_target_y = screen_y;

                                        frame_idx += 1;

                                        // 维护滑动队列
                                        if history_y.len() >= SLIDING_WINDOW {
                                            history_y.pop_front();
                                            history_x.pop_front();
                                        }
                                        history_y.push_back(bobber_y);
                                        history_x.push_back(bobber_x);

                                        // 1. 滑动中位数基准线 (Robust Baseline)
                                        let mut sorted_y: Vec<f32> = history_y.iter().copied().collect();
                                        sorted_y.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                        let median_y = sorted_y[sorted_y.len() / 2];

                                        let mut sorted_x: Vec<f32> = history_x.iter().copied().collect();
                                        sorted_x.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                                        let median_x = sorted_x[sorted_x.len() / 2];

                                        // 2. 动态本底振幅 (Dynamic Noise Floor)
                                        let dynamic_noise: f32 = history_y.iter()
                                            .map(|&y| (y - median_y).abs())
                                            .sum::<f32>() / (history_y.len() as f32).max(1.0);

                                        // 3. 动态自适应下沉阈值 (平静水面更灵敏，波动水面防误触)
                                        let sens_scale = (config.sensitivity / 3.0).clamp(0.7, 1.5);
                                        let dyn_sink_threshold = ((dynamic_noise * 2.2 + 2.0) * sens_scale).clamp(2.8, 6.5);

                                        let delta_y = bobber_y - median_y; // 屏幕向下为正，>0 表示下沉
                                        let delta_x = bobber_x - median_x;
                                        let displacement = (delta_x * delta_x + delta_y * delta_y).sqrt();

                                        // 4. 瞬时垂直阶跃速度 (当前帧与上一帧的差值)
                                        let instant_speed_y = if let Some(py) = prev_bobber_y {
                                            bobber_y - py
                                        } else {
                                            0.0
                                        };
                                        prev_bobber_y = Some(bobber_y);

                                        // 控制台输出（每 5 帧打印一次，避免日志刷屏；咬钩时必报）
                                        if frame_idx % 5 == 0 {
                                            emit_event(&app_handle,
                                                FishingStage::WatchingBite { x: screen_x, y: screen_y },
                                                catch_count,
                                                format!("🎯 [动态追踪 #{}] 坐标: ({}, {}) | 垂直ΔY: {:+.1}px (门限: {:.1}px) | 瞬时跳水: {:+.1}px | 本底振幅: {:.1}px | 置信: {:.1}%",
                                                    frame_idx, screen_x, screen_y, delta_y, dyn_sink_threshold, instant_speed_y, dynamic_noise, conf * 100.0),
                                            );
                                        }

                                        // 5. 核心判定条件（必须已收集至少 8 帧建立起可靠水面基线）
                                        let is_sink = delta_y >= dyn_sink_threshold;
                                        let is_step_jerk = instant_speed_y >= 2.5 && delta_y >= 2.0;
                                        let is_water_splash = displacement >= (dyn_sink_threshold * 1.5).max(4.5);

                                        if (is_sink || is_step_jerk || is_water_splash) && history_y.len() >= 8 {
                                            bite_confirm_counter += 1;
                                            // 超强剧烈下顿单帧直接提竿，或连续 2 帧稳定超标触发起竿（40ms 极速闭环且免疫单帧噪点）
                                            let is_huge_strike = delta_y >= dyn_sink_threshold + 1.5 || instant_speed_y >= 3.5;
                                            if is_huge_strike || bite_confirm_counter >= 2 {
                                                let reason = if is_step_jerk {
                                                    format!("瞬态急坠速度: {:+.1}px/帧 (ΔY: {:+.1}px >= {:.1}px)", instant_speed_y, delta_y, dyn_sink_threshold)
                                                } else if is_sink {
                                                    format!("动态垂直下顿: ΔY {:+.1}px >= 动态门限 {:.1}px (水面振幅 {:.1}px)", delta_y, dyn_sink_threshold, dynamic_noise)
                                                } else {
                                                    format!("水花翻滚剧烈突变: 综合震幅 {:.1}px >= {:.1}px", displacement, dyn_sink_threshold * 1.5)
                                                };

                                                emit_event(
                                                    &app_handle,
                                                    FishingStage::BiteTriggered { delta_y, miss_count: 0 },
                                                    catch_count,
                                                    format!("💥 【咬钩确认！】坐标: ({}, {}) | 动量: ({})",
                                                        screen_x, screen_y, reason),
                                                );
                                                hooked = true;
                                                break;
                                            }
                                        } else {
                                            bite_confirm_counter = 0;
                                        }
                                    } else {
                                        miss_streak += 1;
                                        bite_confirm_counter = 0;
                                        // 绝不将偶尔的丢帧误判为咬钩！仅记录水雾遮挡
                                        if miss_streak % 15 == 0 {
                                            emit_event(&app_handle,
                                                FishingStage::WatchingBite { x: cur_target_x, y: cur_target_y },
                                                catch_count,
                                                format!("⚠️ 浮漂暂被水花掩盖 (连续 {} 帧无信号) | 持续全速锁定视野中心 ({}, {})...",
                                                    miss_streak, cur_target_x, cur_target_y),
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    emit_event(&app_handle,
                                        FishingStage::WatchingBite { x: cur_target_x, y: cur_target_y },
                                        catch_count,
                                        format!("⚠️ 视觉推理帧异常: {}", e),
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            emit_event(&app_handle,
                                FishingStage::WatchingBite { x: cur_target_x, y: cur_target_y },
                                catch_count,
                                format!("⚠️ 屏幕采集异常: {}", e),
                            );
                        }
                    }

                    // 零抽帧全速推进（仅休眠 3ms 让出 CPU 片刻，DirectML GPU 以 40~50 FPS 实时追踪）
                    thread::sleep(Duration::from_millis(3));
                }

                let target_x = cur_target_x;
                let target_y = cur_target_y;

                // 刹车拦截点：如果用户在咬钩等待期间停止，绝不收杆
                if !running_flag.load(Ordering::SeqCst) {
                    break;
                }

                // 7. 收杆提钩 (Hooking)
                if hooked {
                    // 拟人生理反应与让鱼充分咬实延时 (随机 600ms ~ 1100ms，平均约 0.85 秒)
                    let human_reaction_ms = rand_range(600, 1100);
                    let (cur_x, cur_y) = get_cursor_pos();
                    emit_event(
                        &app_handle,
                        FishingStage::Hooking,
                        catch_count,
                        format!("⚡ 咬钩确认！拟人反应缓冲 {}ms (让鱼咬实)... 当前光标: ({}, {}) → 目标鱼漂: ({}, {})", human_reaction_ms, cur_x, cur_y, target_x, target_y),
                    );
                    sleep_check(human_reaction_ms, &running_flag);

                    if !running_flag.load(Ordering::SeqCst) {
                        break;
                    }

                    // 1. 人体工学平滑缓动滑至目标鱼漂 (50ms Ease-Out，100% 绝对到位，误差为 0，永不飞出)
                    smooth_move_cursor(target_x, target_y);
                    thread::sleep(Duration::from_millis(60));

                    let (final_x, final_y) = get_cursor_pos();
                    emit_event(
                        &app_handle,
                        FishingStage::Hooking,
                        catch_count,
                        format!("🎯 光标到位: 实际落在 ({}, {}) (与目标偏差: ΔX={:+}px, ΔY={:+}px)，触发 RP2040 硬件右键提竿！",
                            final_x, final_y, target_x - final_x, target_y - final_y),
                    );

                    // 2. RP2040 纯硬件物理右键点击提竿
                    {
                        let mut lock = controller_arc.lock().unwrap();
                        if let Some(c) = lock.as_mut() {
                            let _ = c.mouse_click("right");
                        }
                    }

                    catch_count += 1;

                    // 8. 提竿后等待拾取完成，随后将鼠标平滑向左避让，防止遮挡下一轮鱼漂视野
                    emit_event(
                        &app_handle,
                        FishingStage::WaitingLoot,
                        catch_count,
                        format!("🎣 提竿成功 (已累计收获 {} 条)，等待 1.2 秒拾取鱼获...", catch_count),
                    );
                    sleep_check(1200, &running_flag);

                    if running_flag.load(Ordering::SeqCst) {
                        let (cur_x, cur_y) = get_cursor_pos();
                        // 往左侧平移 280 像素，避免遮挡水面鱼漂，保底不小于 80 像素
                        let safe_left_x = (cur_x - 280).max(80);
                        let safe_left_y = cur_y;
                        smooth_move_cursor(safe_left_x, safe_left_y);

                        emit_event(
                            &app_handle,
                            FishingStage::WaitingLoot,
                            catch_count,
                            format!("📦 拾取完毕！鼠标已平滑向左移开至 ({}, {}) 避让防遮挡，等待水面平息后开启下一轮...", safe_left_x, safe_left_y),
                        );
                        sleep_check(800, &running_flag);
                    }
                } else {
                    // 若超时未咬钩，短暂平息 1 秒后准备重新抛竿
                    sleep_check(1000, &running_flag);
                }
            }

            emit_event(
                &app_handle,
                FishingStage::Stopped,
                catch_count,
                "🛑 自动钓鱼已安全停止。".to_string(),
            );

            // 钓鱼停止后恢复 Tauri 窗口
            if let Some(win) = app_handle.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
            }
        });
    }
}

fn emit_event(app: &AppHandle, stage: FishingStage, catch_count: u32, message: String) {
    let evt = FishingEvent {
        stage,
        catch_count,
        message,
    };
    let _ = app.emit("fishing_event", evt);
}

fn sleep_check(ms: u64, flag: &AtomicBool) {
    let step = 20;
    let iterations = ms / step;
    for _ in 0..iterations {
        if !flag.load(Ordering::SeqCst) {
            break;
        }
        thread::sleep(Duration::from_millis(step));
    }
}

fn rand_range(min: u64, max: u64) -> u64 {
    if min >= max {
        return min;
    }
    let now = Instant::now().elapsed().as_nanos();
    min + ((now as u64) % (max - min + 1))
}
