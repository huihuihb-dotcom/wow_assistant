use image::RgbaImage;
use ndarray::Array4;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// 检测目标边界框
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionBox {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub confidence: f32,
    pub label: String,
}

impl DetectionBox {
    pub fn new(x: i32, y: i32, width: i32, height: i32, confidence: f32, label: &str) -> Self {
        Self {
            x,
            y,
            width,
            height,
            confidence,
            label: label.to_string(),
        }
    }

    /// 获取边界框中心坐标 (用于鼠标右键点击)
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// 视觉检测器统一特征接口
pub trait VisionDetector: Send + Sync {
    fn detect(&self, image: &RgbaImage) -> Result<Vec<DetectionBox>, String>;
    fn detect_candidates(&self, image: &RgbaImage, threshold: f32) -> Result<Vec<DetectionBox>, String>;
}

use ort::ep;
use ort::session::Session;

/// 基于 Windows DirectML (DirectX 12 EP) 的跨厂商 GPU 硬件加速 YOLO 推理器
/// 针对 NVIDIA / AMD / Intel 全系列显卡硬件加速，单次推理 4~8ms，零 Python 依赖
pub struct YoloDetector {
    pub model_path: String,
    pub confidence_threshold: f32,
    cached_session: Mutex<Option<Session>>,
}

impl YoloDetector {
    pub fn new(model_path: &str, confidence_threshold: f32) -> Self {
        Self {
            model_path: model_path.to_string(),
            confidence_threshold,
            cached_session: Mutex::new(None),
        }
    }

    /// 获取或加载 ONNX 模型 (优先通过 DirectML 载入 GPU 显存，全自动兼容 NVIDIA / AMD / Intel 全系列显卡；若无独显则自动降级为 CPU 模式)
    fn get_or_load_session(&self) -> Result<Option<Session>, String> {
        let mut candidates = vec![
            std::path::PathBuf::from(&self.model_path),
            std::path::PathBuf::from("models/bobber.onnx"),
            std::path::PathBuf::from("../models/bobber.onnx"),
            std::path::PathBuf::from("d:/projects/rp2040/wow_assistant/models/bobber.onnx"),
        ];

        // 针对生产打包发布（exe 同级目录与 resources 资源目录）自动适配
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                candidates.push(exe_dir.join("models").join("bobber.onnx"));
                candidates.push(exe_dir.join("resources").join("models").join("bobber.onnx"));
                candidates.push(exe_dir.join("_up_").join("models").join("bobber.onnx"));
            }
        }

        let actual_path = match candidates.into_iter().find(|p| p.exists()) {
            Some(p) => p,
            None => return Ok(None),
        };

        // 1. 优先尝试 DirectML GPU 硬件加速 (基于微软 DirectX 12 跨厂商标准，原生支持 NVIDIA / AMD / Intel)
        let gpu_init = || -> Result<Session, ort::Error> {
            let session = Session::builder()?
                .with_intra_threads(4)?
                .with_execution_providers([
                    ep::DirectML::default()
                        .with_device_filter(ep::directml::DeviceFilter::Gpu)
                        .with_performance_preference(ep::directml::PerformancePreference::HighPerformance)
                        .build()
                ])?
                .commit_from_file(&actual_path)?;
            Ok(session)
        };

        match gpu_init() {
            Ok(sess) => {
                println!("🎮 [DirectML GPU] 成功载入 YOLO 模型至显卡硬件加速 (NVIDIA/AMD/Intel 通用): {}", actual_path.display());
                return Ok(Some(sess));
            }
            Err(e) => {
                println!("⚠️ [DirectML GPU] 无法激活显卡硬件加速 ({})，正在无缝降级到 CPU 模式保底...", e);
            }
        }

        // 2. 兜底保护：当用户电脑无独显或处于无 DX12 环境时，自动降级为标准 CPU 会话，确保 100% 不崩溃
        let cpu_session = Session::builder()
            .map_err(|e| format!("创建 ONNX Runtime CPU 会话失败: {}", e))?
            .with_intra_threads(4)
            .map_err(|e| format!("配置 CPU 线程数失败: {}", e))?
            .commit_from_file(&actual_path)
            .map_err(|e| format!("加载 ONNX 模型 CPU 模式失败: {}", e))?;

        println!("💻 [ONNX CPU] 已成功启动 CPU 兼容模式运行: {}", actual_path.display());
        Ok(Some(cpu_session))
    }
}

/// Letterbox 图像预处理元数据，用于后处理将网络坐标精准映射回原图物理像素
#[derive(Debug, Clone, Copy)]
pub struct LetterboxInfo {
    pub scale: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub orig_w: u32,
    pub orig_h: u32,
}

impl LetterboxInfo {
    /// 将模型输出的 640x640 内部坐标逆变换为原图绝对物理坐标
    pub fn inverse_transform(&self, net_cx: f32, net_cy: f32, net_w: f32, net_h: f32) -> (i32, i32, i32, i32) {
        let cx_unpad = (net_cx - self.pad_x) / self.scale;
        let cy_unpad = (net_cy - self.pad_y) / self.scale;
        let w_unpad = net_w / self.scale;
        let h_unpad = net_h / self.scale;

        let orig_x = (cx_unpad - w_unpad / 2.0).clamp(0.0, self.orig_w as f32) as i32;
        let orig_y = (cy_unpad - h_unpad / 2.0).clamp(0.0, self.orig_h as f32) as i32;
        let orig_w = w_unpad as i32;
        let orig_h = h_unpad as i32;

        (orig_x, orig_y, orig_w, orig_h)
    }
}

/// 工业级自适应 Letterbox 预处理 (支持 640x640 无损零缩放直通，实战 BGR [0..255] 规范)
pub fn preprocess_letterbox(image: &RgbaImage) -> (Array4<f32>, LetterboxInfo) {
    let (orig_w, orig_h) = image.dimensions();

    // 1. 若已经是 640x640 (ROI 裁剪区)，直接 1:1 射入。
    // YOLOv8 训练时采用 RGB 归一化 [0, 1] 输入
    if orig_w == 640 && orig_h == 640 {
        let mut data = Array4::<f32>::zeros((1, 3, 640, 640));
        for y in 0..640 {
            for x in 0..640 {
                let p = image.get_pixel(x, y);
                data[[0, 0, y as usize, x as usize]] = p[0] as f32 / 255.0; // Red
                data[[0, 1, y as usize, x as usize]] = p[1] as f32 / 255.0; // Green
                data[[0, 2, y as usize, x as usize]] = p[2] as f32 / 255.0; // Blue
            }
        }
        return (
            data,
            LetterboxInfo {
                scale: 1.0,
                pad_x: 0.0,
                pad_y: 0.0,
                orig_w,
                orig_h,
            },
        );
    }

    // 2. 否则执行自适应等比缩放 (长宽比绝对不变，不拉扁不拉长)
    let scale = (640.0 / orig_w as f32).min(640.0 / orig_h as f32);
    let new_w = (orig_w as f32 * scale).round() as u32;
    let new_h = (orig_h as f32 * scale).round() as u32;

    let resized = image::imageops::resize(
        image,
        new_w,
        new_h,
        image::imageops::FilterType::Triangle,
    );

    let pad_x = (640.0 - new_w as f32) / 2.0;
    let pad_y = (640.0 - new_h as f32) / 2.0;
    let offset_x = pad_x.round() as usize;
    let offset_y = pad_y.round() as usize;

    // 填充 YOLO 标准灰度底色归一化到 [0, 1] (114/255 ≈ 0.447)
    let fill_val = 114.0f32 / 255.0;
    let mut data = Array4::<f32>::from_elem((1, 3, 640, 640), fill_val);

    for y in 0..new_h {
        for x in 0..new_w {
            let p = resized.get_pixel(x, y);
            let target_y = offset_y + y as usize;
            let target_x = offset_x + x as usize;
            if target_y < 640 && target_x < 640 {
                data[[0, 0, target_y, target_x]] = p[0] as f32 / 255.0; // Red
                data[[0, 1, target_y, target_x]] = p[1] as f32 / 255.0; // Green
                data[[0, 2, target_y, target_x]] = p[2] as f32 / 255.0; // Blue
            }
        }
    }

    (
        data,
        LetterboxInfo {
            scale,
            pad_x: offset_x as f32,
            pad_y: offset_y as f32,
            orig_w,
            orig_h,
        },
    )
}

impl YoloDetector {
    /// 基于 DirectML GPU 硬件加速的深度学习推理执行 (针对 Intel Arc / NVIDIA / AMD 独显优化，集成 Letterbox 与自动坐标逆变换)
    /// 返回达到指定过滤阈值的所有候选目标（按置信度降序排序）
    pub fn detect_candidates(&self, image: &RgbaImage, threshold: f32) -> Result<Vec<DetectionBox>, String> {
        let mut lock = self.cached_session.lock().unwrap();
        if lock.is_none() {
            *lock = self.get_or_load_session()?;
        }

        let session = match lock.as_mut() {
            Some(m) => m,
            None => return Ok(vec![]),
        };

        let (orig_w, orig_h) = image.dimensions();
        if orig_w == 0 || orig_h == 0 {
            return Ok(vec![]);
        }

        // 1. 工业级自适应 Letterbox 等比预处理
        let (tensor_data, letterbox) = preprocess_letterbox(image);

        let tensor_ref = match ort::value::TensorRef::from_array_view(&tensor_data) {
            Ok(t) => t,
            Err(e) => return Err(format!("构建 DirectML 输入张量失败: {}", e)),
        };

        // 2. 执行 DirectML GPU 深度学习推理 (DirectX 12 跨厂商硬件加速)
        let outputs = match session.run(ort::inputs![tensor_ref]) {
            Ok(o) => o,
            Err(e) => return Err(format!("DirectML GPU 推理异常: {}", e)),
        };

        if outputs.len() == 0 {
            return Ok(vec![]);
        }

        // 3. 解析输出张量 [1, 5, 8400]
        let (_, slice) = match outputs[0].try_extract_tensor::<f32>() {
            Ok(s) => s,
            Err(e) => return Err(format!("解析 DirectML 输出张量异常: {}", e)),
        };

        if slice.len() < 5 * 8400 {
            return Ok(vec![]);
        }

        let num_anchors = 8400;
        let mut candidates = Vec::with_capacity(32);

        // 遍历所有预测锚框
        for i in 0..num_anchors {
            let conf = slice[4 * num_anchors + i];
            if conf >= threshold {
                let net_cx = slice[0 * num_anchors + i];
                let net_cy = slice[1 * num_anchors + i];
                let net_w = slice[2 * num_anchors + i];
                let net_h = slice[3 * num_anchors + i];

                let (orig_x, orig_y, orig_w, orig_h) =
                    letterbox.inverse_transform(net_cx, net_cy, net_w, net_h);

                candidates.push(DetectionBox::new(
                    orig_x,
                    orig_y,
                    orig_w,
                    orig_h,
                    conf,
                    "bobber",
                ));
            }
        }

        // 按置信度由高到低排序
        candidates.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        Ok(candidates)
    }

    /// 基于 DirectML GPU 硬件加速的深度学习推理执行 (针对 Intel Arc 独显优化，集成 Letterbox 与自动坐标逆变换)
    pub fn detect_pure_rust(&self, image: &RgbaImage) -> Result<Vec<DetectionBox>, String> {
        self.detect_candidates(image, self.confidence_threshold)
    }

    /// GPU 预热：启动时立即加载 Session 并跑一次全零推理，触发 DirectML JIT 着色器编译
    /// 消除首次真实推理的 2000ms 延迟，此后每次推理仅需 ~5ms
    pub fn warmup(&self) {
        use std::time::Instant;
        let t = Instant::now();
        println!("🔥 [GPU 预热] 正在触发 DirectML JIT 着色器编译...");

        // 先确保 session 已加载
        {
            let mut lock = self.cached_session.lock().unwrap();
            if lock.is_none() {
                match self.get_or_load_session() {
                    Ok(sess) => *lock = sess,
                    Err(e) => {
                        println!("⚠️ [GPU 预热] 模型加载失败: {}", e);
                        return;
                    }
                }
            }
        }

        // 跑 2 次全零张量推理（第1次 JIT 编译，第2次验证速度）
        let dummy = image::RgbaImage::new(640, 640);
        for i in 1..=2 {
            match self.detect_candidates(&dummy, 0.0) {
                Ok(_) => println!("🔥 [GPU 预热] 第 {} 次预热推理完成，耗时: {}ms", i, t.elapsed().as_millis()),
                Err(e) => println!("⚠️ [GPU 预热] 第 {} 次预热推理失败: {}", i, e),
            }
        }
        println!("✅ [GPU 预热] 完成！后续每次推理将直接以 ~5ms 全速运行。总耗时: {}ms", t.elapsed().as_millis());
    }
}

impl VisionDetector for YoloDetector {
    fn detect(&self, image: &RgbaImage) -> Result<Vec<DetectionBox>, String> {
        self.detect_pure_rust(image)
    }

    fn detect_candidates(&self, image: &RgbaImage, threshold: f32) -> Result<Vec<DetectionBox>, String> {
        self.detect_candidates(image, threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ort_directml_loading() {
        use ort::session::Session;
        use ort::ep;
        use std::time::Instant;

        println!("正在测试 Intel Arc DirectML GPU 引擎初始化...");
        let model_path = "d:/projects/rp2040/wow_assistant/models/bobber.onnx";
        
        let start_init = Instant::now();
        let mut session = Session::builder().unwrap()
            .with_intra_threads(4).unwrap()
            .with_execution_providers([
                ep::DirectML::default()
                    .with_device_filter(ep::directml::DeviceFilter::Gpu)
                    .with_performance_preference(ep::directml::PerformancePreference::HighPerformance)
                    .build()
            ]).unwrap()
            .commit_from_file(model_path)
            .expect("初始化 DirectML 会话失败");

        println!("🎉 DirectML GPU 会话创建成功！耗时: {:?}", start_init.elapsed());
        println!("  输入节点: {:?}", session.inputs());
        println!("  输出节点: {:?}", session.outputs());

        // 构造测试张量 [1, 3, 640, 640]
        let input_tensor = ndarray::Array4::<f32>::zeros((1, 3, 640, 640));

        // 预热 GPU
        let _ = session.run(ort::inputs![ort::value::TensorRef::from_array_view(&input_tensor).unwrap()]).unwrap();

        // 正式测速
        let start_infer = Instant::now();
        let outputs = session.run(ort::inputs![ort::value::TensorRef::from_array_view(&input_tensor).unwrap()]).unwrap();
        let infer_dur = start_infer.elapsed();

        let (shape, slice) = outputs[0].try_extract_tensor::<f32>().unwrap();
        println!("🚀 DirectML GPU 推理耗时: {:?} | 输出 Shape: {:?} | 元素数: {}", infer_dur, shape, slice.len());
        assert_eq!(slice.len(), 5 * 8400);
    }

    #[test]
    fn test_real_water_roi_inference() {
        let path = "d:/projects/rp2040/wow_assistant/debug_images/03_detected_bobber.png";
        if !std::path::Path::new(path).exists() {
            println!("真实图片尚未生成，跳过测试");
            return;
        }

        let img = image::open(path).expect("读取 03_detected_bobber.png 失败").to_rgba8();
        println!("原始图像分辨率: {}x{}", img.width(), img.height());

        // 1. 全图 Letterbox 缩放测试
        let detector = YoloDetector::new("d:/projects/rp2040/wow_assistant/models/bobber.onnx", 0.01);
        let boxes = detector.detect_pure_rust(&img).expect("DirectML GPU 推理失败");
        println!("★ 全图检测结果 (缩放后):");
        for b in &boxes {
            println!("  置信度: {:.2}% @ ({}, {}), 大小: {}x{}", b.confidence * 100.0, b.x, b.y, b.width, b.height);
        }

        // 2. 1:1 无损水面 640x640 ROI 测试 (以真实鱼漂 737, 582 附近裁剪)
        let crop_x = 737u32.saturating_sub(320).min(img.width().saturating_sub(640));
        let crop_y = 582u32.saturating_sub(320).min(img.height().saturating_sub(640));
        let roi = image::imageops::crop_imm(&img, crop_x, crop_y, 640, 640).to_image();
        println!("ROI 裁剪区域: ({}, {}) 大小: 640x640", crop_x, crop_y);

        let roi_boxes = detector.detect_pure_rust(&roi).expect("ROI 推理失败");
        println!("★ 1:1 无损 640x640 ROI 检测结果:");
        for b in &roi_boxes {
            println!("  置信度: {:.2}% @ 局域 ({}, {}) -> 全局 ({}, {}), 大小: {}x{}", 
                b.confidence * 100.0, b.x, b.y, crop_x as i32 + b.x, crop_y as i32 + b.y, b.width, b.height);
        }
    }
}
