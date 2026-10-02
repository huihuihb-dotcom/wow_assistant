use image::RgbaImage;

/// 鱼漂咬钩检测状态
#[derive(Debug, Clone)]
pub enum BiteStatus {
    /// 等待模板初始化（首帧）
    WaitingTemplate,
    /// 处于基线采样期（帧数不足）
    Calibrating { frames_collected: usize },
    /// 鱼漂正常浮在水面
    Calm { match_score: f32, bobber_y: f32 },
    /// 【触发咬钩】鱼漂消失或急剧下沉
    BiteDetected { reason: BiteReason, bobber_y: f32, baseline_y: f32 },
}

/// 咬钩触发原因
#[derive(Debug, Clone)]
pub enum BiteReason {
    /// 模板匹配分数骤降（鱼漂没入水中，外观消失）
    TemplateVanished { score: f32, threshold: f32 },
    /// 鱼漂 Y 坐标急剧下移
    BobberDropped { delta_y: f32, threshold: f32 },
}

/// 基于模板匹配的鱼漂咬钩检测器
///
/// 原理：
///   1. 首帧：从 YOLO 检测框裁出鱼漂外观作为「模板」
///   2. 后续帧：在稍大的搜索区域内滑动模板，找最佳匹配位置
///   3. 追踪匹配位置的 Y 坐标，建立平静基线
///   4. 触发条件：
///      (A) 匹配分数骤降 → 鱼漂没入水下，外观大变
///      (B) Y 坐标急剧下移 → 鱼漂正在下沉过程中
///
/// 优点：完全颜色无关，适配任何鱼漂皮肤
pub struct BobberBiteDetector {
    /// 鱼漂外观模板（YOLO 首帧截取）
    template: Option<Vec<f32>>,
    template_w: u32,
    template_h: u32,
    /// Y 坐标历史，用于建立基线
    y_history: Vec<f32>,
    /// 匹配分数历史，用于建立基线
    score_history: Vec<f32>,
    /// 稳定基线 Y
    baseline_y: f32,
    /// 正常匹配分数基线
    baseline_score: f32,
    frame_count: usize,
    sensitivity: f32,
}

impl BobberBiteDetector {
    /// `sensitivity` 触发阈值倍数（推荐 1.5~3.0，默认 2.0）
    pub fn new(sensitivity: f32) -> Self {
        Self {
            template: None,
            template_w: 0,
            template_h: 0,
            y_history: Vec::with_capacity(20),
            score_history: Vec::with_capacity(20),
            baseline_y: 0.0,
            baseline_score: 0.0,
            frame_count: 0,
            sensitivity: sensitivity.clamp(1.0, 5.0),
        }
    }

    pub fn reset(&mut self) {
        self.template = None;
        self.template_w = 0;
        self.template_h = 0;
        self.y_history.clear();
        self.score_history.clear();
        self.baseline_y = 0.0;
        self.baseline_score = 0.0;
        self.frame_count = 0;
    }

    /// 设置鱼漂模板（在 YOLO 检测到鱼漂后立即调用）
    ///
    /// `roi` 是以鱼漂为中心的 ROI，`box_w/box_h` 是 YOLO 检测框大小
    pub fn set_template(&mut self, roi: &RgbaImage, box_w: u32, box_h: u32) {
        let (roi_w, roi_h) = roi.dimensions();
        // 取 YOLO 检测框大小作为模板，居中裁剪
        let tw = box_w.min(roi_w);
        let th = box_h.min(roi_h);
        let ox = (roi_w.saturating_sub(tw)) / 2;
        let oy = (roi_h.saturating_sub(th)) / 2;

        let template = extract_gray_patch(roi, ox, oy, tw, th);
        self.template = Some(template);
        self.template_w = tw;
        self.template_h = th;
    }

    /// 处理单帧 ROI（以鱼漂位置为中心的搜索区域）
    pub fn process_frame(&mut self, roi: &RgbaImage) -> BiteStatus {
        // 如果还没有模板，用当前帧中心区域初始化
        if self.template.is_none() {
            let (w, h) = roi.dimensions();
            let tw = (w / 2).max(20);
            let th = (h / 2).max(20);
            self.set_template(roi, tw, th);
            return BiteStatus::WaitingTemplate;
        }

        let (roi_w, roi_h) = roi.dimensions();
        let tw = self.template_w;
        let th = self.template_h;

        if roi_w < tw || roi_h < th {
            return BiteStatus::Calm { match_score: 1.0, bobber_y: self.baseline_y };
        }

        self.frame_count += 1;

        // ── 模板匹配：在 ROI 内滑动搜索 ─────────────────────────────────────
        let (best_x, best_y, best_score) = template_match_ncc(
            roi,
            self.template.as_ref().unwrap(),
            tw, th,
        );
        // NCC 结果在 [-1, 1]，转换为 [0, 1] 相似度
        let similarity = (best_score + 1.0) / 2.0;
        // 最佳匹配位置的中心 Y（相对于 ROI）
        let matched_y = best_y as f32 + th as f32 / 2.0;

        // ── 前 8 帧建立基线 ──────────────────────────────────────────────────
        if self.frame_count <= 8 {
            self.y_history.push(matched_y);
            self.score_history.push(similarity);
            self.baseline_y = mean(&self.y_history);
            self.baseline_score = mean(&self.score_history);
            return BiteStatus::Calibrating { frames_collected: self.frame_count };
        }

        // ── 判决 A：匹配分数骤降（鱼漂消失/没入水中）─────────────────────
        // 平静时相似度通常 > 0.6，咬钩后鱼漂下沉外观大变，分数会骤降
        let score_threshold = (self.baseline_score * 0.5).max(0.3);
        if similarity < score_threshold {
            return BiteStatus::BiteDetected {
                reason: BiteReason::TemplateVanished {
                    score: similarity,
                    threshold: score_threshold,
                },
                bobber_y: matched_y,
                baseline_y: self.baseline_y,
            };
        }

        // ── 判决 B：Y 坐标急剧下移 ──────────────────────────────────────────
        let delta_y = matched_y - self.baseline_y;

        // 自适应噪声：近期 Y 偏移标准差
        let noise = std_dev(&self.y_history, self.baseline_y).max(1.5);
        let y_threshold = self.sensitivity * noise;

        if delta_y >= y_threshold && delta_y >= 3.0 {
            return BiteStatus::BiteDetected {
                reason: BiteReason::BobberDropped {
                    delta_y,
                    threshold: y_threshold,
                },
                bobber_y: matched_y,
                baseline_y: self.baseline_y,
            };
        }

        // ── 平静时更新滑动窗口基线 ───────────────────────────────────────────
        if self.y_history.len() >= 20 { self.y_history.remove(0); }
        if self.score_history.len() >= 20 { self.score_history.remove(0); }
        self.y_history.push(matched_y);
        self.score_history.push(similarity);
        self.baseline_y = mean(&self.y_history);
        self.baseline_score = mean(&self.score_history);

        let _ = (best_x, best_score); // 抑制 unused 警告
        BiteStatus::Calm { match_score: similarity, bobber_y: matched_y }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 辅助函数
// ════════════════════════════════════════════════════════════════════════════

/// 从图像中提取灰度 patch（归一化到 [0, 1]）
fn extract_gray_patch(img: &RgbaImage, ox: u32, oy: u32, w: u32, h: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity((w * h) as usize);
    let (img_w, img_h) = img.dimensions();
    for y in oy..(oy + h).min(img_h) {
        for x in ox..(ox + w).min(img_w) {
            let p = img.get_pixel(x, y);
            let gray = (p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114) / 255.0;
            out.push(gray);
        }
    }
    out
}

/// 归一化互相关（NCC）模板匹配
///
/// 返回 (最佳匹配左上角 x, y, NCC 分数[-1,1])
/// NCC = 1.0 表示完美匹配，0.0 表示不相关，-1.0 表示反相关
fn template_match_ncc(
    img: &RgbaImage,
    template: &[f32],
    tw: u32,
    th: u32,
) -> (u32, u32, f32) {
    let (iw, ih) = img.dimensions();

    // 搜索范围：ROI 内可能放下模板的所有位置
    let search_w = iw.saturating_sub(tw) + 1;
    let search_h = ih.saturating_sub(th) + 1;

    if search_w == 0 || search_h == 0 {
        return (0, 0, 0.0);
    }

    // 计算模板均值和标准差（用于 NCC）
    let t_mean = template.iter().sum::<f32>() / template.len() as f32;
    let t_std: f32 = {
        let var = template.iter().map(|v| (v - t_mean).powi(2)).sum::<f32>() / template.len() as f32;
        var.sqrt().max(1e-6)
    };

    let mut best_score = f32::NEG_INFINITY;
    let mut best_x = 0u32;
    let mut best_y = 0u32;

    // 滑动搜索（步长 2，加速搜索，精度损失可接受）
    let step = 2u32;
    let mut y = 0u32;
    while y < search_h {
        let mut x = 0u32;
        while x < search_w {
            // 提取当前窗口的灰度 patch
            let patch = extract_gray_patch(img, x, y, tw, th);
            if patch.len() < template.len() {
                x += step;
                continue;
            }
            let p_mean = patch.iter().sum::<f32>() / patch.len() as f32;
            let p_std: f32 = {
                let var = patch.iter().map(|v| (v - p_mean).powi(2)).sum::<f32>() / patch.len() as f32;
                var.sqrt().max(1e-6)
            };

            // NCC = Σ[(t_i - t_mean)(p_i - p_mean)] / (n * t_std * p_std)
            let ncc: f32 = template.iter().zip(patch.iter())
                .map(|(t, p)| (t - t_mean) * (p - p_mean))
                .sum::<f32>()
                / (template.len() as f32 * t_std * p_std);

            if ncc > best_score {
                best_score = ncc;
                best_x = x;
                best_y = y;
            }
            x += step;
        }
        y += step;
    }

    (best_x, best_y, best_score)
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}

fn std_dev(v: &[f32], m: f32) -> f32 {
    if v.len() < 2 { return 0.0; }
    let var = v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32;
    var.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn solid(w: u32, h: u32, color: [u8; 3]) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for p in img.pixels_mut() { *p = Rgba([color[0], color[1], color[2], 255]); }
        img
    }

    fn draw_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
        let (iw, ih) = img.dimensions();
        for dy in 0..h {
            for dx in 0..w {
                let px = (x + dx).min(iw - 1);
                let py = (y + dy).min(ih - 1);
                img.put_pixel(px, py, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }

    #[test]
    fn test_ncc_match() {
        // 蓝灰色背景 + 白色方块（模拟任意颜色的鱼漂）
        let mut bg = solid(90, 90, [80, 100, 130]);
        draw_rect(&mut bg, 30, 20, 20, 20, [240, 230, 200]); // 米白色鱼漂

        let template = extract_gray_patch(&bg, 30, 20, 20, 20);
        let (bx, by, score) = template_match_ncc(&bg, &template, 20, 20);
        println!("匹配位置: ({}, {}), NCC 分数: {:.3}", bx, by, score);
        assert!(score > 0.9, "完美匹配 NCC 应 > 0.9, 实际: {:.3}", score);
        assert_eq!(by, 20, "匹配 Y 应为 20");
    }

    #[test]
    fn test_bite_template_vanish() {
        let mut det = BobberBiteDetector::new(2.0);
        let mut frame = solid(90, 90, [80, 100, 130]);
        draw_rect(&mut frame, 30, 20, 20, 20, [240, 230, 200]);

        // 首帧初始化模板
        det.process_frame(&frame);
        // 校准
        for _ in 0..9 { det.process_frame(&frame); }
        // 平静
        assert!(matches!(det.process_frame(&frame), BiteStatus::Calm { .. }));
        // 咬钩：鱼漂消失（全蓝背景）
        let empty = solid(90, 90, [80, 100, 130]);
        let status = det.process_frame(&empty);
        println!("消失检测: {:?}", status);
        assert!(matches!(status, BiteStatus::BiteDetected { .. }));
    }

    #[test]
    fn test_bite_drop() {
        let mut det = BobberBiteDetector::new(1.5);
        let mut normal = solid(90, 90, [80, 100, 130]);
        draw_rect(&mut normal, 30, 10, 20, 20, [240, 230, 200]); // 漂在上方

        det.process_frame(&normal);
        for _ in 0..9 { det.process_frame(&normal); }
        assert!(matches!(det.process_frame(&normal), BiteStatus::Calm { .. }));

        // 鱼漂下沉到下方
        let mut drop = solid(90, 90, [80, 100, 130]);
        draw_rect(&mut drop, 30, 60, 20, 20, [240, 230, 200]);
        let status = det.process_frame(&drop);
        println!("下沉检测: {:?}", status);
        assert!(matches!(status, BiteStatus::BiteDetected { .. }));
    }
}
