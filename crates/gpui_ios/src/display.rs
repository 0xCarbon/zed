use anyhow::Result;
use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, Point, Size, px};
use objc2::MainThreadMarker;
use objc2_ui_kit::UIScreen;

/// The device's built-in screen.
///
/// iOS has exactly one display from the app's point of view, and its bounds are
/// snapshotted at creation time: `UIScreen` is main-thread-only, while
/// `PlatformDisplay` may be read from any context.
#[derive(Debug)]
pub(crate) struct IosDisplay {
    bounds: Bounds<Pixels>,
    uuid: uuid::Uuid,
}

impl IosDisplay {
    pub(crate) fn primary() -> Result<Self> {
        let main_thread = MainThreadMarker::new()
            .ok_or_else(|| anyhow::anyhow!("IosDisplay must be created on the main thread"))?;
        #[allow(deprecated)]
        let screen_bounds = UIScreen::mainScreen(main_thread).bounds();
        Ok(Self {
            bounds: Bounds {
                origin: Point::new(
                    px(screen_bounds.origin.x as f32),
                    px(screen_bounds.origin.y as f32),
                ),
                size: Size {
                    width: px(screen_bounds.size.width as f32),
                    height: px(screen_bounds.size.height as f32),
                },
            },
            uuid: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"gpui-ios-main-screen"),
        })
    }
}

impl PlatformDisplay for IosDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(1)
    }

    fn uuid(&self) -> Result<uuid::Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}
