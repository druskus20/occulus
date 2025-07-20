use egui_tiles::{Tile, TileId, Tiles};
use std::sync::atomic::AtomicUsize;

use crate::prelude::*;

use super::FrameState;

pub struct Tabs {
    tree: egui_tiles::Tree<Pane>,

    root_tile_id: egui_tiles::TileId,
    behavior: TreeBehavior,
    next_panel_id: AtomicUsize,
}

impl std::fmt::Debug for Tabs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tabs")
            .field("tree", &"...")
            .field("behavior", &"...")
            .field("next_panel_id", &self.next_panel_id)
            .finish()
    }
}

impl Tabs {
    pub fn empty() -> Self {
        let (tree, base_tile_id) = {
            let mut tiles = egui_tiles::Tiles::default();

            let root = tiles.insert_tab_tile(vec![]);

            (egui_tiles::Tree::new("my_tree", root, tiles), root)
        };

        Self {
            tree,
            behavior: TreeBehavior::default(),
            next_panel_id: AtomicUsize::new(2),
            root_tile_id: base_tile_id,
        }
    }

    pub fn root_tile(&self) -> egui_tiles::TileId {
        self.root_tile_id
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, frame_state: &mut FrameState) {
        self.tree.ui(&mut self.behavior, ui);
        if let Some(parent) = self.behavior.add_child_to.take() {
            //self.add_new_pane_to(parent);
            frame_state.add_pane_child_to = Some(parent);
        }
    }

    pub fn add_new_pane_to(&mut self, parent: egui_tiles::TileId) -> TileId {
        let new_pane = Pane::with_nr(
            self.next_panel_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        let new_tile = self.tree.tiles.insert_pane(new_pane);
        if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) =
            self.tree.tiles.get_mut(parent)
        {
            info!("Adding new pane to parent tile: {:?}", parent);
            tabs.add_child(new_tile);
            tabs.set_active(new_tile);
        } else {
            panic!("Parent tile is not a tab container");
        }

        return new_tile;
    }

    pub fn get_pane_with_id(&self, tile_id: TileId) -> Option<&Pane> {
        if let Some(tile) = self.tree.tiles.get(tile_id) {
            match tile {
                Tile::Pane(pane) => Some(pane),
                Tile::Container(_) => None,
            }
        } else {
            None
        }
    }
}

pub(crate) struct TreeBehavior {
    simplification_options: egui_tiles::SimplificationOptions,
    tab_bar_height: f32,
    gap_width: f32,
    add_child_to: Option<egui_tiles::TileId>,
}

impl Default for TreeBehavior {
    fn default() -> Self {
        Self {
            simplification_options: egui_tiles::SimplificationOptions {
                all_panes_must_have_tabs: true,
                join_nested_linear_containers: true,
                prune_empty_tabs: true,
                prune_empty_containers: true,
                prune_single_child_tabs: true,
                prune_single_child_containers: true,
            },
            tab_bar_height: 24.0,
            gap_width: 2.0,
            add_child_to: None,
        }
    }
}

impl TreeBehavior {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let Self {
            simplification_options,
            tab_bar_height,
            gap_width,
            add_child_to: _,
        } = self;

        egui::Grid::new("behavior_ui")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("All panes must have tabs:");
                ui.checkbox(&mut simplification_options.all_panes_must_have_tabs, "");
                ui.end_row();

                ui.label("Join nested containers:");
                ui.checkbox(
                    &mut simplification_options.join_nested_linear_containers,
                    "",
                );
                ui.end_row();

                ui.label("Tab bar height:");
                ui.end_row();

                ui.label("Gap width:");
                //ui.add(egui::DragValue::new(gap_width).range(0.0..=20.0).speed(1.0));
                ui.end_row();
            });
    }
}

impl egui_tiles::Behavior<Pane> for TreeBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        view: &mut Pane,
    ) -> egui_tiles::UiResponse {
        view.ui(ui)
    }

    fn tab_title_for_pane(&mut self, view: &Pane) -> egui::WidgetText {
        format!("View {}", view.nr).into()
    }

    fn top_bar_right_ui(
        &mut self,
        _tiles: &egui_tiles::Tiles<Pane>,
        ui: &mut egui::Ui,
        tile_id: egui_tiles::TileId,
        _tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        // Direction is reversed, right to left
        ui.add_space(4.0);
        if ui.button("+").clicked() {
            self.add_child_to = Some(tile_id);
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        self.tab_bar_height
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        self.gap_width
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        self.simplification_options
    }

    fn is_tab_closable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        // only if it's not the last tab
        if _tiles.len() > 1 { true } else { false }
    }

    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId) -> bool {
        if let Some(tile) = tiles.get(tile_id) {
            match tile {
                Tile::Pane(pane) => {
                    // Single pane removal
                    let tab_title = self.tab_title_for_pane(pane);
                }
                Tile::Container(container) => {
                    // Container removal
                    let children_ids = container.children();
                    for child_id in children_ids {
                        if let Some(Tile::Pane(pane)) = tiles.get(*child_id) {
                            let tab_title = self.tab_title_for_pane(pane);
                        }
                    }
                }
            }
        }

        // Proceed to removing the tab
        true
    }
}

pub struct Pane {
    nr: usize,
}

impl Pane {
    pub fn with_nr(nr: usize) -> Self {
        Self { nr }
    }

    pub fn ui(&self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        let color = egui::epaint::Hsva::new(0.103 * self.nr as f32, 0.5, 0.5, 1.0);
        ui.painter().rect_filled(ui.max_rect(), 0.0, color);

        // this makes the pane itself draggable - which is not what we want
        //
        //let dragged = ui
        //    .allocate_rect(ui.max_rect(), egui::Sense::click_and_drag())
        //    .on_hover_cursor(egui::CursorIcon::Grab)
        //    .dragged();
        //if dragged {
        //    egui_tiles::UiResponse::DragStarted
        //} else {
        //    egui_tiles::UiResponse::None
        //}

        egui_tiles::UiResponse::None
    }
}
