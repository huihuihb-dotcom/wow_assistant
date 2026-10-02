use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
use serialport::{SerialPort, SerialPortType};

/// RP2040 硬件级键鼠控制器驱动库 (纯 Rust 实现)
/// 专为 Tauri / Rust 后端打造，无需任何 Python 运行环境
pub struct Rp2040Controller {
    port_name: String,
    port: Box<dyn SerialPort>,
}

impl Rp2040Controller {
    /// 自动扫描并匹配 RP2040 的 COM 端口 (依据树莓派 VID: 0x2E8A)
    pub fn auto_detect_port() -> Option<String> {
        if let Ok(ports) = serialport::available_ports() {
            for p in ports {
                if let SerialPortType::UsbPort(info) = p.port_type {
                    // 树莓派官方 VID 为 0x2E8A
                    if info.vid == 0x2E8A {
                        return Some(p.port_name);
                    }
                }
            }
        }
        // 若未匹配到 USB 描述，默认尝试 COM3
        Some("COM3".to_string())
    }

    /// 自动连接 RP2040
    pub fn connect_auto() -> Result<Self, String> {
        let port_name = Self::auto_detect_port().ok_or_else(|| "未找到 RP2040 开发板串口".to_string())?;
        Self::connect(&port_name)
    }

    /// 连接指定串口 (如 "COM3")
    pub fn connect(port_name: &str) -> Result<Self, String> {
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(1500))
            .open()
            .map_err(|e| format!("打开串口 {} 失败: {}", port_name, e))?;

        // 关键：Windows 下 USB CDC 必须拉高 DTR 和 RTS 信号，否则单片机判定无终端连接不回传数据
        let _ = port.write_data_terminal_ready(true);
        let _ = port.write_request_to_send(true);

        let controller = Self {
            port_name: port_name.to_string(),
            port,
        };

        // 短暂休眠并清空旧缓冲区
        std::thread::sleep(Duration::from_millis(500));
        let _ = controller.port.clear(serialport::ClearBuffer::All);

        Ok(controller)
    }

    /// 获取当前连接的串口名称
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// 向 RP2040 发送单行指令并读取响应
    pub fn send_command(&mut self, cmd: &str) -> Result<String, String> {
        let msg = format!("{}\n", cmd.trim());
        self.port
            .write_all(msg.as_bytes())
            .map_err(|e| format!("写入串口失败: {}", e))?;
        self.port.flush().map_err(|e| format!("刷新串口失败: {}", e))?;

        let mut reader = BufReader::new(&mut self.port);
        let mut response = String::new();
        reader
            .read_line(&mut response)
            .map_err(|e| format!("读取串口响应超时/失败: {}", e))?;

        Ok(response.trim().to_string())
    }

    /// 握手测试 (PING -> PONG)
    pub fn ping(&mut self) -> Result<bool, String> {
        let resp = self.send_command("PING")?;
        Ok(resp.contains("PONG"))
    }

    // ================= 鼠标接口 =================

    /// 【核心】片上拟人化鼠标移动 (WindMouse 物理引擎自主解算手颤、弧线和过冲)
    pub fn human_move(&mut self, dx: i32, dy: i32, speed: Option<f32>) -> Result<String, String> {
        let cmd = match speed {
            Some(s) => format!("HM,{},{},{:.2}", dx, dy, s),
            None => format!("HM,{},{}", dx, dy),
        };
        self.send_command(&cmd)
    }

    /// 瞬发相对移动鼠标
    pub fn mouse_move(&mut self, dx: i32, dy: i32, wheel: Option<i32>) -> Result<String, String> {
        let cmd = match wheel {
            Some(w) => format!("M,{},{},{}", dx, dy, w),
            None => format!("M,{},{}", dx, dy),
        };
        self.send_command(&cmd)
    }

    /// 鼠标点击 (left / right / middle)
    pub fn mouse_click(&mut self, button: &str) -> Result<String, String> {
        self.send_command(&format!("MC,{}", button))
    }

    /// 鼠标按下按键不放
    pub fn mouse_down(&mut self, button: &str) -> Result<String, String> {
        self.send_command(&format!("MD,{}", button))
    }

    /// 鼠标松开按键
    pub fn mouse_up(&mut self, button: &str) -> Result<String, String> {
        self.send_command(&format!("MU,{}", button))
    }

    // ================= 键盘接口 =================

    /// 键盘敲击单键 (按下并松开，如 "A", "ENTER", "SPACE", "F1")
    pub fn key_press(&mut self, key: &str) -> Result<String, String> {
        self.send_command(&format!("KP,{}", key))
    }

    /// 键盘按键按下不放
    pub fn key_down(&mut self, key: &str) -> Result<String, String> {
        self.send_command(&format!("KD,{}", key))
    }

    /// 键盘按键松开
    pub fn key_up(&mut self, key: &str) -> Result<String, String> {
        self.send_command(&format!("KU,{}", key))
    }

    /// 直接输入一串英文字符串
    pub fn type_text(&mut self, text: &str) -> Result<String, String> {
        self.send_command(&format!("TEXT,{}", text))
    }

    // ================= 状态与保护接口 =================

    /// 设置开发板 WS2812 RGB 彩灯颜色 (r, g, b 取值 0~255)
    pub fn set_led(&mut self, r: u8, g: u8, b: u8) -> Result<String, String> {
        self.send_command(&format!("LED,{},{},{}", r, g, b))
    }

    /// 安全释放所有键盘按键和鼠标按键 (防止卡键死锁)
    pub fn reset_all(&mut self) -> Result<String, String> {
        self.send_command("RESET")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_connection() {
        println!("正在尝试连接 RP2040 硬件控制器...");
        let mut ctrl = Rp2040Controller::connect_auto().expect("无法连接到 RP2040 开发板");
        println!("连接成功！串口: {}", ctrl.port_name());

        let ping_ok = ctrl.ping().expect("PING 测试失败");
        assert!(ping_ok, "板卡未返回 PONG");
        println!("PING 握手成功！");

        println!("测试片上纯硬件拟人移动 HM,30,20...");
        let resp = ctrl.human_move(30, 20, None).expect("HM 指令失败");
        println!("开发板响应: {}", resp);
        assert!(resp.starts_with("OK:HM"));

        let _ = ctrl.human_move(-30, -20, None);
        println!("测试完毕！");
    }
}
