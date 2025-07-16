use crate::{FrontendSide, TokioEguiBridge, prelude::*};
use argus::tracing::oculus::{DashboardEvent, Level};
use eframe::egui;
use egui::{Button, text::LayoutJob};

// Add the tracing log display module
use egui::{Color32, RichText, ScrollArea, TextFormat as EguiTextFormat, Ui};
use std::collections::HashMap;

const COLOR_ERROR: Color32 = Color32::from_rgb(255, 85, 85); // soft red
const COLOR_WARNING: Color32 = Color32::from_rgb(255, 204, 0); // amber
const COLOR_INFO: Color32 = Color32::from_rgb(80, 250, 123); // neon green
const COLOR_DEBUG: Color32 = Color32::from_rgb(139, 233, 253); // cyan
const COLOR_TRACE: Color32 = Color32::from_rgb(128, 128, 128); // medium gray

const _COLOR_TEXT: Color32 = Color32::from_rgb(255, 255, 255); // white text on dark backgrounds
const COLOR_TEXT_INV: Color32 = Color32::from_rgb(0, 0, 0); // black text on colored backgrounds

#[derive(Debug, Clone, Copy)]
pub struct DisplaySettings {
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

impl Default for DisplaySettings {
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

pub fn run_egui(frontend: FrontendSide, tokio_egui_bridge: TokioEguiBridge) -> Result<()> {
    eframe::run_native(
        "Tracing Log Viewer",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(EguiApp::new(cc, frontend, tokio_egui_bridge)))),
    )
    .expect("Failed to launch eframe app");
    Ok(())
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

fn format_timestamp_utc(timestamp_ms: u64) -> String {
    use chrono::{DateTime, Utc};

    let dt = DateTime::<Utc>::from_timestamp_millis(timestamp_ms as i64).unwrap_or_else(|| {
        // 0.0
        DateTime::<Utc>::from_timestamp(0, 0).expect("Failed to create timestamp")
    });

    //dt.format("%Y-%m-%d %H:%M:%S").to_string()
    dt.format("%d/%m/%y %H:%M:%S").to_string()
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
    frontend_side: FrontendSide,
}

impl EguiApp {
    fn apply_filter(&mut self, new_filter: LogLevelFilter) -> bool {
        if self.frontend_side.settings.level_filter != new_filter {
            self.frontend_side.settings.level_filter = new_filter;
            self.frontend_side
                .to_backend
                .send(UiEvent::LogDisplaySettingsChanged(
                    self.frontend_side.settings,
                ))
                .unwrap_or_else(|err| {
                    error!("Failed to send log display settings change: {err}");
                });
            return true;
        }
        false
    }

    fn show_controls(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(
                    &mut self.frontend_side.settings.show_timestamps,
                    "Timestamps",
                )
                .changed();
            changed |= ui
                .checkbox(&mut self.frontend_side.settings.show_targets, "Targets")
                .changed();
            changed |= ui
                .checkbox(&mut self.frontend_side.settings.show_file_info, "File Info")
                .changed();
            changed |= ui
                .checkbox(&mut self.frontend_side.settings.show_span_info, "Span Info")
                .changed();
            changed |= ui
                .checkbox(&mut self.frontend_side.settings.auto_scroll, "Auto Scroll")
                .changed();

            ui.separator();
        });

        changed
    }

    fn render_logs(&mut self, ui: &mut Ui) {
        let text_style = egui::TextStyle::Monospace;
        let row_height = ui.text_style_height(&text_style);

        let logs = &self
            .frontend_side
            .data_buffer_rx
            .output_buffer_mut()
            .filtered_logs;
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.frontend_side.settings.auto_scroll)
            .show_rows(ui, row_height, logs.len(), |ui, row_range| {
                for row in row_range {
                    // only compute the layout job for the visible rows
                    // TODO: cache the layout jobs - logs dont change unless settings do
                    if let Some(job) = logs.get(row) {
                        display_log_line(
                            ui,
                            job,
                            self.frontend_side.settings,
                            self.frontend_side.to_backend.clone(),
                        );
                    }
                }
            });
    }
}

fn add_colored_button(
    name: &str,
    color: Color32,
    count: usize,
    active: bool,
    mut clicked_f: impl FnMut() -> bool,
    ui: &mut Ui,
) {
    // Error button
    let mut button =
        Button::new(RichText::new(format!("{name}: {count}")).color(COLOR_TEXT_INV)).fill(color);

    if active {
        button = button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
    }

    let button = ui.add(button);
    if button.clicked() {
        clicked_f();
    }
}
impl EguiApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        frontend: FrontendSide,
        tokio_egui_bridge: TokioEguiBridge,
    ) -> Self {
        // register the egui context globally
        let ctx = cc.egui_ctx.clone();
        tokio_egui_bridge.register_egui_context(ctx);
        Self {
            tokio_bridge: tokio_egui_bridge,
            frontend_side: frontend,
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Single update for the entire frame. From here onwwards we use output_buffer_mut() to
        // get the latest data.
        self.frontend_side.data_buffer_rx.update();

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
            let settings_changed = self.show_controls(ui);
            if settings_changed {
                self.frontend_side
                    .to_backend
                    .send(UiEvent::LogDisplaySettingsChanged(
                        self.frontend_side.settings,
                    ))
                    .unwrap_or_else(|err| {
                        error!("Failed to send log display settings change: {err}");
                    });
            }

            ui.separator();

            ui.horizontal(|ui| {
                // LOG COUNTS as colored buttons
                let log_counts = self
                    .frontend_side
                    .data_buffer_rx
                    .output_buffer_mut()
                    .log_counts;

                // Total count (non-clickable)
                ui.label(format!("Total: {}", log_counts.total));

                add_colored_button(
                    "Error",
                    COLOR_ERROR,
                    log_counts.error,
                    self.frontend_side.settings.level_filter == LogLevelFilter::Error,
                    || self.apply_filter(LogLevelFilter::Error),
                    ui,
                );
                add_colored_button(
                    "Warn",
                    COLOR_WARNING,
                    log_counts.warn,
                    self.frontend_side.settings.level_filter == LogLevelFilter::Warn,
                    || self.apply_filter(LogLevelFilter::Warn),
                    ui,
                );
                add_colored_button(
                    "Info",
                    COLOR_INFO,
                    log_counts.info,
                    self.frontend_side.settings.level_filter == LogLevelFilter::Info,
                    || self.apply_filter(LogLevelFilter::Info),
                    ui,
                );
                add_colored_button(
                    "Debug",
                    COLOR_DEBUG,
                    log_counts.debug,
                    self.frontend_side.settings.level_filter == LogLevelFilter::Debug,
                    || self.apply_filter(LogLevelFilter::Debug),
                    ui,
                );
                add_colored_button(
                    "Trace",
                    COLOR_TRACE,
                    log_counts.trace,
                    self.frontend_side.settings.level_filter == LogLevelFilter::Trace,
                    || self.apply_filter(LogLevelFilter::Trace),
                    ui,
                );
            });

            ui.separator();

            // LOGS
            self.render_logs(ui);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.tokio_bridge.cancel();
    }
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    LogDisplaySettingsChanged(DisplaySettings),
    OpenInEditor { path: String, line: u32 },
}

pub fn display_log_line(
    ui: &mut Ui,
    event: &DashboardEvent,
    settings: DisplaySettings,
    to_backend: tokio::sync::mpsc::UnboundedSender<UiEvent>,
) {
    let mut job = LayoutJob::default();
    // Timestamp
    if settings.show_timestamps {
        let timestamp = format_timestamp_utc(event.timestamp);
        job.append(
            &format!("{timestamp} "),
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
        if let Some(span_meta) = &event.span_meta {
            let span_name = &span_meta.name;
            let span_text = if let Some(parent_id) = event.parent_span_id {
                format!("[{parent_id}→{span_name}] ")
            } else {
                format!("[{span_name}] ")
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
    // Use horizontal layout to display everything inline
    ui.horizontal_top(|ui| {
        // First part: everything up to the file info
        ui.label(job);
        // File info as clickable hyperlink
        if settings.show_file_info {
            if let (Some(file_path), Some(line)) = (&event.file, event.line) {
                let file_info = format!("{file_path}:{line}");
                let line_num = line;
                let hyperlink = egui::RichText::new(&file_info)
                    .color(Color32::LIGHT_BLUE)
                    .font(egui::FontId::monospace(10.0))
                    .underline();
                if ui
                    .link(hyperlink)
                    .on_hover_text("Click to open in editor")
                    .clicked()
                {
                    to_backend
                        .send(UiEvent::OpenInEditor {
                            path: file_path.clone(),
                            line: line_num,
                        })
                        .unwrap_or_else(|_| error!("Failed to send OpenInEditor event"));
                }
                ui.label(" "); // Space after file info
            }
        }
        // Create a new job for the message and fields
        let mut message_job = LayoutJob::default();
        // Main message
        message_job.append(
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
            message_job.append(
                &format_fields(&event.fields),
                0.0,
                EguiTextFormat {
                    color: Color32::LIGHT_GRAY,
                    font_id: egui::FontId::monospace(12.0),
                    ..Default::default()
                },
            );
        }
        ui.label(message_job);
    });
}
