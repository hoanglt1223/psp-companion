//! Now playing from the Windows media session (any app that shows in the volume flyout: Spotify,
//! browsers, the Media Player). Read-only; the cover is fetched once per track.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::Interface;
use windows_future::IAsyncOperation;

use super::model::{Asset, NowPlaying, PlayState, WidgetValue};
use super::{WidgetHandle, cover_asset};

pub const COVER_SIZE: u32 = 64;
const POLL: Duration = Duration::from_secs(1);
const MAX_COVER_BYTES: u64 = 4 * 1024 * 1024;

fn read_cover(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<Vec<u8>> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_COVER_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let load: IAsyncOperation<u32> = reader.LoadAsync(size as u32).ok()?.cast().ok()?;
    let n = load.join().ok()?;
    let mut buf = vec![0u8; n as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

struct Poller {
    manager: Manager,
    track: Option<(String, String)>,
    cover: Option<Arc<Asset>>,
}

impl Poller {
    fn read(&mut self) -> NowPlaying {
        let none = NowPlaying { state: PlayState::None, title: String::new(), artist: String::new(), cover: None };
        let Ok(session) = self.manager.GetCurrentSession() else {
            self.track = None;
            self.cover = None;
            return none;
        };
        let Some(props) = session.TryGetMediaPropertiesAsync().ok().and_then(|op| op.join().ok()) else {
            return none;
        };
        let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
        let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        let state = match session.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()) {
            Ok(Status::Playing) => PlayState::Playing,
            Ok(Status::Paused) => PlayState::Paused,
            Ok(_) => PlayState::Stopped,
            Err(_) => PlayState::None,
        };
        let key = (title.clone(), artist.clone());
        if self.track.as_ref() != Some(&key) {
            self.track = Some(key);
            // Some players publish the cover a moment after the title; a later poll retries.
            self.cover = read_cover(&props).and_then(|b| cover_asset(&b, COVER_SIZE));
        } else if self.cover.is_none() {
            self.cover = read_cover(&props).and_then(|b| cover_asset(&b, COVER_SIZE));
        }
        NowPlaying { state, title, artist, cover: self.cover.clone() }
    }
}

pub fn spawn(handle: WidgetHandle) {
    let spawned = std::thread::Builder::new().name("widget-media".into()).spawn(move || {
        if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            debug!("RoInitialize: {e}");
        }
        let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => m,
            Err(e) => {
                info!("now playing disabled: media session manager unavailable: {e}");
                return;
            }
        };
        let mut poller = Poller { manager, track: None, cover: None };
        loop {
            if !handle.blocking_update(WidgetValue::NowPlaying(poller.read())) {
                break;
            }
            std::thread::sleep(POLL);
        }
    });
    if let Err(e) = spawned {
        info!("now playing disabled: cannot start thread: {e}");
    }
}
