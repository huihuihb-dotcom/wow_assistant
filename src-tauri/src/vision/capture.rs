use image::RgbaImage;
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, GetWindowRect, SetCursorPos};
use xcap::Monitor;

/// 获取当前鼠标光标在屏幕上的绝对物理像素坐标 (x, y)
pub fn get_cursor_pos() -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

/// 将鼠标光标精准设置到屏幕物理像素坐标 (x, y)
pub fn set_cursor_pos(x: i32, y: i32) {
    unsafe {
        SetCursorPos(x, y);
    }
}

/// 缓动拟人平滑移动光标到屏幕物理目标坐标 (target_x, target_y)
/// 采用人体工学 Ease-Out 减速曲线，50ms 内自然滑至目标，终点绝对 100% 落在目标点，误差永远为 0
pub fn smooth_move_cursor(target_x: i32, target_y: i32) {
    let (cur_x, cur_y) = get_cursor_pos();
    let dx = target_x - cur_x;
    let dy = target_y - cur_y;
    if dx == 0 && dy == 0 {
        return;
    }

    let steps = 8;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        // 人体甩手缓动减速曲线 (Ease-Out Quad)
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        let nx = cur_x + ((dx as f32) * ease).round() as i32;
        let ny = cur_y + ((dy as f32) * ease).round() as i32;
        set_cursor_pos(nx, ny);
        std::thread::sleep(std::time::Duration::from_millis(6));
    }
    // 强制锚定绝对终点，确保 0 像素误差
    set_cursor_pos(target_x, target_y);
}

/// 捕获主显示器全屏画面并转换为 RgbaImage
pub fn capture_primary_screen() -> Result<RgbaImage, String> {
    let monitors = Monitor::all().map_err(|e| format!("获取显示器列表失败: {}", e))?;
    let primary = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| "未找到主显示器".to_string())?;

    let img = primary
        .capture_image()
        .map_err(|e| format!("屏幕截图失败: {}", e))?;

    Ok(img)
}

/// 窗口截图捕获结果与物理屏幕位置
#[derive(Debug, Clone)]
pub struct WindowCapture {
    pub image: RgbaImage,
    pub win_x: i32,
    pub win_y: i32,
    pub win_w: u32,
    pub win_h: u32,
    pub is_window_specific: bool,
}

/// 极速精准捕获魔兽世界游戏画面（直接通过主显示器硬件表面捕获 + Win32 客户区精准定位与无损裁剪）
pub fn capture_wow_window() -> Result<WindowCapture, String> {
    // 1. 直接抓取主显示器画面（耗时仅 10-20ms）
    let screen = capture_primary_screen()?;
    let (sw, sh) = screen.dimensions();

    // 2. Win32 毫秒级锁定魔兽窗口真实渲染客户区 (Client Area)，彻底排除标题栏、窗口边框及桌面其他应用
    if let Some(hwnd) = crate::window::find_wow_hwnd() {
        unsafe {
            let mut client_rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let mut pt = POINT { x: 0, y: 0 };

            if GetClientRect(hwnd, &mut client_rect) != 0 && ClientToScreen(hwnd, &mut pt) != 0 {
                let client_w = client_rect.right as u32;
                let client_h = client_rect.bottom as u32;

                if client_w >= 200 && client_h >= 200 && pt.x < sw as i32 && pt.y < sh as i32 {
                    let safe_x = pt.x.clamp(0, (sw - 1) as i32) as u32;
                    let safe_y = pt.y.clamp(0, (sh - 1) as i32) as u32;
                    let safe_w = client_w.min(sw.saturating_sub(safe_x));
                    let safe_h = client_h.min(sh.saturating_sub(safe_y));

                    if safe_w >= 200 && safe_h >= 200 {
                        if let Ok(cropped_game) = crop_roi(&screen, safe_x, safe_y, safe_w, safe_h) {
                            return Ok(WindowCapture {
                                image: cropped_game,
                                win_x: safe_x as i32,
                                win_y: safe_y as i32,
                                win_w: safe_w,
                                win_h: safe_h,
                                is_window_specific: true,
                            });
                        }
                    }
                }
            }

            // 备用兼容方案：如果 GetClientRect 获取失败，回退到 GetWindowRect 窗口外框裁剪
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if GetWindowRect(hwnd, &mut rect) != 0 {
                let w = (rect.right - rect.left) as u32;
                let h = (rect.bottom - rect.top) as u32;
                if w >= 200 && h >= 200 && rect.left < sw as i32 && rect.top < sh as i32 {
                    let safe_x = rect.left.clamp(0, (sw - 1) as i32) as u32;
                    let safe_y = rect.top.clamp(0, (sh - 1) as i32) as u32;
                    let safe_w = w.min(sw.saturating_sub(safe_x));
                    let safe_h = h.min(sh.saturating_sub(safe_y));
                    if safe_w >= 200 && safe_h >= 200 {
                        if let Ok(cropped_game) = crop_roi(&screen, safe_x, safe_y, safe_w, safe_h) {
                            return Ok(WindowCapture {
                                image: cropped_game,
                                win_x: safe_x as i32,
                                win_y: safe_y as i32,
                                win_w: safe_w,
                                win_h: safe_h,
                                is_window_specific: true,
                            });
                        }
                    }
                }
            }
        }
    }

    // 3. 兜底保护：当游戏窗口未找到或最小化时，使用全屏幕保底 (win_x=0, win_y=0)
    Ok(WindowCapture {
        image: screen,
        win_x: 0,
        win_y: 0,
        win_w: sw,
        win_h: sh,
        is_window_specific: false,
    })
}

/// 从游戏窗口中裁剪水面落漂区 ROI (覆盖鱼漂真实落点区域)
/// 鱼漂通常落在角色前方、屏幕 10%~88% 高度的水面黄金区域
/// 返回 (ROI图像, 相对窗口左侧偏移 offset_x, 相对窗口顶部偏移 offset_y)
pub fn get_fishing_water_roi(cap: &WindowCapture) -> (RgbaImage, u32, u32) {
    let (img_w, img_h) = cap.image.dimensions();

    // ROI 宽度：取窗口宽度的 85%（最大 1440），水平居中
    let roi_w = (img_w * 85 / 100).clamp(640, 1440).min(img_w);

    // ROI 高度：垂直覆盖 10%~88% 的屏幕高度，彻底囊括远近所有抛投点
    let y_top = (img_h as f32 * 0.10) as u32;      // 从 10% 开始（跳过顶部 UI 栏）
    let y_bot = (img_h as f32 * 0.88) as u32;      // 到 88% 结束（覆盖角色前方近水域）
    let roi_h = y_bot.saturating_sub(y_top).clamp(480, img_h).min(img_h - y_top);

    // 水平居中
    let offset_x = (img_w.saturating_sub(roi_w)) / 2;
    // 垂直偏移
    let offset_y = y_top;

    let cropped = image::imageops::crop_imm(&cap.image, offset_x, offset_y, roi_w, roi_h).to_image();
    (cropped, offset_x, offset_y)
}

/// 裁剪屏幕特定区域 (ROI)
pub fn crop_roi(
    img: &RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<RgbaImage, String> {
    let (img_w, img_h) = img.dimensions();
    if x >= img_w || y >= img_h {
        return Err("ROI 起始坐标超出屏幕范围".to_string());
    }

    let actual_w = width.min(img_w - x);
    let actual_h = height.min(img_h - y);

    let cropped = image::imageops::crop_imm(img, x, y, actual_w, actual_h).to_image();
    Ok(cropped)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    #[test]
    fn test_dump_all_system_windows() {
        use std::io::Write;
        let mut file = std::fs::File::create("d:/projects/rp2040/wow_assistant/debug_windows.txt").unwrap();
        
        unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let file_ptr = lparam as *mut std::fs::File;
            let file = unsafe { &mut *file_ptr };

            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd, &mut rect);
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if w <= 100 || h <= 100 {
                return 1;
            }

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let class_name = crate::window::is_wow_window(hwnd); // 顺便测试我们的判定

            let mut buf_cls: [u16; 256] = [0; 256];
            let len_cls = GetClassNameW(hwnd, buf_cls.as_mut_ptr(), 256);
            let str_cls = if len_cls > 0 {
                OsString::from_wide(&buf_cls[..len_cls as usize]).to_string_lossy().to_string()
            } else {
                String::new()
            };

            let mut buf_title: [u16; 256] = [0; 256];
            let len_title = GetWindowTextW(hwnd, buf_title.as_mut_ptr(), 256);
            let str_title = if len_title > 0 {
                OsString::from_wide(&buf_title[..len_title as usize]).to_string_lossy().to_string()
            } else {
                String::new()
            };

            let line = format!(
                "HWND: {:?} | PID: {} | Size: ({}x{}) @ ({},{}) | Class: '{}' | Title: '{}' | is_wow: {}\n",
                hwnd, pid, w, h, rect.left, rect.top, str_cls, str_title, class_name
            );
            let _ = file.write_all(line.as_bytes());
            1
        }

        unsafe {
            EnumWindows(Some(callback), &mut file as *mut std::fs::File as LPARAM);
        }
    }

    #[test]
    fn test_print_xcap_windows() {
        if let Ok(windows) = xcap::Window::all() {
            println!("xcap 发现窗口总数: {}", windows.len());
            for w in windows {
                let title = w.title().unwrap_or_default();
                let app = w.app_name().unwrap_or_default();
                let w_w = w.width().unwrap_or(0);
                let w_h = w.height().unwrap_or(0);
                if title.to_lowercase().contains("wow") || app.to_lowercase().contains("wow") || title.contains("魔兽") {
                    println!(
                        "★ 发现魔兽相关 xcap 窗口: id = {:?}, title = '{}', app = '{}', size = {}x{}",
                        w.id(), title, app, w_w, w_h
                    );
                }
            }
        }
    }

    #[test]
    fn test_cursor_and_screen_coordinates() {
        let (cx, cy) = super::get_cursor_pos();
        println!("★ 当前光标位置 GetCursorPos: ({}, {})", cx, cy);
        if let Ok(monitors) = xcap::Monitor::all() {
            for (i, m) in monitors.iter().enumerate() {
                println!(
                    "★ 显示器 #{}: name={}, width={}, height={}, is_primary={:?}",
                    i, m.name().unwrap_or_default(), m.width().unwrap_or(0), m.height().unwrap_or(0), m.is_primary()
                );
            }
        }
    }
}
