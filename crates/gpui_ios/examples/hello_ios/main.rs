//! Hello-world example for GPUI on iOS.
//!
//! There is no touch input support yet, so this example proves out the render
//! loop, executors, and text rendering instead: a clock driven by a spawned
//! task, over an animated background.

use gpui::prelude::*;
use gpui::{App, Context, SharedString, Task, Window, WindowOptions, div, hsla, rgb};
use std::time::Duration;

struct HelloIos {
    seconds_running: usize,
    _tick_task: Task<()>,
}

impl HelloIos {
    fn new(cx: &mut Context<Self>) -> Self {
        let tick_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    this.seconds_running += 1;
                    cx.notify();
                }) else {
                    break;
                };
            }
        });

        Self {
            seconds_running: 0,
            _tick_task: tick_task,
        }
    }
}

impl Render for HelloIos {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let hue = (self.seconds_running % 60) as f32 / 60.0;
        let uptime: SharedString = format!(
            "{:02}:{:02}",
            self.seconds_running / 60,
            self.seconds_running % 60
        )
        .into();

        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(hsla(hue, 0.35, 0.15, 1.0))
            .child(
                div()
                    .text_2xl()
                    .text_color(rgb(0xffffff))
                    .child("Hello from GPUI on iOS!"),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .rounded_lg()
                    .bg(hsla(hue, 0.5, 0.3, 1.0))
                    .text_color(rgb(0xffffff))
                    .child(uptime),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(hsla(0.0, 0.0, 1.0, 0.6))
                    .child(format!("rendering at {}x scale", window.scale_factor())),
            )
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(HelloIos::new))
            .expect("failed to open window");
        cx.activate(true);
    });
}
