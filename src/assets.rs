//! 应用资源入口，组合品牌图标、补充图标与 GPUI Kit 内置资源。
//!
//! 作者：Andersen

use gpui_kit::*;
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Layers,
        RadioTower,
        Ellipsis,
        Trash,
        SlidersHorizontal,
        Clock,
        Send,
        Save,
        Braces,
        Download,
        CircleStop,
        CircleQuestionMark,
        Monitor,
        Keyboard,
        ListX,
        RotateCcw,
        RefreshCw,
        FolderInput,
        FileInput,
        Cable,
        Network
    ]
);
/// 为原生界面提供编译期嵌入的品牌图标与组件库资源。
pub struct AppAssets;
impl AssetSource for AppAssets {
    /// 按路径查找品牌资源、补充图标和内置资源；读取失败沿用资源源的错误。
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == "brand/flying-cat.svg" {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../assets/flying-cat.svg"
            ))));
        }
        if let Some(data) = ExtraIcons.load(path)? {
            return Ok(Some(data));
        }
        gpui_kit::assets::Assets.load(path)
    }
    /// 合并组件库与补充图标的路径列表；品牌 SVG 通过固定路径直接加载。
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}
