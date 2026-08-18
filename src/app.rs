#![allow(dead_code)]
use crate::backend::seanime::SeanimeWsEvent;

pub enum UiCommand {
    Play,
    Pause,
    Seek(i64),
    LoadView(String),
    OpenMedia(String),
}

pub enum AppEvent {
    UiCommand(UiCommand),
    MpvEvent(i32), // event_id
    SeanimeWs(SeanimeWsEvent),
    ScriptReloaded(String),
}
