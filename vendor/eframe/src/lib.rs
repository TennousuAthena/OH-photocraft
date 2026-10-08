//! eframe's platform independent application seam for the PhotoCraft OHOS runner.
//!
//! This crate is selected only by the port workspace's Cargo patch. PhotoCraft's
//! desktop and browser workspaces keep the original eframe integration.
//! The OHOS runner owns native windows, rendering, input and persistence.

#![forbid(unsafe_code)]

pub use egui;
#[cfg(feature = "wgpu_no_default_features")]
pub use egui_wgpu;
#[cfg(feature = "wgpu_no_default_features")]
pub use egui_wgpu::wgpu;

/// The platform independent eframe 0.36 application callbacks used by PhotoCraft.
pub trait App {
    /// Update state before drawing, or while the viewport is hidden.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut Frame) {
        let _ = (ctx, frame);
    }

    /// Draw the root viewport into a UI with no margin or background.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame);

    /// Filter or append input before the runner starts an egui pass.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let _ = (ctx, raw_input);
    }

    /// Persist app state; the runner determines when to call this.
    fn save(&mut self, _storage: &mut dyn Storage) {}

    /// Notify the app that its integration is shutting down.
    fn on_exit(&mut self) {}

    /// Desired interval between persistence callbacks.
    fn auto_save_interval(&self) -> core::time::Duration {
        core::time::Duration::from_secs(30)
    }

    /// Surface clear colour in gamma space, matching eframe 0.36.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32()
    }

    /// Whether the integration should persist egui's memory.
    fn persist_egui_memory(&self) -> bool {
        true
    }
}

/// Information about the previous rendered frame.
#[derive(Clone, Debug, Default)]
pub struct IntegrationInfo {
    pub cpu_usage: Option<f32>,
}

/// Integration state passed to the app. PhotoCraft 4337a62 does not read it.
#[derive(Default)]
pub struct Frame {
    info: IntegrationInfo,
    storage: Option<Box<dyn Storage>>,
    #[cfg(feature = "wgpu_no_default_features")]
    pub wgpu_render_state: Option<egui_wgpu::RenderState>,
}

impl Frame {
    /// Create an empty integration frame, matching eframe's public test constructor.
    #[doc(hidden)]
    pub fn _new_kittest() -> Self {
        Self::default()
    }

    /// The OHOS integration is a native application.
    pub fn is_web(&self) -> bool {
        false
    }

    pub fn info(&self) -> &IntegrationInfo {
        &self.info
    }

    pub fn storage(&self) -> Option<&dyn Storage> {
        self.storage.as_deref()
    }

    pub fn storage_mut(&mut self) -> Option<&mut (dyn Storage + 'static)> {
        self.storage.as_deref_mut()
    }

    #[cfg(feature = "wgpu_no_default_features")]
    pub fn wgpu_render_state(&self) -> Option<&egui_wgpu::RenderState> {
        self.wgpu_render_state.as_ref()
    }

    #[cfg(feature = "wgpu_no_default_features")]
    pub fn wgpu_surface_config(&self) -> Option<egui_wgpu::SurfaceConfig> {
        self.wgpu_render_state
            .as_ref()
            .map(|state| state.surface_config)
    }

    #[cfg(feature = "wgpu_no_default_features")]
    pub fn set_wgpu_surface_config(&mut self, config: egui_wgpu::SurfaceConfig) {
        if let Some(state) = &mut self.wgpu_render_state {
            state.surface_config = config;
        }
    }
}

/// Key/value persistence supplied by the platform integration.
pub trait Storage {
    fn get_string(&self, key: &str) -> Option<String>;
    fn set_string(&mut self, key: &str, value: String);
    fn remove_string(&mut self, key: &str);
    fn flush(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter {
        logic_calls: usize,
        ui_calls: usize,
    }

    impl App for Counter {
        fn logic(&mut self, _ctx: &egui::Context, _frame: &mut Frame) {
            self.logic_calls += 1;
        }

        fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut Frame) {
            self.ui_calls += 1;
        }
    }

    #[test]
    fn custom_integration_drives_the_application_callbacks() {
        let ctx = egui::Context::default();
        let mut frame = Frame::_new_kittest();
        let mut app = Counter {
            logic_calls: 0,
            ui_calls: 0,
        };
        let mut input = egui::RawInput::default();
        app.raw_input_hook(&ctx, &mut input);
        let output = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut frame);
            app.ui(ui, &mut frame);
        });
        output.drop_without_applying_deltas();
        assert_eq!(app.logic_calls, 1);
        assert_eq!(app.ui_calls, 1);
        assert!(!frame.is_web());
        assert!(frame.storage().is_none());
        assert!(frame.info().cpu_usage.is_none());
    }
}
