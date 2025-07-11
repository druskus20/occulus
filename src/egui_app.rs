use crate::{
    TokioEguiBridge,
    data_task::{self, LogAppendBufReader},
    prelude::*,
};
use argus::tracing::oculus::{DashboardEvent, Level};
use eframe::egui;
use egui::{epaint::color, text::LayoutJob};
use std::{
    collections::VecDeque,
    sync::{Arc, atomic::AtomicBool},
};
use tokio::sync::{broadcast, mpsc::UnboundedSender, oneshot};
use tracing::info;

// Add the tracing log display module
use egui::{Color32, RichText, ScrollArea, TextFormat, TextFormat as EguiTextFormat, Ui};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

#[derive(Debug, Clone, Copy)]
pub struct LogDisplaySettings {
    pub show_timestamps: bool,
    pub show_targets: bool,
    pub show_file_info: bool,
    pub show_span_info: bool,
    pub auto_scroll: bool,
    pub level_filter: LogLevelFilter,
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum LogLevelFilter {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Into<argus::tracing::oculus::Level> for LogLevelFilter {
    fn into(self) -> argus::tracing::oculus::Level {
        match self {
            LogLevelFilter::Trace => argus::tracing::oculus::Level::TRACE,
            LogLevelFilter::Debug => argus::tracing::oculus::Level::DEBUG,
            LogLevelFilter::Info => argus::tracing::oculus::Level::INFO,
            LogLevelFilter::Warn => argus::tracing::oculus::Level::WARN,
            LogLevelFilter::Error => argus::tracing::oculus::Level::ERROR,
        }
    }
}

impl Default for LogDisplaySettings {
    fn default() -> Self {
        Self {
            show_timestamps: false,
            show_targets: false,
            show_file_info: false,
            show_span_info: false,
            auto_scroll: true,
            level_filter: LogLevelFilter::Trace,
        }
    }
}

// Cache entry for pre-rendered log lines
#[derive(Clone)]
struct CachedLogLine {
    layout_job: LayoutJob,
    event_hash: u64,    // Hash of the event for cache invalidation
    settings_hash: u64, // Hash of relevant settings
}

pub struct TracingLogDisplay {
    settings: LogDisplaySettings,
    scroll_to_bottom: bool,
    last_log_count: usize,
}

impl TracingLogDisplay {
    pub fn new(initial_settings: LogDisplaySettings) -> Self {
        Self {
            settings: initial_settings,
            scroll_to_bottom: false,
            last_log_count: 0,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        to_data: &UnboundedSender<UiEvent>,
        log_buffer: &VecDeque<Arc<DashboardEvent>>,
    ) {
        ui.separator();

        let settings_changed = self.show_controls(ui);
        if settings_changed {
            to_data
                .send(UiEvent::LogDisplaySettingsChanged(self.settings))
                .unwrap_or_else(|err| {
                    error!("Failed to send log display settings change: {err}");
                });
        }
        self.render_logs(ui, log_buffer);
    }

    fn show_controls(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(&mut self.settings.show_timestamps, "Timestamps")
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.show_targets, "Targets")
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.show_file_info, "File Info")
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.show_span_info, "Span Info")
                .changed();
            changed |= ui
                .checkbox(&mut self.settings.auto_scroll, "Auto Scroll")
                .changed();

            ui.separator();

            ui.label("Level:");
            let old_filter = self.settings.level_filter;
            egui::ComboBox::from_id_salt("level_filter")
                .selected_text(format!("{:?}", self.settings.level_filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.settings.level_filter,
                        LogLevelFilter::Trace,
                        "Trace",
                    );
                    ui.selectable_value(
                        &mut self.settings.level_filter,
                        LogLevelFilter::Debug,
                        "Debug",
                    );
                    ui.selectable_value(
                        &mut self.settings.level_filter,
                        LogLevelFilter::Info,
                        "Info",
                    );
                    ui.selectable_value(
                        &mut self.settings.level_filter,
                        LogLevelFilter::Warn,
                        "Warn",
                    );
                    ui.selectable_value(
                        &mut self.settings.level_filter,
                        LogLevelFilter::Error,
                        "Error",
                    );
                });
            changed |= old_filter != self.settings.level_filter;
        });

        changed
    }

    fn render_logs(&mut self, ui: &mut Ui, filtered_logs: &VecDeque<Arc<DashboardEvent>>) {
        let text_style = egui::TextStyle::Monospace;
        let row_height = ui.text_style_height(&text_style);

        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.settings.auto_scroll)
            .show_rows(ui, row_height, filtered_logs.len(), |ui, row_range| {
                for row in row_range {
                    if let Some(job) = filtered_logs.get(row) {
                        // only compute the layout job for the visible rows
                        // TODO: cache the layout jobs - logs dont change unless settings do
                        ui.label(create_layout_job(job, self.settings));
                    }
                }
            });
    }

    //fn should_show_event(&self, event: &DashboardEvent) -> bool {
    //    // Level filtering
    //    if !self.level_matches(&event.level) {
    //        return false;
    //    }

    //    true
    //}

    //fn level_matches(&self, level: &str) -> bool {
    //    match self.settings.level_filter {
    //        LogLevelFilter::All => true,
    //        LogLevelFilter::Trace => true,
    //        LogLevelFilter::Debug => !level.eq_ignore_ascii_case("trace"),
    //        LogLevelFilter::Info => {
    //            matches!(level.to_lowercase().as_str(), "info" | "warn" | "error")
    //        }
    //        LogLevelFilter::Warn => matches!(level.to_lowercase().as_str(), "warn" | "error"),
    //        LogLevelFilter::Error => level.eq_ignore_ascii_case("error"),
    //    }
    //}
}

fn color_for_log_level(level: &Level) -> Color32 {
    match level {
        &Level::TRACE => Color32::from_rgb(200, 200, 200), // Light gray for TRACE
        &Level::DEBUG => Color32::from_rgb(100, 150, 255), // Light blue for DEBUG
        &Level::INFO => Color32::from_rgb(0, 200, 0),      // Green for INFO
        &Level::WARN => Color32::from_rgb(255, 165, 0),    // Orange for WARN
        &Level::ERROR => Color32::from_rgb(255, 0, 0),     // Red for ERROR
    }
}
impl std::hash::Hash for LogLevelFilter {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

fn format_timestamp(timestamp: u64) -> String {
    format!("{timestamp}")
}

fn format_fields(fields: &HashMap<String, String>) -> String {
    if fields.is_empty() {
        return String::new();
    }

    let mut formatted = String::new();
    formatted.push_str(" {");

    let mut first = true;
    for (key, value) in fields {
        if !first {
            formatted.push_str(", ");
        }
        formatted.push_str(&format!("{key}={value}"));
        first = false;
    }

    formatted.push('}');
    formatted
}

#[derive(Debug, Clone)]
struct EguiState {
    tokio_egui_bridge: TokioEguiBridge,
}

struct EguiApp {
    state: EguiState,
    logs: triple_buffer::Output<VecDeque<Arc<DashboardEvent>>>,
    log_display: TracingLogDisplay,
    to_data: UnboundedSender<UiEvent>,
}

impl EguiApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        tokio_egui_bridge: TokioEguiBridge,
        log_buffer: triple_buffer::Output<VecDeque<Arc<DashboardEvent>>>,
        initial_settings: LogDisplaySettings,
        to_data: UnboundedSender<UiEvent>,
    ) -> Self {
        // register the egui context globally
        let ctx = cc.egui_ctx.clone();
        tokio_egui_bridge.register_egui_context(ctx);
        Self {
            state: EguiState { tokio_egui_bridge },
            logs: log_buffer,
            log_display: TracingLogDisplay::new(initial_settings),
            to_data,
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Oculus");
            self.logs.update();
            let log_buffer = self.logs.read();
            self.log_display.show(ui, &self.to_data, log_buffer);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.tokio_egui_bridge.cancel();
    }
}

pub fn run_egui(
    log_buffer: triple_buffer::Output<VecDeque<Arc<DashboardEvent>>>,
    tokio_egui_bridge: TokioEguiBridge,
    initial_settings: LogDisplaySettings,
    to_data: UnboundedSender<UiEvent>,
) -> Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Tracing Log Viewer",
        native_options,
        Box::new(|cc| {
            Ok(Box::new(EguiApp::new(
                cc,
                tokio_egui_bridge,
                log_buffer,
                initial_settings,
                to_data,
            )))
        }),
    )
    .expect("Failed to launch eframe app");
    Ok(())
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    LogDisplaySettingsChanged(LogDisplaySettings),
}

pub fn create_layout_job(event: &DashboardEvent, settings: LogDisplaySettings) -> LayoutJob {
    let mut job = LayoutJob::default();

    // Timestamp
    if settings.show_timestamps {
        let timestamp = format_timestamp(event.timestamp);
        job.append(
            &format!("{} ", timestamp),
            0.0,
            EguiTextFormat {
                color: Color32::GRAY,
                font_id: egui::FontId::monospace(12.0),
                ..Default::default()
            },
        );
    }

    // Level with color
    let level_text = format!("{:>5} ", event.level.to_string().to_uppercase());
    job.append(
        &level_text,
        0.0,
        EguiTextFormat {
            color: color_for_log_level(&event.level),
            font_id: egui::FontId::monospace(12.0),
            ..Default::default()
        },
    );

    // Target
    if settings.show_targets {
        job.append(
            &format!("{} ", event.target),
            0.0,
            EguiTextFormat {
                color: Color32::LIGHT_BLUE,
                font_id: egui::FontId::monospace(12.0),
                ..Default::default()
            },
        );
    }

    // Span info
    if settings.show_span_info {
        if let Some(span_id) = event.span_id {
            let span_text = if let Some(parent_id) = event.parent_span_id {
                format!("[{parent_id}→{span_id}] ")
            } else {
                format!("[{span_id}] ")
            };
            job.append(
                &span_text,
                0.0,
                EguiTextFormat {
                    color: Color32::YELLOW,
                    font_id: egui::FontId::monospace(12.0),
                    ..Default::default()
                },
            );
        }
    }

    // File info
    if settings.show_file_info {
        if let (Some(file), Some(line)) = (&event.file, event.line) {
            let file_info = format!("{file}:{line} ");
            job.append(
                &file_info,
                0.0,
                EguiTextFormat {
                    color: Color32::GRAY,
                    font_id: egui::FontId::monospace(10.0),
                    ..Default::default()
                },
            );
        }
    }

    // Main message
    job.append(
        &event.message,
        0.0,
        EguiTextFormat {
            color: Color32::WHITE,
            font_id: egui::FontId::monospace(12.0),
            ..Default::default()
        },
    );

    // Fields (if any)
    if !event.fields.is_empty() {
        job.append(
            &format_fields(&event.fields),
            0.0,
            EguiTextFormat {
                color: Color32::LIGHT_GRAY,
                font_id: egui::FontId::monospace(12.0),
                ..Default::default()
            },
        );
    }

    job
}
