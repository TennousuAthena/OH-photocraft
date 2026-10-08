use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect, Vec2};

const MAX_EVENTS: usize = 4096;

/// NDK coordinates are converted to logical points by the native bridge.
#[derive(Default)]
pub struct InputState {
    events: Vec<Event>,
    modifiers: Modifiers,
    down: Vec<Key>,
    pointer: Pos2,
    buttons: Vec<PointerButton>,
}

impl InputState {
    /// Inject native IME and explicit clipboard events into the bounded frame queue.
    pub fn event(&mut self, event: Event) {
        self.push(event);
    }
    fn push(&mut self, event: Event) {
        if self.events.len() < MAX_EVENTS {
            self.events.push(event);
        }
    }

    pub fn pointer(&mut self, x: f32, y: f32, action: i32, button: i32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.pointer = Pos2::new(x, y);
        if action == 3 || action == 4 {
            for button in std::mem::take(&mut self.buttons) {
                self.push(Event::PointerButton {
                    pos: self.pointer,
                    button,
                    pressed: false,
                    modifiers: self.modifiers,
                });
            }
            self.push(Event::PointerGone);
            return;
        }
        self.push(Event::PointerMoved(self.pointer));
        if action == 0 || action == 1 {
            let button = match button {
                2 => PointerButton::Secondary,
                4 => PointerButton::Middle,
                8 => PointerButton::Extra1,
                16 => PointerButton::Extra2,
                _ => PointerButton::Primary,
            };
            let pressed = action == 0;
            if pressed && !self.buttons.contains(&button) {
                self.buttons.push(button);
            } else if !pressed {
                self.buttons.retain(|b| *b != button);
            }
            self.push(Event::PointerButton {
                pos: self.pointer,
                button,
                pressed,
                modifiers: self.modifiers,
            });
        }
    }

    pub fn key(
        &mut self,
        code: i32,
        pressed: bool,
        ctrl: bool,
        shift: bool,
        alt: bool,
        text: &str,
    ) {
        let modifiers = Modifiers {
            ctrl,
            shift,
            alt,
            command: ctrl,
            mac_cmd: false,
        };
        if modifiers != self.modifiers {
            self.modifiers = modifiers;
            self.push(Event::ModifiersChanged(modifiers));
        }
        if code < 0 && !pressed {
            for key in std::mem::take(&mut self.down) {
                self.push(Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: false,
                    repeat: false,
                    modifiers,
                });
            }
            return;
        }
        if let Some(key) = key_from_ohos(code) {
            let repeat = pressed && self.down.contains(&key);
            if pressed && !repeat {
                self.down.push(key);
            } else if !pressed {
                self.down.retain(|k| *k != key);
            }
            self.push(Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat,
                modifiers: self.modifiers,
            });
        }
        if pressed && !ctrl && !alt && !text.is_empty() && text.len() <= 64 * 1024 {
            self.push(Event::Text(text.to_string()));
        }
    }

    /// Per-update pinch multiplier; native cumulative scales are differenced by the bridge.
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.push(Event::Zoom(factor.clamp(0.1, 10.0)));
        }
    }

    pub fn scroll(&mut self, dx: f32, dy: f32) {
        if dx.is_finite() && dy.is_finite() {
            self.push(Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(dx.clamp(-10000.0, 10000.0), dy.clamp(-10000.0, 10000.0)),
                modifiers: self.modifiers,
                phase: egui::TouchPhase::Move,
            });
        }
    }

    pub fn take(&mut self, size: [u32; 2], density: f32, time: f64) -> RawInput {
        self.take_with_zoom(size, density, time, 1.0)
    }

    /// Native pointer coordinates already exclude the OS density. egui points
    /// also exclude its UI zoom, while native_pixels_per_point keeps the OS scale.
    pub fn take_with_zoom(
        &mut self,
        size: [u32; 2],
        density: f32,
        time: f64,
        zoom: f32,
    ) -> RawInput {
        let density = if density.is_finite() && density > 0.0 {
            density
        } else {
            1.0
        };
        let zoom = if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        };
        let mut events = std::mem::take(&mut self.events);
        for event in &mut events {
            match event {
                Event::PointerMoved(pos) | Event::PointerButton { pos, .. } => {
                    *pos = *pos / zoom;
                }
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta,
                    ..
                } => *delta /= zoom,
                _ => {}
            }
        }
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(
                    size[0] as f32 / density / zoom,
                    size[1] as f32 / density / zoom,
                ),
            )),
            time: Some(time),
            events,
            focused: true,
            ..Default::default()
        };
        if let Some(viewport) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(density);
        }
        raw
    }
}

/// Values are from native_xcomponent_key_event.h (SDK API 26).
pub fn key_from_ohos(code: i32) -> Option<Key> {
    Some(match code {
        2000 => Key::Num0,
        2001 => Key::Num1,
        2002 => Key::Num2,
        2003 => Key::Num3,
        2004 => Key::Num4,
        2005 => Key::Num5,
        2006 => Key::Num6,
        2007 => Key::Num7,
        2008 => Key::Num8,
        2009 => Key::Num9,
        2012 => Key::ArrowUp,
        2013 => Key::ArrowDown,
        2014 => Key::ArrowLeft,
        2015 => Key::ArrowRight,
        2017 => Key::A,
        2018 => Key::B,
        2019 => Key::C,
        2020 => Key::D,
        2021 => Key::E,
        2022 => Key::F,
        2023 => Key::G,
        2024 => Key::H,
        2025 => Key::I,
        2026 => Key::J,
        2027 => Key::K,
        2028 => Key::L,
        2029 => Key::M,
        2030 => Key::N,
        2031 => Key::O,
        2032 => Key::P,
        2033 => Key::Q,
        2034 => Key::R,
        2035 => Key::S,
        2036 => Key::T,
        2037 => Key::U,
        2038 => Key::V,
        2039 => Key::W,
        2040 => Key::X,
        2041 => Key::Y,
        2042 => Key::Z,
        2043 => Key::Comma,
        2044 => Key::Period,
        2049 => Key::Tab,
        2050 => Key::Space,
        2054 => Key::Enter,
        2055 => Key::Backspace,
        2056 => Key::Backtick,
        2057 => Key::Minus,
        2058 => Key::Equals,
        2059 => Key::OpenBracket,
        2060 => Key::CloseBracket,
        2061 => Key::Backslash,
        2062 => Key::Semicolon,
        2063 => Key::Quote,
        2064 => Key::Slash,
        2068 => Key::PageUp,
        2069 => Key::PageDown,
        2070 => Key::Escape,
        2071 => Key::Delete,
        2081 => Key::Home,
        2082 => Key::End,
        2083 => Key::Insert,
        2090 => Key::F1,
        2091 => Key::F2,
        2092 => Key::F3,
        2093 => Key::F4,
        2094 => Key::F5,
        2095 => Key::F6,
        2096 => Key::F7,
        2097 => Key::F8,
        2098 => Key::F9,
        2099 => Key::F10,
        2100 => Key::F11,
        2101 => Key::F12,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_keep_ctrl_command_and_repeat() {
        let mut state = InputState::default();
        state.key(2035, true, true, true, false, "s");
        state.key(2035, true, true, true, false, "s");
        let input = state.take([1600, 1200], 2.0, 1.0);
        assert_eq!(
            input.screen_rect.map(|r| r.size()),
            Some(Vec2::new(800.0, 600.0))
        );
        assert_eq!(input.events.len(), 3);
        assert!(matches!(
            input.events.get(2),
            Some(Event::Key {
                key: Key::S,
                repeat: true,
                modifiers: Modifiers {
                    command: true,
                    shift: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn cancel_releases_pointer_and_ignores_nonfinite_coordinates() {
        let mut state = InputState::default();
        state.pointer(3.0, 4.0, 0, 1);
        state.pointer(f32::NAN, 0.0, 2, 1);
        state.pointer(3.0, 4.0, 3, 1);
        let input = state.take([10, 10], 1.0, 0.0);
        assert!(matches!(
            input.events.get(2),
            Some(Event::PointerButton { pressed: false, .. })
        ));
        assert!(matches!(input.events.get(3), Some(Event::PointerGone)));
    }

    #[test]
    fn ui_zoom_scales_coordinates_without_changing_native_density() {
        let mut state = InputState::default();
        state.pointer(240.0, 160.0, 0, 1);
        state.scroll(40.0, -80.0);
        let input = state.take_with_zoom([1600, 1200], 2.0, 1.0, 2.0);
        assert_eq!(
            input.screen_rect.map(|r| r.size()),
            Some(Vec2::new(400.0, 300.0))
        );
        assert_eq!(
            input.viewports[&egui::ViewportId::ROOT].native_pixels_per_point,
            Some(2.0)
        );
        assert!(
            matches!(input.events.first(), Some(Event::PointerMoved(pos)) if *pos == Pos2::new(120.0, 80.0))
        );
        assert!(
            matches!(input.events.get(1), Some(Event::PointerButton { pos, pressed: true, .. }) if *pos == Pos2::new(120.0, 80.0))
        );
        assert!(
            matches!(input.events.get(2), Some(Event::MouseWheel { delta, .. }) if *delta == Vec2::new(20.0, -40.0))
        );

        // Keep unscaled native coordinates between frames; rescale each event
        // once using the zoom in effect for the frame that consumes it.
        state.pointer(240.0, 160.0, 1, 1);
        let input = state.take_with_zoom([1600, 1200], 2.0, 2.0, 1.0);
        assert!(
            matches!(input.events.first(), Some(Event::PointerMoved(pos)) if *pos == Pos2::new(240.0, 160.0))
        );
    }

    #[test]
    fn invalid_ui_zoom_uses_unscaled_coordinates() {
        for zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mut state = InputState::default();
            state.pointer(20.0, 40.0, 2, 1);
            let input = state.take_with_zoom([400, 200], 2.0, 0.0, zoom);
            assert_eq!(
                input.screen_rect.map(|r| r.size()),
                Some(Vec2::new(200.0, 100.0))
            );
            assert!(
                matches!(input.events.first(), Some(Event::PointerMoved(pos)) if *pos == Pos2::new(20.0, 40.0))
            );
        }
    }

    #[test]
    fn insert_and_navigation_keys_match_sdk() {
        assert_eq!(key_from_ohos(2083), Some(Key::Insert));
        assert_eq!(key_from_ohos(2071), Some(Key::Delete));
        assert_eq!(key_from_ohos(-1), None);
    }

    #[test]
    fn window_blur_releases_pressed_keys() {
        let mut state = InputState::default();
        state.key(2035, true, true, false, false, "");
        state.key(-1, false, false, false, false, "");
        let input = state.take([10, 10], 1.0, 0.0);
        assert!(matches!(
            input.events.last(),
            Some(Event::Key {
                key: Key::S,
                pressed: false,
                ..
            })
        ));
        assert!(state.down.is_empty());
    }
    #[test]
    fn pinch_multipliers_reach_real_context_zoom_delta_and_ignore_invalid_values() {
        let ctx = egui::Context::default();
        let mut input = InputState::default();
        for (factor, expected) in [
            (1.25, 1.25),
            (0.8, 0.8),
            (1000.0, 10.0),
            (0.0, 1.0),
            (-1.0, 1.0),
            (f32::NAN, 1.0),
            (f32::INFINITY, 1.0),
        ] {
            input.zoom(factor);
            let mut observed = 0.0;
            let output = ctx.run_ui(input.take([1280, 800], 1.0, 0.0), |ui| {
                observed = ui.ctx().input(|input| input.zoom_delta());
            });
            output.drop_without_applying_deltas();
            assert!((observed - expected).abs() < 0.0001);
        }
    }
}
