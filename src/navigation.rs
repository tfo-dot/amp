use crate::api::{AmpError, DynProvider, MediaItemType, RawImage};
use crate::app_state::{AppState, PlaylistItem};
use crate::image_cache::ImageCache;
use crate::player::{MpvHandle, open_player};
use crate::{AppWindow, GridItem};
use slint::{Image, Model, SharedPixelBuffer, Weak};
use std::sync::{Arc, Mutex};

pub fn format_time(seconds: i64) -> String {
    let total_seconds = seconds;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;

    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}", minutes, secs)
    }
}

pub fn image_from_raw(raw: RawImage) -> Image {
    let slint_buf =
        SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&raw.rgba8, raw.width, raw.height);

    Image::from_rgba8(slint_buf)
}

pub async fn load_folder(
    client: DynProvider,
    p_id: String,
    folder_id: Option<String>,
    folder_name: String,
    ui_weak: Weak<AppWindow>,
    state_arc: Arc<Mutex<AppState>>,
    cache: Arc<ImageCache>,
) -> Result<(), AmpError> {
    let items = if let Some(ref id) = folder_id {
        client.get_children(id).await?
    } else {
        client.get_root().await?
    };

    let ui_handle = ui_weak.clone();
    let folder_name_c = folder_name.clone();
    let items_for_ui = items.clone();
    let p_id_c = p_id.clone();
    let state_arc_c = state_arc.clone();

    slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_handle.upgrade() {
            let mut ids = Vec::new();
            let grid_items: Vec<GridItem> = items_for_ui
                .into_iter()
                .map(|item| {
                    ids.push((p_id_c.clone(), item.id.clone()));
                    GridItem {
                        id: item.id.into(),
                        name: item.name.into(),
                        thumbnail: Default::default(),
                        description: item
                            .duration_secs
                            .map(format_time)
                            .unwrap_or_default()
                            .into(),
                        meta: item
                            .resume_position_secs
                            .map(format_time)
                            .unwrap_or_default()
                            .into(),
                        series_name: item.series_name.unwrap_or_default().into(),
                        is_folder: item.item_type == MediaItemType::Folder,
                        index: item.index.unwrap_or(0),
                        season_index: item.season_index.unwrap_or(0),
                    }
                })
                .collect();

            state_arc_c.lock().unwrap().current_items_ids = ids;

            ui.set_current_items(slint::ModelRc::from(std::rc::Rc::new(
                slint::VecModel::from(grid_items),
            )));
            ui.set_current_folder_name(folder_name_c.into());
            ui.set_current_screen("library".into());
            ui.set_is_loading(false);
        }
    })
    .map_err(|e| AmpError::Unknown(e.to_string()))?;

    use futures::StreamExt;

    let mut stream = futures::stream::iter(items.into_iter().enumerate())
        .map(|(index, item)| {
            let cache_c = cache.clone();
            let client_c = client.clone();
            let ui_weak_c = ui_weak.clone();
            async move {
                if let Some(raw_img) = cache_c.get_or_fetch(&item.id, client_c).await {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak_c.upgrade() {
                            let model = ui.get_current_items();
                            if let Some(mut row) = model.row_data(index) {
                                row.thumbnail = image_from_raw(raw_img);
                                model.set_row_data(index, row);
                            }
                        }
                    });
                }
            }
        })
        .buffer_unordered(4);

    while stream.next().await.is_some() {}

    Ok(())
}

pub async fn load_dashboard(
    state_arc: Arc<Mutex<AppState>>,
    ui_weak: Weak<AppWindow>,
    cache: Arc<ImageCache>,
) -> Result<(), AmpError> {
    let providers = {
        let state = state_arc.lock().unwrap();
        state.active_providers.get_providers()
    };

    let mut fetch_futures = Vec::new();

    for (p_id, client) in providers {
        let cache_clone = cache.clone();
        fetch_futures.push(async move {
            let mut results = Vec::new();

            if let Ok(next_up) = client.get_next_up().await {
                for s in next_up {
                    let client_clone = client.clone();

                    results.push((
                        p_id.clone(),
                        s.clone(),
                        cache_clone.get_or_fetch(&s.id, client_clone).await,
                    ));
                }
            }
            results
        });
    }

    let all_results_nested = futures::future::join_all(fetch_futures).await;
    let all_next_up: Vec<_> = all_results_nested.into_iter().flatten().collect();

    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let mut nu_ids = Vec::new();
            let mut nu_list = Vec::new();

            for (p_id, item, buffer) in all_next_up {
                nu_ids.push((p_id.clone(), item.id.clone()));
                nu_list.push(GridItem {
                    id: item.id.into(),
                    name: item.name.into(),
                    thumbnail: buffer.map(image_from_raw).unwrap_or_default(),
                    description: format!(
                        "S{}E{}",
                        item.season_index.unwrap_or(0),
                        item.index.unwrap_or(0)
                    )
                    .into(),
                    meta: p_id.clone().into(),
                    series_name: item.series_name.unwrap_or_default().into(),
                    is_folder: false,
                    index: item.index.unwrap_or(0),
                    season_index: item.season_index.unwrap_or(0),
                });
            }

            {
                let mut state = state_arc.lock().unwrap();
                state.next_up_ids = nu_ids;
            }

            ui.set_next_up_list(slint::ModelRc::from(std::rc::Rc::new(
                slint::VecModel::from(nu_list),
            )));
            ui.set_is_loading(false);
        }
    });

    Ok(())
}

//Prov is (id, provider), position: (series, ep)
pub async fn play_episode_with_series_playlist(
    provider_id: String,
    item_id: String,
    item: GridItem,
    ui_weak: Weak<AppWindow>,
    state_arc: Arc<Mutex<AppState>>,
    mpv: MpvHandle,
) {
    let mut playlist_items = Vec::new();
    let mut current_idx = 0;

    let provider = {
        let s = state_arc.lock().unwrap();
        s.active_providers.get_provider(&provider_id).unwrap()
    };

    // Check if we can load the full series playlist from the provider
    if let Some(rest) = item_id.strip_prefix("ep_") {
        let parts: Vec<&str> = rest.split('_').collect();
        if parts.len() >= 2 {
            let media_id = parts[0];
            let folder_id = format!("media_{}", media_id);
            if let Ok(children) = provider.get_children(&folder_id).await {
                for (idx, child) in children.into_iter().enumerate() {
                    if child.id == item_id {
                        current_idx = idx;
                    }
                    playlist_items.push(PlaylistItem {
                        p_id: provider_id.clone(),
                        item_id: child.id,
                        name: child.name,
                        series_name: child
                            .series_name
                            .unwrap_or_else(|| item.series_name.to_string()),
                        index: child.index.unwrap_or(0),
                        season_index: child.season_index.unwrap_or(0),
                    });
                }
            }
        }
    }

    // Fallback if playlist is empty: check state.current_items_ids
    if playlist_items.is_empty() {
        let s = state_arc.lock().unwrap();
        if let Some(ui) = ui_weak.upgrade() {
            let model = ui.get_current_items();
            for (i, id_pair) in s.current_items_ids.iter().enumerate() {
                if let Some(it) = model.row_data(i)
                    && !it.is_folder
                {
                    if id_pair.1 == item_id {
                        current_idx = playlist_items.len();
                    }
                    playlist_items.push(PlaylistItem {
                        p_id: id_pair.0.clone(),
                        item_id: id_pair.1.clone(),
                        name: it.name.to_string(),
                        series_name: it.series_name.to_string(),
                        index: it.index,
                        season_index: it.season_index,
                    });
                }
            }
        }
    }

    // Final fallback
    if playlist_items.is_empty() {
        playlist_items.push(PlaylistItem {
            p_id: provider_id.clone(),
            item_id: item_id.clone(),
            name: item.name.to_string(),
            series_name: item.series_name.to_string(),
            season_index: item.season_index,
            index: item.index,
        });
    }

    {
        let mut s = state_arc.lock().unwrap();
        s.active_playlist = Some((playlist_items, current_idx));
        s.current_title = item.name.to_string();
        s.current_artist = item.series_name.to_string();
        s.current_series_name = Some(item.series_name.to_string());
        s.current_season_index = Some(item.season_index);
        s.current_episode_index = Some(item.index);
        s.current_item_id = Some((provider_id, item_id));
    }

    let _ = slint::invoke_from_event_loop({
        let ui_weak = ui_weak.clone();
        move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_is_loading(true);
                ui.set_current_screen("player".into());
            }
        }
    });

    open_player(ui_weak, state_arc, mpv).await;
}
