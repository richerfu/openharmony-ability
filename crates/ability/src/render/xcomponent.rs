use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use napi_ohos::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking;
use napi_ohos::{Env, Error, Result};
use ohos_arkui_binding::component::attribute::{ArkUICommonAttribute, ArkUIGesture};
use ohos_arkui_binding::gesture::{gesture_data::GestureData, inner_gesture::Gesture};
use ohos_arkui_binding::types::{
    gesture_direction::GestureDirection, gesture_event::GestureEventAction,
};
use ohos_arkui_binding::{ArkUIHandle, RootNode, XComponent};
use ohos_ime_binding::IME;
use ohos_xcomponent_binding::{XComponentOffset, XComponentSize};

use crate::{
    input, set_main_thread_env, ArkUiInputEvent, AxisEventData, Event, GestureEvent, GesturePhase,
    InputEvent, IntervalInfo, OpenHarmonyApp, PanGestureEvent, PointerInputData, Rect, Size,
    SwipeGestureEvent, TapGestureEvent, XComponentInputEvent,
};

const PAN_GESTURE_DISTANCE: f64 = 8.0;
const SWIPE_GESTURE_MIN_SPEED: f64 = 100.0;
static NEXT_XCOMPONENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct PanDeltaTracker {
    previous_offset: Option<(f32, f32)>,
}

struct RenderOwnerGuard {
    app: OpenHarmonyApp,
    owner: String,
    armed: bool,
}

impl RenderOwnerGuard {
    fn new(app: OpenHarmonyApp, owner: String) -> Self {
        Self {
            app,
            owner,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RenderOwnerGuard {
    fn drop(&mut self) {
        if self.armed {
            self.app.release_render(&self.owner);
        }
    }
}

impl PanDeltaTracker {
    fn next(&mut self, phase: GesturePhase, offset_x: f32, offset_y: f32) -> (f32, f32) {
        if phase == GesturePhase::Cancel {
            self.previous_offset = None;
            return (0.0, 0.0);
        }

        let (previous_x, previous_y) = self.previous_offset.unwrap_or_default();
        let delta = (offset_x - previous_x, offset_y - previous_y);
        self.previous_offset = if phase == GesturePhase::End {
            None
        } else {
            Some((offset_x, offset_y))
        };
        delta
    }
}

fn gesture_phase(action: &GestureEventAction) -> Option<GesturePhase> {
    if action.contains(GestureEventAction::Accept) {
        Some(GesturePhase::Start)
    } else if action.contains(GestureEventAction::Update) {
        Some(GesturePhase::Update)
    } else if action.contains(GestureEventAction::End) {
        Some(GesturePhase::End)
    } else if action.contains(GestureEventAction::Cancel) {
        Some(GesturePhase::Cancel)
    } else {
        None
    }
}

fn dispatch_input(app: &OpenHarmonyApp, owner: &str, event: InputEvent) {
    if !app.is_render_surface_active(owner) {
        return;
    }
    if let Some(ref mut handler) = *app.event_loop.borrow_mut() {
        match app.render_window_id(owner) {
            Some(0) => handler(Event::Input(event)),
            Some(window_id) => handler(Event::SubWindowInput { window_id, event }),
            None => {}
        }
    }
}

fn release_gestures(xcomponent: &XComponent, gestures: &mut Vec<Gesture>) {
    for gesture in gestures.drain(..) {
        let _ = xcomponent.remove_gesture(&gesture);
        let _ = gesture.dispose();
    }
}

fn register_gestures(
    xcomponent: &XComponent,
    render_owner: &str,
    app: &OpenHarmonyApp,
) -> Result<Vec<Gesture>> {
    let mut gestures = Vec::with_capacity(3);

    let tap_app = app.clone();
    let tap_owner = render_owner.to_owned();
    let tap = match Gesture::create_tap_gesture_with_distance_threshold(1, 1, PAN_GESTURE_DISTANCE)
    {
        Ok(tap) => tap,
        Err(_) => Gesture::create_tap_gesture(1, 1)
            .map_err(|error| Error::from_reason(error.reason.to_string()))?,
    };
    if let Err(error) = tap.on_gesture(GestureEventAction::Accept, move |event| {
        let Some(pointer) = event.input.map(PointerInputData::from) else {
            return;
        };
        dispatch_input(
            &tap_app,
            &tap_owner,
            InputEvent::ArkUi(ArkUiInputEvent::Gesture(GestureEvent::Tap(
                TapGestureEvent { pointer },
            ))),
        );
    }) {
        tap.dispose()
            .map_err(|dispose_error| Error::from_reason(dispose_error.reason.to_string()))?;
        return Err(Error::from_reason(error.reason.to_string()));
    }
    if let Err(error) = xcomponent.add_gesture_ref(&tap, None, None) {
        tap.dispose()
            .map_err(|dispose_error| Error::from_reason(dispose_error.reason.to_string()))?;
        return Err(Error::from_reason(error.reason.to_string()));
    }
    gestures.push(tap);

    let pan_app = app.clone();
    let pan_owner = render_owner.to_owned();
    let pan_tracker = Rc::new(RefCell::new(PanDeltaTracker::default()));
    let pan = match xcomponent.on_pan_gesture(
        1,
        GestureDirection::All,
        PAN_GESTURE_DISTANCE,
        move |event| {
            let Some(phase) = gesture_phase(&event.event_action_type) else {
                return;
            };
            let Some(pointer) = event.input.map(PointerInputData::from) else {
                return;
            };
            let GestureData::Pan(data) = event.event_action_data else {
                return;
            };
            let (delta_x, delta_y) =
                pan_tracker
                    .borrow_mut()
                    .next(phase, data.offset_x, data.offset_y);
            dispatch_input(
                &pan_app,
                &pan_owner,
                InputEvent::ArkUi(ArkUiInputEvent::Gesture(GestureEvent::Pan(
                    PanGestureEvent {
                        pointer,
                        phase,
                        delta_x,
                        delta_y,
                        offset_x: data.offset_x,
                        offset_y: data.offset_y,
                        velocity: data.velocity,
                        velocity_x: data.velocity_x,
                        velocity_y: data.velocity_y,
                    },
                ))),
            );
        },
    ) {
        Ok(pan) => pan,
        Err(error) => {
            release_gestures(xcomponent, &mut gestures);
            return Err(Error::from_reason(error.reason.to_string()));
        }
    };
    gestures.push(pan);

    let swipe_app = app.clone();
    let swipe_owner = render_owner.to_owned();
    let swipe = match xcomponent.on_swipe_gesture(
        1,
        GestureDirection::All,
        SWIPE_GESTURE_MIN_SPEED,
        move |event| {
            let Some(phase) = gesture_phase(&event.event_action_type) else {
                return;
            };
            let Some(pointer) = event.input.map(PointerInputData::from) else {
                return;
            };
            let GestureData::Swipe(data) = event.event_action_data else {
                return;
            };
            dispatch_input(
                &swipe_app,
                &swipe_owner,
                InputEvent::ArkUi(ArkUiInputEvent::Gesture(GestureEvent::Swipe(
                    SwipeGestureEvent {
                        pointer,
                        phase,
                        angle: data.angle,
                        velocity: data.velocity,
                    },
                ))),
            );
        },
    ) {
        Ok(swipe) => swipe,
        Err(error) => {
            release_gestures(xcomponent, &mut gestures);
            return Err(Error::from_reason(error.reason.to_string()));
        }
    };
    gestures.push(swipe);

    Ok(gestures)
}

/// create lifecycle object and return to arkts
pub fn render(
    env: &Env,
    slot: ArkUIHandle,
    render_owner: String,
    app: OpenHarmonyApp,
) -> Result<RootNode> {
    render_for_window(env, slot, render_owner, app, 0)
}

pub fn render_for_window(
    env: &Env,
    slot: ArkUIHandle,
    render_owner: String,
    app: OpenHarmonyApp,
    window_id: i64,
) -> Result<RootNode> {
    set_main_thread_env(*env);

    // All eager `get_helper()` TSFN inits (statusbar tray, vibrancy, clipboard image,
    // updater, account, autostart, opener) have been removed. They were all dead
    // paths: `set_helper` is never called after the #[ability] derive refactor, so
    // every `get_helper()` returned None and these inits logged "ArkHelper not
    // initialized" on every startup (noise, not functional bugs). Consumers have
    // migrated to typed bridge facades (WindowClient / ClipboardClient /
    // AutostartClient / plugin-statusbar / plugin-menu) or still call legacy
    // top-level fns directly (opener::reveal_in_dir — see decoupling task #6
    // B-group; the updater / account facades live in the dedicated
    // plugin-updater / plugin-account crates). Do NOT re-add eager `get_helper()`
    // TSFN inits here — new capability goes through bridge plugins.

    let mut root = RootNode::new(slot);
    let xcomponent_native =
        XComponent::new().map_err(|e| Error::from_reason(e.reason.to_string()))?;
    // The native binding's multi_mode registry is keyed by XComponentId.
    // ArkUI-created native nodes do not get a distinct id automatically, so
    // every surface must be named before obtaining its native handle.
    crate::warn!("native XComponent before id: {:?}", xcomponent_native.native_xcomponent().id());
    xcomponent_native
        .set_x_component_id(format!(
            "ability-xc-{}",
            NEXT_XCOMPONENT_ID.fetch_add(1, Ordering::Relaxed)
        ))
        .map_err(|e| {
            crate::warn!("native XComponent set id failed: {e:?}");
            Error::from_reason(e.reason.to_string())
        })?;
    crate::warn!("native XComponent after id: {:?}", xcomponent_native.native_xcomponent().id());
    xcomponent_native
        .background_color(0x0000_0000)
        .map_err(|e| Error::from_reason(e.reason.to_string()))?;

    let xcomponent = xcomponent_native.native_xcomponent();

    let touch_input_delivery =
        app.begin_render(&render_owner, window_id, xcomponent_native.clone())?;
    let mut render_guard = RenderOwnerGuard::new(app.clone(), render_owner.clone());

    let xc = xcomponent.clone();

    let on_surface_created_app = app.clone();
    let on_surface_created_owner = render_owner.clone();
    let redraw_app = app.clone();

    let (
        insert_text_callback_tsfn,
        on_ime_hide_callback_tsfn,
        on_backspace_callback_tsfn,
        on_ime_enter_callback_tsfn,
    ) = input::ime_ts_fn(env, app.clone(), render_owner.clone())?;
    let insert_text_callback_tsfn = Arc::new(insert_text_callback_tsfn);
    let on_ime_hide_callback_tsfn = Arc::new(on_ime_hide_callback_tsfn);
    let on_backspace_callback_tsfn = Arc::new(on_backspace_callback_tsfn);
    let on_ime_enter_callback_tsfn = Arc::new(on_ime_enter_callback_tsfn);

    xcomponent.on_surface_created(move |xc_raw, win| {
        // NDK callback boundary: a panic here aborts the process (no unwinding
        // across "extern C"). Degrade to 0x0 + warn instead (issue #87 minor-2);
        // a subsequent on_surface_changed carries the real geometry.
        let size = xc_raw.size(win).unwrap_or_else(|e| {
            crate::warn!("on_surface_created: size() failed: {e:?}, degrading to 0x0");
            XComponentSize {
                width: 0,
                height: 0,
            }
        });
        let offset = xc_raw.offset(win).unwrap_or_else(|e| {
            crate::warn!("on_surface_created: offset() failed: {e:?}, degrading to 0,0");
            XComponentOffset { x: 0.0, y: 0.0 }
        });
        let rect = Rect {
            top: offset.y as _,
            left: offset.x as _,
            width: size.width as _,
            height: size.height as _,
        };
        if !on_surface_created_app.activate_render_surface(
            &on_surface_created_owner,
            xc.native_window(),
            rect,
        ) {
            return Ok(());
        }

        // We need to create IME instance when app is focused.
        let ime = IME::new(Default::default());

        {
            let insert_text_callback_tsfn = insert_text_callback_tsfn.clone();
            let on_ime_hide_callback_tsfn = on_ime_hide_callback_tsfn.clone();
            let on_backspace_callback_tsfn = on_backspace_callback_tsfn.clone();
            let on_ime_enter_callback_tsfn = on_ime_enter_callback_tsfn.clone();

            // // run in other thread
            ime.insert_text(move |s| {
                insert_text_callback_tsfn.call(s, NonBlocking);
            });
            ime.on_status_change(move |s| {
                on_ime_hide_callback_tsfn.call(s.into(), NonBlocking);
            });
            ime.on_backspace(move |len| {
                on_backspace_callback_tsfn.call(len, NonBlocking);
            });
            ime.on_enter(move |key| {
                on_ime_enter_callback_tsfn.call(key as i32, NonBlocking);
            });
        }

        if window_id == 0 {
            *on_surface_created_app.ime.borrow_mut() = Some(ime);
        } else {
            on_surface_created_app
                .sub_ime
                .borrow_mut()
                .insert(window_id, ime);
        }

        {
            if let Some(ref mut h) = *on_surface_created_app.event_loop.borrow_mut() {
                if window_id == 0 {
                    h(Event::SurfaceCreate)
                } else {
                    h(Event::SubWindowSurfaceCreate(window_id))
                }
            }
        }

        let inner_redraw_app = redraw_app.clone();
        let inner_redraw_owner = on_surface_created_owner.clone();
        xc.on_frame_callback(move |_xcomponent, _time, _time_stamp| {
            if !inner_redraw_app.is_render_surface_active(&inner_redraw_owner) {
                return Ok(());
            }
            if let Some(ref mut h) = *inner_redraw_app.event_loop.borrow_mut() {
                let interval = IntervalInfo {
                    time_stamp: _time_stamp as _,
                    target_time_stamp: _time as _,
                };
                if window_id == 0 {
                    h(Event::WindowRedraw(interval))
                } else {
                    h(Event::SubWindowRedraw {
                        window_id,
                        interval,
                    })
                }
            }
            Ok(())
        })?;
        Ok(())
    });

    let on_surface_destroyed_app = app.clone();
    let on_surface_destroyed_owner = render_owner.clone();
    xcomponent.on_surface_destroyed(move |_, _| {
        if on_surface_destroyed_app.deactivate_render_surface(&on_surface_destroyed_owner) {
            if window_id == 0 {
                on_surface_destroyed_app.dispatch_surface_destroy();
            } else if let Some(ref mut handler) = *on_surface_destroyed_app.event_loop.borrow_mut()
            {
                handler(Event::SubWindowSurfaceDestroy(window_id));
            }
        }
        Ok(())
    });

    let on_surface_changed_app = app.clone();
    let on_surface_changed_owner = render_owner.clone();
    xcomponent.on_surface_changed(move |xc, win| {
        // NDK callback boundary: never panic (issue #87 minor-2). If the geometry
        // can't be read, skip this change event rather than dispatching a bogus
        // 0x0 WindowResize into tao — the next surface-changed callback carries
        // the real values.
        let size = match xc.size(win) {
            Ok(size) => size,
            Err(e) => {
                crate::warn!("on_surface_changed: size() failed: {e:?}, skipping rect update");
                return Ok(());
            }
        };
        let offset = match xc.offset(win) {
            Ok(offset) => offset,
            Err(e) => {
                crate::warn!("on_surface_changed: offset() failed: {e:?}, skipping rect update");
                return Ok(());
            }
        };
        if on_surface_changed_app.update_render_surface_rect(
            &on_surface_changed_owner,
            Rect {
                top: offset.y as _,
                left: offset.x as _,
                width: size.width as _,
                height: size.height as _,
            },
        ) {
            if let Some(ref mut h) = *on_surface_changed_app.event_loop.borrow_mut() {
                // Phase 3 (design.md D6 / task 3.5): XComponent is the main window's
                // surface, so window_id is always 0. Carrying it explicitly lets tao's
                // run_loop route WindowResize uniformly (instead of a special-cased ZST).
                h(Event::WindowResize {
                    window_id,
                    size: Size {
                        width: size.width as _,
                        height: size.height as _,
                    },
                })
            }
        }
        Ok(())
    });

    if touch_input_delivery.delivers_raw_touch() {
        let on_touch_event_app = app.clone();
        let on_touch_event_owner = render_owner.clone();
        xcomponent.on_touch_event(move |_, _, data| {
            dispatch_input(
                &on_touch_event_app,
                &on_touch_event_owner,
                InputEvent::XComponent(XComponentInputEvent::Touch(data)),
            );
            Ok(())
        });
    }

    let on_key_event_app = app.clone();
    let on_key_event_owner = render_owner.clone();
    let _ = xcomponent.on_key_event(move |_, _, data| {
        dispatch_input(
            &on_key_event_app,
            &on_key_event_owner,
            InputEvent::XComponent(XComponentInputEvent::Key(data)),
        );
        Ok(())
    });

    let on_mouse_event_app = app.clone();
    let on_mouse_event_owner = render_owner.clone();
    xcomponent.on_mouse_event(move |_, _, data| {
        dispatch_input(
            &on_mouse_event_app,
            &on_mouse_event_owner,
            InputEvent::XComponent(XComponentInputEvent::Mouse(data)),
        );
        Ok(())
    })?;
    xcomponent.register_mouse_event_callback()?;

    let on_axis_event_app = app.clone();
    let on_axis_event_owner = render_owner.clone();
    xcomponent.on_ui_input_event(move |_, data| {
        let event = AxisEventData {
            pointer: PointerInputData::from_arkui_event(&data),
            delta_x: data.get_scroll_delta_x().unwrap_or_default(),
            delta_y: data.get_scroll_delta_y().unwrap_or_default(),
        };
        dispatch_input(
            &on_axis_event_app,
            &on_axis_event_owner,
            InputEvent::ArkUi(ArkUiInputEvent::Axis(event)),
        );
        Ok(())
    })?;

    if touch_input_delivery.delivers_arkui_gestures() {
        let gestures = register_gestures(&xcomponent_native, &render_owner, &app)?;
        app.set_render_gestures(&render_owner, gestures)?;
    }

    xcomponent.register_callback()?;

    root.mount(xcomponent_native)
        .map_err(|error| Error::from_reason(error.reason.to_string()))?;
    render_guard.disarm();

    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_delta_tracker_converts_cumulative_offsets() {
        let mut tracker = PanDeltaTracker::default();

        assert_eq!(tracker.next(GesturePhase::Start, 3.0, 5.0), (3.0, 5.0));
        assert_eq!(tracker.next(GesturePhase::Update, 7.0, 4.0), (4.0, -1.0));
        assert_eq!(tracker.next(GesturePhase::End, 9.0, 10.0), (2.0, 6.0));
        assert_eq!(tracker.next(GesturePhase::Start, 1.0, 2.0), (1.0, 2.0));
    }

    #[test]
    fn cancelled_pan_resets_delta_state() {
        let mut tracker = PanDeltaTracker::default();
        tracker.next(GesturePhase::Start, 4.0, 8.0);

        assert_eq!(tracker.next(GesturePhase::Cancel, 6.0, 9.0), (0.0, 0.0));
        assert_eq!(tracker.next(GesturePhase::Start, 2.0, 3.0), (2.0, 3.0));
    }
}
