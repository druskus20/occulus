use crate::{
    TokioEguiBridge,
    data_task::{DisplayData, LogCounts, OculusInternalMetrics},
    prelude::*,
};
use argus::tracing::oculus::{DashboardEvent, Level};
use color_eyre::owo_colors::OwoColorize;
use eframe::egui;
use egui::{Button, text::LayoutJob};
use std::marker::PhantomData;
use tokio::sync::mpsc::UnboundedSender;

// Add the tracing log display module
use egui::{Color32, RichText, ScrollArea, TextFormat as EguiTextFormat, Ui};
use std::collections::HashMap;

const COLOR_ERROR: Color32 = Color32::from_rgb(255, 85, 85); // soft red
const COLOR_WARNING: Color32 = Color32::from_rgb(255, 204, 0); // amber
const COLOR_INFO: Color32 = Color32::from_rgb(80, 250, 123); // neon green
const COLOR_DEBUG: Color32 = Color32::from_rgb(139, 233, 253); // cyan
const COLOR_TRACE: Color32 = Color32::from_rgb(128, 128, 128); // medium gray

const COLOR_TEXT: Color32 = Color32::from_rgb(255, 255, 255); // white text on dark backgrounds
const COLOR_TEXT_INV: Color32 = Color32::from_rgb(0, 0, 0); // black text on colored backgrounds

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

impl From<LogLevelFilter> for argus::tracing::oculus::Level {
    fn from(val: LogLevelFilter) -> Self {
        match val {
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
        display_data: &Vec<LayoutJob>,
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
        self.render_logs(ui, display_data);
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

    fn render_logs(&mut self, ui: &mut Ui, display_data: &Vec<LayoutJob>) {
        let text_style = egui::TextStyle::Monospace;
        let row_height = ui.text_style_height(&text_style);

        let logs = &display_data;
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.settings.auto_scroll)
            .show_rows(ui, row_height, logs.len(), |ui, row_range| {
                for row in row_range {
                    // only compute the layout job for the visible rows
                    // TODO: cache the layout jobs - logs dont change unless settings do
                    if let Some(job) = logs.get(row) {
                        ui.add(egui::Label::new(job));
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
        Level::TRACE => COLOR_TRACE,
        Level::DEBUG => COLOR_DEBUG,
        Level::INFO => COLOR_INFO,
        Level::WARN => COLOR_WARNING,
        Level::ERROR => COLOR_ERROR,
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

struct EguiApp {
    tokio_bridge: TokioEguiBridge,
    display_data_rx: Vec<LayoutJob>,
    log_display: TracingLogDisplay,
    metrics: triple_buffer::Output<PhantomData<()>>,
    internal_metrics: triple_buffer::Output<OculusInternalMetrics>,
    to_data: UnboundedSender<UiEvent>,

    benchmark_total_fps: u64,
}

impl EguiApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        tokio_egui_bridge: TokioEguiBridge,
        internal_metrics: triple_buffer::Output<OculusInternalMetrics>,
        metrics: triple_buffer::Output<PhantomData<()>>,
        initial_settings: LogDisplaySettings,
        to_data: UnboundedSender<UiEvent>,
    ) -> Self {
        let mut display_data_rx = Vec::new();
        for i in 0..100_000 {
            let log = DashboardEvent {
                timestamp: i as u64,
                level: Level::INFO,
                target: "test_target".to_string(),
                message: format!("Test log message {}", i),
                span_id: Some(i as u64),
                parent_span_id: None,
                file: Some("test_file.rs".to_string()),
                line: Some(i as u32),
                fields: HashMap::new(),
                event_type: "log".to_string(),
            };
            let job = create_layout_job(&log, initial_settings);

            display_data_rx.push(job);
        }

        // register the egui context globally
        let ctx = cc.egui_ctx.clone();
        tokio_egui_bridge.register_egui_context(ctx);
        Self {
            tokio_bridge: tokio_egui_bridge,
            display_data_rx,
            log_display: TracingLogDisplay::new(initial_settings),
            to_data,
            metrics,
            internal_metrics,
            benchmark_total_fps: 0,
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        info!("EguiApp::update called");
        self.benchmark_total_fps += 1;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Oculus");

            // FPS
            egui::Area::new("fps_display".into())
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 10.0))
                .show(ctx, |ui| {
                    let fps = 1.0 / ctx.input(|i| i.stable_dt.max(1e-5));
                    ui.label(format!("FPS: {fps:.1}"));
                });

            ui.separator();

            // DISPLAY SETTINGS
            let settings_changed = self.log_display.show_controls(ui);
            if settings_changed {
                self.to_data
                    .send(UiEvent::LogDisplaySettingsChanged(
                        self.log_display.settings,
                    ))
                    .unwrap_or_else(|err| {
                        error!("Failed to send log display settings change: {err}");
                    });
            }

            ui.separator();

            //self.display_data_rx.update();
            //let display_data = self.display_data_rx.read();

            // LOG COUNTS as colored buttons
            let log_counts = LogCounts::default();
            ui.horizontal(|ui| {
                ui.label("Log Counts:");

                // Total count (non-clickable)
                ui.label(format!("Total: {}", log_counts.total));

                // Error button
                let mut error_button = Button::new(
                    RichText::new(format!("Error: {}", log_counts.error)).color(COLOR_TEXT_INV),
                )
                .fill(COLOR_ERROR);

                if self.log_display.settings.level_filter == LogLevelFilter::Error {
                    error_button = error_button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
                }

                let error_button = ui.add(error_button);
                if error_button.clicked() {
                    self.log_display.settings.level_filter = LogLevelFilter::Error;
                    self.to_data
                        .send(UiEvent::LogDisplaySettingsChanged(
                            self.log_display.settings,
                        ))
                        .unwrap_or_else(|err| {
                            error!("Failed to send log display settings change: {err}");
                        });
                }

                // Warn button
                let mut warn_button = Button::new(
                    RichText::new(format!("Warn: {}", log_counts.warn)).color(COLOR_TEXT_INV),
                )
                .fill(COLOR_WARNING);
                if self.log_display.settings.level_filter == LogLevelFilter::Warn {
                    warn_button = warn_button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
                }
                let warn_button = ui.add(warn_button);
                if warn_button.clicked() {
                    self.log_display.settings.level_filter = LogLevelFilter::Warn;
                    self.to_data
                        .send(UiEvent::LogDisplaySettingsChanged(
                            self.log_display.settings,
                        ))
                        .unwrap_or_else(|err| {
                            error!("Failed to send log display settings change: {err}");
                        });
                }

                // Info button
                let mut info_button = Button::new(
                    RichText::new(format!("Info: {}", log_counts.info)).color(COLOR_TEXT_INV),
                )
                .fill(COLOR_INFO);
                if self.log_display.settings.level_filter == LogLevelFilter::Info {
                    info_button = info_button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
                }
                let info_button = ui.add(info_button);
                if info_button.clicked() {
                    self.log_display.settings.level_filter = LogLevelFilter::Info;
                    self.to_data
                        .send(UiEvent::LogDisplaySettingsChanged(
                            self.log_display.settings,
                        ))
                        .unwrap_or_else(|err| {
                            error!("Failed to send log display settings change: {err}");
                        });
                }

                // Debug button
                let mut debug_button = Button::new(
                    RichText::new(format!("Debug: {}", log_counts.debug)).color(COLOR_TEXT_INV),
                )
                .fill(COLOR_DEBUG);
                if self.log_display.settings.level_filter == LogLevelFilter::Debug {
                    debug_button = debug_button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
                }
                let debug_button = ui.add(debug_button);
                if debug_button.clicked() {
                    self.log_display.settings.level_filter = LogLevelFilter::Debug;
                    self.to_data
                        .send(UiEvent::LogDisplaySettingsChanged(
                            self.log_display.settings,
                        ))
                        .unwrap_or_else(|err| {
                            error!("Failed to send log display settings change: {err}");
                        });
                }

                // Trace button
                let mut trace_button = Button::new(
                    RichText::new(format!("Trace: {}", log_counts.trace)).color(COLOR_TEXT_INV),
                )
                .fill(COLOR_TRACE);
                if self.log_display.settings.level_filter == LogLevelFilter::Trace {
                    trace_button = trace_button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
                }
                let trace_button = ui.add(trace_button);
                if trace_button.clicked() {
                    self.log_display.settings.level_filter = LogLevelFilter::Trace;
                    self.to_data
                        .send(UiEvent::LogDisplaySettingsChanged(
                            self.log_display.settings,
                        ))
                        .unwrap_or_else(|err| {
                            error!("Failed to send log display settings change: {err}");
                        });
                }
            });

            ui.separator();

            // LOGS
            self.log_display.render_logs(ui, &self.display_data_rx);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.tokio_bridge.cancel();
    }
}

pub fn run_egui(
    display_data_rx: triple_buffer::Output<DisplayData>,
    internal_metrics: triple_buffer::Output<OculusInternalMetrics>,
    metrics: triple_buffer::Output<PhantomData<()>>,
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
                internal_metrics,
                metrics,
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
