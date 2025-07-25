use crate::{
    data::{BackendCommForStream, FrontendCommForStream, Level},
    prelude::*,
};
use egui_tiles::{Tile, TileId, Tiles};

use super::{ScopedUIEvent, tiles::TreeBehavior};

#[derive(Debug)]
pub struct Pane {
    pub nr: usize,
    pub associated_stream_id: usize,

    comm: Option<FrontendCommForStream>,
}

impl Pane {
    pub fn new(nr: usize, stream_id: usize) -> Self {
        Self {
            nr,
            associated_stream_id: stream_id,
            comm: None,
        }
    }

    pub(crate) fn init_comm(&mut self, comm: FrontendCommForStream) {
        self.comm = Some(comm);
    }
}

use crate::data::DisplaySettings;
use argus::tracing::oculus::DashboardEvent;
use eframe::egui;
use egui::{
    Button, Color32, RichText, ScrollArea, TextFormat as EguiTextFormat, Ui, text::LayoutJob,
};
use std::collections::HashMap;

// Constants from your original code
const COLOR_ERROR: Color32 = Color32::from_rgb(255, 85, 85);
const COLOR_WARNING: Color32 = Color32::from_rgb(255, 204, 0);
const COLOR_INFO: Color32 = Color32::from_rgb(80, 250, 123);
const COLOR_DEBUG: Color32 = Color32::from_rgb(139, 233, 253);
const COLOR_TRACE: Color32 = Color32::from_rgb(189, 147, 249);
const COLOR_LIGHT_MAGENTA: Color32 = Color32::from_rgb(255, 121, 198);
const COLOR_TEXT_INV: Color32 = Color32::from_rgb(0, 0, 0);

impl Pane {
    pub fn ui(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        if let Some(comm) = &mut self.comm {
            // Update the data buffer for this frame
            comm.data_buffer_rx.update();

            // Extract values we need before the closure
            let stream_id = self.associated_stream_id;

            ui.vertical(|ui| {
                // Header with stream info
                ui.horizontal(|ui| {
                    ui.heading(format!("Stream {}", stream_id));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Memory usage
                        //let memory_usage = comm
                        //    .data_buffer_rx
                        //    .output_buffer_mut()
                        //    .oculus_memory_usage_mb;
                        //ui.label(format!("Memory: {:.2} MB", memory_usage));
                    });
                });

                ui.separator();

                // Controls
                ui.horizontal(|ui| {
                    let settings_changed = show_controls(ui, comm);
                    if settings_changed {
                        comm.to_backend
                            .send(ScopedUIEvent::LogDisplaySettingsChanged(
                                comm.settings.clone(),
                            ))
                            .unwrap_or_else(|err| {
                                eprintln!("Failed to send settings change: {}", err);
                            });
                    }

                    ui.separator();

                    // Clear button
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Clear").on_hover_text("Clear all logs").clicked() {
                            comm.to_backend
                                .send(ScopedUIEvent::Clear)
                                .unwrap_or_else(|err| {
                                    eprintln!("Failed to send clear event: {}", err);
                                });
                        }
                    });
                });

                ui.separator();

                // Log level filter buttons
                ui.horizontal(|ui| {
                    let log_counts = comm.data_buffer_rx.output_buffer_mut().log_counts;

                    ui.label(format!("Total: {}", log_counts.total));

                    add_filter_button(
                        "Error",
                        COLOR_ERROR,
                        log_counts.error,
                        Level::Error,
                        ui,
                        comm,
                    );
                    add_filter_button(
                        "Warn",
                        COLOR_WARNING,
                        log_counts.warn,
                        Level::Warn,
                        ui,
                        comm,
                    );
                    add_filter_button("Info", COLOR_INFO, log_counts.info, Level::Info, ui, comm);
                    add_filter_button(
                        "Debug",
                        COLOR_DEBUG,
                        log_counts.debug,
                        Level::Debug,
                        ui,
                        comm,
                    );
                    add_filter_button(
                        "Trace",
                        COLOR_TRACE,
                        log_counts.trace,
                        Level::Trace,
                        ui,
                        comm,
                    );
                });

                ui.separator();

                // Logs table
                render_logs_table(ui, comm);
            });
        } else {
            // Show loading state if comm is not initialized
            ui.centered_and_justified(|ui| {
                ui.label("Initializing stream...");
            });
        }

        egui_tiles::UiResponse::None
    }
}

fn show_controls(ui: &mut Ui, comm: &mut FrontendCommForStream) -> bool {
    let mut changed = false;

    changed |= ui
        .checkbox(&mut comm.settings.show_timestamps, "Timestamps")
        .changed();
    changed |= ui
        .checkbox(&mut comm.settings.show_targets, "Targets")
        .changed();
    changed |= ui
        .checkbox(&mut comm.settings.show_file_info, "File Info")
        .changed();
    changed |= ui
        .checkbox(&mut comm.settings.show_span_info, "Span Info")
        .changed();
    changed |= ui
        .checkbox(&mut comm.settings.auto_scroll, "Auto Scroll")
        .changed();
    changed |= ui.checkbox(&mut comm.settings.wrap, "Wrap Text").changed();

    ui.separator();

    //ui.label("Search:");
    //changed |= ui
    //    .text_edit_singleline(&mut comm.settings.search_string)
    //    .changed();

    //if ui.button("Clear").on_hover_text("Clear search").clicked() {
    //    comm.settings.search_string.clear();
    //    changed = true;
    //}

    changed
}

fn add_filter_button(
    name: &str,
    color: Color32,
    count: usize,
    level: Level,
    ui: &mut Ui,
    comm: &mut FrontendCommForStream,
) {
    let active = comm.settings.level_filter == level;
    let mut button =
        Button::new(RichText::new(format!("{}: {}", name, count)).color(COLOR_TEXT_INV))
            .fill(color);

    if active {
        button = button.stroke(egui::Stroke::new(1.0, Color32::WHITE));
    }

    if ui.add(button).clicked() {
        if comm.settings.level_filter != level {
            comm.settings.level_filter = level;
            comm.to_backend
                .send(ScopedUIEvent::LogDisplaySettingsChanged(
                    comm.settings.clone(),
                ))
                .unwrap_or_else(|err| {
                    eprintln!("Failed to send filter change: {}", err);
                });
        }
    }
}

fn render_logs_table(ui: &mut Ui, comm: &mut FrontendCommForStream) {
    let text_style = egui::TextStyle::Monospace;
    let row_height = ui.text_style_height(&text_style);
    let logs = &comm.data_buffer_rx.output_buffer_mut().filtered_logs;

    use egui_extras::{Column, TableBuilder};

    ScrollArea::horizontal().show(ui, |ui| {
        let mut table_builder = TableBuilder::new(ui);

        // Add columns based on settings
        if comm.settings.show_timestamps {
            table_builder = table_builder.column(Column::auto().at_least(100.0).resizable(true));
        }
        table_builder = table_builder.column(Column::auto().at_least(40.0).resizable(true)); // Level
        if comm.settings.show_targets {
            table_builder = table_builder.column(Column::auto().resizable(true));
        }
        if comm.settings.show_span_info {
            table_builder = table_builder.column(Column::auto().at_least(60.0).resizable(true));
        }
        if comm.settings.show_file_info {
            table_builder = table_builder.column(Column::auto().resizable(true));
        }
        table_builder = table_builder.column(Column::remainder().at_least(200.0).resizable(true)); // Message

        table_builder
            .auto_shrink([false; 2])
            .cell_layout(egui::Layout::left_to_right(egui::Align::LEFT))
            .stick_to_bottom(comm.settings.auto_scroll)
            .header(20.0, |mut header| {
                if comm.settings.show_timestamps {
                    header.col(|ui| {
                        ui.label("Timestamp");
                    });
                }
                header.col(|ui| {
                    ui.label("Level");
                });
                if comm.settings.show_targets {
                    header.col(|ui| {
                        ui.label("Target");
                    });
                }
                if comm.settings.show_span_info {
                    header.col(|ui| {
                        ui.label("Span Info");
                    });
                }
                if comm.settings.show_file_info {
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
                        display_log_line_columns(&mut row, event, &comm.settings, &comm.to_backend);
                    }
                });
            });
    });
}

fn display_log_line_columns(
    row: &mut egui_extras::TableRow,
    event: &DashboardEvent,
    settings: &DisplaySettings,
    to_backend: &tokio::sync::mpsc::UnboundedSender<ScopedUIEvent>,
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
                .color(color_for_log_level(&event.level.into()))
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
                    format!("{}→{}", parent_id, span_name)
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
                let file_info = format!("{}:{}", file, line_num);

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
                        .send(ScopedUIEvent::OpenInEditor {
                            path: file_path.clone(),
                            line: line_num,
                        })
                        .unwrap_or_else(|_| eprintln!("Failed to send OpenInEditor event"));
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

        // Context menu for copying
        label_response.context_menu(|ui| {
            if ui.button("Copy to clipboard").clicked() {
                ui.ctx().copy_text(event.message.clone());
                ui.close_menu();
            }
        });
    });
}

// Helper functions from your original code
fn color_for_log_level(level: &Level) -> Color32 {
    match level {
        Level::Trace => COLOR_TRACE,
        Level::Debug => COLOR_DEBUG,
        Level::Info => COLOR_INFO,
        Level::Warn => COLOR_WARNING,
        Level::Error => COLOR_ERROR,
    }
}

fn format_timestamp_utc(timestamp_ms: u64) -> String {
    use chrono::{DateTime, Utc};

    let dt = DateTime::<Utc>::from_timestamp_millis(timestamp_ms as i64).unwrap_or_else(|| {
        DateTime::<Utc>::from_timestamp(0, 0).expect("Failed to create timestamp")
    });

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
        formatted.push_str(&format!("{}={}", key, value));
        first = false;
    }

    formatted.push('}');
    formatted
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
