use crate::{
    TokioEguiBridge,
    data_task::{DisplayData, OculusInternalMetrics},
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

    //pub fn show(
    //    &mut self,
    //    ui: &mut Ui,
    //    to_data: &UnboundedSender<UiEvent>,
    //    display_data: &DisplayData,
    //) {
    //    ui.separator();

    //    let settings_changed = self.show_controls(ui);
    //    if settings_changed {
    //        to_data
    //            .send(UiEvent::LogDisplaySettingsChanged(self.settings))
    //            .unwrap_or_else(|err| {
    //                error!("Failed to send log display settings change: {err}");
    //            });
    //    }
    //    self.render_logs(ui, display_data);
    //}

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
        });

        changed
    }

    fn render_logs(
        &mut self,
        ui: &mut Ui,
        display_data: &DisplayData,
        cache: &mut LogDisplayCache,
    ) {
        let text_style = egui::TextStyle::Monospace;
        let row_height = ui.text_style_height(&text_style);

        let logs = &display_data.filtered_logs;
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.settings.auto_scroll)
            .show_rows(ui, row_height, logs.len(), |ui, row_range| {
                for row in row_range {
                    // only compute the layout job for the visible rows
                    // TODO: cache the layout jobs - logs dont change unless settings do
                    if let Some(job) = logs.get(row) {
                        //ui.label(create_layout_job(job, self.settings));
                        ui.label(cache.get_or_create_layout_job(job, self.settings));
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
    display_data_rx: triple_buffer::Output<DisplayData>,
    log_display_cache: LogDisplayCache,
    log_display: TracingLogDisplay,
    metrics: triple_buffer::Output<PhantomData<()>>,
    internal_metrics: triple_buffer::Output<OculusInternalMetrics>,
    to_data: UnboundedSender<UiEvent>,
}

impl EguiApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        tokio_egui_bridge: TokioEguiBridge,
        display_data_rx: triple_buffer::Output<DisplayData>,
        internal_metrics: triple_buffer::Output<OculusInternalMetrics>,
        metrics: triple_buffer::Output<PhantomData<()>>,
        initial_settings: LogDisplaySettings,
        to_data: UnboundedSender<UiEvent>,
    ) -> Self {
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
            log_display_cache: LogDisplayCache::new(1000, EvictionPolicy::LRU),
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

            self.display_data_rx.update();
            let display_data = self.display_data_rx.read();

            // LOG COUNTS as colored buttons
            let log_counts = &display_data.log_counts;
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
            self.log_display
                .render_logs(ui, display_data, &mut self.log_display_cache);
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
                display_data_rx,
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

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

// Enhanced cache entry with metadata
#[derive(Clone)]
struct CachedLogLine {
    layout_job: LayoutJob,
    event_hash: u64,
    settings_hash: u64,
    created_at: Instant,
    last_accessed: Instant,
    access_count: u32,
}

// Cache statistics for monitoring
#[derive(Debug, Default)]
struct CacheStats {
    hits: u64,
    misses: u64,
    evictions: u64,
    invalidations: u64,
}

impl CacheStats {
    fn hit_rate(&self) -> f64 {
        if self.hits + self.misses == 0 {
            0.0
        } else {
            self.hits as f64 / (self.hits + self.misses) as f64
        }
    }
}

// Different eviction policies
#[derive(Debug, Clone)]
enum EvictionPolicy {
    /// LRU - Least Recently Used
    LRU,
    /// LFU - Least Frequently Used
    LFU,
    /// FIFO - First In, First Out
    FIFO,
    /// TTL - Time To Live based eviction
    TTL(Duration),
    /// Adaptive - Combines LRU and LFU with weights
    Adaptive, // Combination of LRU + access frequency
}

/// Advanced cache with multiple invalidation strategies
struct LogDisplayCache {
    cache: HashMap<u64, CachedLogLine>,
    settings_version: u64, // Incremented when settings change
    max_size: usize,
    eviction_policy: EvictionPolicy,
    stats: CacheStats,

    // For partial invalidation
    event_to_keys: HashMap<u64, HashSet<u64>>, // event_hash -> cache_keys
    settings_to_keys: HashMap<u64, HashSet<u64>>, // settings_hash -> cache_keys
}

impl LogDisplayCache {
    fn new(max_size: usize, eviction_policy: EvictionPolicy) -> Self {
        Self {
            cache: HashMap::new(),
            settings_version: 0,
            max_size,
            eviction_policy,
            stats: CacheStats::default(),
            event_to_keys: HashMap::new(),
            settings_to_keys: HashMap::new(),
        }
    }

    /// Get cached layout job with LRU updating
    fn get_or_create_layout_job(
        &mut self,
        event: &DashboardEvent,
        settings: LogDisplaySettings,
    ) -> LayoutJob {
        let cache_key = self.create_cache_key(event, &settings);

        // Check cache first
        if let Some(cached) = self.cache.get_mut(&cache_key) {
            // Update access metadata
            cached.last_accessed = Instant::now();
            cached.access_count += 1;
            self.stats.hits += 1;
            return cached.layout_job.clone();
        }

        // Cache miss - create new layout job
        self.stats.misses += 1;
        let layout_job = create_layout_job(event, settings);

        // Ensure we have space
        self.ensure_cache_space();

        // Cache the new entry
        let event_hash = self.hash_event(event);
        let settings_hash = self.hash_settings(&settings);
        let now = Instant::now();

        let cached_entry = CachedLogLine {
            layout_job: layout_job.clone(),
            event_hash,
            settings_hash,
            created_at: now,
            last_accessed: now,
            access_count: 1,
        };

        self.cache.insert(cache_key, cached_entry);

        // Update reverse mappings for partial invalidation
        self.event_to_keys
            .entry(event_hash)
            .or_default()
            .insert(cache_key);
        self.settings_to_keys
            .entry(settings_hash)
            .or_default()
            .insert(cache_key);

        layout_job
    }

    /// 1. GRANULAR INVALIDATION - Only invalidate affected entries
    fn invalidate_by_settings_change(
        &mut self,
        old_settings: &LogDisplaySettings,
        new_settings: &LogDisplaySettings,
    ) {
        let old_hash = self.hash_settings(old_settings);
        let new_hash = self.hash_settings(new_settings);

        // If settings hash is the same, no invalidation needed
        if old_hash == new_hash {
            return;
        }

        // Only invalidate entries that used the old settings
        if let Some(keys_to_remove) = self.settings_to_keys.remove(&old_hash) {
            for key in &keys_to_remove {
                self.cache.remove(key);
                self.stats.invalidations += 1;
            }

            // Clean up reverse mappings
            self.cleanup_reverse_mappings(&keys_to_remove);
        }

        self.settings_version += 1;
    }

    /// 2. VERSIONED INVALIDATION - Track settings versions
    fn invalidate_by_version(&mut self, settings_changed: bool) {
        if settings_changed {
            self.settings_version += 1;
        }

        // Remove entries from old settings versions
        let current_version = self.settings_version;
        let keys_to_remove: Vec<_> = self
            .cache
            .iter()
            .filter(|(_, entry)| {
                // Custom logic to determine if entry is from old version
                // This is a simplified example - you'd need to track versions per entry
                entry.created_at.elapsed() > Duration::from_secs(30) // Simple time-based heuristic
            })
            .map(|(key, _)| *key)
            .collect();

        for key in keys_to_remove {
            self.cache.remove(&key);
            self.stats.invalidations += 1;
        }
    }

    /// 3. SMART INVALIDATION - Only invalidate what actually changed
    fn smart_invalidate(&mut self, settings_diff: &SettingsDiff) {
        let keys_to_remove: Vec<_> = self
            .cache
            .iter()
            .filter(|(_, entry)| {
                // Only invalidate if this entry would be affected by the change
                self.entry_affected_by_diff(entry, settings_diff)
            })
            .map(|(key, _)| *key)
            .collect();

        for key in keys_to_remove {
            self.cache.remove(&key);
            self.stats.invalidations += 1;
        }
    }

    /// 4. LAZY INVALIDATION - Mark as invalid but don't remove
    fn lazy_invalidate(&mut self, settings: &LogDisplaySettings) {
        // In this approach, we'd add an `is_valid` flag to CachedLogLine
        // and check it during retrieval, but for simplicity, we'll do immediate removal
        self.invalidate_by_settings_change(&LogDisplaySettings::default(), settings);
    }

    /// 5. BATCHED INVALIDATION - Collect invalidations and process in batches
    fn batch_invalidate(&mut self, invalidation_requests: Vec<InvalidationRequest>) {
        let mut keys_to_remove = HashSet::new();

        for request in invalidation_requests {
            match request {
                InvalidationRequest::ByEvent(event_hash) => {
                    if let Some(keys) = self.event_to_keys.get(&event_hash) {
                        keys_to_remove.extend(keys);
                    }
                }
                InvalidationRequest::BySettings(settings_hash) => {
                    if let Some(keys) = self.settings_to_keys.get(&settings_hash) {
                        keys_to_remove.extend(keys);
                    }
                }
                InvalidationRequest::ByAge(max_age) => {
                    let cutoff = Instant::now() - max_age;
                    keys_to_remove.extend(
                        self.cache
                            .iter()
                            .filter(|(_, entry)| entry.created_at < cutoff)
                            .map(|(key, _)| *key),
                    );
                }
            }
        }

        // Process all invalidations at once
        for key in keys_to_remove {
            self.cache.remove(&key);
            self.stats.invalidations += 1;
        }
    }

    /// Ensure cache doesn't exceed max size using the configured eviction policy
    fn ensure_cache_space(&mut self) {
        if self.cache.len() < self.max_size {
            return;
        }

        let entries_to_remove = self.cache.len() - self.max_size + 1;
        let keys_to_remove = self.select_eviction_candidates(entries_to_remove);

        for key in keys_to_remove {
            self.cache.remove(&key);
            self.stats.evictions += 1;
        }
    }

    /// Select entries for eviction based on policy
    fn select_eviction_candidates(&self, count: usize) -> Vec<u64> {
        match &self.eviction_policy {
            EvictionPolicy::LRU => {
                let mut entries: Vec<_> = self.cache.iter().collect();
                entries.sort_by_key(|(_, entry)| entry.last_accessed);
                entries
                    .into_iter()
                    .take(count)
                    .map(|(key, _)| *key)
                    .collect()
            }
            EvictionPolicy::LFU => {
                let mut entries: Vec<_> = self.cache.iter().collect();
                entries.sort_by_key(|(_, entry)| entry.access_count);
                entries
                    .into_iter()
                    .take(count)
                    .map(|(key, _)| *key)
                    .collect()
            }
            EvictionPolicy::FIFO => {
                let mut entries: Vec<_> = self.cache.iter().collect();
                entries.sort_by_key(|(_, entry)| entry.created_at);
                entries
                    .into_iter()
                    .take(count)
                    .map(|(key, _)| *key)
                    .collect()
            }
            EvictionPolicy::TTL(duration) => {
                let cutoff = Instant::now() - *duration;
                self.cache
                    .iter()
                    .filter(|(_, entry)| entry.created_at < cutoff)
                    .take(count)
                    .map(|(key, _)| *key)
                    .collect()
            }
            EvictionPolicy::Adaptive => {
                // Combine LRU and LFU with weights
                let mut entries: Vec<_> = self.cache.iter().collect();
                entries.sort_by(|(_, a), (_, b)| {
                    let a_score = a.access_count as f64 / a.last_accessed.elapsed().as_secs_f64();
                    let b_score = b.access_count as f64 / b.last_accessed.elapsed().as_secs_f64();
                    a_score
                        .partial_cmp(&b_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                entries
                    .into_iter()
                    .take(count)
                    .map(|(key, _)| *key)
                    .collect()
            }
        }
    }

    /// Helper functions
    fn entry_affected_by_diff(&self, entry: &CachedLogLine, diff: &SettingsDiff) -> bool {
        // Check if this cache entry would be visually different with the new settings
        diff.show_timestamps.is_some()
            || diff.show_targets.is_some()
            || diff.show_file_info.is_some()
            || diff.show_span_info.is_some()
    }

    fn cleanup_reverse_mappings(&mut self, removed_keys: &HashSet<u64>) {
        // Remove references to deleted keys from reverse mappings
        for keys in self.event_to_keys.values_mut() {
            keys.retain(|k| !removed_keys.contains(k));
        }
        for keys in self.settings_to_keys.values_mut() {
            keys.retain(|k| !removed_keys.contains(k));
        }
    }

    // ... (keep existing hash methods)
    fn create_cache_key(&self, event: &DashboardEvent, settings: &LogDisplaySettings) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash_event(event).hash(&mut hasher);
        self.hash_settings(settings).hash(&mut hasher);
        hasher.finish()
    }

    fn hash_event(&self, event: &DashboardEvent) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        event.timestamp.hash(&mut hasher);
        event.level.hash(&mut hasher);
        event.target.hash(&mut hasher);
        event.message.hash(&mut hasher);
        event.span_id.hash(&mut hasher);
        event.parent_span_id.hash(&mut hasher);
        event.file.hash(&mut hasher);
        event.line.hash(&mut hasher);
        for (key, value) in &event.fields {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn hash_settings(&self, settings: &LogDisplaySettings) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        settings.show_timestamps.hash(&mut hasher);
        settings.show_targets.hash(&mut hasher);
        settings.show_file_info.hash(&mut hasher);
        settings.show_span_info.hash(&mut hasher);
        settings.level_filter.hash(&mut hasher);
        hasher.finish()
    }

    /// Get cache statistics
    fn get_stats(&self) -> &CacheStats {
        &self.stats
    }
}
// Supporting types for advanced invalidation
#[derive(Debug)]
struct SettingsDiff {
    show_timestamps: Option<bool>,
    show_targets: Option<bool>,
    show_file_info: Option<bool>,
    show_span_info: Option<bool>,
    level_filter: Option<LogLevelFilter>,
}

#[derive(Debug)]
enum InvalidationRequest {
    ByEvent(u64),
    BySettings(u64),
    ByAge(Duration),
}
