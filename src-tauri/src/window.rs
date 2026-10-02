use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, SetFocus, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, BringWindowToTop, EnumWindows, FindWindowW, GetClassNameW,
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    PostMessageW, SetForegroundWindow, ShowWindow, SystemParametersInfoW, SW_RESTORE, SW_SHOW,
    SPI_SETFOREGROUNDLOCKTIMEOUT, SPIF_SENDCHANGE, WM_KEYDOWN, WM_KEYUP,
};

#[link(name = "user32")]
extern "system" {
    fn SwitchToThisWindow(hwnd: HWND, fUnknown: BOOL);
}

/// 禁用 Windows 前台激活锁（Foreground Lock）
///
/// Windows Vista+ 默认在 200ms 内无用户输入就拒绝跨进程 SetForegroundWindow。
/// 调用此函数将超时设为 0，使 SetForegroundWindow 在任何时候都能生效。
/// AutoHotkey、OBS、各类窗口管理器均使用这个方法。
/// 调用一次即可，效果持续到系统重启。
pub fn disable_foreground_lock() {
    unsafe {
        let timeout: u32 = 0;
        SystemParametersInfoW(
            SPI_SETFOREGROUNDLOCKTIMEOUT,
            0,
            &timeout as *const u32 as *mut _,
            SPIF_SENDCHANGE,
        );
    }
}


pub fn find_wow_hwnd() -> Option<HWND> {
    // 1. 优先使用 xcap 进行多维度扫描 (与截图一致，实测最可靠)
    if let Ok(windows) = xcap::Window::all() {
        for w in windows {
            let title = w.title().unwrap_or_default().to_lowercase();
            let app = w.app_name().unwrap_or_default().to_lowercase();
            if title.contains("world of warcraft")
                || title.contains("魔兽世界")
                || app.contains("wow")
                || title.starts_with("wow")
            {
                if let Ok(id) = w.id() {
                    let hwnd = id as usize as HWND;
                    if hwnd != 0 as HWND {
                        return Some(hwnd);
                    }
                }
            }
        }
    }

    // 2. 遍历 Win32 顶层窗口
    let mut target_hwnd: HWND = 0 as HWND;
    unsafe {
        EnumWindows(
            Some(enum_window_callback),
            &mut target_hwnd as *mut HWND as LPARAM,
        );
    }
    if target_hwnd != 0 as HWND {
        return Some(target_hwnd);
    }

    // 3. 经典类名直接查找 (GxWindowClass)
    unsafe {
        let gx_class: Vec<u16> = "GxWindowClass\0".encode_utf16().collect();
        let direct_hwnd = FindWindowW(gx_class.as_ptr(), std::ptr::null());
        if direct_hwnd != 0 as HWND {
            return Some(direct_hwnd);
        }
    }

    None
}

/// 检查并激活魔兽世界游戏窗口到系统前台
/// 最多重试 3 次，每次间隔 500ms（用于首次启动）
pub fn activate_wow_window() -> Result<bool, String> {
    unsafe {
        let fg_hwnd = GetForegroundWindow();
        if fg_hwnd != 0 as HWND && is_wow_window(fg_hwnd) {
            return Ok(true);
        }

        let target_hwnd = match find_wow_hwnd() {
            Some(h) => h,
            None => return Ok(false),
        };

        // AllowSetForegroundWindow 显式授权 WoW 进程可以成为前台
        let mut wow_pid: u32 = 0;
        GetWindowThreadProcessId(target_hwnd, &mut wow_pid);
        if wow_pid > 0 {
            AllowSetForegroundWindow(wow_pid);
        }

        // 最多重试 3 次
        for attempt in 0..3 {
            force_foreground(target_hwnd);
            std::thread::sleep(std::time::Duration::from_millis(500));

            let cur_fg = GetForegroundWindow();
            if cur_fg == target_hwnd || is_wow_window(cur_fg) {
                return Ok(true);
            }

            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }

        Ok(false)
    }
}

/// 轻量版：仅尝试激活一次（不重试），用于倒计时/循环中的状态检查
/// 避免每次调用都 sleep 2+ 秒阻塞主循环
pub fn try_activate_wow_once() -> bool {
    unsafe {
        let fg_hwnd = GetForegroundWindow();
        if fg_hwnd != 0 as HWND && is_wow_window(fg_hwnd) {
            return true;
        }
        let target_hwnd = match find_wow_hwnd() {
            Some(h) => h,
            None => return false,
        };
        let mut wow_pid: u32 = 0;
        GetWindowThreadProcessId(target_hwnd, &mut wow_pid);
        if wow_pid > 0 {
            AllowSetForegroundWindow(wow_pid);
        }
        force_foreground(target_hwnd);
        // 短暂等待（200ms）
        std::thread::sleep(std::time::Duration::from_millis(200));
        let cur_fg = GetForegroundWindow();
        cur_fg == target_hwnd || is_wow_window(cur_fg)
    }
}

/// 仅检查 WoW 是否在前台，不做任何激活操作
pub fn is_wow_foreground() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        fg != 0 as HWND && is_wow_window(fg)
    }
}

/// 强力穿透将目标窗口拉至前台
/// 使用 VK_MENU(Alt键) + AttachThreadInput + SwitchToThisWindow 组合技
pub unsafe fn force_foreground(hwnd: HWND) {
    const VK_MENU: u8 = 0x12; // Alt 键虚拟键码

    if hwnd == 0 as HWND {
        return;
    }

    // 步骤 0：发送 Alt 键按下/释放
    // Alt 键是 Windows 前台激活的「魔法钥匙」——系统检测到 Alt 事件后
    // 会暂时放宽 SetForegroundWindow 的安全限制
    keybd_event(VK_MENU, 0, 0, 0);
    keybd_event(VK_MENU, 0, KEYEVENTF_KEYUP, 0);

    // 步骤 1：恢复最小化
    ShowWindow(hwnd, SW_RESTORE);

    // 步骤 2：SwitchToThisWindow（模拟 Alt+Tab 效果）
    SwitchToThisWindow(hwnd, 1);

    // 步骤 3：线程输入绑定 + SetForegroundWindow（双保险）
    let fg_hwnd = GetForegroundWindow();
    let fg_tid = if fg_hwnd != 0 as HWND {
        GetWindowThreadProcessId(fg_hwnd, std::ptr::null_mut())
    } else {
        0
    };
    let target_tid = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());

    if fg_tid != 0 && fg_tid != target_tid {
        AttachThreadInput(fg_tid, target_tid, 1);
        SetForegroundWindow(hwnd);
        BringWindowToTop(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(hwnd);
        AttachThreadInput(fg_tid, target_tid, 0);
    } else {
        SetForegroundWindow(hwnd);
        BringWindowToTop(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(hwnd);
    }
}

unsafe extern "system" fn enum_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if is_wow_window(hwnd) {
        let out_ptr = lparam as *mut HWND;
        *out_ptr = hwnd;
        return 0; // 找到目标，停止枚举
    }

    1
}

/// 全方位综合判定窗口是否属于魔兽世界 (进程名 + 窗口类名 + 标题)
pub unsafe fn is_wow_window(hwnd: HWND) -> bool {
    // 1. 优先通过进程名判定 (最可靠，完全免疫怀旧服空标题问题)
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid > 0 {
        let h_proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h_proc != std::ptr::null_mut() {
            let mut buf: [u16; 512] = [0; 512];
            let mut size: u32 = 512;
            if QueryFullProcessImageNameW(h_proc, 0, buf.as_mut_ptr(), &mut size) != 0 && size > 0 {
                let proc_path = OsString::from_wide(&buf[..size as usize])
                    .to_string_lossy()
                    .to_lowercase();
                windows_sys::Win32::Foundation::CloseHandle(h_proc);

                if proc_path.contains("wowclassic")
                    || proc_path.contains("wow.exe")
                    || proc_path.contains("wow-64")
                    || proc_path.contains("world of warcraft")
                {
                    return true;
                }
            } else {
                windows_sys::Win32::Foundation::CloseHandle(h_proc);
            }
        }
    }

    // 2. 类名判定 (GxWindowClass)
    let class_name = get_window_class(hwnd).to_lowercase();
    if class_name.contains("gxwindow") || class_name == "gxwindowclass" {
        return true;
    }

    // 3. 标题判定
    let title = get_window_title(hwnd).to_lowercase();
    if title.contains("world of warcraft") || title.contains("魔兽世界") || title.starts_with("wow") {
        return true;
    }

    false
}

unsafe fn get_window_class(hwnd: HWND) -> String {
    let mut buf: [u16; 256] = [0; 256];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), 256);
    if len <= 0 {
        return String::new();
    }
    OsString::from_wide(&buf[..len as usize])
        .to_string_lossy()
        .to_string()
}

unsafe fn get_window_title(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }

    let mut buf: Vec<u16> = vec![0; (len + 1) as usize];
    let actual_len = GetWindowTextW(hwnd, buf.as_mut_ptr(), len + 1);
    if actual_len <= 0 {
        return String::new();
    }

    buf.truncate(actual_len as usize);
    OsString::from_wide(&buf).to_string_lossy().to_string()
}

/// 将常见按键字符串解析为 Windows 虚拟键码 (Virtual-Key Code)
pub fn key_str_to_vk(key: &str) -> Option<u16> {
    let trimmed = key.trim().to_lowercase();
    if trimmed.is_empty() {
        return None;
    }

    // 单字符情况
    if trimmed.chars().count() == 1 {
        let ch = trimmed.chars().next().unwrap();
        match ch {
            '0'..='9' => return Some(0x30 + (ch as u16 - '0' as u16)),
            'a'..='z' => return Some(0x41 + (ch as u16 - 'a' as u16)),
            ' ' => return Some(0x20), // VK_SPACE
            _ => {}
        }
    }

    // 常用多字符按键名称
    match trimmed.as_str() {
        "space" => Some(0x20),
        "enter" | "return" => Some(0x0D),
        "esc" | "escape" => Some(0x1B),
        "tab" => Some(0x09),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        _ => None,
    }
}

/// 借鉴 FishingFun 核心机制：通过 Win32 PostMessage 跨进程直投键盘消息到魔兽世界主窗口
/// 优势：
/// 1. 即使魔兽世界不在系统前台或无键盘焦点，魔兽消息队列依然能接收并触发技能（如抛竿）
/// 2. 彻底绕过 Windows 11 UIPI 与前台激活限制
pub fn post_key_to_wow(key: &str) -> bool {
    let hwnd = match find_wow_hwnd() {
        Some(h) => h,
        None => return false,
    };
    let vk = match key_str_to_vk(key) {
        Some(code) => code,
        None => return false,
    };

    unsafe {
        // 投递 WM_KEYDOWN
        PostMessageW(hwnd, WM_KEYDOWN, vk as usize, 0);
        std::thread::sleep(std::time::Duration::from_millis(60));
        // 投递 WM_KEYUP
        PostMessageW(hwnd, WM_KEYUP, vk as usize, 0);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_activation() {
        let _ = activate_wow_window();
    }
}
