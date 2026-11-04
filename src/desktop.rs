//! 桌面平台的窗口尺寸、紧凑布局判断与启动提示，不持有应用业务数据。
//!
//! 作者：Andersen

use gpui_kit::{Bounds, Pixels, Size, point, px, size};

/// 在排除任务栏或 Dock 的工作区居中放置窗口，尺寸和最小限制均不超过工作区。
pub fn window_geometry(visible: Bounds<Pixels>) -> (Bounds<Pixels>, Size<Pixels>) {
    let width = (f32::from(visible.size.width) - 32.).max(1.).min(1440.);
    let height = (f32::from(visible.size.height) - 32.).max(1.).min(940.);
    let initial = size(px(width), px(height));
    let origin = visible.origin
        + point(
            (visible.size.width - initial.width) / 2.,
            (visible.size.height - initial.height) / 2.,
        );
    (
        Bounds::new(origin, initial),
        size(px(width.min(800.)), px(height.min(500.))),
    )
}

/// 当应用缩放后的内容宽度不足时，将集合侧栏改为临时展开面板。
pub fn compact_width(width: Pixels, scale: f32) -> bool {
    f32::from(width) / scale < 1050.
}

/// 矮窗口以可滚动的上下请求区域替代受最小高度约束的拖动分栏。
pub fn compact_height(height: Pixels, scale: f32) -> bool {
    f32::from(height) / scale < 680.
}

/// 显示启动阶段提示；Windows 正式版没有控制台，额外使用原生消息框。
pub fn startup_notice(message: &str, error: bool) {
    eprintln!("{message}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
        };
        let title: Vec<u16> = "ReqSucc\0".encode_utf16().collect();
        let message: Vec<u16> = message
            .replace('\0', " ")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // 消息框同步返回；两个以零结尾的 UTF-16 缓冲区在调用期间有效，启动时没有父窗口。
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK
                    | if error {
                        MB_ICONERROR
                    } else {
                        MB_ICONINFORMATION
                    },
            );
        }
    }
    #[cfg(not(windows))]
    let _ = error;
}

/// 验证高缩放小屏、非零屏幕原点与桌面布局断点，不启动 GUI。
#[cfg(test)]
mod tests {
    use super::*;

    /// 初始窗口和最小尺寸均位于工作区内，保留任务栏之外的屏幕原点。
    #[test]
    fn window_fits_small_and_offset_work_areas() {
        for (width, height) in [(1920., 1040.), (1280., 680.), (960., 500.), (640., 400.)] {
            let work = Bounds::new(point(px(-1280.), px(40.)), size(px(width), px(height)));
            let (bounds, minimum) = window_geometry(work);
            assert!(bounds.origin.x >= work.origin.x && bounds.origin.y >= work.origin.y);
            assert!(bounds.right() <= work.right() && bounds.bottom() <= work.bottom());
            assert!(minimum.width <= bounds.size.width && minimum.height <= bounds.size.height);
        }
    }

    /// 应用内缩放参与紧凑布局，正常大窗口维持原分栏。
    #[test]
    fn compact_layout_accounts_for_application_scale() {
        assert!(!compact_width(px(1440.), 1.));
        assert!(compact_width(px(1100.), 1.25));
        assert!(compact_height(px(720.), 1.25));
        assert!(!compact_height(px(940.), 1.));
    }
}
