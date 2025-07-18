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
const COLOR_TRACE: Color32 = Color32::from_rgb(189, 147, 249); // light purple

const _COLOR_LIHT_PURPLE: Color32 = Color32::from_rgb(189, 147, 249); // light purple
const COLOR_LIGHT_MAGENTA: Color32 = Color32::from_rgb(255, 121, 198); // light magenta

const _COLOR_TEXT: Color32 = Color32::from_rgb(255, 255, 255); // white text on dark backgrounds
const COLOR_TEXT_INV: Color32 = Color32::from_rgb(0, 0, 0); // black text on colored backgrounds

#[derive(Debug, Clone)]
pub struct DisplaySettings {
    pub search_string: String,
    pub show_timestamps: bool,
    pub show_targets: bool,
    pub show_file_info: bool,
    pub show_span_info: bool,
    pub auto_scroll: bool,
    pub level_filter: LogLevelFilter,
    pub wrap: bool,
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
            search_string: "".to_string(),
            show_timestamps: false,
            show_targets: false,
            show_file_info: false,
            show_span_info: false,
            auto_scroll: true,
            level_filter: LogLevelFilter::Trace,
            wrap: false,
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

    toasts: egui_notify::Toasts,

    // Hack to detect if the wrap setting has changed
    // and re-render the table - otherwise the layouting is wrong
    // bcause of the horizontal scroll area
    previous_wrap_setting: bool,
}

impl EguiApp {
    fn apply_filter(&mut self, new_filter: LogLevelFilter) -> bool {
        if self.frontend_side.settings.level_filter != new_filter {
            self.frontend_side.settings.level_filter = new_filter;
            self.frontend_side
                .to_backend
                .send(UiEvent::LogDisplaySettingsChanged(
                    self.frontend_side.settings.clone(),
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
        changed |= ui
            .checkbox(&mut self.frontend_side.settings.wrap, "Wrap Text")
            .changed();

        ui.separator();

        ui.label("Search:");
        changed |= ui
            .text_edit_singleline(&mut self.frontend_side.settings.search_string)
            .changed();
        // clear
        let button = ui.button("Clear").on_hover_text("Clear search");
        button.clicked().then(|| {
            self.frontend_side.settings.search_string.clear();
            changed = true;
        });
        changed |= button.clicked();

        changed
    }

    fn render_logs_table(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let text_style = egui::TextStyle::Monospace;
        let row_height = ui.text_style_height(&text_style);
        let logs = &self
            .frontend_side
            .data_buffer_rx
            .output_buffer_mut()
            .filtered_logs;

        use egui_extras::{Column, TableBuilder};

        ScrollArea::horizontal().show(ui, |ui| {
            let mut table_builder = TableBuilder::new(ui);

            // Add columns based on settings
            if self.frontend_side.settings.show_timestamps {
                table_builder =
                    table_builder.column(Column::auto().at_least(100.0).resizable(true));
                // Timestamp
            }
            table_builder = table_builder.column(Column::auto().at_least(40.0).resizable(true)); // Level
            if self.frontend_side.settings.show_targets {
                table_builder = table_builder.column(Column::auto().resizable(true));
                // Target
            }
            if self.frontend_side.settings.show_span_info {
                table_builder = table_builder.column(Column::auto().at_least(60.0).resizable(true));
                // Span
            }
            if self.frontend_side.settings.show_file_info {
                table_builder = table_builder.column(Column::auto().resizable(true));
                // File info
            }

            // hack to force re-layout if wrap setting changed, otherwise the horizontal scroll
            // area does not force the column to shrink
            let wrap_setting_changed =
                self.frontend_side.settings.wrap != self.previous_wrap_setting;
            let msg_col = Column::remainder()
                .at_least(200.0)
                .resizable(true)
                .clip(wrap_setting_changed);

            table_builder = table_builder.column(msg_col); // Message + fields

            table_builder
                .auto_shrink([false; 2])
                .cell_layout(egui::Layout::left_to_right(egui::Align::LEFT))
                .stick_to_bottom(self.frontend_side.settings.auto_scroll)
                .header(20.0, |mut header| {
                    if self.frontend_side.settings.show_timestamps {
                        header.col(|ui| {
                            ui.label("Timestamp");
                        });
                    }
                    header.col(|ui| {
                        ui.label("Level");
                    });
                    if self.frontend_side.settings.show_targets {
                        header.col(|ui| {
                            ui.label("Target");
                        });
                    }
                    if self.frontend_side.settings.show_span_info {
                        header.col(|ui| {
                            ui.label("Span Info");
                        });
                    }
                    if self.frontend_side.settings.show_file_info {
                        header.col(|ui| {
                            ui.label("File Info");
                        });
                    }
                    header.col(|ui| {
                        ui.label("Message");
                    });
                })
                .body(|body| {
                    let num_rows = logs.len();
                    body.rows(row_height, num_rows, |mut row| {
                        let row_index = row.index();
                        if let Some(event) = logs.get(row_index) {
                            display_log_line_columns(
                                ctx,
                                &mut self.toasts,
                                &mut row,
                                event,
                                self.frontend_side.settings.clone(),
                                self.frontend_side.to_backend.clone(),
                            );
                        }
                    });
                });
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
            previous_wrap_setting: frontend.settings.wrap,
            frontend_side: frontend,
            toasts: egui_notify::Toasts::default(),
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.previous_wrap_setting = self.frontend_side.settings.wrap;
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
            ui.horizontal(|ui| {
                let settings_changed = self.show_controls(ui);
                if settings_changed {
                    self.frontend_side
                        .to_backend
                        .send(UiEvent::LogDisplaySettingsChanged(
                            self.frontend_side.settings.clone(),
                        ))
                        .unwrap_or_else(|err| {
                            error!("Failed to send log display settings change: {err}");
                        });
                }
                ui.separator();

                // align to the right
                ui.with_layout(egui::Layout::right_to_left(egui::Align::LEFT), |ui| {
                    // Clear logs button
                    ui.button("Clear")
                        .on_hover_text("Clear all logs")
                        .clicked()
                        .then(|| {
                            self.frontend_side
                                .to_backend
                                .send(UiEvent::Clear)
                                .unwrap_or_else(|err| {
                                    error!("Failed to send log display settings change: {err}");
                                });
                        });
                    ui.horizontal(|ui| {
                        // memory usage
                        let memory_usage = self
                            .frontend_side
                            .data_buffer_rx
                            .output_buffer_mut()
                            .oculus_memory_usage_mb;

                        ui.label(format!("Memory Usage: {memory_usage:.2} MB"));
                    });
                });
            });

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
            self.render_logs_table(ctx, ui);

            self.toasts.show(ctx);
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
    Clear,
}

pub fn display_log_line_columns(
    ctx: &egui::Context,
    toasts: &mut egui_notify::Toasts,
    row: &mut egui_extras::TableRow,
    event: &DashboardEvent,
    settings: DisplaySettings,
    to_backend: tokio::sync::mpsc::UnboundedSender<UiEvent>,
) {
    // Timestamp column
    if settings.show_timestamps {
        row.col(|ui| {
            let timestamp = format_timestamp_utc(event.timestamp);
            ui.label(
                egui::RichText::new(timestamp)
                    .color(Color32::GRAY)
                    .font(egui::FontId::monospace(12.0)),
            );
        });
    }

    // Level column
    row.col(|ui| {
        let level_text = event.level.to_string().to_uppercase();
        ui.label(
            egui::RichText::new(level_text)
                .color(color_for_log_level(&event.level))
                .font(egui::FontId::monospace(12.0)),
        );
    });

    // Target column
    if settings.show_targets {
        row.col(|ui| {
            ui.label(
                egui::RichText::new(&event.target)
                    .color(Color32::LIGHT_BLUE)
                    .font(egui::FontId::monospace(12.0)),
            );
        });
    }

    // Span info column
    if settings.show_span_info {
        row.col(|ui| {
            if let Some(span_meta) = &event.span_meta {
                let span_name = &span_meta.name;
                let span_text = if let Some(parent_id) = event.parent_span_id {
                    format!("{parent_id}→{span_name}")
                } else {
                    span_name.clone()
                };
                ui.label(
                    egui::RichText::new(span_text)
                        .color(Color32::YELLOW)
                        .font(egui::FontId::monospace(12.0)),
                );
            }
        });
    }

    // File info column
    if settings.show_file_info {
        row.col(|ui| {
            if let (Some(file_path), Some(line_num)) = (&event.file, event.line) {
                let PathParts { project, file } = path_parts(file_path);
                let file_info = format!("{file}:{line_num}");

                ui.label(
                    egui::RichText::new(project)
                        .color(COLOR_LIGHT_MAGENTA)
                        .font(egui::FontId::monospace(10.0)),
                );

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

                    toasts.info(format!("Opening {file_path} in editor..."));
                }
            }
        });
    }

    // Message + fields column
    row.col(|ui| {
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

        let label_response = if settings.wrap {
            ui.add(egui::Label::new(message_job).wrap())
        } else {
            ui.add(egui::Label::new(message_job))
        };

        let _ctx_menu = label_response.context_menu(|ui| {
            if ui.button("Copy to clipboard").clicked() {
                ctx.copy_text(event.message.clone());
                ui.close_menu();
            }
        });
    });
}

struct PathParts<'a> {
    pub project: &'a str,
    pub file: &'a str,
}

fn path_parts(path: &str) -> PathParts<'_> {
    match path.rsplit_once("/src/") {
        Some((before_src, after_src)) => {
            let project = before_src.rsplit('/').next().unwrap_or("");
            PathParts {
                project,
                file: after_src,
            }
        }
        None => PathParts {
            project: "",
            file: path,
        },
    }
}
