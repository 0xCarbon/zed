use crate::display::IosDisplay;
use anyhow::Context as _;
use gpui::{
    AnyWindowHandle, BackgroundExecutor, Bounds, Capslock, DevicePixels, DispatchEventResult,
    ForegroundExecutor, GpuSpecs, Modifiers, Pixels, PlatformAtlas, PlatformDisplay, PlatformInput,
    PlatformInputHandler, PlatformWindow, Point, PromptButton, PromptLevel, RequestFrameOptions,
    Scene, Size, Task, WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowParams,
    px,
};
use gpui_wgpu::{GpuContext, WgpuRenderer, WgpuSurfaceConfig};
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_quartz_core::CAMetalLayer;
use objc2_ui_kit::{UIScreen, UIView, UIViewController, UIWindow};
use raw_window_handle as rwh;
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

define_class!(
    // A `UIView` whose backing layer is a `CAMetalLayer`, so wgpu can render
    // into the view directly instead of attaching a sublayer that would need
    // manual resizing.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "GPUIMetalView"]
    pub(crate) struct MetalView;

    impl MetalView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            CAMetalLayer::class()
        }
    }
);

/// Raw pointers handed to wgpu for surface creation.
///
/// Safety: wgpu requires `Send + Sync` on the handle provider, but only uses it
/// on the thread that creates the surface. The `UIView` outlives the renderer
/// because `IosWindow` retains it for its whole lifetime.
#[derive(Debug, Clone, Copy)]
struct RawWindow {
    ui_view: NonNull<c_void>,
}

unsafe impl Send for RawWindow {}
unsafe impl Sync for RawWindow {}

impl rwh::HasWindowHandle for RawWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let handle = rwh::UiKitWindowHandle::new(self.ui_view);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl rwh::HasDisplayHandle for RawWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Ok(rwh::DisplayHandle::uikit())
    }
}

#[derive(Default)]
pub(crate) struct IosWindowCallbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    hover_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    close: Option<Box<dyn FnOnce()>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
    hit_test_window_control: Option<Box<dyn FnMut() -> Option<gpui::WindowControlArea>>>,
}

pub(crate) struct IosWindowState {
    renderer: WgpuRenderer,
    bounds: Bounds<Pixels>,
    scale_factor: f32,
    input_handler: Option<PlatformInputHandler>,
}

pub(crate) struct IosWindowInner {
    state: RefCell<IosWindowState>,
    callbacks: RefCell<IosWindowCallbacks>,
}

pub(crate) struct IosWindow {
    inner: Rc<IosWindowInner>,
    display: Rc<dyn PlatformDisplay>,
    #[allow(dead_code)]
    handle: AnyWindowHandle,
    ui_window: Retained<UIWindow>,
    #[allow(dead_code)]
    view_controller: Retained<UIViewController>,
    view: Retained<MetalView>,
    // TODO(ios): drive frames with CADisplayLink instead of a fixed-interval timer.
    _frame_task: Task<()>,
}

impl IosWindow {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        _params: WindowParams,
        gpu_context: GpuContext,
        foreground_executor: ForegroundExecutor,
        background_executor: BackgroundExecutor,
    ) -> anyhow::Result<Self> {
        let main_thread =
            MainThreadMarker::new().context("IosWindow must be created on the main thread")?;

        #[allow(deprecated)]
        let screen = UIScreen::mainScreen(main_thread);
        let screen_bounds = screen.bounds();
        let scale_factor = screen.nativeScale() as f32;

        let ui_window: Retained<UIWindow> =
            unsafe { msg_send![UIWindow::alloc(main_thread), initWithFrame: screen_bounds] };
        let view: Retained<MetalView> =
            unsafe { msg_send![MetalView::alloc(main_thread), initWithFrame: screen_bounds] };
        view.setContentScaleFactor(scale_factor as f64);

        let view_controller: Retained<UIViewController> =
            unsafe { msg_send![UIViewController::alloc(main_thread), init] };
        view_controller.setView(Some(&view));
        ui_window.setRootViewController(Some(&view_controller));
        ui_window.makeKeyAndVisible();

        let bounds = Bounds {
            origin: Point::new(
                px(screen_bounds.origin.x as f32),
                px(screen_bounds.origin.y as f32),
            ),
            size: Size {
                width: px(screen_bounds.size.width as f32),
                height: px(screen_bounds.size.height as f32),
            },
        };

        let device_size = Size {
            width: DevicePixels((screen_bounds.size.width as f32 * scale_factor) as i32),
            height: DevicePixels((screen_bounds.size.height as f32 * scale_factor) as i32),
        };

        let raw_window = RawWindow {
            ui_view: NonNull::from(&*view).cast(),
        };

        let renderer = WgpuRenderer::new(
            gpu_context,
            &raw_window,
            WgpuSurfaceConfig {
                size: device_size,
                transparent: false,
                preferred_present_mode: None,
            },
            None,
        )?;

        let inner = Rc::new(IosWindowInner {
            state: RefCell::new(IosWindowState {
                renderer,
                bounds,
                scale_factor,
                input_handler: None,
            }),
            callbacks: RefCell::new(IosWindowCallbacks::default()),
        });

        let frame_task = foreground_executor.spawn({
            let inner = Rc::downgrade(&inner);
            let timer_executor = background_executor;
            async move {
                loop {
                    timer_executor.timer(FRAME_INTERVAL).await;
                    let Some(inner) = inner.upgrade() else { break };
                    let mut callbacks = inner.callbacks.borrow_mut();
                    if let Some(request_frame) = callbacks.request_frame.as_mut() {
                        request_frame(RequestFrameOptions {
                            require_presentation: false,
                            force_render: false,
                        });
                    }
                }
            }
        });

        Ok(Self {
            inner,
            display: Rc::new(IosDisplay::primary()?),
            handle,
            ui_window,
            view_controller,
            view,
            _frame_task: frame_task,
        })
    }
}

impl rwh::HasWindowHandle for IosWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let view: NonNull<c_void> = NonNull::from(&*self.view).cast();
        let handle = rwh::UiKitWindowHandle::new(view);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl rwh::HasDisplayHandle for IosWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Ok(rwh::DisplayHandle::uikit())
    }
}

impl PlatformWindow for IosWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.inner.state.borrow().bounds
    }

    fn is_maximized(&self) -> bool {
        true
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Fullscreen(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.inner.state.borrow().bounds.size
    }

    fn resize(&mut self, _size: Size<Pixels>) {
        log::error!("IosWindow::resize is not supported on iOS");
    }

    fn scale_factor(&self) -> f32 {
        self.inner.state.borrow().scale_factor
    }

    fn appearance(&self) -> WindowAppearance {
        // TODO(ios): read the trait collection's user interface style.
        WindowAppearance::Light
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.display.clone())
    }

    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }

    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }

    fn capslock(&self) -> Capslock {
        Capslock::default()
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.inner.state.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.inner.state.borrow_mut().input_handler.take()
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        // TODO(ios): implement with UIAlertController.
        None
    }

    fn activate(&self) {
        self.ui_window.makeKeyAndVisible();
    }

    fn is_active(&self) -> bool {
        self.ui_window.isKeyWindow()
    }

    fn is_hovered(&self) -> bool {
        false
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }

    fn set_title(&mut self, _title: &str) {}

    fn set_background_appearance(&self, _background: WindowBackgroundAppearance) {}

    fn minimize(&self) {
        log::error!("IosWindow::minimize is not supported on iOS");
    }

    fn zoom(&self) {
        log::error!("IosWindow::zoom is not supported on iOS");
    }

    fn toggle_fullscreen(&self) {}

    fn is_fullscreen(&self) -> bool {
        true
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.inner.callbacks.borrow_mut().request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.inner.callbacks.borrow_mut().input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.inner.callbacks.borrow_mut().active_status_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.inner.callbacks.borrow_mut().hover_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.inner.callbacks.borrow_mut().resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.borrow_mut().moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.inner.callbacks.borrow_mut().should_close = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.inner.callbacks.borrow_mut().close = Some(callback);
    }

    fn on_hit_test_window_control(
        &self,
        callback: Box<dyn FnMut() -> Option<gpui::WindowControlArea>>,
    ) {
        self.inner.callbacks.borrow_mut().hit_test_window_control = Some(callback);
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner.callbacks.borrow_mut().appearance_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        self.inner.state.borrow_mut().renderer.draw(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.inner.state.borrow().renderer.sprite_atlas().clone()
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        self.inner
            .state
            .borrow()
            .renderer
            .supports_dual_source_blending()
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        Some(self.inner.state.borrow().renderer.gpu_specs())
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
}
