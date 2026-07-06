# GPUI iOS Support — Design Synthesis & Handoff

This document captures the decisions, design principles, and current state of the
initial iOS platform work, so that future sessions (human or agent) can pick up
where this one left off.

## Context

Goal: add iOS support to GPUI (in-tree) so Delta — and eventually other GPUI
apps — can run on a phone. This branch contains:

1. `gpui_ios: Add work-in-progress iOS platform`
2. `gpui: Add first-class touch input events`

Status: a GPUI app builds, installs, and runs in the iOS simulator with working
rendering, text, executors, and raw multi-touch input (`script/ios-example`).

## Architectural facts (load-bearing for all future work)

- GPUI platforms are separate crates (`gpui_macos`, `gpui_linux`, `gpui_windows`,
  `gpui_web`), selected by `current_platform()` in `gpui_platform`. iOS is the new
  crate `crates/gpui_ios`, wired in with one `#[cfg(target_os = "ios")]` arm.
  No cfg-gating inside `gpui` core.
- `gpui` core and `gpui_wgpu` already compiled cleanly for
  `aarch64-apple-ios{,-sim}` before this work started. The
  [gpui-mobile](https://github.com/itsbalamurali/gpui-mobile) author previously
  upstreamed mobile surface-lifecycle hooks into `gpui_wgpu`
  (`unconfigure_surface`/`replace_surface`, PR #50815) — use these for
  backgrounding/rotation later.
- `gpui_web` is the template for new platforms: smallest `Platform` impl, wgpu
  renderer + `CosmicTextSystem` with bundled fonts from `assets/fonts/`, and a
  standalone example app excluded from the workspace via its own `[workspace]`
  table plus `autoexamples = false` on the parent crate.
- The macOS GCD dispatcher is portable to iOS nearly verbatim (`dispatch2`
  crate); `gpui_ios` copies it minus the mach realtime-thread policies (TODO).
- gpui-mobile is tri-licensed including Apache-2.0, so it is a legal reference;
  prefer coordinating with its author over silently duplicating their work.

## Key decisions and rationale

### 1. Renderer: `gpui_wgpu` (Metal via wgpu), not the native `metal_renderer`

Path of least resistance; same stack as web; validated by gpui-mobile. One fix
was needed: `WgpuContext::instance()` hardcoded `VULKAN | GL` backends — it now
selects `METAL` on `macos`/`ios`. Extracting the native Metal renderer from
`gpui_macos` is a possible later optimization, not a blocker.

### 2. App model: the OS owns lifecycle and window ("the inversion")

`IosPlatform::run` stashes `on_finish_launching` in a thread-local, defines a
`UIApplicationDelegate` via `objc2::define_class!`, and hands control to
`UIApplication::main` (which never returns). `open_window` binds the one
conceptual window: it creates a `UIWindow` + root view controller + a
`MetalView` whose `layerClass` is `CAMetalLayer`, so wgpu renders into the view
directly (no sublayer resize hazards). Frames are driven by a fixed 16.6ms
foreground-executor timer — TODO: `CADisplayLink`.

### 3. Desktop-shaped `Platform` APIs: three-tier policy

- **Tier 1 — must work** (called unconditionally by gpui core): executors, text
  system, displays, keyboard mapper, `open_window`, `window_appearance`.
- **Tier 2 — plausibly called by shared app code** (`quit`, `hide`, `activate`,
  `set_menus`): `log::error!` or no-op. Never crash the phone for a benign call
  from code shared with desktop.
- **Tier 3 — genuinely nonsensical on mobile** (`restart`, `hide_other_apps`,
  `unhide_other_apps`): `panic!` with clear messages. Loud failure is a feature
  during the WIP phase; downgrade deliberately as real code paths are found.

Specialized mobile APIs (scene connect, enter background/foreground, safe-area
insets) should be designed once for iOS *and* Android — Android's
`InitWindow`/`TerminateWindow` has the same shape. A type-safe capability split
of the `Platform` trait is desired but deferred; GPUI cannot express it today
without a large refactor.

### 4. Touch: first-class events, no mouse synthesis (firm decision)

Touch and mouse UIs are not compatible. Synthesizing mouse events from touches
is the source of historical web jank (stuck hover states, tap delays) and
exists only to support legacy content GPUI does not have. **Never fake
`MouseDown`/`MouseMove`/hover/cursor from touches.**

The nuance: some existing GPUI events are already semantic rather than
mouse-mechanical. `ScrollWheelEvent` is really "scroll by delta" (it already
carries a `TouchPhase` from macOS trackpads) and `ClickEvent` is an enum of
input sources (`Mouse | Keyboard`). Recognized gestures emitting those events
(pan → scroll, tap → a new `ClickEvent` touch variant) is the sanctioned path,
so existing components (`on_click`, scroll containers) keep working untouched.

### 5. Touch abstraction survey → chosen model

Surveyed: web Touch Events (the bad old API), web Pointer Events (best raw data
shape), React Native's Gesture Responder System (**do not copy** — RN's own
community abandoned it for react-native-gesture-handler, and RN is migrating to
Pointer Events), UIKit recognizers (gold standard for feel, too ObjC-shaped to
copy), Android `MotionEvent`, and Flutter's pointer events + gesture arena
(**best portable architecture**, built for exactly this problem). Result:

- **Raw layer (done):** `PlatformInput::Touch(TouchEvent { id: TouchId, phase,
  position, force: Option<f32> })`, shaped so a future mouse/touch/pen
  "pointer" unification is a rename rather than a redesign. `TouchPhase` gained
  `Cancelled` — mandatory, because iOS steals touches for system gestures and
  incoming calls.
- **Gesture layer (next):** a Flutter-style arena in gpui core (portable to
  Android for free). Recognizers are registered by elements, winners claim
  touches, losers receive cancellation. Disambiguation (tap-vs-scroll slop,
  delays) lives in exactly one place. Write a short design doc before building.

### 6. Touch dispatch semantics (implemented in gpui core)

- A dispatch path fully separate from mouse: touches never update
  `mouse_position`, hover state, the hit-test cache, or cursor style.
- **Implicit capture:** a touch is hit-tested once at `Started`
  (occlusion-aware, reusing `Frame::hit_test`); all subsequent events for that
  `TouchId` are delivered to the elements under the starting position, even
  after the finger moves outside them. State lives in
  `Window::active_touches: FxHashMap<TouchId, ActiveTouch>` and is removed on
  `Ended`/`Cancelled`. This matches the convergent semantics of UIKit, Android,
  and web pointer capture.
- Listener plumbing mirrors the mouse path: `Frame::touch_listeners` (with
  `PaintIndex.touch_listeners_index` and the `reuse_paint` extension — do not
  forget these when adding frame-scoped listener lists), `Window::on_touch_event`,
  capture-then-bubble over the flat listener list, the element API
  `InteractiveElement::on_touch` / `Interactivity::on_touch`, and the targeting
  query `Hitbox::contains_touch` / `HitboxId::contains_touch`.
- iOS side: `MetalView` overrides `touchesBegan/Moved/Ended/Cancelled:withEvent:`;
  `TouchId` is the `UITouch` object address (stable per touch); force is
  normalized by `maximumPossibleForce`; `multipleTouchEnabled` is set. The view
  reaches window state through a `define_class!` ivar holding
  `RefCell<Weak<IosWindowInner>>`, late-bound because the view must exist before
  the renderer and window state do.

### 7. IME/text input requirement (shapes the text-input milestone)

Mobile IME must be first-class: implement `UITextInput` on the view and attach
`UITextInteraction` (not a bare `UIKeyInput` shim) so UIKit provides the
magnifier, selection handles, floating cursor, dictation, and marked-text
composition — all bridged to GPUI's existing `PlatformInputHandler`, which
already has the right shape (`selected_text_range`, `marked_text_range`,
`replace_text_in_range`, `bounds_for_range`). `UITextSelectionDisplayInteraction`
(iOS 17+) lets UIKit own the selection chrome while GPUI owns text drawing.

## Build/run mechanics

- `script/ios-example [--release]` builds `crates/gpui_ios/examples/hello_ios`
  for `aarch64-apple-ios-sim`, assembles a minimal `.app` with a hand-written
  Info.plist, ad-hoc codesigns it, and boots/installs/launches it via `simctl`
  with console streaming. No Xcode project; xcodegen only becomes necessary for
  physical-device signing and entitlements.
- Info.plist details: `UILaunchScreen = {}` (empty dict) is required for
  full-screen rendering; there is intentionally no `UIApplicationSceneManifest`,
  so UIKit uses the legacy app-delegate lifecycle our delegate implements.
- The example is a standalone cargo workspace (its own `[workspace]` table,
  `autoexamples = false` on `gpui_ios`), mirroring `hello_web`.
- Gotchas already hit:
  - macOS ships bash 3.2, which cannot expand empty arrays under `set -u`.
  - `objc2` framework crates feature-gate every class; compile errors name the
    missing feature.
  - Use `UIApplication::main(...)`; the raw `UIApplicationMain` binding is
    deprecated and needs manual argc/argv.
  - `simctl launch --console-pty` blocks following logs (run with a timeout);
    verify rendering with `xcrun simctl io booted screenshot`.
  - Simulator multi-touch: ⌥ drag = pinch, ⌥⇧ drag = two parallel fingers.

## Verification state

`script/clippy -p gpui -p gpui_wgpu -p gpui_platform -p gpui_ios` clean
(including cargo-machete and typos); all gpui tests pass; `terminal` (the only
`TouchPhase::Cancelled` fallout) compiles; `gpui_ios` compiles to an empty lib
on non-iOS hosts (crate root is `#![cfg(target_os = "ios")]`, dependencies
target-gated).

## Roadmap (agreed order)

1. **Gesture layer in gpui core** (next; design-doc first): tap → `ClickEvent`
   touch variant, pan → scroll events with momentum, long-press, pinch →
   existing `PinchEvent`; arena-style disambiguation on top of
   `Window::on_touch_event`.
2. **Text input/IME:** `UITextInput` + `UITextInteraction` bridged to
   `PlatformInputHandler`; on-screen keyboard show/hide and avoidance.
3. **Lifecycle:** background/foreground → `unconfigure_surface`/`replace_surface`;
   rotation/resize; safe-area insets (needs a small `PlatformWindow` addition —
   GPUI has no concept yet); dark mode via the trait collection; `CADisplayLink`
   frame pacing; clipboard (UIPasteboard); Keychain credentials; thermal state
   (`NSProcessInfo`).
4. **Device builds:** xcodegen project + signing, then Delta on the phone
   (data dir → `NSDocumentDirectory`; reqwest works on iOS).
5. **CI:** add `cargo check -p gpui_ios --target aarch64-apple-ios` to the macOS
   job so the platform cannot rot.

## Open questions

- Eventual unification of mouse and touch into a pointer-event model in gpui
  core (north star; the `TouchEvent` shape anticipates it).
- A type-safe split of the `Platform` trait into per-capability traits, so
  desktop-only APIs are not callable on mobile at compile time.
- Whether the gesture arena should also mediate mouse-driven gestures
  (trackpad pinch/scroll) eventually, or remain touch-only.
- Android: most of the core-side work (touch dispatch, gesture layer, lifecycle
  API shape) is deliberately Android-ready; the platform crate itself is future
  work.
