//! Hello-world example for GPUI on iOS.
//!
//! Exercises the render loop, executors, text rendering, and raw touch input:
//! a clock driven by a spawned task, plus a multi-touch visualizer that draws
//! a dot under every active finger.

use gpui::prelude::*;
use gpui::{
    App, Context, Pixels, Point, SharedString, Task, TouchId, TouchPhase, Window, WindowOptions,
    div, hsla, px, rgb,
};
use std::collections::HashMap;
use std::time::Duration;

const DOT_SIZE: Pixels = px(88.);

struct ActiveTouch {
    position: Point<Pixels>,
    force: Option<f32>,
}

struct HelloIos {
    seconds_running: usize,
    touches: HashMap<TouchId, ActiveTouch>,
    touches_seen: usize,
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
            touches: HashMap::new(),
            touches_seen: 0,
            _tick_task: tick_task,
        }
    }
}

impl Render for HelloIos {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hue = (self.seconds_running % 60) as f32 / 60.0;
        let uptime: SharedString = format!(
            "{:02}:{:02}",
            self.seconds_running / 60,
            self.seconds_running % 60
        )
        .into();

        let status: SharedString = if self.touches.is_empty() {
            format!("touch the screen! ({} so far)", self.touches_seen).into()
        } else {
            format!("{} active touches", self.touches.len()).into()
        };

        let touch_dots = self.touches.iter().map(|(id, touch)| {
            let dot_hue = (id.0 % 7) as f32 / 7.0;
            let dot_size = DOT_SIZE * touch.force.map_or(1.0, |force| 0.5 + force);
            div()
                .absolute()
                .left(touch.position.x - dot_size / 2.0)
                .top(touch.position.y - dot_size / 2.0)
                .size(dot_size)
                .rounded_full()
                .bg(hsla(dot_hue, 0.8, 0.6, 0.7))
        });

        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(hsla(hue, 0.35, 0.15, 1.0))
            .on_touch(cx.listener(|this, event: &gpui::TouchEvent, _window, cx| {
                match event.phase {
                    TouchPhase::Started => {
                        this.touches_seen += 1;
                        this.touches.insert(
                            event.id,
                            ActiveTouch {
                                position: event.position,
                                force: event.force,
                            },
                        );
                    }
                    TouchPhase::Moved => {
                        if let Some(touch) = this.touches.get_mut(&event.id) {
                            touch.position = event.position;
                            touch.force = event.force;
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        this.touches.remove(&event.id);
                    }
                }
                cx.notify();
            }))
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
                    .child(status),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(hsla(0.0, 0.0, 1.0, 0.6))
                    .child(format!("rendering at {}x scale", window.scale_factor())),
            )
            .children(touch_dots)
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
