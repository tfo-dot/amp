#![allow(dead_code)]
use crate::player::MediaSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewState {
    Library,
    Player,
    Settings,
    ProviderSelect,
}

pub struct Router {
    pub current_view: ViewState,
}

impl Router {
    pub fn new() -> Self {
        Self {
            current_view: ViewState::Library,
        }
    }

    pub fn switch_view<M: MediaSession>(&mut self, view: ViewState, session: &mut M) {
        // When exiting the player view, tear down the FBO to prevent Wayland leaks
        if matches!(self.current_view, ViewState::Player) && !matches!(view, ViewState::Player) {
            session.teardown();
        }
        self.current_view = view;
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}
