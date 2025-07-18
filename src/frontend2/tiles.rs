pub struct TabbedLayout {
    tree: egui_tiles::Tree<Pane>,
    behavior: TreeBehavior,

    next_panel_id: AtomicUsize,
}

impl TabbedLayout {
    pub fn new() -> Self {
        let tree = create_tree();
        let behavior = TreeBehavior::default();
        Self {
            tree,
            behavior,
            next_panel_id: AtomicUsize::new(0),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.tree.ui(&mut self.behavior, ui);
        if let Some(parent) = self.behavior.add_child_to.take() {
            let new_child = self.tree.tiles.insert_pane(Pane::with_nr(
                self.next_panel_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Tabs(tabs))) =
                self.tree.tiles.get_mut(parent)
            {
                tabs.add_child(new_child);
                tabs.set_active(new_child);
            }
        }
    }
}

use std::sync::atomic::AtomicUsize;

use egui_tiles::{Tile, TileId, Tiles};

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
        true
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

pub fn create_tree() -> egui_tiles::Tree<Pane> {
    let mut next_view_nr = 0;
    let mut gen_pane = || {
        let pane = Pane { nr: next_view_nr };
        next_view_nr += 1;
        pane
    };

    let mut tiles = egui_tiles::Tiles::default();

    // Create panes first
    let pane1 = tiles.insert_pane(gen_pane());
    let pane2 = tiles.insert_pane(gen_pane());
    let pane4 = tiles.insert_pane(gen_pane());

    // Wrap each group of panes in their own tab tile
    let tab1 = tiles.insert_tab_tile(vec![pane1]);
    let tab2 = tiles.insert_tab_tile(vec![pane2]);
    let tab3 = tiles.insert_tab_tile(vec![pane4]);

    // Make a top-level tab tile to hold all of them
    let root = tiles.insert_tab_tile(vec![tab1, tab2, tab3]);

    egui_tiles::Tree::new("my_tree", root, tiles)
}
