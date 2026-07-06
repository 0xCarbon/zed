use crate::dispatcher::IosDispatcher;
use crate::display::IosDisplay;
use crate::keyboard::IosKeyboardLayout;
use crate::window::IosWindow;
use anyhow::Result;
use futures::channel::oneshot;
use gpui::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, DummyKeyboardMapper,
    ForegroundExecutor, Keymap, Menu, MenuItem, PathPromptOptions, Platform, PlatformDisplay,
    PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Task,
    ThermalState, WindowAppearance, WindowParams,
};
use gpui_wgpu::GpuContext;
use objc2::runtime::NSObjectProtocol;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSDictionary, NSObject, NSString};
use objc2_ui_kit::UIApplication;
use std::{
    borrow::Cow,
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

static BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBoldItalic.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Bold.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Italic.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-BoldItalic.ttf"),
];

thread_local! {
    // `UIApplicationMain` instantiates the delegate class itself, so the
    // launch callback is smuggled to `application:didFinishLaunchingWithOptions:`
    // through this thread-local rather than through the delegate instance.
    static FINISH_LAUNCHING: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GPUIApplicationDelegate"]
    struct ApplicationDelegate;

    unsafe impl NSObjectProtocol for ApplicationDelegate {}

    impl ApplicationDelegate {
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        fn application_did_finish_launching(
            &self,
            _application: &UIApplication,
            _launch_options: Option<&NSDictionary>,
        ) -> bool {
            if let Some(callback) = FINISH_LAUNCHING.take() {
                callback();
            }
            true
        }
    }
);

pub struct IosPlatform {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    gpu_context: GpuContext,
    active_window: RefCell<Option<AnyWindowHandle>>,
    callbacks: RefCell<IosPlatformCallbacks>,
}

#[derive(Default)]
struct IosPlatformCallbacks {
    open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
    quit: Option<Box<dyn FnMut()>>,
    reopen: Option<Box<dyn FnMut()>>,
    app_menu_action: Option<Box<dyn FnMut(&dyn Action)>>,
    will_open_app_menu: Option<Box<dyn FnMut()>>,
    validate_app_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    keyboard_layout_change: Option<Box<dyn FnMut()>>,
    thermal_state_change: Option<Box<dyn FnMut()>>,
}

impl IosPlatform {
    pub fn new(_headless: bool) -> Self {
        let dispatcher = Arc::new(IosDispatcher::new());
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher);

        let text_system = Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
            "IBM Plex Sans",
        ));
        let fonts = BUNDLED_FONTS
            .iter()
            .map(|bytes| Cow::Borrowed(*bytes))
            .collect();
        if let Err(error) = text_system.add_fonts(fonts) {
            log::error!("failed to load bundled fonts: {error:#}");
        }

        Self {
            background_executor,
            foreground_executor,
            text_system,
            gpu_context: Rc::new(RefCell::new(None)),
            active_window: RefCell::new(None),
            callbacks: RefCell::new(IosPlatformCallbacks::default()),
        }
    }
}

impl Platform for IosPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        let main_thread =
            MainThreadMarker::new().expect("IosPlatform::run must be called on the main thread");
        FINISH_LAUNCHING.set(Some(on_finish_launching));

        let delegate_class_name = NSString::from_class(ApplicationDelegate::class());
        UIApplication::main(None, Some(&delegate_class_name), main_thread);
    }

    fn quit(&self) {
        // iOS applications are terminated by the system, never by themselves.
        log::error!("IosPlatform::quit is not supported on iOS");
    }

    fn restart(&self, _binary_path: Option<PathBuf>) {
        panic!("restart is not supported on iOS");
    }

    fn activate(&self, _ignoring_other_apps: bool) {}

    fn hide(&self) {
        log::error!("IosPlatform::hide is not supported on iOS");
    }

    fn hide_other_apps(&self) {
        panic!("hide_other_apps is not supported on iOS");
    }

    fn unhide_other_apps(&self) {
        panic!("unhide_other_apps is not supported on iOS");
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        match IosDisplay::primary() {
            Ok(display) => vec![Rc::new(display)],
            Err(error) => {
                log::error!("failed to read the main screen: {error:#}");
                Vec::new()
            }
        }
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        match IosDisplay::primary() {
            Ok(display) => Some(Rc::new(display)),
            Err(error) => {
                log::error!("failed to read the main screen: {error:#}");
                None
            }
        }
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        *self.active_window.borrow()
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        params: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let window = IosWindow::new(
            handle,
            params,
            self.gpu_context.clone(),
            self.foreground_executor.clone(),
            self.background_executor.clone(),
        )?;
        *self.active_window.borrow_mut() = Some(handle);
        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        // TODO(ios): read UITraitCollection's user interface style.
        WindowAppearance::Light
    }

    fn open_url(&self, url: &str) {
        let Some(main_thread) = MainThreadMarker::new() else {
            log::error!("open_url must be called on the main thread");
            return;
        };
        let Some(url) = objc2_foundation::NSURL::URLWithString(&NSString::from_str(url)) else {
            log::error!("failed to parse URL");
            return;
        };
        let application = UIApplication::sharedApplication(main_thread);
        #[allow(deprecated)]
        let opened: bool = unsafe { msg_send![&application, openURL: &*url] };
        if !opened {
            log::error!("failed to open URL");
        }
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.callbacks.borrow_mut().open_urls = Some(callback);
    }

    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow::anyhow!(
            "register_url_scheme is not supported on iOS; declare URL schemes in Info.plist"
        )))
    }

    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        tx.send(Err(anyhow::anyhow!(
            "prompt_for_paths is not implemented on iOS"
        )))
        .ok();
        rx
    }

    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        tx.send(Err(anyhow::anyhow!(
            "prompt_for_new_path is not implemented on iOS"
        )))
        .ok();
        rx
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    fn reveal_path(&self, _path: &Path) {
        log::error!("IosPlatform::reveal_path is not supported on iOS");
    }

    fn open_with_system(&self, _path: &Path) {
        log::error!("IosPlatform::open_with_system is not implemented on iOS");
    }

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().quit = Some(callback);
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().reopen = Some(callback);
    }

    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.callbacks.borrow_mut().app_menu_action = Some(callback);
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().will_open_app_menu = Some(callback);
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.callbacks.borrow_mut().validate_app_menu_command = Some(callback);
    }

    fn thermal_state(&self) -> ThermalState {
        // TODO(ios): read NSProcessInfo.thermalState.
        ThermalState::Nominal
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().thermal_state_change = Some(callback);
    }

    fn compositor_name(&self) -> &'static str {
        "UIKit"
    }

    fn app_path(&self) -> Result<PathBuf> {
        Err(anyhow::anyhow!("app_path is not supported on iOS"))
    }

    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow::anyhow!(
            "path_for_auxiliary_executable is not supported on iOS"
        ))
    }

    fn set_cursor_style(&self, _style: CursorStyle) {}

    fn hide_cursor_until_mouse_moves(&self) {}

    fn is_cursor_visible(&self) -> bool {
        false
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        true
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        // TODO(ios): implement with UIPasteboard.
        None
    }

    fn write_to_clipboard(&self, _item: ClipboardItem) {
        // TODO(ios): implement with UIPasteboard.
        log::error!("IosPlatform::write_to_clipboard is not implemented on iOS yet");
    }

    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        // TODO(ios): implement with the Keychain.
        Task::ready(Err(anyhow::anyhow!(
            "credential storage is not implemented on iOS yet"
        )))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow::anyhow!(
            "credential storage is not implemented on iOS yet"
        )))
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(IosKeyboardLayout)
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().keyboard_layout_change = Some(callback);
    }
}
