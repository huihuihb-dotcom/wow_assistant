# YOLO 模型导入说明 (WoW Assistant Vision Models)

本目录用于存放魔兽世界自动化助手的 YOLO 目标检测模型权重（推荐导出为 `.onnx` 格式）。

## 1. 推荐模型格式
- **鱼漂模型**：`bobber.onnx`
  - 任务：检测魔兽世界中的鱼漂（羽毛/浮标红白特征）
  - 类别：`0: bobber`
  - 推荐尺寸：`640x640` 或 `320x320`（Nano 极速轻量模型）
- **采矿准星/矿脉模型**：`mining.onnx`
  - 任务：检测屏幕中心插件准星或小地图矿点
  - 类别：`0: crosshair`, `1: node`

## 2. 导出方法 (Ultralytics YOLOv8)
在训练完成后，使用 Python 执行：
```python
from ultralytics import YOLO

# 加载训练完成的权重
model = YOLO("runs/detect/train/weights/best.pt")

# 导出为轻量高性能 ONNX 格式
model.export(format="onnx", imgsz=640, dynamic=False, simplify=True)
```
将导出的 `best.onnx` 重命名为 `bobber.onnx` 或 `mining.onnx` 放入本目录即可！

## 3. 当前运行模式
- 系统已内置**零门槛自适应启发式特征检测器**（基于水面羽毛红色色相聚类与溅水物理能量差分），即使不放 ONNX 模型，也可以直接点击“启动钓鱼”进行全流程测试。
- 一旦本目录下检测到 ONNX 权重文件或本地推理服务，系统将优先以微秒级吞吐运行深度学习视觉预测。
