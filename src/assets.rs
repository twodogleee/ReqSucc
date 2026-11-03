use gpui_kit::*;
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Layers,
        Ellipsis,
        Trash,
        SlidersHorizontal,
        Clock,
        Send,
        Save,
        Braces,
        Download,
        CircleStop,
        CircleQuestionMark
    ]
);
pub struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == "brand/reqsucc.png" {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../assets/reqsucc-icon.png"
            ))));
        }
        if let Some(data) = ExtraIcons.load(path)? {
            return Ok(Some(data));
        }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}
