//! 在控件的实际绘制范围内居中图标，补偿设备像素取整引入的半像素偏移。
//!
//! 作者：Andersen

use gpui_kit::{
    AvailableSpace, Bounds, IntoElement, Pixels, Point, Styled, Transformation, canvas,
    component::{Icon, Sizable, Size},
    point, px, size,
};

/// 根据已取整的容器边界和图标边长，计算图标精确居中时的左上角。
fn ideal_origin(bounds: Bounds<Pixels>, icon_size: Pixels) -> Point<Pixels> {
    bounds.center() - point(icon_size / 2., icon_size / 2.)
}

/// 返回网格上的图标原点及补偿量；调用方提供与当前窗口一致的设备像素取整。
fn centered_placement(
    bounds: Bounds<Pixels>,
    icon_size: Pixels,
    snap: impl FnOnce(Point<Pixels>) -> Point<Pixels>,
) -> (Point<Pixels>, Point<Pixels>) {
    let origin = ideal_origin(bounds, icon_size);
    let snapped_origin = snap(origin);
    (snapped_origin, origin - snapped_origin)
}

/// 绘制保留资源与颜色的图标，使其中心与占满的父内容区中心一致。
///
/// `icon_size` 为缩放后的逻辑像素边长。图标的原有变换会被居中平移替换，
/// 因此调用方只应传入没有旋转或其他变换的图标。
pub(super) fn centered_icon(icon: Icon, icon_size: Pixels) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let icon_size = window.pixel_snap(icon_size);
            if icon_size <= px(0.) || bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
                return None;
            }

            // 布局与 SVG 栅格化分别落在设备像素网格上；先确定绘制原点，
            // 再以变换补偿奇偶尺寸不同时无法用整数原点表达的半像素。
            let (snapped_origin, translation) =
                centered_placement(bounds, icon_size, |origin| window.pixel_snap_point(origin));
            let mut child = icon
                .with_size(Size::Size(icon_size))
                .transform(Transformation::translate(translation))
                .into_any_element();
            child.prepaint_as_root(
                snapped_origin,
                size(
                    AvailableSpace::Definite(icon_size),
                    AvailableSpace::Definite(icon_size),
                ),
                window,
                cx,
            );
            Some(child)
        },
        |_, child, window, cx| {
            if let Some(mut child) = child {
                child.paint(window, cx);
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 GPUI 对设备像素中点向零取整的规则模拟布局边界。
    fn snap(value: f32, density: f32) -> f32 {
        let scaled = value * density;
        (scaled.abs() - 0.5).ceil().copysign(scaled) / density
    }

    /// 验证奇偶不同的容器与图标在多种缩放、密度和位置下仍具有同一物理像素中心。
    #[test]
    fn centers_icons_at_fractional_device_pixel_positions() {
        let mut found_half_pixel = false;
        for density in [1., 1.25, 1.5, 2., 3.] {
            for scale in [0.9, 1., 1.1, 1.25] {
                for offset in [-17., 0., 31.] {
                    let left = snap(offset, density);
                    let top = snap(offset + 9., density);
                    let width = snap(28. * scale, density);
                    let height = snap(32. * scale, density);
                    let icon_size = snap(12.25 * scale, density);
                    let bounds = Bounds::new(point(px(left), px(top)), size(px(width), px(height)));
                    let (snapped, translation) =
                        centered_placement(bounds, px(icon_size), |origin| {
                            point(
                                px(snap(f32::from(origin.x), density)),
                                px(snap(f32::from(origin.y), density)),
                            )
                        });
                    let center =
                        snapped + translation + point(px(icon_size / 2.), px(icon_size / 2.));
                    assert!(f32::from(center.x - bounds.center().x).abs() * density < 0.0001);
                    assert!(f32::from(center.y - bounds.center().y).abs() * density < 0.0001);
                    assert!(f32::from(translation.x).abs() * density <= 0.5001);
                    assert!(f32::from(translation.y).abs() * density <= 0.5001);
                    found_half_pixel |= f32::from(translation.x).abs() * density > 0.49
                        || f32::from(translation.y).abs() * density > 0.49;
                }
            }
        }
        assert!(found_half_pixel, "必须覆盖实际需要半像素补偿的情况");
    }
}
