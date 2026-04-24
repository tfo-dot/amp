use amp_api::DynProvider;
use std::collections::HashMap;

#[derive(Clone)]
pub struct PlaylistItem {
    pub p_id: String,
    pub item_id: String,
    pub name: String,
    pub series_name: String,
    pub index: i32,
    pub season_index: i32,
}

pub struct AppState {
    pub nav_stack: Vec<(String, String)>, // (id, name)
    pub current_items_ids: Vec<(String, String)>, // (provider_id, item_id)
    pub next_up_ids: Vec<(String, String)>,
    pub active_playlist: Option<(Vec<PlaylistItem>, usize)>,
    pub search_results_ids: Vec<(String, String)>,
    pub current_item_id: Option<(String, String)>,
    pub current_title: String,
    pub current_artist: String,
    pub current_series_name: Option<String>,
    pub current_season_index: Option<i32>,
    pub current_episode_index: Option<i32>,
    pub active_providers: HashMap<String, DynProvider>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            nav_stack: Vec::new(),
            current_items_ids: Vec::new(),
            next_up_ids: Vec::new(),
            active_playlist: None,
            search_results_ids: Vec::new(),
            current_item_id: None,
            current_title: String::new(),
            current_artist: String::new(),
            current_series_name: None,
            current_season_index: None,
            current_episode_index: None,
            active_providers: HashMap::new(),
        }
    }
}
